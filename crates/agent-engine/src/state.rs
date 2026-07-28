//! Os 23 estados do `Run` da máquina [`PROMPT MESTRE` §6.1].
//!
//! A enum é **fechada** — adicionar uma variante exige ADR e atualização
//! do spec [`agent-state-machine.md`] e da tabela [`super::transition::TRANSITIONS`].
//!
//! [`PROMPT MESTRE` §6.1]: https://example.invalid/prompt-mestre (referência textual; spec local em `docs/architecture/agent-state-machine.md`)
//! [`agent-state-machine.md`]: ../../docs/architecture/agent-state-machine.md

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::RunStateParseError;

/// Os 22 estados pelos quais um `Run` passa durante sua vida.
///
/// Ordem: igual à do `PROMPT MESTRE` §6.1 e ao spec `agent-state-machine.md` §"Estados".
/// Ver `RunState::COUNT` para a nota sobre a divergência original
/// (spec dizia 23, lista tem 22) reconciliada na Etapa 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// `Run` foi criado mas ainda não foi enfileirado. Estado inicial
    /// obrigatório — toda execução persiste aqui antes de qualquer
    /// I/O.
    Created,
    /// Em fila, aguardando o escalonador escolher.
    Queued,
    /// Construindo o contexto da conversa (últimas N mensagens, system
    /// prompt, etc.).
    PreparingContext,
    /// Buscando memórias relevantes (escopo da conversa, projeto,
    /// usuário). Pode ser pulado em 0ms se nada recuperado.
    RetrievingMemory,
    /// Validando que o modelo, ferramentas e permissões batem com o
    /// que o `Run` precisa.
    ValidatingCapabilities,
    /// Fazendo a chamada HTTP inicial ao provedor (request
    /// `chat.completions` / `messages`).
    CallingModel,
    /// Stream aberto; recebendo deltas de tokens. Pode transicionar
    /// para `WaitingToolCall` (tool_call emitido) ou
    /// `ContinuingModel` (modelo terminou sem tool_call).
    Streaming,
    /// Modelo pediu uma ferramenta; aguardando o validador do motor
    /// pegar o `tool_call`.
    WaitingToolCall,
    /// Validador aplicando os 10 passos do
    /// [`tool-registry-specification.md`] (versão, saúde, permissões,
    /// schema, jail, etc.).
    ///
    /// [`tool-registry-specification.md`]: ../../docs/architecture/tool-registry-specification.md
    ValidatingToolCall,
    /// Pediu uma ferramenta que exige aprovação do usuário; em fila
    /// até o usuário responder.
    WaitingUserApproval,
    /// Ferramenta aprovada (ou não precisava de aprovação); em
    /// execução. Pode ser in-process (Etapa 2) ou via worker
    /// sidecar (Fase 5+).
    ExecutingTool,
    /// Ferramenta retornou; o motor está validando o
    /// `ToolResult` (formato, completude, jail).
    ValidatingToolResult,
    /// Resultado válido; voltando a chamar o modelo com a resposta da
    /// ferramenta no contexto.
    ContinuingModel,
    /// Modelo pediu geração de artefato (PDF, DOCX, XLSX). A Etapa 4
    /// delega ao `document-engine`.
    GeneratingArtifact,
    /// Arquivo foi escrito no disco; o validador confere
    /// existência, formato, e (Fase 5) regras de auditoria.
    ValidatingArtifact,
    /// Estado explícito em que o checkpoint do `Run` está sendo
    /// serializado. Garante que toda execução tem um ponto
    /// recuperável antes de fechar.
    Checkpointing,
    /// Modelo gerou artefato inválido; o motor vai pedir de novo sem
    /// criar um novo `Run` (preserva histórico e artefatos já
    /// produzidos, spec §6.3 item 8).
    Retrying,
    /// Usuário pausou; recuperável via `Resume`.
    Paused,
    /// Estado terminal: execução fechou com sucesso.
    Completed,
    /// Estado terminal: erro irrecuperável.
    Failed,
    /// Estado terminal: usuário cancelou.
    Cancelled,
    /// Estado terminal: watchdog matou por falta de heartbeat, ou
    /// recuperação de crash do app marcou o `Run` como precisando
    /// de intervenção humana.
    Interrupted,
}

impl RunState {
    /// Total de variantes. Útil para testes parametrizados e para o
    /// teste de cobertura da view `runs_with_status` em `storage`.
    ///
    /// **Nota:** o spec `agent-state-machine.md` (estado `especificado`)
    /// dizia "23 estados" no preâmbulo mas listava 22. A Etapa 1 da
    /// Fase 3 reconciliou o spec com a contagem real (22) e adicionou
    /// nota "Alterado em relação ao plano original: spec dizia 23,
    /// contagem real é 22 — recontagem na Etapa 1 em 2026-07-27".
    /// A `CHECK` constraint do SQLite e a view `runs_with_status` usam
    /// este `COUNT` como número canônico.
    pub const COUNT: usize = 22;

