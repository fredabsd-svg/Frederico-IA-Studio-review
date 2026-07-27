//! `TauriEventSink` — implementação do trait `EventSink` que
//! emite via `tauri::Window::emit`. Esta é a **única** peça do
//! `provider-engine` que toca em Tauri diretamente — fica na
//! casca (não em `crates/`) para preservar a regra de pureza do
//! ADR-0003.

use frederico_core::RunId;
use frederico_provider_engine::EventSink;
use frederico_storage::RunStatus;
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriEventSink {
    handle: AppHandle,
}

impl TauriEventSink {
    #[must_use]
    pub fn new(handle: AppHandle) -> Self {
        Self { handle }
    }

    fn channel(&self, run_id: RunId, suffix: &str) -> String {
        format!("run://{}/{}", run_id.0, suffix)
    }
}

impl EventSink for TauriEventSink {
    fn emit_run_event(&self, run_id: RunId, payload: serde_json::Value) {
        let channel = self.channel(run_id, "event");
        // `emit` falha silenciosa se a janela estiver fechada. O
        // journal no SQLite é a fonte de verdade; a janela
        // recarrega dele.
        if let Some(window) = self.handle.get_webview_window("main") {
            let _ = window.emit(&channel, payload);
        }
    }

    fn emit_run_status(&self, run_id: RunId, status: RunStatus) {
        let channel = self.channel(run_id, "status");
        let payload = serde_json::json!({ "status": status.as_str() });
        if let Some(window) = self.handle.get_webview_window("main") {
            let _ = window.emit(&channel, payload);
        }
    }
}
