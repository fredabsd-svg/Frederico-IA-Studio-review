//! Mapping `StreamEvent → RunState` (Fase 3, Etapa 5.x; Fase 6, Etapa 2).
//!
//! A cada `StreamEvent` que o [`RunExecutor`] consome, o executor consulta
//! o portão único de mudança de estado: `run_state_for_event(current, event)`
//! chama `apply_transition` da `agent-engine` (função pura, testada por
//! par) e devolve o `RunState` resultante ou erro estruturado.
//!
//! ## Por que aqui, não no `agent-engine`?
//!
//! O `agent-engine` é **puro** (não conhece `StreamEvent` nem o
//! `provider-engine` — a enum `RunState` e o `RunEventKind` são
//! independentes de plataforma). O mapping depende do `provider-engine`,
//! então vive no `execution-engine`, que já depende de ambos. A função
//! `apply_transition` mora no `agent-engine` e é o que a Etapa 2 da
//! Fase 6 fecha como portão (ADR-0029 §D1).
//!
//! ## Mapeamento (consistente com o spec `agent-state-machine.md`)
//!
//! | `StreamEvent`                  | `RunEventKind`            | Notas                          |
//! | ------------------------------ | ------------------------- | ------------------------------ |
//! | `Delta` (de `CallingModel`)    | `FirstToken`              | transição → `Streaming`        |
//! | `Delta` (de `Streaming`)       | — (no-op)                 | continuação, sem mudança       |
//! | `Delta` (de outro estado)      | —                         | erro `InvalidTransition`       |
//! | `Usage`                        | — (no-op)                 | só atualiza contadores         |
//! | `ToolCall` (de `Streaming`)    | `ToolCallEmitted`         | aresta 7 → `WaitingToolCall`   |
//! | `ToolCall` (de `CallingModel`) | `ToolCallEmitted`         | aresta 23 (Etapa 2)            |
//! | `ToolResult`                   | `ToolReturned`            | aresta 12 → `ValidatingToolResult` |
//! | `Done { Stop | Length }`       | `MessageComplete` × 2 + `CheckpointPersisted` | walk: `... → ContinuingModel → Checkpointing → Completed` |
//! | `Done { ToolCalls }`           | `ToolCallEmitted`         | aresta 7 ou 23                 |
//! | `Done { Error }`               | `UnrecoverableError`      | global → `Failed`              |
//! | `Error(_)`                     | `UnrecoverableError`      | global → `Failed`              |
//! | `Cancelled`                    | `UserCancel`              | global → `Cancelled`           |
//!
//! ## Por que `Done { Stop | Length }` faz walk de 3 transições
//!
//! A Etapa 2 fechou o portão (ADR-0029 §D1): cada mudança de estado
//! passa por `apply_transition`. O estado `Completed` é terminal e
//! inalterável, e a única aresta de entrada é `Checkpointing +
//! CheckpointPersisted → Completed` (aresta 19). Pra chegar a
//! `Completed` partindo de `Streaming` ou `CallingModel`, o caminho é:
//!
//! 1. `MessageComplete` (aresta 8 ou 22) → `ContinuingModel`
//! 2. `MessageComplete` (aresta 24, Etapa 2) → `Checkpointing`
//! 3. `CheckpointPersisted` (aresta 19) → `Completed`
//!
//! A `state_mapping` faz esse walk em uma única chamada e devolve
//! `Completed` como estado final. Cada passo é uma aresta válida da
//! tabela — não há compressão nem "atalho" que pula o portão.
//!
//! Se qualquer passo do walk falhar (estado inválido pra um dos
//! eventos), o portão retorna `Err(TransitionError)` e o executor
//! aborta o run com `Failed` (mesma semântica de "rejeição por
//! transição inválida" do ADR-0029 §D1).

use frederico_agent_engine::{apply_transition, RunEventKind, RunState, TransitionError};
use frederico_provider_engine::{StopReason, StreamEvent};

