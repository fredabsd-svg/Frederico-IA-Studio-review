//! Tabela de transições e a função [`apply_transition`].
//!
//! A tabela codifica o grafo do spec `agent-state-machine.md` §"Tabela
//! de transições" e divide-se em duas listas:
//!
//! - [`TRANSITIONS`] — arestas **estruturais**: cada uma casa um par
//!   (`from`, `event`) com um único `to`. Adicionar uma aresta exige
//!   atualizar o spec e o teste de par correspondente.
//! - [`GLOBAL_TRANSITIONS`] — arestas **globais**: eventos que
//!   partem de qualquer estado não-terminal
//!   (`UserPause`/`UserCancel`/`WatchdogTimeout`/`AppCrashRecovery`/
//!   `UnrecoverableError`). A `apply_transition` consulta essa lista
//!   apenas se a busca estrutural falhar — assim, "qualquer não-
//!   terminal" é codificado uma vez, sem ter que enumerar 19 × 5 = 95
//!   combinações.
//!
//! A invariante **"transições inválidas são erro estruturado, nunca
//! ignoradas"** (spec `agent-state-machine.md` §"Invariantes") é
//! garantida por [`apply_transition`] — qualquer par (`from`, `event`)
//! sem aresta correspondente devolve [`TransitionError`].

use crate::error::TransitionError;
use crate::event::RunEventKind;
use crate::state::RunState;

/// Uma aresta da tabela de transições. `from` é o estado atual, `to` é
/// o estado resultante quando o `event` ocorre. Determinístico: dado
/// (`from`, `event`), o `to` é único.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// Estado atual do `Run` (origem da aresta).
    pub from: RunState,
    /// Evento que causa a transição.
    pub event: RunEventKind,
    /// Estado resultante.
    pub to: RunState,
}

impl Transition {
    /// Constrói uma aresta.
    pub const fn new(from: RunState, event: RunEventKind, to: RunState) -> Self {
        Self { from, event, to }
    }
}

