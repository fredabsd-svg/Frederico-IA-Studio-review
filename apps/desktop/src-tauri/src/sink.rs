//! `TauriEventSink` — implementação do trait `EventSink` que
//! emite via `tauri::Window::emit`. Esta é a **única** peça do
//! `provider-engine` que toca em Tauri diretamente — fica na
//! casca (não em `crates/`) para preservar a regra de pureza do
//! ADR-0003.

use frederico_core::RunId;
use frederico_provider_engine::event_sink::{
    run_event_channel_for_event, run_event_channel_for_status,
};
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
}

impl EventSink for TauriEventSink {
    fn emit_run_event(&self, run_id: RunId, payload: serde_json::Value) {
        let channel = run_event_channel_for_event(&run_id);
        // A janela `main` é resolvida na hora. Se a janela estiver
        // fechada (recarregamento, quit, etc.), o `get_webview_window`
        // devolve `None` — descartamos silenciosamente porque o
        // journal no SQLite é a fonte de verdade; a janela recarrega
        // dele via `reloadStreamingMessage` + `RunGetEvents`. Sem
        // janela aberta, **não há listener** e a falha de emit
        // também não é importante.
        //
        // **Mas se a janela ESTÁ aberta e o `emit` falha** (ex.:
        // serialização do payload quebrada, listener interno do
        // Tauri com erro), precisamos saber — esse é o caminho
        // silencioso do bug "resposta não aparece na UI" (PR
        // do stream run_id): o backend emite, o canal está certo,
        // mas o `let _ = ...` engole a falha e a UI fica vazia sem
        // nenhuma indicação no log. Agora registramos via
        // `tracing::warn!` — o evento que não chega aparece no
        // stderr e em qualquer coletor que leia o `frederico-mind.log`.
        let Some(window) = self.handle.get_webview_window("main") else {
            return;
        };
        if let Err(e) = window.emit(&channel, payload) {
            tracing::warn!(
                run_id = %run_id,
                channel = %channel,
                error = %e,
                "TauriEventSink: falha ao emitir evento de stream — UI pode ter ficado sem o delta. \
                 O journal no SQLite tem o evento; o reload (RunGetEvents) recupera. \
                 Se o sintoma for 'resposta não aparece', este log é a primeira coisa a olhar."
            );
        }
    }

    fn emit_run_status(&self, run_id: RunId, status: RunStatus) {
        let channel = run_event_channel_for_status(&run_id);
        let payload = serde_json::json!({ "status": status.as_str() });
        let Some(window) = self.handle.get_webview_window("main") else {
            return;
        };
        if let Err(e) = window.emit(&channel, payload) {
            tracing::warn!(
                run_id = %run_id,
                channel = %channel,
                status = %status.as_str(),
                error = %e,
                "TauriEventSink: falha ao emitir status final do run — UI pode não ter atualizado o estado."
            );
        }
    }
}
