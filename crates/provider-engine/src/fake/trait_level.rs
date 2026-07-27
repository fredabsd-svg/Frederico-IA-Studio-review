//! Fake em nível de trait — `FakeProviderAdapter`.
//!
//! Implementa `ProviderAdapter` diretamente, devolvendo `StreamEvent`
//! pré-construídos. **Não exercita o parser de SSE** — é o nível
//! errado pra isso (ver §3.1 do ADR-0008). Serve só para testes
//! rápidos de camadas **acima** do parser: orquestrador, mapeamento
//! de erro, cálculo de custo, persistência do journal.
//!
//! Para testar o parser, use [`crate::fake::transport`].

use async_trait::async_trait;
use frederico_core::{ModelId, ProviderId};
use futures::stream::{self, BoxStream};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::provider::{AdapterCapabilities, CostModel, ProviderAdapter, RunHandle};
use crate::types::{ChatRequest, ChatResponse, ProviderError, StopReason, StreamEvent, Usage};

/// Adapter fake. Emite um script de eventos quando `stream()` é
/// chamado, e registra `cancel()` para asserção em teste.
pub struct FakeProviderAdapter {
    id: ProviderId,
    cost: CostModel,
    events: Vec<StreamEvent>,
    /// Tabela `RunHandle → cancelado?` — registra os `cancel()` que
    /// aconteceram, para asserção em teste.
    cancelled: Mutex<HashMap<RunHandle, bool>>,
    next_handle: AtomicU64,
}

impl FakeProviderAdapter {
    #[must_use]
    pub fn new(id: impl Into<ProviderId>) -> Self {
        Self {
            id: id.into(),
            cost: CostModel::default(),
            events: vec![
                StreamEvent::Delta {
                    content: "ok".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
            cancelled: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }

    #[must_use]
    pub fn with_cost(mut self, cost: CostModel) -> Self {
        self.cost = cost;
        self
    }

    /// Substitui o script de eventos.
    #[must_use]
    pub fn with_events(mut self, events: Vec<StreamEvent>) -> Self {
        self.events = events;
        self
    }

    /// Gera um handle novo (sem side effects no stream). Útil em
    /// testes para chamar `cancel()` antes de iniciar o stream.
    #[must_use]
    pub fn fresh_handle(&self) -> RunHandle {
        RunHandle(self.next_handle.fetch_add(1, Ordering::SeqCst) as u128)
    }

    /// Verdadeiro se `cancel(handle)` foi chamado alguma vez.
    #[must_use]
    pub fn was_cancelled(&self, handle: RunHandle) -> bool {
        self.cancelled
            .lock()
            .unwrap()
            .get(&handle)
            .copied()
            .unwrap_or(false)
    }
}

#[async_trait]
impl ProviderAdapter for FakeProviderAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: true,
        }
    }

    fn cost_model(&self) -> CostModel {
        self.cost
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Ok(ChatResponse {
            content: "ok".to_string(),
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
        })
    }

    fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        // O trait-level fake não observa o `cancel` — é o trabalho do
        // orquestrador (que virá na Leva 3) casar `cancel` com o fim
        // do stream. Para o parser e o journal, o que importa é que
        // os eventos saem na ordem certa.
        let _ = request.cancel;
        let events = self.events.clone();
        Ok(Box::pin(stream::iter(events)))
    }

    fn cancel(&self, handle: RunHandle) -> Result<(), ProviderError> {
        self.cancelled.lock().unwrap().insert(handle, true);
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![(ModelId::new("fake-model-v1"), "Simulated Model v1")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;
    use futures::StreamExt;

    #[tokio::test]
    async fn fake_emits_deltas_then_done() {
        let adapter = FakeProviderAdapter::new(ProviderId::new("simulated"));
        let req = ChatRequest::new(
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("oi")],
        );
        let mut s = adapter.stream(req).unwrap();
        let mut events = Vec::new();
        while let Some(ev) = s.next().await {
            events.push(ev);
        }
        assert!(matches!(events[0], StreamEvent::Delta { .. }));
        assert!(matches!(events[1], StreamEvent::Done { .. }));
    }

    #[test]
    fn fake_cancel_marks_handle() {
        let adapter = FakeProviderAdapter::new(ProviderId::new("simulated"));
        let h = adapter.fresh_handle();
        assert!(!adapter.was_cancelled(h));
        adapter.cancel(h).unwrap();
        assert!(adapter.was_cancelled(h));
    }

    #[tokio::test]
    async fn fake_completes_non_streaming() {
        let adapter = FakeProviderAdapter::new(ProviderId::new("simulated"));
        let req = ChatRequest::new(
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("oi")],
        );
        let resp = adapter.complete(req).await.unwrap();
        assert_eq!(resp.content, "ok");
        assert_eq!(resp.stop_reason, StopReason::Stop);
    }

    #[tokio::test]
    async fn fake_with_custom_events() {
        let adapter = FakeProviderAdapter::new(ProviderId::new("simulated")).with_events(vec![
            StreamEvent::Delta {
                content: "olá".to_string(),
            },
            StreamEvent::Delta {
                content: " mundo".to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            },
        ]);
        let req = ChatRequest::new(
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("oi")],
        );
        let events: Vec<StreamEvent> = adapter.stream(req).unwrap().collect().await;
        assert_eq!(events.len(), 3);
    }
}
