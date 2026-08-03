//! Testes de cancelamento do `RunExecutor` (Etapa 4.x).
//!
//! O `RunExecutor` checa o `CancellationToken` (passado no `new`):
//! 1. **Entre rounds** do loop externo — antes do `tick` do budget.
//! 2. **Entre eventos** do stream — antes de cada `persist_journal`.
//!
//! Quando o cancel chega, o executor finaliza como
//! `MessageStatus::Cancelled` / `RunStatus::Cancelled` e retorna
//! `RunOutcome { final_state: RunState::Cancelled, ... }`.
//!
//! **Limitação conhecida** (Etapa 4.x): o executor **não**
//! interrompe a request HTTP em andamento no adapter. O
//! `OpenAiCompatAdapter` da Fase 2 já recebe o `cancel:
//! CancellationToken` no `ChatRequest`, mas a Etapa 5 (watchdog)
//! introduz o `tokio::select!` com `reqwest` observando o token
//! de verdade. Por enquanto, a checagem cobre o caso "cancel
//! chegou entre eventos" — o adapter pode emitir mais alguns
//! deltas antes de o loop interno perceber.

#![cfg(not(doctest))]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use frederico_agent_engine::Budget;
use frederico_core::{MessageId, ModelId, ProviderId, RunId, ToolId};
use frederico_execution_engine::RunExecutor;
use frederico_provider_engine::provider::{AdapterCapabilities, CostModel, RunHandle};
use frederico_provider_engine::{
    ChatMessage, ChatRequest, ChatResponse, ProviderAdapter, ProviderError, StopReason, StreamEvent,
};
use frederico_storage::{
    ConversationRepo, Database, MessageEventRepo, MessageRepo, MessageStatus, RunRepo, RunStatus,
};
use frederico_tool_registry::{
    FilesReadTool, Jail, PermissionSet as Perms, Tool, ToolCategory, ToolManifestBuilder,
    ToolRegistry,
};
use futures::stream::{self, BoxStream};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// ScriptedAdapter com cancelamento programável
// ---------------------------------------------------------------------------

/// Adapter que respeita o `cancel: CancellationToken` do `ChatRequest`
/// emitindo `Cancelled` quando ele é disparado.
struct CancelableAdapter {
    id: ProviderId,
    call_count: Mutex<u32>,
}

impl CancelableAdapter {
    fn new(id: impl Into<ProviderId>) -> Self {
        Self {
            id: id.into(),
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ProviderAdapter for CancelableAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: false,
            supports_usage_in_stream: false,
        }
    }
    fn cost_model(&self) -> CostModel {
        CostModel::default()
    }
    async fn complete(&self, _r: ChatRequest) -> Result<ChatResponse, ProviderError> {
        unimplemented!()
    }
    fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        let cancel = request.cancel.clone();
        // Stream que emite um Delta, espera o cancel, e depois emite
        // Cancelled (ou Done, se o cancel não chegar — fallback de
        // 200ms pra evitar deadlock em teste mal-configurado).
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::Delta {
                content: "olá ".to_string(),
            });
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(StreamEvent::Cancelled);
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    let _ = tx.send(StreamEvent::Done { stop_reason: StopReason::Stop });
                }
            }
        });
        let stream = stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        );
        Ok(Box::pin(stream))
    }
    fn cancel(&self, _h: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }
    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Tempdir(PathBuf);

