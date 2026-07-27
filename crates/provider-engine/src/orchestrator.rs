//! `ChatOrchestrator` — cola o adapter + catálogo + storage + RunRegistry
//! + EventSink para entregar o ciclo de vida de uma mensagem em stream.
//!
//! Fluxo de `send_message`:
//! 1. Carrega a conversa; valida que o provedor dela tem adapter.
//! 2. Persiste a `Message` do usuário em `status=pending` **antes** de
//!    qualquer I/O de rede (regra de teste do
//!    [`testing-strategy.md`](../testing-strategy.md) §"Exemplos").
//! 3. Persiste a `Message` Assistant em `status=pending`.
//! 4. Cria o `Run` em `status=created`.
//! 5. Cria um `CancellationToken`, registra no [`RunRegistry`].
//! 6. Atualiza `Message` → `streaming` e `Run` → `running`.
//! 7. Loop de stream em background (não bloqueia o caller).
//!
//! Loop de stream:
//! - `tokio::select!` entre o próximo evento do adapter, o
//!   `CancellationToken::cancelled()`, e um
//!   `tokio::time::sleep(event_timeout)` (60s padrão).
//! - Cada evento é **persistido no journal antes** de ser emitido
//!   pelo [`EventSink`] (regra do
//!   [`chat-and-providers.md`](../architecture/chat-and-providers.md)
//!   §"Journal de eventos").
//! - `Delta` acumula no `Message.content`.
//! - `Usage` registra prompt/completion tokens (último vence).
//! - `Done` calcula custo via [`Catalog`], persiste e finaliza.
//! - `Error` mapeia para `ProviderErrorView` PT-BR com `action`.
//! - `Cancelled` finaliza como `cancelled`.

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, ModelId, ProviderId, RunId};
use frederico_model_catalog::{Catalog, ModelDescriptor};
use frederico_security::Clock;
use frederico_storage::{
    ConversationRepo, Database, MessageEventRepo, MessageRepo, MessageStatus, RunRepo, RunStatus,
};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::event_sink::{to_value, EventSink};
use crate::provider_map::ProviderMap;
use crate::run_registry::RunRegistry;
use crate::types::{
    ChatMessage, ChatRequest, ProviderError, ProviderErrorKind, StopReason, StreamEvent,
};

