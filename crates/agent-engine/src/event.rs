//! Eventos que causam transições no `Run`.
//!
//! Cada [`RunEventKind`] é uma **causa** determinística: dado o estado
//! atual e o evento, a função [`super::transition::apply_transition`]
//! calcula o próximo estado (ou devolve erro). A enum é fechada —
//! adicionar uma variante exige ADR, atualização do spec
//! `agent-state-machine.md` e entrada na tabela
//! [`super::transition::TRANSITIONS`].
//!
//! Há duas classes de eventos:
//!
//! - **Eventos estruturais**: cada um mapeia uma aresta específica da
//!   tabela de transições (ex.: `Enqueue` só sai de `Created`).
//! - **Eventos globais** (`UserPause`, `UserCancel`,
//!   `WatchdogTimeout`, `AppCrashRecovery`, `UnrecoverableError`): podem
//!   partir de **qualquer** estado não-terminal. A tabela
//!   [`super::transition::GLOBAL_TRANSITIONS`] codifica isso separado
//!   das arestas estruturais.

use chrono::{DateTime, Utc};
use frederico_core::RunId;
use serde::{Deserialize, Serialize};

use crate::error::RunEventKindParseError;

/// Causa de uma transição. Discriminada — cada variante tem um nome em
/// `snake_case` para serialização (journal, logs, `serde_json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventKind {
    // ---- Eventos estruturais (arestas específicas) --------------------
    /// Executor enfileirou o `Run` (created → queued).
    Enqueue,
    /// Escalonador pegou o `Run` da fila (queued → preparing_context).
    Dequeue,
    /// Contexto da conversa está pronto (preparing_context → retrieving_memory).
    ContextReady,
    /// Recuperação de memória terminou, com ou sem resultados
    /// (retrieving_memory → validating_capabilities).
    MemoryDone,
    /// Modelo + ferramentas + permissões validados
    /// (validating_capabilities → calling_model).
    CapabilitiesOk,
    /// Primeiro token chegou (calling_model → streaming).
    FirstToken,
    /// Modelo pediu uma ferramenta (streaming → waiting_tool_call).
    ToolCallEmitted,
    /// Modelo terminou a resposta sem tool_call
    /// (streaming/calling_model → continuing_model).
    MessageComplete,
    /// Executor vai validar o tool_call (waiting_tool_call → validating_tool_call).
    ValidateCall,
    /// tool_call exige aprovação do usuário
    /// (validating_tool_call → waiting_user_approval).
    ApprovalRequired,
    /// tool_call aprovado (ou não precisava de aprovação)
    /// (validating_tool_call → executing_tool).
    ApprovalGrantedOrNotRequired,
    /// Ferramenta retornou (executing_tool → validating_tool_result).
    ToolReturned,
    /// Resultado validado (validating_tool_result → continuing_model).
    ResultValid,
    /// Próxima iteração do loop (continuing_model → calling_model
    /// ou retrying → calling_model).
    NextIteration,
    /// Modelo pediu geração de artefato (continuing_model → generating_artifact).
    ArtifactRequested,
    /// Arquivo foi escrito (generating_artifact → validating_artifact).
    ArtifactEmitted,
    /// Artefato validado (validating_artifact → checkpointing).
    ArtifactValid,
    /// Artefato inválido (validating_artifact → retrying).
    ArtifactInvalid,
    /// Checkpoint foi persistido (checkpointing → completed).
    CheckpointPersisted,
    /// Usuário retomou do pause (paused → preparing_context).
    Resume,

    // ---- Eventos globais (qualquer não-terminal) ----------------------
    /// Usuário pediu pausa.
    UserPause,
    /// Usuário pediu cancelamento.
    UserCancel,
    /// Watchdog matou por falta de heartbeat.
    WatchdogTimeout,
    /// App foi recuperado de crash e o `Run` estava pendente.
    AppCrashRecovery,
    /// Erro estrutural do qual o executor não sabe se recuperar.
    UnrecoverableError,
}