impl Tempdir {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir();
        let counter = TEMPDIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let unique = format!(
            "frederico-execution-engine-cancel-{label}-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            counter,
        );
        let dir = base.join(&unique);
        std::fs::create_dir(&dir).expect("criou tempdir");
        Self(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Tempdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn setup() -> (Database, ToolRegistry, Jail, MessageId, RunId) {
    let workspace = Tempdir::new("ws");
    std::fs::write(workspace.path().join("hello.txt"), "oi").unwrap();
    let db_path = workspace.path().join("cancel.db");
    let db = Database::open(&db_path).await.expect("abre db");
    let mut registry = ToolRegistry::new();
    registry.register(
        ToolManifestBuilder::new(ToolId::new("files.read"), "files")
            .version("0.1.0")
            .display_name("Ler")
            .description("Lê arquivo.")
            .category(ToolCategory::Files)
            .requires_file_read(true)
            .input_schema(frederico_tool_registry::JsonSchema(serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            })))
            .output_schema(frederico_tool_registry::JsonSchema(serde_json::json!({
                "type": "object"
            })))
            .build()
            .unwrap(),
    );
    let jail = Jail::new(workspace.path()).unwrap();
    let conv = ConversationRepo::new(&db)
        .create(&ProviderId::new("simulated"), &ModelId::new("fake"), None)
        .await
        .unwrap();
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .unwrap();
    let run = RunRepo::new(&db).create(&conv.id, &asst.id).await.unwrap();
    (db, registry, jail, asst.id, run.id)
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

/// Cancela **durante** o stream: o adapter emite `Delta "olá "`,
/// depois espera o cancel chegar e emite `Cancelled`. O executor
/// processa o Delta (accumulated = "olá ") e o Cancelled (fecha
/// como Cancelled).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_via_provider_cancelled_event() {
    let (db, registry, jail, message_id, run_id) = setup().await;
    let adapter = Arc::new(CancelableAdapter::new("simulated"));
    let cancel_token = CancellationToken::new();

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];
    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        frederico_tool_registry::static_jail_resolver(jail.clone()),
        db.clone(),
        Perms::default(),
        vec![],
        tools,
        Budget::default(),
        cancel_token.clone(),
    );

    // Spawn que dispara o cancel **durante** o stream (depois do
    // Delta "olá " ser emitido, antes do Done de 200ms).
    let token_clone = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        token_clone.cancel();
    });

    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake"),
            vec![ChatMessage::user("oi")],
        )
        .await
        .expect("executa");

    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Cancelled
    );
    // O Delta "olá " foi persistido antes do cancel chegar.
    assert!(
        outcome.accumulated_content.starts_with("olá "),
        "accumulated_content = {:?}",
        outcome.accumulated_content
    );

    let msg = MessageRepo::new(&db).get(&message_id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Cancelled.as_str());
    let run = RunRepo::new(&db).get(&run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Cancelled.as_str());
}

/// Cancela **antes** do primeiro `tick`: o executor checa o token
/// no passo 0 do loop externo e finaliza como Cancelled imediato,
/// sem processar nenhum evento.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_via_token_before_first_tick() {
    let (db, registry, jail, message_id, run_id) = setup().await;

    /// Adapter que devolve 1 round: Delta + Done. O cancel vai
    /// chegar **antes** do `executor.run()` ser chamado, então o
    /// executor não vai nem entrar no stream.
    struct DoneAdapter(ProviderId);
    #[async_trait]
    impl ProviderAdapter for DoneAdapter {
        fn id(&self) -> ProviderId {
            self.0.clone()
        }
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities {
                supports_stream: true,
                supports_tools: false,
                supports_usage_in_stream: false,
            }
        }
        fn cost_model(&self) -> CostModel {
            CostModel::default()
        }
        async fn complete(&self, _r: ChatRequest) -> Result<ChatResponse, ProviderError> {
            unimplemented!()
        }
        fn stream(
            &self,
            _request: ChatRequest,
        ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
            let events: Vec<StreamEvent> = vec![
                StreamEvent::Delta {
                    content: "x".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ];
            Ok(Box::pin(stream::iter(events)))
        }
        fn cancel(&self, _h: RunHandle) -> Result<(), ProviderError> {
            Ok(())
        }
        fn known_models(&self) -> Vec<(ModelId, &'static str)> {
            vec![]
        }
    }

    let adapter = Arc::new(DoneAdapter(ProviderId::new("simulated")));
    let cancel_token = CancellationToken::new();
    cancel_token.cancel(); // cancela ANTES do run

    let tools: Vec<Arc<dyn Tool>> = vec![];
    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        frederico_tool_registry::static_jail_resolver(jail.clone()),
        db.clone(),
        Perms::default(),
        vec![],
        tools,
        Budget::default(),
        cancel_token,
    );

    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake"),
            vec![ChatMessage::user("oi")],
        )
        .await
        .expect("executa");

    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Cancelled
    );
    // accumulated_content fica vazia porque nenhum Delta foi processado.
    assert_eq!(outcome.accumulated_content, "");
}

#[allow(dead_code)]
fn _silence_unused() {
    let _ = (
        std::marker::PhantomData::<VecDeque<()>>,
        std::marker::PhantomData::<MessageEventRepo<'static>>,
    );
}