/// Uma transição aplicada pelo portão. `from` é o estado anterior
/// (lido da `current_state` do executor), `to` é o estado resultante
/// (calculado por `apply_transition`), `kind` é o `RunEventKind` que
/// causou a transição. Cada chamada de `run_state_for_event` pode
/// retornar **várias** transições (o "walk" do `Done { Stop | Length }`
/// é o caso canônico) — o executor grava **cada uma** como um
/// `RunEvent` separado no journal, pra que a invariante "journal é
/// honesto" (`apply_transition(from, kind) == to`) se mantenha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunStateTransition {
    pub from: RunState,
    pub to: RunState,
    pub kind: RunEventKind,
}

/// Resultado do portão. Lista de transições aplicadas (vazia se o
/// evento não causa mudança de estado — ex.: `Usage` é contador).
/// `Err(...)` se o portão rejeitou (estado terminal ou aresta
/// ausente); a transição que falhou está documentada no `TransitionError`.
pub type StateMappingResult = Result<Vec<RunStateTransition>, TransitionError>;

/// Portão único de mudança de estado: dado o estado atual e o evento do
/// provider, consulta `apply_transition` (função pura da `agent-engine`)
/// e devolve a sequência de transições aplicadas ou erro estruturado.
///
/// **Comportamento:**
/// - `Ok(vec![])` — o evento não muda o estado (`Usage` é contador;
///   `Delta` de `Streaming` é continuação). O executor grava o
///   `MessageEvent` mas não o `RunEvent`.
/// - `Ok(vec![t])` — o evento causou uma transição única. O
///   executor grava 1 `RunEvent` no journal + `set_state` no SQLite.
/// - `Ok(vec![t1, t2, t3])` — o evento causou um walk (ex.:
///   `Done { Stop | Length }`). O executor grava 3 `RunEvent`s
///   separados, **cada um com a transição exata que o portão
///   aceitou** (sem "compressão"). O journal fica honesto:
///   `apply_transition(t.from, t.kind) == t.to` vale para cada `t`.
/// - `Err(TransitionError::FromTerminal { from })` — o estado atual é
///   terminal (`Completed`/`Failed`/`Cancelled`/`Interrupted`). O
///   portão rejeita imediatamente. O executor aborta o run.
/// - `Err(TransitionError::InvalidTransition { from, event, to })` —
///   o par (`from`, `event`) não tem aresta. O executor aborta o
///   run com `Failed` e grava `UnrecoverableError` no journal.
pub fn run_state_for_event(current: RunState, event: &StreamEvent) -> StateMappingResult {
    match event {
        StreamEvent::Delta { .. } => delta_mapping(current),
        StreamEvent::Usage { .. } => Ok(vec![]),
        StreamEvent::ToolCall { .. } => tool_call_mapping(current),
        StreamEvent::ToolResult { .. } => tool_result_mapping(current),
        StreamEvent::Done { stop_reason } => done_mapping(current, *stop_reason),
        StreamEvent::Error(_) => single_transition(current, RunEventKind::UnrecoverableError),
        StreamEvent::Cancelled => single_transition(current, RunEventKind::UserCancel),
    }
}

/// Helper: aplica uma única transição e devolve `vec![t]`.
/// Usado quando o evento causa exatamente uma transição
/// (sem walk).
fn single_transition(
    current: RunState,
    kind: RunEventKind,
) -> Result<Vec<RunStateTransition>, TransitionError> {
    let to = apply_transition(current, kind)?;
    Ok(vec![RunStateTransition {
        from: current,
        to,
        kind,
    }])
}

/// `Delta` é "modelo emitiu um chunk de texto". De `CallingModel`
/// transiciona pra `Streaming` via `FirstToken`. De `Streaming` é
/// continuação (sem mudança de estado, `vec![]`). De qualquer outro
/// estado (incluindo terminais como `Completed`/`Failed`/
/// `Cancelled`/`Interrupted`) delega ao `apply_transition`, que
/// retorna `FromTerminal` se o estado for terminal ou
/// `InvalidTransition` se for não-terminal mas sem aresta.
fn delta_mapping(current: RunState) -> StateMappingResult {
    if current == RunState::Streaming {
        return Ok(vec![]);
    }
    single_transition(current, RunEventKind::FirstToken)
}