/// Tabela de arestas estruturais. Espelha o spec
/// `agent-state-machine.md` §"Tabela de transições" mais três arestas
/// implícitas que o spec menciona mas não tabula:
///
/// - `checkpointing → completed via CheckpointPersisted` (o spec §"Checkpoints"
///   lista `checkpointing` como estado, e o ponto "depois de gerar
///   artefato" persiste antes de fechar; o `completed` vem após).
/// - `retrying → calling_model via NextIteration` (a invariante
///   "`retrying` preserva o `RunId`" implica que o loop volta a chamar
///   o modelo sem criar novo `Run`).
/// - `paused → preparing_context via Resume` (spec §"Invariantes" diz
///   "`paused` recupera com `resume`"; o estado de retomada é o
///   começo do pipeline).
///
/// Se alguma dessas três arestas implícitas divergir do que a Etapa 4
/// (integração) descobrir ser o comportamento real do executor, este
/// arquivo e o spec são atualizados no mesmo commit.
pub const TRANSITIONS: &[Transition] = &[
    // 1. created → queued
    Transition::new(RunState::Created, RunEventKind::Enqueue, RunState::Queued),
    // 2. queued → preparing_context
    Transition::new(
        RunState::Queued,
        RunEventKind::Dequeue,
        RunState::PreparingContext,
    ),
    // 3. preparing_context → retrieving_memory
    Transition::new(
        RunState::PreparingContext,
        RunEventKind::ContextReady,
        RunState::RetrievingMemory,
    ),
    // 4. retrieving_memory → validating_capabilities (pode ser pulado
    // em 0ms se nada recuperado, mas a aresta existe)
    Transition::new(
        RunState::RetrievingMemory,
        RunEventKind::MemoryDone,
        RunState::ValidatingCapabilities,
    ),
    // 5. validating_capabilities → calling_model
    Transition::new(
        RunState::ValidatingCapabilities,
        RunEventKind::CapabilitiesOk,
        RunState::CallingModel,
    ),
    // 6. calling_model → streaming (ou continuing_model se não-streaming;
    // a aresta não-streaming fica como `message_complete` partindo de
    // `calling_model` — ver aresta 22 abaixo)
    Transition::new(
        RunState::CallingModel,
        RunEventKind::FirstToken,
        RunState::Streaming,
    ),
    // 7. streaming → waiting_tool_call
    Transition::new(
        RunState::Streaming,
        RunEventKind::ToolCallEmitted,
        RunState::WaitingToolCall,
    ),
    // 8. streaming → continuing_model
    Transition::new(
        RunState::Streaming,
        RunEventKind::MessageComplete,
        RunState::ContinuingModel,
    ),
    // 9. waiting_tool_call → validating_tool_call
    Transition::new(
        RunState::WaitingToolCall,
        RunEventKind::ValidateCall,
        RunState::ValidatingToolCall,
    ),
    // 10. validating_tool_call → waiting_user_approval
    Transition::new(
        RunState::ValidatingToolCall,
        RunEventKind::ApprovalRequired,
        RunState::WaitingUserApproval,
    ),
    // 11. validating_tool_call → executing_tool
    Transition::new(
        RunState::ValidatingToolCall,
        RunEventKind::ApprovalGrantedOrNotRequired,
        RunState::ExecutingTool,
    ),
    // 12. executing_tool → validating_tool_result
    Transition::new(
        RunState::ExecutingTool,
        RunEventKind::ToolReturned,
        RunState::ValidatingToolResult,
    ),
    // 13. validating_tool_result → continuing_model
    Transition::new(
        RunState::ValidatingToolResult,
        RunEventKind::ResultValid,
        RunState::ContinuingModel,
    ),
    // 14. continuing_model → calling_model (próxima iteração)
    Transition::new(
        RunState::ContinuingModel,
        RunEventKind::NextIteration,
        RunState::CallingModel,
    ),
    // 15. continuing_model → generating_artifact
    Transition::new(
        RunState::ContinuingModel,
        RunEventKind::ArtifactRequested,
        RunState::GeneratingArtifact,
    ),
    // 16. generating_artifact → validating_artifact
    Transition::new(
        RunState::GeneratingArtifact,
        RunEventKind::ArtifactEmitted,
        RunState::ValidatingArtifact,
    ),
    // 17. validating_artifact → checkpointing (sucesso)
    Transition::new(
        RunState::ValidatingArtifact,
        RunEventKind::ArtifactValid,
        RunState::Checkpointing,
    ),
    // 18. validating_artifact → retrying (falha, ver spec §19.6)
    Transition::new(
        RunState::ValidatingArtifact,
        RunEventKind::ArtifactInvalid,
        RunState::Retrying,
    ),
    // 19. checkpointing → completed (persistência do checkpoint fechou)
    Transition::new(
        RunState::Checkpointing,
        RunEventKind::CheckpointPersisted,
        RunState::Completed,
    ),
    // 20. retrying → calling_model (preserva RunId, volta a chamar o modelo)
    Transition::new(
        RunState::Retrying,
        RunEventKind::NextIteration,
        RunState::CallingModel,
    ),
    // 21. paused → preparing_context (resume volta ao começo do pipeline)
    Transition::new(
        RunState::Paused,
        RunEventKind::Resume,
        RunState::PreparingContext,
    ),
    // 22. calling_model → continuing_model (modelo não-streaming terminou)
    // — aresta implícita coberta pelo spec §"calling_model" ("ou
    // continuing_model se não-streaming"). Mantida explícita pra não
    // deixar `CallingModel` sem saída alternativa.
    Transition::new(
        RunState::CallingModel,
        RunEventKind::MessageComplete,
        RunState::ContinuingModel,
    ),
];

/// Tabela de arestas globais: cada evento aqui listado parte de
/// **qualquer** estado não-terminal. A `apply_transition` consulta
/// essa lista apenas se a busca em `TRANSITIONS` falhar.
///
/// Os 4 destinos terminais são os do spec §"Invariantes":
///
/// - `user_cancel → cancelled`
/// - `watchdog_timeout → interrupted`
/// - `app_crash_recovery → interrupted`
/// - `unrecoverable_error → failed`
///
/// E o estado de pausa: `user_pause → paused`.
pub const GLOBAL_TRANSITIONS: &[(RunState, RunEventKind)] = &[
    (RunState::Paused, RunEventKind::UserPause),
    (RunState::Cancelled, RunEventKind::UserCancel),
    (RunState::Interrupted, RunEventKind::WatchdogTimeout),
    (RunState::Interrupted, RunEventKind::AppCrashRecovery),
    (RunState::Failed, RunEventKind::UnrecoverableError),
];

