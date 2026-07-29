//! `ChatOrchestrator` — cola o adapter + catálogo + storage +
//! `RunRegistry` + `EventSink` + `RunExecutor` pra entregar o
//! ciclo de vida de uma mensagem em stream.
//!
//! Esta é a **Etapa 4.x.y** (Fase 3): o `ChatOrchestrator` foi
//! movido do `frederico-provider-engine` (que tem os adapters e
//! o parser SSE) pro `frederico-execution-engine` (que tem o
//! `RunExecutor`). A razão é o **ciclo de dependência**:
//! `ChatOrchestrator` precisa chamar o `RunExecutor` (que vive no
//! `execution-engine`) e o `RunExecutor` precisa de tipos do
//! `provider-engine` (`ProviderAdapter`, `StreamEvent`, etc.).
//! Com o `ChatOrchestrator` no `provider-engine`, o ciclo
//! `provider-engine ↔ execution-engine` se fecha. Movendo o
//! `ChatOrchestrator` pro `execution-engine`, o grafo fica:
//!
//! ```text
//! provider-engine ──► execution-engine ──► provider-engine
//!                      (ChatOrchestrator, RunExecutor)
//! ```
//!
//! `provider-engine` continua exportando `ChatOrchestrator` via
//! `pub use frederico_execution_engine::orchestrator::*;` pra
//! manter a compat com a casca Tauri e os tests da Fase 2.
//!
//! ## Fluxo de `send_message`
//!
//! 1. Carrega a conversa; valida que o provedor dela tem adapter.
//! 2. Persiste a `Message` do usuário em `status=pending` **antes**
//!    de qualquer I/O de rede (regra de teste do `testing-strategy.md`
//!    §"Exemplos").
//! 3. Persiste a `Message` Assistant em `status=pending`.
//! 4. Cria o `Run` em `status=created`.
//! 5. Cria um `CancellationToken`, registra no [`RunRegistry`].
//! 6. Atualiza `Message` → `streaming` e `Run` → `running`.
//! 7. Loop de stream em background (não bloqueia o caller).
//!
//! ## O loop de stream
//!
//! O `ChatOrchestrator` **não tem mais loop próprio** (a partir
//! da Etapa 4.x.y). O loop vive no [`RunExecutor`] (Etapa 4):
//!
//! - `tokio::select!` entre o próximo evento do adapter, o
//!   `CancellationToken::cancelled()`, e um
//!   `tokio::time::sleep(event_timeout)` (60s padrão).
//! - Cada evento é **persistido no journal antes** de ser
//!   processado (regra do `chat-and-providers.md` §"Journal de
//!   eventos").
//! - `Delta` acumula no `Message.content`.
//! - `Usage` registra prompt/completion tokens.
//! - `ToolCall` valida via `validate_tool_call` e executa via
//!   `Tool::execute` — emite `ToolResult` e volta a chamar o
//!   modelo.
//! - `Done` finaliza o `Run` como `Completed` e calcula custo
//!   via [`Catalog`].
//! - `Error` finaliza como `Failed` com `ProviderErrorView`
//!   PT-BR com `action`.
//! - `Cancelled` finaliza como `Cancelled`.
//! - Watchdog (Etapa 5) finaliza como `Interrupted` /
//!   `RunStatus::Timeout`.
//!
//! O `ChatOrchestrator` apenas monta o `RunExecutor`, chama
//! `run().await`, e usa o [`RunOutcome`] pra:
//!
//! 1. Calcular custo via `descriptor.cost_microcents(p, c)` e
//!    chamar `MessageRepo::set_usage_and_cost` + `ConversationRepo::add_cost`.
//! 2. Emitir o `RunStatus` final pro `EventSink`.
//! 3. `unregister(run_id)` no `RunRegistry`.

use std::sync::Arc;

