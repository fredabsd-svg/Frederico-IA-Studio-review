//! Mapping `StreamEvent → RunState` (Fase 3, Etapa 5.x).
//!
//! A cada `StreamEvent` que o [`RunExecutor`] consome, o executor
//! mapeia o evento pra um [`RunState`] granular e persiste a
//! transição numa **única transação** com
//! `runs.last_heartbeat_at` + `runs.last_event_seq` (ver
//! `RunRepo::set_state_and_heartbeat_tx`).
//!
//! ## Por que aqui, não no `agent-engine`?
//!
//! O `agent-engine` é **puro** (não conhece `StreamEvent` nem o
//! `provider-engine` — a enum `RunState` é independente de
//! plataforma). O mapping depende do `provider-engine`, então vive
//! no `execution-engine`, que já depende de ambos.
//!
//! ## Mapeamento (consistente com o spec `agent-state-machine.md`)
//!
//! | `StreamEvent`                  | `RunState`              |
//! | ------------------------------ | ----------------------- |
//! | `Delta`                        | `Streaming`             |
//! | `Usage`                        | `Streaming` (sem mudança) |
//! | `ToolCall`                     | `WaitingToolCall`       |
//! | `ToolResult`                   | `ValidatingToolResult`  |
//! | `Done { stop_reason: Stop }`   | `Completed`             |
//! | `Done { stop_reason: Length }` | `Completed`             |
//! | `Done { stop_reason: ToolCalls }` | `WaitingToolCall`   |
//! | `Done { stop_reason: Error }`  | `Failed`                |
//! | `Error(_)`                     | `Failed`                |
//! | `Cancelled`                    | `Cancelled`             |
//!
//! `None` é retornado para `Usage` (não muda o state — é só
//! atualização de contadores) e o mapping é idempotente (chamar
//! duas vezes com o mesmo `Delta` não muda nada).

use frederico_agent_engine::RunState;
use frederico_provider_engine::{StopReason, StreamEvent};

/// Mapeia um `StreamEvent` pro `RunState` correspondente. `None` se
/// o evento não muda o state (`Usage` é um contador, não uma
/// transição).
#[must_use]
pub fn run_state_for_event(event: &StreamEvent) -> Option<RunState> {
    match event {
        StreamEvent::Delta { .. } => Some(RunState::Streaming),
        StreamEvent::Usage { .. } => None,
        StreamEvent::ToolCall { .. } => Some(RunState::WaitingToolCall),
        StreamEvent::ToolResult { .. } => Some(RunState::ValidatingToolResult),
        StreamEvent::Done { stop_reason } => match stop_reason {
            StopReason::Stop | StopReason::Length => Some(RunState::Completed),
            StopReason::ToolCalls => Some(RunState::WaitingToolCall),
            StopReason::Error => Some(RunState::Failed),
        },
        StreamEvent::Error(_) => Some(RunState::Failed),
        StreamEvent::Cancelled => Some(RunState::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_core::ToolId;
    use frederico_provider_engine::ProviderError;

    #[test]
    fn delta_maps_to_streaming() {
        let ev = StreamEvent::Delta {
            content: "x".to_string(),
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::Streaming));
    }

    #[test]
    fn usage_does_not_change_state() {
        let ev = StreamEvent::Usage {
            prompt_tokens: 1,
            completion_tokens: 2,
        };
        assert_eq!(run_state_for_event(&ev), None);
    }

    #[test]
    fn tool_call_maps_to_waiting() {
        let ev = StreamEvent::ToolCall {
            id: "call_1".to_string(),
            name: ToolId::new("files.read").to_string(),
            arguments_json: "{}".to_string(),
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::WaitingToolCall));
    }

    #[test]
    fn tool_result_maps_to_validating() {
        let ev = StreamEvent::ToolResult {
            id: "call_1".to_string(),
            ok: true,
            output: serde_json::json!({}),
        };
        assert_eq!(
            run_state_for_event(&ev),
            Some(RunState::ValidatingToolResult)
        );
    }

    #[test]
    fn done_stop_maps_to_completed() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Stop,
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::Completed));
    }

    #[test]
    fn done_length_maps_to_completed() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Length,
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::Completed));
    }

    #[test]
    fn done_toolcalls_maps_to_waiting() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::ToolCalls,
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::WaitingToolCall));
    }

    #[test]
    fn done_error_maps_to_failed() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Error,
        };
        assert_eq!(run_state_for_event(&ev), Some(RunState::Failed));
    }

    #[test]
    fn error_event_maps_to_failed() {
        let ev = StreamEvent::Error(ProviderError::auth());
        assert_eq!(run_state_for_event(&ev), Some(RunState::Failed));
    }

    #[test]
    fn cancelled_maps_to_cancelled() {
        let ev = StreamEvent::Cancelled;
        assert_eq!(run_state_for_event(&ev), Some(RunState::Cancelled));
    }
}