    /// Estados terminais segundo o spec: `completed`, `failed`,
    /// `cancelled`, `interrupted`. Nenhuma transição sai de um estado
    /// terminal — `apply_transition` rejeita antes mesmo de olhar a
    /// tabela.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Forma em `snake_case` canônica para serialização (SQLite,
    /// `serde_json`, logs). Usada pela migração `0003_runs_and_checkpoints.sql`
    /// na `CHECK` constraint e pela view `runs_with_status`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::PreparingContext => "preparing_context",
            Self::RetrievingMemory => "retrieving_memory",
            Self::ValidatingCapabilities => "validating_capabilities",
            Self::CallingModel => "calling_model",
            Self::Streaming => "streaming",
            Self::WaitingToolCall => "waiting_tool_call",
            Self::ValidatingToolCall => "validating_tool_call",
            Self::WaitingUserApproval => "waiting_user_approval",
            Self::ExecutingTool => "executing_tool",
            Self::ValidatingToolResult => "validating_tool_result",
            Self::ContinuingModel => "continuing_model",
            Self::GeneratingArtifact => "generating_artifact",
            Self::ValidatingArtifact => "validating_artifact",
            Self::Checkpointing => "checkpointing",
            Self::Retrying => "retrying",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    /// Todas as variantes, na ordem do spec. Útil em testes
    /// parametrizados e na `CHECK` constraint do SQLite (gerada por
    /// script — REGRAS §1.9).
    #[must_use]
    pub const fn all() -> [Self; Self::COUNT] {
        [
            Self::Created,
            Self::Queued,
            Self::PreparingContext,
            Self::RetrievingMemory,
            Self::ValidatingCapabilities,
            Self::CallingModel,
            Self::Streaming,
            Self::WaitingToolCall,
            Self::ValidatingToolCall,
            Self::WaitingUserApproval,
            Self::ExecutingTool,
            Self::ValidatingToolResult,
            Self::ContinuingModel,
            Self::GeneratingArtifact,
            Self::ValidatingArtifact,
            Self::Checkpointing,
            Self::Retrying,
            Self::Paused,
            Self::Completed,
            Self::Failed,
            Self::Cancelled,
            Self::Interrupted,
        ]
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunState {
    type Err = RunStateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(Self::Created),
            "queued" => Ok(Self::Queued),
            "preparing_context" => Ok(Self::PreparingContext),
            "retrieving_memory" => Ok(Self::RetrievingMemory),
            "validating_capabilities" => Ok(Self::ValidatingCapabilities),
            "calling_model" => Ok(Self::CallingModel),
            "streaming" => Ok(Self::Streaming),
            "waiting_tool_call" => Ok(Self::WaitingToolCall),
            "validating_tool_call" => Ok(Self::ValidatingToolCall),
            "waiting_user_approval" => Ok(Self::WaitingUserApproval),
            "executing_tool" => Ok(Self::ExecutingTool),
            "validating_tool_result" => Ok(Self::ValidatingToolResult),
            "continuing_model" => Ok(Self::ContinuingModel),
            "generating_artifact" => Ok(Self::GeneratingArtifact),
            "validating_artifact" => Ok(Self::ValidatingArtifact),
            "checkpointing" => Ok(Self::Checkpointing),
            "retrying" => Ok(Self::Retrying),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(RunStateParseError(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_matches_spec() {
        // O spec `agent-state-machine.md` (estado `especificado`) diz
        // "23 estados" no preâmbulo mas lista 22. A Etapa 1 corrigiu
        // a contagem para 22. Se a enum ganhar ou perder uma
        // variante, esse teste falha — proteção barata contra
        // regressão silenciosa.
        assert_eq!(RunState::all().len(), 22);
    }

    #[test]
    fn terminal_states_are_the_four_documented() {
        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Failed.is_terminal());
        assert!(RunState::Cancelled.is_terminal());
        assert!(RunState::Interrupted.is_terminal());
        for state in RunState::all() {
            if matches!(
                state,
                RunState::Completed
                    | RunState::Failed
                    | RunState::Cancelled
                    | RunState::Interrupted
            ) {
                continue;
            }
            assert!(
                !state.is_terminal(),
                "{state} não-terminal foi marcado como terminal"
            );
        }
    }

    #[test]
    fn display_roundtrips_via_from_str() {
        for state in RunState::all() {
            let s = state.to_string();
            let back: RunState = s.parse().expect("parse deve aceitar o Display");
            assert_eq!(state, back);
        }
    }

    #[test]
    fn from_str_rejects_garbage() {
        let err = "not-a-state".parse::<RunState>().unwrap_err();
        assert_eq!(err.0, "not-a-state");
    }

    #[test]
    fn as_str_matches_snake_case_in_spec() {
        // Pinos contra typos: cada variante tem um nome canônico.
        assert_eq!(RunState::PreparingContext.as_str(), "preparing_context");
        assert_eq!(
            RunState::ValidatingCapabilities.as_str(),
            "validating_capabilities"
        );
        assert_eq!(RunState::CallingModel.as_str(), "calling_model");
        assert_eq!(RunState::WaitingToolCall.as_str(), "waiting_tool_call");
        assert_eq!(
            RunState::WaitingUserApproval.as_str(),
            "waiting_user_approval"
        );
        assert_eq!(RunState::ExecutingTool.as_str(), "executing_tool");
        assert_eq!(
            RunState::ValidatingToolResult.as_str(),
            "validating_tool_result"
        );
        assert_eq!(RunState::ContinuingModel.as_str(), "continuing_model");
        assert_eq!(RunState::GeneratingArtifact.as_str(), "generating_artifact");
        assert_eq!(RunState::ValidatingArtifact.as_str(), "validating_artifact");
        assert_eq!(RunState::Checkpointing.as_str(), "checkpointing");
    }
}