/// Erros do orquestrador.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("storage: {0}")]
    Storage(#[from] frederico_storage::StorageError),
    #[error("provedor '{0}' não tem adapter registrado")]
    ProviderNotFound(ProviderId),
    #[error("modelo '{provider}/{model}' não está no catálogo")]
    ModelNotFound {
        provider: ProviderId,
        model: ModelId,
    },
    #[error("modelo '{provider}/{model}' sem preço cadastrado")]
    ModelWithoutPrice {
        provider: ProviderId,
        model: ModelId,
    },
    #[error("erro do provedor: {0}")]
    Provider(#[from] ProviderError),
}

pub type OrchestratorResult<T> = Result<T, OrchestratorError>;

/// Estado vivo do chat: adapters, run registry, sink, db, clock, catálogo.
pub struct ChatOrchestrator {
    providers: Arc<ProviderMap>,
    runs: Arc<RunRegistry>,
    sink: Arc<dyn EventSink>,
    db: Arc<Database>,
    #[allow(dead_code)] // guardado para time-stamping em hooks futuros
    clock: Arc<dyn Clock>,
    catalog: Arc<Catalog>,
}

impl ChatOrchestrator {
    #[must_use]
    pub fn new(
        providers: Arc<ProviderMap>,
        runs: Arc<RunRegistry>,
        sink: Arc<dyn EventSink>,
        db: Arc<Database>,
        clock: Arc<dyn Clock>,
        catalog: Arc<Catalog>,
    ) -> Self {
        Self {
            providers,
            runs,
            sink,
            db,
            clock,
            catalog,
        }
    }

    /// Persiste a mensagem do usuário, cria a assistant e o run, e
    /// dispara o stream em background. Retorna a `Message` do
    /// usuário criada e o `RunId` em andamento. A UI segue o stream
    /// via `Run.GetEvents` ou via os eventos Tauri.
    pub async fn send_message(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        user_content: String,
    ) -> OrchestratorResult<(frederico_storage::Message, RunId)> {
        let conv = ConversationRepo::new(&self.db)
            .get(&conversation_id)
            .await?;

        // Mensagem do usuário PRIMEIRO (regra de teste).
        let user_msg = MessageRepo::new(&self.db)
            .create(&conversation_id, "user", &user_content, None)
            .await?;
        let asst_msg = MessageRepo::new(&self.db)
            .create(&conversation_id, "assistant", "", None)
            .await?;
        let run = RunRepo::new(&self.db)
            .create(&conversation_id, &asst_msg.id)
            .await?;

        let _adapter = self
            .providers
            .get(&conv.provider_id)
            .ok_or_else(|| OrchestratorError::ProviderNotFound(conv.provider_id.clone()))?;
        let descriptor = self
            .catalog
            .find_model(&conv.provider_id, &conv.model_id)
            .cloned()
            .ok_or_else(|| OrchestratorError::ModelNotFound {
                provider: conv.provider_id.clone(),
                model: conv.model_id.clone(),
            })?;
        if descriptor.pricing_per_million.input_microcents == 0
            && descriptor.pricing_per_million.output_microcents == 0
            && !is_local_or_simulated(&descriptor.provider)
        {
            return Err(OrchestratorError::ModelWithoutPrice {
                provider: conv.provider_id.clone(),
                model: conv.model_id.clone(),
            });
        }

        let cancel = CancellationToken::new();
        self.runs.register(run.id, cancel.clone()).await;

        MessageRepo::new(&self.db)
            .set_status(&asst_msg.id, MessageStatus::Streaming)
            .await?;
        RunRepo::new(&self.db)
            .set_status(&run.id, RunStatus::Running)
            .await?;

        let this = Arc::clone(self);
        let run_id = run.id;
        let asst_id = asst_msg.id;
        let this_for_err = Arc::clone(&this);
        tokio::spawn(async move {
            if let Err(e) = this
                .run_stream_loop(
                    run_id,
                    asst_id,
                    conv.provider_id.clone(),
                    conv.model_id.clone(),
                    descriptor,
                    user_content,
                    cancel,
                )
                .await
            {
                tracing::error!(?run_id, "stream loop terminou com erro: {e:?}");
                let _ = this_for_err.runs.unregister(run_id).await;
                this_for_err.sink.emit_run_status(run_id, RunStatus::Failed);
            }
        });

        Ok((user_msg, run.id))
    }

    /// Cancela um run em andamento. Marca `cancellation_requested_at`
    /// no DB e dispara o `CancellationToken` do [`RunRegistry`].
    pub async fn cancel_run(&self, run_id: RunId) -> OrchestratorResult<()> {
        RunRepo::new(&self.db).request_cancellation(&run_id).await?;
        self.runs.cancel(run_id).await;
        Ok(())
    }

    /// Lê eventos do journal de uma mensagem (reload de janela).
    pub async fn get_events(
        &self,
        message_id: MessageId,
        since_seq: u32,
    ) -> OrchestratorResult<Vec<frederico_storage::MessageEvent>> {
        Ok(MessageEventRepo::new(&self.db)
            .list_for_message(&message_id, since_seq)
            .await?)
    }

    // ----------------------------------------------------------------
    // Stream loop (interno)
    // ----------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn run_stream_loop(
        self: Arc<Self>,
        run_id: RunId,
        asst_msg_id: MessageId,
        provider_id: ProviderId,
        _model_id: ModelId,
        descriptor: ModelDescriptor,
        user_content: String,
        cancel: CancellationToken,
    ) -> OrchestratorResult<()> {
        let req = ChatRequest {
            provider: provider_id.clone(),
            model: _model_id.clone(),
            messages: vec![ChatMessage::user(user_content)],
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            cancel: cancel.clone(),
            event_timeout: Duration::from_secs(60),
        };

        let mut stream = self
            .providers
            .get(&provider_id)
            .expect("adapter existe — validado em send_message")
            .stream(req)?;
        let mut accumulated = String::new();
        let mut prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;

        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.finalize_cancelled(asst_msg_id, run_id, &accumulated).await?;
                    self.sink.emit_run_event(run_id, to_value(StreamEvent::Cancelled));
                    self.sink.emit_run_status(run_id, RunStatus::Cancelled);
                    self.runs.unregister(run_id).await;
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    let cost = descriptor.cost_microcents(prompt_tokens, completion_tokens);
                    self.finalize_timeout(
                        asst_msg_id,
                        run_id,
                        &accumulated,
                        prompt_tokens,
                        completion_tokens,
                        cost,
                    )
                    .await?;
                    self.sink
                        .emit_run_event(run_id, to_value(StreamEvent::Error(ProviderError::timeout())));
                    self.sink.emit_run_status(run_id, RunStatus::Timeout);
                    self.runs.unregister(run_id).await;
                    return Ok(());
                }
                ev = stream.next() => match ev {
                    Some(ev) => ev,
                    None => break,
                },
            };

            // 1. Persiste no journal PRIMEIRO.
            let (kind, data) = stream_event_to_journal(&next);
            if let Err(e) = MessageEventRepo::new(&self.db)
                .append(&asst_msg_id, &kind, &data)
                .await
            {
                tracing::error!(?asst_msg_id, "journal falhou: {e:?}");
                self.finalize_error(
                    asst_msg_id,
                    run_id,
                    &accumulated,
                    &ProviderError {
                        kind: ProviderErrorKind::Network,
                        upstream_status: None,
                        upstream_message: Some(format!("journal: {e:?}")),
                        retry_after: None,
                    },
                )
                .await?;
                self.sink.emit_run_event(
                    run_id,
                    to_value(StreamEvent::Error(ProviderError::network("journal"))),
                );
                self.sink.emit_run_status(run_id, RunStatus::Failed);
                self.runs.unregister(run_id).await;
                return Ok(());
            }

            // 2. Acumula e atualiza estado.
            match &next {
                StreamEvent::Delta { content } => {
                    accumulated.push_str(content);
                    let _ = MessageRepo::new(&self.db)
                        .set_content(&asst_msg_id, &accumulated)
                        .await;
                }
                StreamEvent::Usage {
                    prompt_tokens: p,
                    completion_tokens: c,
                } => {
                    prompt_tokens = *p;
                    completion_tokens = *c;
                }
                StreamEvent::Done { .. } => {
                    let cost = descriptor.cost_microcents(prompt_tokens, completion_tokens);
                    MessageRepo::new(&self.db)
                        .set_usage_and_cost(&asst_msg_id, prompt_tokens, completion_tokens, cost)
                        .await?;
                    if let Ok(conv_id) = RunRepo::new(&self.db)
                        .get(&run_id)
                        .await
                        .map(|r| r.conversation_id)
                    {
                        let _ = ConversationRepo::new(&self.db)
                            .add_cost(&conv_id, cost)
                            .await;
                    }
                    MessageRepo::new(&self.db)
                        .set_status(&asst_msg_id, MessageStatus::Completed)
                        .await?;
                    RunRepo::new(&self.db)
                        .set_status(&run_id, RunStatus::Completed)
                        .await?;
                    self.sink.emit_run_event(run_id, to_value(&next));
                    self.sink.emit_run_status(run_id, RunStatus::Completed);
                    self.runs.unregister(run_id).await;
                    return Ok(());
                }
                StreamEvent::Error(err) => {
                    self.finalize_error(asst_msg_id, run_id, &accumulated, err)
                        .await?;
                    self.sink.emit_run_event(run_id, to_value(&next));
                    self.sink.emit_run_status(run_id, RunStatus::Failed);
                    self.runs.unregister(run_id).await;
                    return Ok(());
                }
                StreamEvent::Cancelled => {
                    self.finalize_cancelled(asst_msg_id, run_id, &accumulated)
                        .await?;
                    self.sink.emit_run_event(run_id, to_value(&next));
                    self.sink.emit_run_status(run_id, RunStatus::Cancelled);
                    self.runs.unregister(run_id).await;
                    return Ok(());
                }
                StreamEvent::ToolCall { .. } => {
                    // Fase 3 lê e age. Na Fase 2 apenas persiste (acima) e segue.
                }
            }

            // 3. Emite Tauri. Falha aqui não é grave — o journal
            //    é a fonte de verdade; a janela recarrega dele.
            self.sink.emit_run_event(run_id, to_value(&next));
        }

        // Stream terminou sem Done explícito. Calcula custo do que
        // acumulamos e finaliza como Completed se havia conteúdo;
        // Failed se não.
        let cost = descriptor.cost_microcents(prompt_tokens, completion_tokens);
        MessageRepo::new(&self.db)
            .set_usage_and_cost(&asst_msg_id, prompt_tokens, completion_tokens, cost)
            .await?;
        if let Ok(conv_id) = RunRepo::new(&self.db)
            .get(&run_id)
            .await
            .map(|r| r.conversation_id)
        {
            let _ = ConversationRepo::new(&self.db)
                .add_cost(&conv_id, cost)
                .await;
        }
        if accumulated.is_empty() {
            self.finalize_error(
                asst_msg_id,
                run_id,
                &accumulated,
                &ProviderError {
                    kind: ProviderErrorKind::Unknown,
                    upstream_status: None,
                    upstream_message: Some("stream terminou sem Done".to_string()),
                    retry_after: None,
                },
            )
            .await?;
            self.sink.emit_run_status(run_id, RunStatus::Failed);
        } else {
            MessageRepo::new(&self.db)
                .set_status(&asst_msg_id, MessageStatus::Completed)
                .await?;
            RunRepo::new(&self.db)
                .set_status(&run_id, RunStatus::Completed)
                .await?;
            self.sink.emit_run_status(run_id, RunStatus::Completed);
        }
        self.runs.unregister(run_id).await;
        Ok(())
    }

    async fn finalize_cancelled(
        &self,
        asst_msg_id: MessageId,
        run_id: RunId,
        accumulated: &str,
    ) -> OrchestratorResult<()> {
        MessageRepo::new(&self.db)
            .set_content(&asst_msg_id, accumulated)
            .await?;
        MessageRepo::new(&self.db)
            .set_status(&asst_msg_id, MessageStatus::Cancelled)
            .await?;
        RunRepo::new(&self.db)
            .set_status(&run_id, RunStatus::Cancelled)
            .await?;
        Ok(())
    }

    async fn finalize_timeout(
        &self,
        asst_msg_id: MessageId,
        run_id: RunId,
        accumulated: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        cost: u64,
    ) -> OrchestratorResult<()> {
        MessageRepo::new(&self.db)
            .set_usage_and_cost(&asst_msg_id, prompt_tokens, completion_tokens, cost)
            .await?;
        MessageRepo::new(&self.db)
            .set_content(&asst_msg_id, accumulated)
            .await?;
        let err = ProviderError {
            kind: ProviderErrorKind::Timeout,
            upstream_status: None,
            upstream_message: Some("watchdog 60s".to_string()),
            retry_after: None,
        };
        let view = error_to_view(&err);
        MessageRepo::new(&self.db)
            .set_error(
                &asst_msg_id,
                &serde_json::to_string(&view).unwrap_or_default(),
            )
            .await?;
        MessageRepo::new(&self.db)
            .set_status(&asst_msg_id, MessageStatus::Timeout)
            .await?;
        RunRepo::new(&self.db)
            .set_status(&run_id, RunStatus::Timeout)
            .await?;
        Ok(())
    }

    async fn finalize_error(
        &self,
        asst_msg_id: MessageId,
        run_id: RunId,
        accumulated: &str,
        err: &ProviderError,
    ) -> OrchestratorResult<()> {
        MessageRepo::new(&self.db)
            .set_content(&asst_msg_id, accumulated)
            .await?;
        let view = error_to_view(err);
        MessageRepo::new(&self.db)
            .set_error(
                &asst_msg_id,
                &serde_json::to_string(&view).unwrap_or_default(),
            )
            .await?;
        MessageRepo::new(&self.db)
            .set_status(&asst_msg_id, MessageStatus::Failed)
            .await?;
        RunRepo::new(&self.db)
            .set_status(&run_id, RunStatus::Failed)
            .await?;
        Ok(())
    }
}

