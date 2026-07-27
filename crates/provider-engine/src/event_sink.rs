//! Trait de saída de eventos do `ChatOrchestrator`.
//!
//! O orquestrador produz eventos durante o stream de uma mensagem
//! (`run_event` por delta/usage/done/error/cancelled) e um evento de
//! status na transição final do run. O trait é uma porta de saída
//! que o casca Tauri implementa para emitir via `Window::emit`.
//!
//! **Por que um trait e não um `tauri::AppHandle` direto?** Porque o
//! `provider-engine` é núcleo puro (sem `tauri`, sem `windows` —
//! ADR-0003). A casca implementa [`EventSink`], o orquestrador
//! aceita `Arc<dyn EventSink>`. Em testes, um `NoopEventSink` ou um
//! `RecordingEventSink` (em memória) basta.

use frederico_core::RunId;
use frederico_storage::RunStatus;
use serde::Serialize;

/// Saída de eventos. Implementada pela casca Tauri; em testes, por
/// um sink em memória.
pub trait EventSink: Send + Sync {
    /// Emite um evento de stream para a janela aberta. O contrato
    /// de canal é `run://<run_id>/event` e o payload é o JSON do
    /// `StreamEvent`. Se a janela estiver fechada, o sink **deve**
    /// descartar silenciosamente — o evento já está persistido no
    /// journal (regra do
    /// [`docs/architecture/chat-and-providers.md`](../architecture/chat-and-providers.md)
    /// §"Journal de eventos").
    fn emit_run_event(&self, run_id: RunId, payload: serde_json::Value);

    /// Emite um evento de status final do run. Canal:
    /// `run://<run_id>/status`. Payload: `{status, finished_at}`.
    fn emit_run_status(&self, run_id: RunId, status: RunStatus);
}

/// Sink que descarta todos os eventos. Útil em testes que não se
/// importam com a UI.
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit_run_event(&self, _run_id: RunId, _payload: serde_json::Value) {}
    fn emit_run_status(&self, _run_id: RunId, _status: RunStatus) {}
}

/// Sink que registra todos os eventos em memória. Útil em testes
/// que querem asserir sobre a sequência de eventos emitidos.
#[derive(Default)]
pub struct RecordingEventSink {
    pub events: std::sync::Mutex<Vec<(RunId, RecordedEvent)>>,
}

#[derive(Debug, Clone)]
pub enum RecordedEvent {
    Stream(serde_json::Value),
    Status(RunStatus),
}

impl EventSink for RecordingEventSink {
    fn emit_run_event(&self, run_id: RunId, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((run_id, RecordedEvent::Stream(payload)));
    }

    fn emit_run_status(&self, run_id: RunId, status: RunStatus) {
        self.events
            .lock()
            .unwrap()
            .push((run_id, RecordedEvent::Status(status)));
    }
}

impl RecordingEventSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot dos eventos para um run específico.
    #[must_use]
    pub fn events_for(&self, run_id: RunId) -> Vec<RecordedEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(rid, _)| *rid == run_id)
            .map(|(_, e)| e.clone())
            .collect()
    }
}

/// Helper que serializa um valor qualquer (que implemente `Serialize`)
/// em `serde_json::Value`, consumindo o resultado de um `Result<_, _>`.
pub fn to_value<T: Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_discards_all() {
        let sink = NoopEventSink;
        sink.emit_run_event(RunId::new(), serde_json::json!({"a": 1}));
        sink.emit_run_status(RunId::new(), RunStatus::Completed);
    }

    #[test]
    fn recording_sink_collects_per_run() {
        let sink = RecordingEventSink::new();
        let r1 = RunId::new();
        let r2 = RunId::new();
        sink.emit_run_event(r1, serde_json::json!({"kind": "delta"}));
        sink.emit_run_event(r2, serde_json::json!({"kind": "delta"}));
        sink.emit_run_status(r1, RunStatus::Completed);
        assert_eq!(sink.events_for(r1).len(), 2);
        assert_eq!(sink.events_for(r2).len(), 1);
    }
}