/// `ToolCall` é "modelo emitiu um tool_call". De `Streaming` ou
/// `CallingModel` (Etapa 2 — aresta 23) transiciona pra
/// `WaitingToolCall` via `ToolCallEmitted`. De qualquer outro
/// estado delega ao `apply_transition`.
fn tool_call_mapping(current: RunState) -> StateMappingResult {
    single_transition(current, RunEventKind::ToolCallEmitted)
}

/// `ToolResult` é "ferramenta terminou e devolveu output". De
/// `ExecutingTool` ou `WaitingToolCall` transiciona pra
/// `ValidatingToolResult` via `ToolReturned`. De qualquer outro
/// estado delega ao `apply_transition`.
fn tool_result_mapping(current: RunState) -> StateMappingResult {
    single_transition(current, RunEventKind::ToolReturned)
}

/// `Done` é o evento final do provider. Cada `stop_reason` tem
/// semântica distinta.
fn done_mapping(current: RunState, stop_reason: StopReason) -> StateMappingResult {
    match stop_reason {
        StopReason::Stop | StopReason::Length => {
            // Walk: current → ... → Completed. O caminho depende
            // de onde o `current` está:
            //
            // - `Streaming` → `MessageComplete` (aresta 8) → `ContinuingModel`
            //   → `MessageComplete` (aresta 24, Etapa 2) → `Checkpointing`
            //   → `CheckpointPersisted` (aresta 19) → `Completed`
            //   (3 passos)
            // - `CallingModel` → `MessageComplete` (aresta 22) → `ContinuingModel`
            //   → `MessageComplete` (aresta 24) → `Checkpointing`
            //   → `CheckpointPersisted` → `Completed`
            //   (3 passos)
            // - `ContinuingModel` → `MessageComplete` (aresta 24) → `Checkpointing`
            //   → `CheckpointPersisted` → `Completed`
            //   (2 passos)
            //
            // Cada passo consulta `apply_transition` (portão). Se
            // qualquer um falhar (terminal, ou estado sem aresta), o
            // `?` propaga o `TransitionError` sem compactar — quem
            // recebe sabe exatamente onde o walk falhou.
            //
            // **Por que o walk é uma sequência de passos e não uma
            // única transição?** A Etapa 2 fecha o portão
            // (ADR-0029 §D1): toda mudança de estado passa por
            // `apply_transition`. A aresta `Streaming → Completed`
            // (que o `state_mapping` original tinha como
            // "compressão") **não existe na tabela** — o caminho
            // natural é passar por `ContinuingModel → Checkpointing
            // → Completed`. Cada passo é uma aresta válida.
            //
            // O retorno é a sequência **completa** de transições
            // (1 a 3 entradas). O executor grava cada uma como
            // `RunEvent` separado, mantendo a invariante "journal
            // é honesto" do teste `valid_transition_persists_in_run_event_journal`.
            //
            // **Última transição é `CheckpointPersisted → Completed`
            // (não `Checkpointing → Completed`)**. O `Completed`
            // é gravado em 2 lugares: pelo último passo do walk E
            // pelo `finalize_status` do executor. A duplicação é
            // evitada porque o `finalize_status` foi ajustado pra
            // NÃO re-gravar a transição terminal (o walk já fez).
            // Ver `executor.rs::finalize_status`.
            let mut out = Vec::with_capacity(3);
            let s1 = apply_transition(current, RunEventKind::MessageComplete)?;
            out.push(RunStateTransition {
                from: current,
                to: s1,
                kind: RunEventKind::MessageComplete,
            });
            let s2 = if s1 == RunState::ContinuingModel {
                let s = apply_transition(s1, RunEventKind::MessageComplete)?;
                out.push(RunStateTransition {
                    from: s1,
                    to: s,
                    kind: RunEventKind::MessageComplete,
                });
                s
            } else {
                s1
            };
            let s3 = apply_transition(s2, RunEventKind::CheckpointPersisted)?;
            out.push(RunStateTransition {
                from: s2,
                to: s3,
                kind: RunEventKind::CheckpointPersisted,
            });
            Ok(out)
        }
        StopReason::ToolCalls => {
            // O modelo terminou de responder e o output contém
            // tool_call(s). O `StreamEvent::ToolCall` que carrega
            // o tool_call em si já chegou antes (e disparou a
            // transição pra `WaitingToolCall` via `ToolCallEmitted`).
            // O `Done { ToolCalls }` é o fim do stream — o estado
            // JÁ é `WaitingToolCall` (ou outro, dependendo do
            // adapter). É no-op, não tenta transição.
            Ok(vec![])
        }
        StopReason::Error => single_transition(current, RunEventKind::UnrecoverableError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_core::ToolId;
    use frederico_provider_engine::ProviderError;

    // ---- Delta -----------------------------------------------------------

    #[test]
    fn delta_from_calling_model_transitions_to_streaming() {
        let ev = StreamEvent::Delta {
            content: "x".to_string(),
        };
        let transitions = run_state_for_event(RunState::CallingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].from, RunState::CallingModel);
        assert_eq!(transitions[0].to, RunState::Streaming);
    }

    #[test]
    fn delta_from_streaming_is_noop() {
        let ev = StreamEvent::Delta {
            content: "x".to_string(),
        };
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert!(transitions.is_empty());
    }

    #[test]
    fn delta_from_invalid_state_is_rejected() {
        let ev = StreamEvent::Delta {
            content: "x".to_string(),
        };
        let err = run_state_for_event(RunState::Created, &ev).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    // ---- Usage -----------------------------------------------------------

    #[test]
    fn usage_is_always_noop() {
        let ev = StreamEvent::Usage {
            prompt_tokens: 1,
            completion_tokens: 2,
        };
        for state in [
            RunState::CallingModel,
            RunState::Streaming,
            RunState::ContinuingModel,
            RunState::WaitingToolCall,
        ] {
            let transitions = run_state_for_event(state, &ev).unwrap();
            assert!(transitions.is_empty(), "Usage de {state} deveria ser no-op");
        }
    }

    // ---- ToolCall --------------------------------------------------------

    #[test]
    fn tool_call_from_streaming_transitions_to_waiting() {
        let ev = StreamEvent::ToolCall {
            id: "call_1".to_string(),
            name: ToolId::new("files.read").to_string(),
            arguments_json: "{}".to_string(),
        };
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::WaitingToolCall);
    }

    #[test]
    fn tool_call_from_calling_model_transitions_to_waiting() {
        // Etapa 2 (Fase 6): modelo não-streaming emitiu tool_call
        // direto. Aresta 23.
        let ev = StreamEvent::ToolCall {
            id: "call_1".to_string(),
            name: ToolId::new("files.read").to_string(),
            arguments_json: "{}".to_string(),
        };
        let transitions = run_state_for_event(RunState::CallingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::WaitingToolCall);
    }

    // ---- ToolResult ------------------------------------------------------

    #[test]
    fn tool_result_from_executing_tool_transitions_to_validating() {
        let ev = StreamEvent::ToolResult {
            id: "call_1".to_string(),
            ok: true,
            output: serde_json::json!({}),
        };
        let transitions = run_state_for_event(RunState::ExecutingTool, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::ValidatingToolResult);
    }

    // ---- Done ------------------------------------------------------------

    #[test]
    fn done_stop_from_streaming_walks_to_completed() {
        // Etapa 2: o walk é uma sequência de arestas válidas (sem
        // compressão). De Streaming, são 3 transições: Streaming →
        // ContinuingModel (MessageComplete) → Checkpointing
        // (MessageComplete) → Completed (CheckpointPersisted).
        // O `finalize_status` do executor não re-grava a transição
        // terminal (o walk já fez).
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Stop,
        };
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[0].from, RunState::Streaming);
        assert_eq!(transitions[0].to, RunState::ContinuingModel);
        assert_eq!(transitions[1].from, RunState::ContinuingModel);
        assert_eq!(transitions[1].to, RunState::Checkpointing);
        assert_eq!(transitions[2].from, RunState::Checkpointing);
        assert_eq!(transitions[2].to, RunState::Completed);
    }

    #[test]
    fn done_length_from_calling_model_walks_to_completed() {
        // De CallingModel, são 3 transições: CallingModel →
        // ContinuingModel (MessageComplete) → Checkpointing →
        // Completed.
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Length,
        };
        let transitions = run_state_for_event(RunState::CallingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[0].from, RunState::CallingModel);
        assert_eq!(transitions[0].to, RunState::ContinuingModel);
        assert_eq!(transitions[1].from, RunState::ContinuingModel);
        assert_eq!(transitions[1].to, RunState::Checkpointing);
        assert_eq!(transitions[2].from, RunState::Checkpointing);
        assert_eq!(transitions[2].to, RunState::Completed);
    }

    #[test]
    fn done_length_from_continuing_model_walks_to_completed() {
        // De ContinuingModel, são 2 transições: ContinuingModel →
        // Checkpointing → Completed.
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Length,
        };
        let transitions = run_state_for_event(RunState::ContinuingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].from, RunState::ContinuingModel);
        assert_eq!(transitions[0].to, RunState::Checkpointing);
        assert_eq!(transitions[1].from, RunState::Checkpointing);
        assert_eq!(transitions[1].to, RunState::Completed);
    }

    #[test]
    fn delta_from_continuing_model_starts_new_round() {
        // Início de nova rodada (após processar tool_result). O
        // aresta 25 da Etapa 2 fecha o portão: `ContinuingModel +
        // FirstToken → Streaming`. Antes da Etapa 2, o
        // `state_mapping` original mapeava `Delta → Streaming`
        // direto (sem consultar `apply_transition`).
        let ev = StreamEvent::Delta {
            content: "ok".to_string(),
        };
        let transitions = run_state_for_event(RunState::ContinuingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::Streaming);
    }

    #[test]
    fn done_toolcalls_is_noop_state_change() {
        // `Done { ToolCalls }` é o fim do stream — o `ToolCall`
        // event já chegou antes e disparou `ToolCallEmitted →
        // WaitingToolCall`. O `Done` em si é no-op (estado já
        // correto).
        let ev = StreamEvent::Done {
            stop_reason: StopReason::ToolCalls,
        };
        // Se o estado já é `WaitingToolCall`, Done não muda.
        let transitions = run_state_for_event(RunState::WaitingToolCall, &ev).unwrap();
        assert!(transitions.is_empty());
        // Se o estado é `Streaming` (modelo emitiu tool_call
        // inline antes do Done, sem ToolCall event separado), o
        // Done também é no-op — quem muda o estado é o ToolCall
        // event que chegou junto.
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert!(transitions.is_empty());
    }

    #[test]
    fn done_error_from_streaming_transitions_to_failed() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Error,
        };
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::Failed);
    }

    #[test]
    fn done_stop_from_invalid_state_is_rejected() {
        // Se o provider manda Done { Stop } antes de a gente ter ido
        // pra `Streaming` ou `CallingModel` (ex.: começou em `Created`),
        // o portão rejeita com InvalidTransition.
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Stop,
        };
        let err = run_state_for_event(RunState::Created, &ev).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    // ---- Error / Cancelled -----------------------------------------------

    #[test]
    fn error_event_from_streaming_transitions_to_failed() {
        let ev = StreamEvent::Error(ProviderError::auth());
        let transitions = run_state_for_event(RunState::Streaming, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::Failed);
    }

    #[test]
    fn cancelled_event_from_calling_model_transitions_to_cancelled() {
        let ev = StreamEvent::Cancelled;
        let transitions = run_state_for_event(RunState::CallingModel, &ev).unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].to, RunState::Cancelled);
    }

    // ---- Portão rejeita de terminal --------------------------------------

    #[test]
    fn from_terminal_is_rejected() {
        // De `Completed` (terminal) o portão rejeita imediatamente,
        // mesmo o evento sendo global.
        let ev = StreamEvent::Delta {
            content: "x".to_string(),
        };
        let err = run_state_for_event(RunState::Completed, &ev).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::FromTerminal {
                from: RunState::Completed
            }
        ));
    }
}