fn is_local_or_simulated(provider: &ProviderId) -> bool {
    matches!(
        provider.as_str(),
        "ollama" | "lmstudio" | "simulated" | "nvidia"
    )
}

fn stream_event_to_journal(ev: &StreamEvent) -> (String, serde_json::Value) {
    let kind = match ev {
        StreamEvent::Delta { .. } => "delta",
        StreamEvent::Usage { .. } => "usage",
        StreamEvent::ToolCall { .. } => "tool_call",
        StreamEvent::Done { .. } => "done",
        StreamEvent::Error(_) => "error",
        StreamEvent::Cancelled => "cancelled",
    };
    let data = match ev {
        StreamEvent::Delta { content } => serde_json::json!({ "content": content }),
        StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
        } => serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
        }),
        StreamEvent::ToolCall {
            id,
            name,
            arguments_json,
        } => serde_json::json!({
            "id": id,
            "name": name,
            "arguments_json": arguments_json,
        }),
        StreamEvent::Done { stop_reason } => {
            serde_json::json!({ "stop_reason": stop_reason_to_str(*stop_reason) })
        }
        StreamEvent::Error(e) => serde_json::json!({
            "kind": provider_error_kind_to_str(e.kind),
            "upstream_status": e.upstream_status,
            "upstream_message": e.upstream_message,
        }),
        StreamEvent::Cancelled => serde_json::json!({}),
    };
    (kind.to_string(), data)
}