impl RunEventKind {
    /// Forma em `snake_case` canônica para serialização (SQLite,
    /// `serde_json`, logs).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enqueue => "enqueue",
            Self::Dequeue => "dequeue",
            Self::ContextReady => "context_ready",
            Self::MemoryDone => "memory_done",
            Self::CapabilitiesOk => "capabilities_ok",
            Self::FirstToken => "first_token",
            Self::ToolCallEmitted => "tool_call_emitted",
            Self::MessageComplete => "message_complete",
            Self::ValidateCall => "validate_call",
            Self::ApprovalRequired => "approval_required",
            Self::ApprovalGrantedOrNotRequired => "approval_granted_or_not_required",
            Self::ToolReturned => "tool_returned",
            Self::ResultValid => "result_valid",
            Self::NextIteration => "next_iteration",
            Self::ArtifactRequested => "artifact_requested",
            Self::ArtifactEmitted => "artifact_emitted",
            Self::ArtifactValid => "artifact_valid",
            Self::ArtifactInvalid => "artifact_invalid",
            Self::CheckpointPersisted => "checkpoint_persisted",
            Self::Resume => "resume",
            Self::UserPause => "user_pause",
            Self::UserCancel => "user_cancel",
            Self::WatchdogTimeout => "watchdog_timeout",
            Self::AppCrashRecovery => "app_crash_recovery",
            Self::UnrecoverableError => "unrecoverable_error",
        }
    }

    /// Total de variantes. Igual à lista do spec.
    pub const COUNT: usize = 25;

    /// `true` se o evento é um dos 5 globais (parte de qualquer estado
    /// não-terminal). Usado pela função `apply_transition` para decidir
    /// se consulta a tabela estrutural ou a global.
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(
            self,
            Self::UserPause
                | Self::UserCancel
                | Self::WatchdogTimeout
                | Self::AppCrashRecovery
                | Self::UnrecoverableError
        )
    }
}

impl std::fmt::Display for RunEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunEventKind {
    type Err = RunEventKindParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "enqueue" => Ok(Self::Enqueue),
            "dequeue" => Ok(Self::Dequeue),
            "context_ready" => Ok(Self::ContextReady),
            "memory_done" => Ok(Self::MemoryDone),
            "capabilities_ok" => Ok(Self::CapabilitiesOk),
            "first_token" => Ok(Self::FirstToken),
            "tool_call_emitted" => Ok(Self::ToolCallEmitted),
            "message_complete" => Ok(Self::MessageComplete),
            "validate_call" => Ok(Self::ValidateCall),
            "approval_required" => Ok(Self::ApprovalRequired),
            "approval_granted_or_not_required" => Ok(Self::ApprovalGrantedOrNotRequired),
            "tool_returned" => Ok(Self::ToolReturned),
            "result_valid" => Ok(Self::ResultValid),
            "next_iteration" => Ok(Self::NextIteration),
            "artifact_requested" => Ok(Self::ArtifactRequested),
            "artifact_emitted" => Ok(Self::ArtifactEmitted),
            "artifact_valid" => Ok(Self::ArtifactValid),
            "artifact_invalid" => Ok(Self::ArtifactInvalid),
            "checkpoint_persisted" => Ok(Self::CheckpointPersisted),
            "resume" => Ok(Self::Resume),
            "user_pause" => Ok(Self::UserPause),
            "user_cancel" => Ok(Self::UserCancel),
            "watchdog_timeout" => Ok(Self::WatchdogTimeout),
            "app_crash_recovery" => Ok(Self::AppCrashRecovery),
            "unrecoverable_error" => Ok(Self::UnrecoverableError),
            other => Err(RunEventKindParseError(other.to_string())),
        }
    }
}