/// Aplica uma transição: dado o estado atual e o evento, devolve o
/// próximo estado ou erro estruturado.
///
/// Ordem da consulta:
///
/// 1. Se `from` é terminal, devolve [`TransitionError::FromTerminal`]
///    imediatamente — terminais são imutáveis.
/// 2. Procura o par (`from`, `event`) em [`TRANSITIONS`]. Se achar,
///    devolve o `to` correspondente.
/// 3. Se o evento é global, procura o evento em [`GLOBAL_TRANSITIONS`].
///    Se achar, devolve o `to` global (independente do `from`, contanto
///    que não-terminal).
/// 4. Caso contrário, devolve [`TransitionError::InvalidTransition`]
///    com `to: None`.
///
/// A função é **pura** — não toca SQLite, não dispara side-effects.
/// Quem monta a transação SQL (`BEGIN IMMEDIATE; ...; COMMIT;`) é o
/// `RunExecutor` da Etapa 4. Aqui, a suíte de testes cobre par a par.
pub fn apply_transition(from: RunState, event: RunEventKind) -> Result<RunState, TransitionError> {
    // 1. Estados terminais não transitam.
    if from.is_terminal() {
        return Err(TransitionError::FromTerminal { from });
    }

    // 2. Aresta estrutural.
    if let Some(target) = lookup_structural(from, event) {
        return Ok(target);
    }

    // 3. Aresta global (só se o evento for global).
    if event.is_global() {
        if let Some((target, _)) = GLOBAL_TRANSITIONS
            .iter()
            .find(|(_, ev)| *ev == event)
            .copied()
        {
            return Ok(target);
        }
    }

    // 4. Sem aresta — erro estruturado.
    Err(TransitionError::InvalidTransition {
        from,
        event,
        to: None,
    })
}