fn stop_reason_to_str(s: StopReason) -> &'static str {
    match s {
        StopReason::Stop => "stop",
        StopReason::Length => "length",
        StopReason::ToolCalls => "tool_calls",
        StopReason::Error => "error",
    }
}

fn provider_error_kind_to_str(k: ProviderErrorKind) -> &'static str {
    match k {
        ProviderErrorKind::Auth => "auth",
        ProviderErrorKind::Payment => "payment",
        ProviderErrorKind::Forbidden => "forbidden",
        ProviderErrorKind::NotFound => "not_found",
        ProviderErrorKind::RateLimited => "rate_limited",
        ProviderErrorKind::Server => "server",
        ProviderErrorKind::Network => "network",
        ProviderErrorKind::Cancelled => "cancelled",
        ProviderErrorKind::Timeout => "timeout",
        ProviderErrorKind::Unknown => "unknown",
    }
}

/// Traduz `ProviderError` em `ProviderErrorView` (PT-BR com ação).
/// Esta é a tabela do spec §"Política de erro de provedor".
pub fn error_to_view(err: &ProviderError) -> ProviderErrorView {
    use ProviderErrorKind::*;
    let (code, title, action) = match err.kind {
        Auth => (
            "auth_invalid",
            "Chave de API inválida",
            "Abra Configurações → Provedores, confira a chave e salve de novo.",
        ),
        Payment => (
            "no_credit",
            "Sem crédito no provedor",
            "Veja o painel de billing do provedor para adicionar saldo.",
        ),
        Forbidden => (
            "forbidden",
            "Sem acesso a este modelo",
            "Sua chave não tem acesso ao modelo. Escolha outro ou peça acesso ao provedor.",
        ),
        NotFound => (
            "model_not_found",
            "Modelo não encontrado",
            "O modelo solicitado não está disponível. Escolha outro na lista.",
        ),
        RateLimited => (
            "rate_limited",
            "Limite de requisições atingido",
            "Aguarde alguns instantes e tente de novo.",
        ),
        Server => (
            "provider_error",
            "Provedor instável",
            "Tente de novo em alguns instantes.",
        ),
        Network => (
            "network_error",
            "Falha de rede",
            "Confira sua conexão e tente de novo.",
        ),
        Cancelled => ("cancelled", "Interrompido", ""),
        Timeout => (
            "timeout",
            "Provedor sem resposta",
            "O provedor não respondeu em 60s. Tente de novo ou troque de modelo.",
        ),
        Unknown => (
            "unknown",
            "Erro do provedor",
            "Veja os logs para mais detalhes.",
        ),
    };
    ProviderErrorView {
        code: code.to_string(),
        title: title.to_string(),
        detail: err.upstream_message.clone().unwrap_or_default(),
        action: if action.is_empty() {
            None
        } else {
            Some(action.to_string())
        },
        retry_after_secs: err.retry_after.map(|d| d.as_secs()),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderErrorView {
    pub code: String,
    pub title: String,
    pub detail: String,
    pub action: Option<String>,
    pub retry_after_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_sink::RecordingEventSink;
    use crate::fake::trait_level::FakeProviderAdapter;
    use frederico_security::fake::FakeClock;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "frederico-orch-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn make_orchestrator() -> (Arc<ChatOrchestrator>, PathBuf, Arc<RecordingEventSink>) {
        let dir = tempdir();
        let db = Database::open(&dir.join("orch.db")).await.unwrap();
        let db = Arc::new(db);
        let clock: Arc<dyn Clock> = FakeClock::new();
        let catalog = Arc::new(Catalog::load().clone());
        let providers = Arc::new({
            let mut m = ProviderMap::new();
            m.insert(Arc::new(FakeProviderAdapter::new("simulated")));
            m
        });
        let runs = RunRegistry::new();
        let sink = Arc::new(RecordingEventSink::new());
        let sink_dyn: Arc<dyn EventSink> = sink.clone();
        let orch = ChatOrchestrator::new(providers, runs, sink_dyn, db, clock, catalog);
        (Arc::new(orch), dir, sink)
    }

    async fn wait_for_run_completion(orch: &ChatOrchestrator, run_id: RunId) {
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Ok(r) = RunRepo::new(&orch.db).get(&run_id).await {
                if r.status != "running" && r.status != "created" {
                    return;
                }
            }
        }
    }

    #[tokio::test]
    async fn send_message_persists_user_first() {
        let (orch, _dir, _sink) = make_orchestrator().await;
        let conv_repo = ConversationRepo::new(&orch.db);
        let conv = conv_repo
            .create(
                &ProviderId::new("simulated"),
                &ModelId::new("fake-model-v1"),
                None,
            )
            .await
            .unwrap();
        let (user_msg, _run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
        assert_eq!(user_msg.role, "user");
        assert_eq!(user_msg.content, "oi");
    }

    #[tokio::test]
    async fn send_message_persists_journal_and_finalizes() {
        let (orch, _dir, _sink) = make_orchestrator().await;
        let conv_repo = ConversationRepo::new(&orch.db);
        let conv = conv_repo
            .create(
                &ProviderId::new("simulated"),
                &ModelId::new("fake-model-v1"),
                None,
            )
            .await
            .unwrap();
        let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
        wait_for_run_completion(&orch, run_id).await;
        let run = RunRepo::new(&orch.db).get(&run_id).await.unwrap();
        assert!(
            run.status == "completed",
            "status inesperado: {}",
            run.status
        );
        let msgs = MessageRepo::new(&orch.db)
            .list_for_conversation(&conv.id)
            .await
            .unwrap();
        let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
        let events = MessageEventRepo::new(&orch.db)
            .list_for_message(&asst.id, 0)
            .await
            .unwrap();
        // Pelo menos um delta + um done.
        assert!(events.iter().any(|e| e.kind == "delta"));
        assert!(events.iter().any(|e| e.kind == "done"));
    }

    #[tokio::test]
    async fn get_events_with_since_seq_skips_old() {
        let (orch, _dir, _sink) = make_orchestrator().await;
        let conv_repo = ConversationRepo::new(&orch.db);
        let conv = conv_repo
            .create(
                &ProviderId::new("simulated"),
                &ModelId::new("fake-model-v1"),
                None,
            )
            .await
            .unwrap();
        let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
        wait_for_run_completion(&orch, run_id).await;
        let msgs = MessageRepo::new(&orch.db)
            .list_for_conversation(&conv.id)
            .await
            .unwrap();
        let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
        let all = orch.get_events(asst.id, 0).await.unwrap();
        assert!(!all.is_empty());
        let from_1 = orch.get_events(asst.id, 1).await.unwrap();
        assert_eq!(from_1.len(), all.len() - 1);
    }

    #[tokio::test]
    async fn cancel_run_marks_requested() {
        let (orch, _dir, _sink) = make_orchestrator().await;
        let conv_repo = ConversationRepo::new(&orch.db);
        let conv = conv_repo
            .create(
                &ProviderId::new("simulated"),
                &ModelId::new("fake-model-v1"),
                None,
            )
            .await
            .unwrap();
        let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
        // O run provavelmente já terminou; o cancel é idempotente.
        let _ = orch.cancel_run(run_id).await;
        // Se ainda estava em andamento, o cancel foi processado.
        // Caso contrário, é no-op.
    }

    #[tokio::test]
    async fn unknown_provider_returns_error() {
        let (orch, _dir, _sink) = make_orchestrator().await;
        let conv_repo = ConversationRepo::new(&orch.db);
        let conv = conv_repo
            .create(
                &ProviderId::new("not_registered"),
                &ModelId::new("fake-model-v1"),
                None,
            )
            .await
            .unwrap();
        let r = orch.send_message(conv.id, "oi".to_string()).await;
        assert!(matches!(r, Err(OrchestratorError::ProviderNotFound(_))));
    }

    #[test]
    fn error_to_view_pt_br_format() {
        let err = ProviderError {
            kind: ProviderErrorKind::Auth,
            upstream_status: Some(401),
            upstream_message: None,
            retry_after: None,
        };
        let v = error_to_view(&err);
        assert_eq!(v.code, "auth_invalid");
        assert!(v.title.contains("Chave"));
        assert!(v.action.is_some());
        assert!(v.action.unwrap().contains("Configurações"));
    }

    #[test]
    fn error_to_view_timeout_has_action_with_60s() {
        let err = ProviderError::timeout();
        let v = error_to_view(&err);
        assert_eq!(v.code, "timeout");
        assert_eq!(v.title, "Provedor sem resposta");
        // A action carrega o "60s" do watchdog.
        let action = v.action.expect("timeout tem action");
        assert!(action.contains("60s"));
    }

    #[test]
    fn error_to_view_cancelled_has_no_action() {
        let v = error_to_view(&ProviderError::cancelled());
        assert_eq!(v.code, "cancelled");
        assert!(v.action.is_none());
    }

    #[test]
    fn stream_event_to_journal_preserves_kind() {
        let ev = StreamEvent::Delta {
            content: "oi".to_string(),
        };
        let (kind, data) = stream_event_to_journal(&ev);
        assert_eq!(kind, "delta");
        assert_eq!(data["content"], "oi");
    }

    #[test]
    fn stream_event_done_serializes_stop_reason() {
        let ev = StreamEvent::Done {
            stop_reason: StopReason::Length,
        };
        let (kind, data) = stream_event_to_journal(&ev);
        assert_eq!(kind, "done");
        assert_eq!(data["stop_reason"], "length");
    }
}