/// Um evento registrado no journal de um `Run`. `seq` é monotônico por
/// `run_id` — a constraint `UNIQUE(run_id, seq)` no SQLite (a entrar na
/// Etapa 4) garante isso do lado do banco.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    /// `Run` ao qual o evento pertence.
    pub run_id: RunId,
    /// Sequência monotônica por `run_id` (1, 2, 3, ...).
    pub seq: u64,
    /// Timestamp do momento em que o evento foi emitido pelo motor.
    pub ts: DateTime<Utc>,
    /// Variante que causou a transição (ou foi gravada sem
    /// transição, como o `UserCancel` no meio de um checkpoint).
    pub kind: RunEventKind,
    /// Payload opaco (eventos diferentes carregam dados diferentes:
    /// `FirstToken` carrega o primeiro pedaço, `ToolCallEmitted` carrega
    /// o `tool_call_id`, `UserCancel` é vazio, etc.). Mantido como
    /// `serde_json::Value` para que o journal aceite qualquer evento
    /// sem migração por novo evento — `REGRAS §1.3` diz que mudança de
    /// contrato exige atualização de spec no mesmo commit, e adicionar
    /// uma variante de `RunEventKind` já passa por esse gate.
    pub payload: serde_json::Value,
}

impl RunEvent {
    /// Constrói um evento com `ts = Utc::now()` e `payload =
    /// serde_json::Value::Null`. Use [`RunEvent::with_payload`] para
    /// anexar dados.
    #[must_use]
    pub fn new(run_id: RunId, seq: u64, kind: RunEventKind) -> Self {
        Self {
            run_id,
            seq,
            ts: Utc::now(),
            kind,
            payload: serde_json::Value::Null,
        }
    }

    /// Substitui o payload do evento. Encadeável.
    #[must_use]
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_matches_spec() {
        // 20 eventos estruturais + 5 globais = 25 (ver spec
        // `agent-state-machine.md` §"Contratos"). Se a enum ganhar
        // uma variante, `COUNT` precisa ser atualizado — o teste
        // grita em vez de mentir em silêncio.
        assert_eq!(RunEventKind::COUNT, 25);
    }

    #[test]
    fn global_events_are_exactly_the_five() {
        let globals = [
            RunEventKind::UserPause,
            RunEventKind::UserCancel,
            RunEventKind::WatchdogTimeout,
            RunEventKind::AppCrashRecovery,
            RunEventKind::UnrecoverableError,
        ];
        for g in globals {
            assert!(g.is_global(), "{g} deveria ser global");
        }
        // Cada um dos outros 20 é estrutural.
        for kind in [
            RunEventKind::Enqueue,
            RunEventKind::Dequeue,
            RunEventKind::ContextReady,
            RunEventKind::MemoryDone,
            RunEventKind::CapabilitiesOk,
            RunEventKind::FirstToken,
            RunEventKind::ToolCallEmitted,
            RunEventKind::MessageComplete,
            RunEventKind::ValidateCall,
            RunEventKind::ApprovalRequired,
            RunEventKind::ApprovalGrantedOrNotRequired,
            RunEventKind::ToolReturned,
            RunEventKind::ResultValid,
            RunEventKind::NextIteration,
            RunEventKind::ArtifactRequested,
            RunEventKind::ArtifactEmitted,
            RunEventKind::ArtifactValid,
            RunEventKind::ArtifactInvalid,
            RunEventKind::CheckpointPersisted,
            RunEventKind::Resume,
        ] {
            assert!(!kind.is_global(), "{kind} não deveria ser global");
        }
    }

    #[test]
    fn display_roundtrips_via_from_str() {
        for kind in [
            RunEventKind::Enqueue,
            RunEventKind::Dequeue,
            RunEventKind::UserPause,
            RunEventKind::UserCancel,
            RunEventKind::WatchdogTimeout,
            RunEventKind::AppCrashRecovery,
            RunEventKind::UnrecoverableError,
        ] {
            let s = kind.to_string();
            let back: RunEventKind = s.parse().expect("parse aceita Display");
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn run_event_new_defaults_payload_to_null() {
        let run = RunId::new();
        let ev = RunEvent::new(run, 1, RunEventKind::Enqueue);
        assert_eq!(ev.run_id, run);
        assert_eq!(ev.seq, 1);
        assert_eq!(ev.kind, RunEventKind::Enqueue);
        assert_eq!(ev.payload, serde_json::Value::Null);
    }

    #[test]
    fn run_event_with_payload_replaces_null() {
        let run = RunId::new();
        let payload = serde_json::json!({"tool_call_id": "abc"});
        let ev = RunEvent::new(run, 2, RunEventKind::ToolCallEmitted).with_payload(payload.clone());
        assert_eq!(ev.payload, payload);
    }
}