fn lookup_structural(from: RunState, event: RunEventKind) -> Option<RunState> {
    TRANSITIONS
        .iter()
        .find(|t| t.from == from && t.event == event)
        .map(|t| t.to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_table_has_no_duplicate_from_event_pairs() {
        // Dois pares (from, event) com mesmo `from` e mesmo `event` mas
        // `to` diferente seria ambíguo — `apply_transition` sempre
        // acharia o primeiro da lista. Pega o erro de digitação antes
        // que vire bug em produção.
        use std::collections::HashSet;
        let mut seen: HashSet<(RunState, RunEventKind)> = HashSet::new();
        for t in TRANSITIONS {
            let key = (t.from, t.event);
            assert!(
                seen.insert(key),
                "par duplicado em TRANSITIONS: from={} event={}",
                t.from,
                t.event
            );
        }
    }

    #[test]
    fn global_transitions_terminal_targets() {
        // Cada destino de uma aresta global é terminal.
        for (to, ev) in GLOBAL_TRANSITIONS {
            assert!(
                to.is_terminal() || *to == RunState::Paused,
                "{ev} → {to} deveria levar a estado terminal (ou paused), não a estado de execução"
            );
        }
    }

    #[test]
    fn global_transitions_only_list_global_events() {
        for (_to, ev) in GLOBAL_TRANSITIONS {
            assert!(
                ev.is_global(),
                "{ev} listado em GLOBAL_TRANSITIONS mas não é global"
            );
        }
    }

    #[test]
    fn from_completed_is_rejected() {
        let err = apply_transition(RunState::Completed, RunEventKind::Enqueue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::FromTerminal {
                from: RunState::Completed
            }
        ));
    }

    #[test]
    fn from_failed_is_rejected() {
        let err = apply_transition(RunState::Failed, RunEventKind::Enqueue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::FromTerminal {
                from: RunState::Failed
            }
        ));
    }

    #[test]
    fn from_cancelled_is_rejected() {
        let err = apply_transition(RunState::Cancelled, RunEventKind::Enqueue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::FromTerminal {
                from: RunState::Cancelled
            }
        ));
    }

    #[test]
    fn from_interrupted_is_rejected() {
        let err = apply_transition(RunState::Interrupted, RunEventKind::Enqueue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::FromTerminal {
                from: RunState::Interrupted
            }
        ));
    }

    #[test]
    fn from_completed_self_transition_is_rejected() {
        // Tentativa "completed → completed" também é terminal → erro.
        let err = apply_transition(RunState::Completed, RunEventKind::MessageComplete).unwrap_err();
        assert!(matches!(err, TransitionError::FromTerminal { .. }));
    }

    #[test]
    fn structural_aresta_created_to_queued() {
        let next = apply_transition(RunState::Created, RunEventKind::Enqueue).unwrap();
        assert_eq!(next, RunState::Queued);
    }

    #[test]
    fn structural_aresta_queued_to_preparing_context() {
        let next = apply_transition(RunState::Queued, RunEventKind::Dequeue).unwrap();
        assert_eq!(next, RunState::PreparingContext);
    }

    #[test]
    fn structural_aresta_streaming_splits_two_ways() {
        // De `streaming` saem duas arestas: `tool_call_emitted` e
        // `message_complete`. Confirma que a função é determinística
        // para cada evento.
        let via_tool =
            apply_transition(RunState::Streaming, RunEventKind::ToolCallEmitted).unwrap();
        assert_eq!(via_tool, RunState::WaitingToolCall);
        let via_done =
            apply_transition(RunState::Streaming, RunEventKind::MessageComplete).unwrap();
        assert_eq!(via_done, RunState::ContinuingModel);
    }

    #[test]
    fn global_aresta_user_cancel_from_any_non_terminal() {
        // De qualquer um dos 19 estados não-terminais, `user_cancel` →
        // `cancelled`.
        for state in RunState::all() {
            if state.is_terminal() {
                let err = apply_transition(state, RunEventKind::UserCancel).unwrap_err();
                assert!(matches!(err, TransitionError::FromTerminal { .. }));
            } else {
                let next = apply_transition(state, RunEventKind::UserCancel).unwrap();
                assert_eq!(next, RunState::Cancelled, "from {state}");
            }
        }
    }

    #[test]
    fn global_aresta_watchdog_timeout_from_any_non_terminal() {
        for state in RunState::all() {
            if state.is_terminal() {
                continue;
            }
            let next = apply_transition(state, RunEventKind::WatchdogTimeout).unwrap();
            assert_eq!(next, RunState::Interrupted, "from {state}");
        }
    }

    #[test]
    fn global_aresta_unrecoverable_error_from_any_non_terminal() {
        for state in RunState::all() {
            if state.is_terminal() {
                continue;
            }
            let next = apply_transition(state, RunEventKind::UnrecoverableError).unwrap();
            assert_eq!(next, RunState::Failed, "from {state}");
        }
    }

    #[test]
    fn global_aresta_user_pause_from_any_non_terminal() {
        for state in RunState::all() {
            if state.is_terminal() {
                continue;
            }
            let next = apply_transition(state, RunEventKind::UserPause).unwrap();
            assert_eq!(next, RunState::Paused, "from {state}");
        }
    }

    #[test]
    fn global_event_from_terminal_is_rejected() {
        // Mesmo um evento global não fura a barreira terminal.
        let err = apply_transition(RunState::Completed, RunEventKind::UserCancel).unwrap_err();
        assert!(matches!(err, TransitionError::FromTerminal { .. }));
    }

    #[test]
    fn invalid_transition_is_structured_error() {
        // `Created` não tem aresta via `FirstToken`.
        let err = apply_transition(RunState::Created, RunEventKind::FirstToken).unwrap_err();
        match err {
            TransitionError::InvalidTransition { from, event, to } => {
                assert_eq!(from, RunState::Created);
                assert_eq!(event, RunEventKind::FirstToken);
                assert!(to.is_none());
            }
            other => panic!("esperava InvalidTransition, veio {other:?}"),
        }
    }

    #[test]
    fn non_global_event_does_not_match_global_table() {
        // `Enqueue` não é global; de um estado que não tem aresta
        // estrutural via `Enqueue` (ex.: `Streaming`), o resultado é
        // `InvalidTransition`, não um `to` global.
        let err = apply_transition(RunState::Streaming, RunEventKind::Enqueue).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn resume_from_paused_returns_to_preparing_context() {
        let next = apply_transition(RunState::Paused, RunEventKind::Resume).unwrap();
        assert_eq!(next, RunState::PreparingContext);
    }

    #[test]
    fn retrying_via_next_iteration_returns_to_calling_model() {
        let next = apply_transition(RunState::Retrying, RunEventKind::NextIteration).unwrap();
        assert_eq!(next, RunState::CallingModel);
    }

    #[test]
    fn checkpointing_persisted_closes_run() {
        let next =
            apply_transition(RunState::Checkpointing, RunEventKind::CheckpointPersisted).unwrap();
        assert_eq!(next, RunState::Completed);
    }

    #[test]
    fn calling_model_can_finish_without_streaming() {
        // Modelo não-streaming emite `message_complete` direto de
        // `calling_model`.
        let next = apply_transition(RunState::CallingModel, RunEventKind::MessageComplete).unwrap();
        assert_eq!(next, RunState::ContinuingModel);
    }
}