use crate::executor::ExecutorError;
use frederico_agent_engine::Budget;
use frederico_core::{ConversationId, MessageId, RunId, ToolId};
use frederico_model_catalog::Catalog;
use frederico_provider_engine::event_sink::EventSink;
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_security::Clock;
use frederico_storage::{Database, RunStatus};
use frederico_tool_registry::{Jail, PermissionSet, Tool, ToolRegistry};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::executor::RunExecutor;

/// Erros do `ChatOrchestrator` (Etapa 4.x.y).
///
/// O `RunExecutor` roda em background dentro do `send_message` (a
/// task é `tokio::spawn`-ed). O `ExecutorError` que ele retorna
/// é logado na task e **não** propaga pro caller — o
/// `send_message` só vê erros síncronos (Storage, Provider,
/// ProviderNotFound, ModelNotFound, ModelWithoutPrice). Por isso
/// o enum **não** tem a variante `Executor` (que seria tentadora
/// mas desnecessária).
#[derive(Debug, Error)]
pub enum ChatOrchestratorError {
    #[error("storage: {0}")]
    Storage(#[from] frederico_storage::StorageError),
    #[error("provedor: {0}")]
    Provider(#[from] frederico_provider_engine::ProviderError),
    #[error("provedor '{0}' não tem adapter registrado")]
    ProviderNotFound(frederico_core::ProviderId),
    #[error("modelo '{provider}/{model}' não está no catálogo")]
    ModelNotFound {
        provider: frederico_core::ProviderId,
        model: frederico_core::ModelId,
    },
    #[error("modelo '{provider}/{model}' sem preço cadastrado")]
    ModelWithoutPrice {
        provider: frederico_core::ProviderId,
        model: frederico_core::ModelId,
    },
}

/// `Result` padrão do `ChatOrchestrator`.
pub type ChatOrchestratorResult<T> = Result<T, ChatOrchestratorError>;

/// Estado vivo do chat: adapters, run registry, sink, db, clock,
/// catálogo, tooling (tool registry + jail + tools + allowed_for_run
/// default).
///
/// O construtor é grande (11 args) mas o `clippy::too_many_arguments`
/// é justificado pela clareza — todos os componentes são injetados
/// e estáveis durante o ciclo de vida.
#[allow(clippy::too_many_arguments)]
pub struct ChatOrchestrator {
    pub providers: Arc<ProviderMap>,
    pub runs: Arc<RunRegistry>,
    pub sink: Arc<dyn EventSink>,
    pub db: Arc<Database>,
    #[allow(dead_code)] // guardado para time-stamping em hooks futuros
    pub clock: Arc<dyn Clock>,
    pub catalog: Arc<Catalog>,
    // ---- Tooling (Etapa 4.x.y) ----
    /// `ToolRegistry` com os manifestos. O `RunExecutor` consulta
    /// pra validar `tool_call`s e construir o `tools:` do
    /// `ChatRequest`.
    pub tool_registry: ToolRegistry,
    /// Jail do workspace atual. O `RunExecutor` passa pro
    /// `validate_tool_call` (Passo 7).
    pub jail: Jail,
    /// Tools concretas (`FilesReadTool` etc.). O `RunExecutor`
    /// consulta pelo `ToolId` no `HashMap` interno.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Allowlist default de `ToolId` por execução. A Etapa 6 (UI
    /// de aprovação) carrega o `PermissionSet` real; aqui
    /// passamos um `default()` deny.
    pub allowed_for_run: Vec<ToolId>,
    /// Handle opcional pro `MemoryExtractor` (Fase 4, Etapa 5).
    /// Quando `Some`, o `send_message` enfileira um
    /// `MemoryExtractionJob` após o run finalizar (Completed
    /// ou Failed — classificador ainda é útil em Failed
    /// porque a última resposta do assistant pode ter sido
    /// gerada antes do timeout). `None` desabilita a
    /// classificação automática (modo legado).
    pub memory_extractor: Option<Arc<frederico_memory::MemoryExtractorHandle>>,
}

impl ChatOrchestrator {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        providers: Arc<ProviderMap>,
        runs: Arc<RunRegistry>,
        sink: Arc<dyn EventSink>,
        db: Arc<Database>,
        clock: Arc<dyn Clock>,
        catalog: Arc<Catalog>,
        tool_registry: ToolRegistry,
        jail: Jail,
        tools: Vec<Arc<dyn Tool>>,
        allowed_for_run: Vec<ToolId>,
        memory_extractor: Option<Arc<frederico_memory::MemoryExtractorHandle>>,
    ) -> Self {
        Self {
            providers,
            runs,
            sink,
            db,
            clock,
            catalog,
            tool_registry,
            jail,
            tools,
            allowed_for_run,
            memory_extractor,
        }
    }

    /// Persiste a mensagem do usuário, cria a assistant e o run, e
    /// dispara o stream em background (delegando pro
    /// [`RunExecutor`]). Retorna a `Message` do usuário criada e o
    /// `RunId` em andamento. A UI segue o stream via
    /// `Run.GetEvents` ou via os eventos Tauri.
    pub async fn send_message(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        user_content: String,
    ) -> ChatOrchestratorResult<(frederico_storage::Message, RunId)> {
        use frederico_core::{ModelId, ProviderId};
        use frederico_provider_engine::ChatMessage;
        use frederico_storage::{ConversationRepo, MessageRepo, MessageStatus, RunRepo};

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

        let adapter = self
            .providers
            .get(&conv.provider_id)
            .ok_or_else(|| ChatOrchestratorError::ProviderNotFound(conv.provider_id.clone()))?;
        let descriptor = self
            .catalog
            .find_model(&conv.provider_id, &conv.model_id)
            .cloned()
            .ok_or_else(|| ChatOrchestratorError::ModelNotFound {
                provider: conv.provider_id.clone(),
                model: conv.model_id.clone(),
            })?;
        if descriptor.pricing_per_million.input_microcents == 0
            && descriptor.pricing_per_million.output_microcents == 0
            && !is_local_or_simulated(&descriptor.provider)
        {
            return Err(ChatOrchestratorError::ModelWithoutPrice {
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

        // Monta o `RunExecutor` e delega o loop.
        let provider_id: ProviderId = conv.provider_id.clone();
        let _provider_id = provider_id; // anchor pra IDE e warning-fix
        let model_id: ModelId = conv.model_id.clone();
        let this = Arc::clone(self);
        let run_id = run.id;
        let asst_id = asst_msg.id;
        let this_for_err = Arc::clone(&this);
        tokio::spawn(async move {
            let permissions = PermissionSet::default(); // Etapa 6 carrega o real
                                                        // `DbAuditSink` (Etapa 5.x, Passo 10) — grava cada
                                                        // tool_call em `tool_audit` (SQLite). A UI da Fase 5/6
                                                        // consome via `ToolAuditRepo::list_for_run`.
            let audit_sink: Arc<dyn frederico_tool_registry::AuditSink> = Arc::new(
                crate::audit_sink::DbAuditSink::new((*this.db).clone(), run_id),
            );
            let mut executor = RunExecutor::new(
                adapter,
                this.tool_registry.clone(),
                this.jail.clone(),
                (*this.db).clone(),
                permissions,
                this.allowed_for_run.clone(),
                this.tools.clone(),
                Budget::default(),
                cancel.clone(),
            )
            .with_audit_sink(audit_sink);
            let outcome = match executor
                .run(
                    asst_id,
                    run_id,
                    model_id.clone(),
                    vec![ChatMessage::user(user_content)],
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(e) => {
                    // Etapa 6.1: `ApprovalRequired` finaliza como
                    // `Cancelled` (com `Message.error` explicando o
                    // motivo). A `approval_queue` já tem a request
                    // enfileirada (pelo `RunExecutor` antes do
                    // `Err`) — a UI consome via `AppOp::ApprovalList`.
                    // Re-executar manualmente após responder.
                    let (status, error_label) = match &e {
                        ExecutorError::ApprovalRequired => {
                            (RunStatus::Cancelled, "aprovação requerida")
                        }
                        _ => (RunStatus::Failed, "erro do executor"),
                    };
                    tracing::error!(
                        ?run_id,
                        error = %error_label,
                        "RunExecutor terminou com erro: {e:?}"
                    );
                    // Persiste o motivo na `Message.error` (best
                    // effort — se a mensagem não existe, segue sem
                    // erro).
                    let _ = frederico_storage::MessageRepo::new(&this_for_err.db)
                        .set_error(&asst_id, &format!("run abortado: {error_label}"))
                        .await;
                    let _ = this_for_err.runs.unregister(run_id).await;
                    this_for_err.sink.emit_run_status(run_id, status);
                    return;
                }
            };

            // Finaliza com base no `RunOutcome`.
            let final_status = match outcome.final_state {
                frederico_agent_engine::RunState::Completed => RunStatus::Completed,
                frederico_agent_engine::RunState::Failed => RunStatus::Failed,
                frederico_agent_engine::RunState::Cancelled => RunStatus::Cancelled,
                frederico_agent_engine::RunState::Interrupted => RunStatus::Timeout,
                // Outros estados (preparando contexto etc.) não
                // chegam aqui — o `RunExecutor` só retorna esses 4
                // via `final_state`. O `_ =>` é defensivo.
                _ => RunStatus::Failed,
            };

            // Cálculo de custo real (reintroduzido na Etapa 4.x.y).
            if outcome.final_state == frederico_agent_engine::RunState::Completed {
                let cost =
                    descriptor.cost_microcents(outcome.prompt_tokens, outcome.completion_tokens);
                let _ = MessageRepo::new(&this_for_err.db)
                    .set_usage_and_cost(
                        &asst_id,
                        outcome.prompt_tokens,
                        outcome.completion_tokens,
                        cost,
                    )
                    .await;
                if let Ok(conv_id) = RunRepo::new(&this_for_err.db)
                    .get(&run_id)
                    .await
                    .map(|r| r.conversation_id)
                {
                    let _ = ConversationRepo::new(&this_for_err.db)
                        .add_cost(&conv_id, cost)
                        .await;
                }
            }

            // Etapa 5 (Fase 4): enfileira um `MemoryExtractionJob`
            // se o `MemoryExtractor` foi configurado. Best-effort:
            // se a carga das mensagens falhar (DB busy, schema
            // drift), loga e segue — o worker da próxima run
            // pega (regra do `ADR-0012 §1`).
            if let Some(extractor) = this_for_err.memory_extractor.as_ref() {
                match build_extraction_job(&this_for_err.db, run_id).await {
                    Ok(job) => {
                        extractor.enqueue(job);
                    }
                    Err(e) => {
                        tracing::warn!(
                            memory.extractor = "enqueue",
                            run_id = %run_id,
                            error = %e,
                            "falha ao construir MemoryExtractionJob — seguindo adiante"
                        );
                    }
                }
            }

            this_for_err.sink.emit_run_status(run_id, final_status);
            let _ = this_for_err.runs.unregister(run_id).await;
        });

        Ok((user_msg, run.id))
    }

    /// Cancela um run em andamento. Marca `cancellation_requested_at`
    /// no DB e dispara o `CancellationToken` do [`RunRegistry`].
    pub async fn cancel_run(&self, run_id: RunId) -> ChatOrchestratorResult<()> {
        use frederico_storage::RunRepo;
        RunRepo::new(&self.db).request_cancellation(&run_id).await?;
        self.runs.cancel(run_id).await;
        Ok(())
    }

    /// Lê eventos do journal de uma mensagem (reload de janela).
    pub async fn get_events(
        &self,
        message_id: MessageId,
        since_seq: u32,
    ) -> ChatOrchestratorResult<Vec<frederico_storage::MessageEvent>> {
        use frederico_storage::MessageEventRepo;
        Ok(MessageEventRepo::new(&self.db)
            .list_for_message(&message_id, since_seq)
            .await?)
    }
}

/// Constrói um `MemoryExtractionJob` a partir do estado pós-run:
/// carrega as últimas N mensagens da conversa (default 6,
/// mesmo teto do `LlmMemoryClassifier`) e mapeia pra
/// `Vec<ConversationMessage>`.
///
/// **Mapeamento `Message.role` → `ConversationMessage.source`:**
/// - `"user"` → `"user_message"`
/// - `"assistant"` → `"assistant_message"`
/// - `"tool"` → `"tool_output:<run_id>"`
/// - outros (`"system"`) → `"unknown"` (não chegam no
///   classificador de v1 — só os 3 primeiros roles entram).
///
/// Best-effort: qualquer erro de DB vira `Err(String)` e o
/// caller loga + segue. Não bloqueia o fluxo de Run.
async fn build_extraction_job(
    db: &Arc<Database>,
    run_id: frederico_core::RunId,
) -> Result<frederico_memory::MemoryExtractionJob, String> {
    use chrono::Utc;
    use frederico_core::{ConversationMessage, MemorySourceType};
    use frederico_storage::{MessageRepo, RunRepo};

    // (1) Carrega o Run pra pegar o `conversation_id`.
    let run = RunRepo::new(db)
        .get(&run_id)
        .await
        .map_err(|e| format!("RunRepo::get: {e}"))?;
    let conversation_id = run.conversation_id;

    // (2) Carrega todas as mensagens da conversa e pega as
    // últimas 6 (default do `LlmMemoryClassifier`).
    let all = MessageRepo::new(db)
        .list_for_conversation(&conversation_id)
        .await
        .map_err(|e| format!("MessageRepo::list_for_conversation: {e}"))?;
    const LAST_N: usize = 6;
    let tail: Vec<_> = all
        .iter()
        .rev()
        .take(LAST_N)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    // (3) Mapeia pra `ConversationMessage`.
    let messages: Vec<ConversationMessage> = tail
        .iter()
        .filter_map(|m| {
            let source = match m.role.as_str() {
                "user" => MemorySourceType::new("user_message"),
                "assistant" => MemorySourceType::new("assistant_message"),
                "tool" => MemorySourceType::new(format!(
                    "tool_output:{}",
                    m.run_id
                        .map(|r| r.0.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )),
                _ => return None, // system, etc — pula
            };
            Some(ConversationMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                source,
            })
        })
        .collect();

    if messages.is_empty() {
        return Err("nenhuma mensagem mapeada — conversa vazia?".to_string());
    }

    Ok(frederico_memory::MemoryExtractionJob::new(
        run_id.0.to_string(),
        conversation_id.0.to_string(),
        messages,
        Utc::now(),
    ))
}

/// Models locais/simulados sem preço (a Etapa 4.x.y reintroduz o
/// cálculo de custo real via `Catalog`; mas modelos locais não
/// precisam de preço cadastrado).
fn is_local_or_simulated(provider: &frederico_core::ProviderId) -> bool {
    matches!(
        provider.as_str(),
        "ollama" | "lmstudio" | "simulated" | "nvidia"
    )
}

// Re-exporta `error_to_view` e tipos relacionados pra quem
// importar de `execution-engine::orchestrator`. (A casca Tauri
// pode usar tanto `provider-engine::orchestrator` quanto
// `execution-engine::orchestrator` — ambos exportam.)
pub use frederico_provider_engine::orchestrator::error_to_view as to_view;

// Os tipos `OrchestratorError`, `OrchestratorResult` e
// `ProviderErrorView` ficam onde estavam (no
// `provider-engine::orchestrator`).
// O `ChatOrchestratorError` deste crate é separado
// (`ChatOrchestratorError::Executor` envolve o do `RunExecutor`).
// A casca Tauri e os tests da Fase 2 podem usar tanto um quanto
// o outro.

// O `RunExecutor` consome o `StreamEvent::ToolResult` que vem do
// próprio executor (não do provider). O `persist_journal` do
// `execution-engine` cuida disso. Aqui só re-exportamos pra
// conveniência.
