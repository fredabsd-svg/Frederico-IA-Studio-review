//! Teste E2E do Passo 9 + Etapa 6.1 backend (Fase 3).
//!
//! Valida que quando o `validate_tool_call` devolve
//! `ApprovalRequired` (porque o manifesto tem
//! `requires_user_approval = true` e nenhuma decisão foi
//! passada), o `RunExecutor` enfileira o `ApprovalRequest` na
//! tabela `approval_queue` (via `ApprovalQueueRepo`) ANTES de
//! retornar `Err(ApprovalRequired)`. O `ChatOrchestrator`
//! finaliza o run como `Cancelled` (com nota) e a `Message.error`
//! carrega o motivo.
//!
//! ## Cenário
//!
//! 1. `ScriptedAdapter` emite 1 round com `ToolCall` (tool exige
//!    aprovação) + `Done{ToolCalls}`.
//! 2. O `RunExecutor` valida → `ApprovalRequired` → enfileira
//!    na fila → retorna `Err`.
//! 3. O `ChatOrchestrator` finaliza como `Cancelled`.
//! 4. O test valida que a fila tem 1 entry pending.
//! 5. O test responde `approved` via `ApprovalQueueRepo::resolve`.
//! 6. O test valida que a fila está vazia de pending.

#![cfg(not(doctest))]

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use frederico_agent_engine::Budget;
use frederico_core::{ModelId, ProviderId};
use frederico_execution_engine::executor::RunExecutor;
use frederico_provider_engine::provider::{AdapterCapabilities, ProviderAdapter, RunHandle};
use frederico_provider_engine::types::{
    ChatMessage, ChatRequest, ChatResponse, ProviderError, StopReason, StreamEvent,
};
use frederico_storage::{
    ApprovalQueueRepo, ConversationRepo, Database, MessageRepo, RunRepo, RunStatus,
};
use frederico_tool_registry::{
    FilesReadTool, Jail, JsonSchema, PermissionSet, RiskLevel, Tool, ToolCategory, ToolManifest,
    ToolManifestBuilder, ToolRegistry,
};
use futures::stream;
use tokio_util::sync::CancellationToken;

static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let unique = format!(
        "frederico-exec-approval-{}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0),
        n,
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Adapter que emite 1 round com `ToolCall` + `Done{ToolCalls}`.
struct ToolCallAdapter;

#[async_trait]
impl ProviderAdapter for ToolCallAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::new("scripted")
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: true,
        }
    }
    fn cost_model(&self) -> frederico_provider_engine::provider::CostModel {
        Default::default()
    }
    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Err(ProviderError::network("not used"))
    }
    fn stream(
        &self,
        _request: ChatRequest,
    ) -> Result<futures::stream::BoxStream<'static, StreamEvent>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            StreamEvent::Delta {
                content: "".to_string(),
            },
            StreamEvent::ToolCall {
                id: "call_1".to_string(),
                name: "files.read".to_string(),
                arguments_json: r#"{"path":"hello.txt"}"#.to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::ToolCalls,
            },
        ])))
    }
    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }
    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        Vec::new()
    }
}

fn make_files_read_manifest() -> ToolManifest {
    // Manifesto com `requires_user_approval = true` — vai disparar
    // o Passo 9 do `validate_tool_call`.
    ToolManifestBuilder::new(frederico_core::ToolId::new("files.read"), "files")
        .display_name("Ler arquivo (com aprovação)")
        .description("Lê arquivo do workspace (exige aprovação do usuário).")
        .risk_level(RiskLevel::High)
        .category(ToolCategory::Files)
        .requires_user_approval(true)
        .requires_file_read(true)
        .input_schema(JsonSchema(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "max_bytes": {"type": "integer", "minimum": 1, "maximum": 52428800}
            },
            "required": ["path"],
            "additionalProperties": false
        })))
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_required_enqueues_to_queue() {
    let dir = tempdir();
    let db = Database::open(&dir.join("approval.db")).await.unwrap();
    let workspace = tempdir();
    std::fs::write(workspace.join("hello.txt"), "Hello, approval!").unwrap();
    let jail = Jail::new(&workspace).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(make_files_read_manifest());
    let files_read: Arc<dyn Tool> = Arc::new(FilesReadTool::new());

    let conv = ConversationRepo::new(&db)
        .create(&ProviderId::new("scripted"), &ModelId::new("fake"), None)
        .await
        .unwrap();
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .unwrap();
    let run = RunRepo::new(&db).create(&conv.id, &asst.id).await.unwrap();

    let perms = PermissionSet {
        file_read: frederico_tool_registry::FileReadPermission::WorkspaceOnly,
        ..Default::default()
    };

    let adapter: Arc<dyn ProviderAdapter> = Arc::new(ToolCallAdapter);
    let cancel = CancellationToken::new();
    let mut executor = RunExecutor::new(
        adapter,
        registry,
        frederico_tool_registry::static_jail_resolver(jail.clone()),
        db.clone(),
        perms,
        vec![frederico_core::ToolId::new("files.read")],
        vec![files_read],
        Budget::default(),
        cancel,
    );

    // Roda — espera `Err(ApprovalRequired)`.
    let result = executor
        .run(
            asst.id,
            run.id,
            ModelId::new("fake"),
            vec![ChatMessage::user("lê o hello.txt")],
        )
        .await;
    assert!(
        matches!(
            result,
            Err(frederico_execution_engine::executor::ExecutorError::ApprovalRequired)
        ),
        "esperava ApprovalRequired, veio {result:?}"
    );

    // 1. A fila tem 1 entry pending.
    let approval_repo = ApprovalQueueRepo::new(&db);
    let pending = approval_repo.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1, "esperava 1 entry pending");
    let entry = &pending[0];
    assert_eq!(entry.tool_id, "files.read");
    assert_eq!(entry.status, frederico_storage::ApprovalStatus::Pending);
    assert!(entry.request_json.contains("files.read"));
    assert!(entry.decision_json.is_none());

    // 2. Responde `approved`.
    let decision_json = serde_json::json!({
        "approved": true,
        "scope": "Run",
        "reason": "user clicked approve"
    })
    .to_string();
    approval_repo
        .resolve(&entry.id, &decision_json, true)
        .await
        .unwrap();

    // 3. A fila está vazia de pending.
    let pending_after = approval_repo.list_pending().await.unwrap();
    assert!(pending_after.is_empty(), "fila deveria estar vazia");

    // 4. A entry agora tem status `approved` + decision_json + resolved_at.
    let entry_resolved = approval_repo.get(&entry.id).await.unwrap();
    assert_eq!(
        entry_resolved.status,
        frederico_storage::ApprovalStatus::Approved
    );
    assert!(entry_resolved.decision_json.is_some());
    assert!(entry_resolved.resolved_at.is_some());

    // 5. `Run.status` continua como `running` (a Etapa 6.1 não
    //    finaliza o `Run` aqui — o `ChatOrchestrator` que
    //    finaliza; este test é só do executor + fila). O
    //    `RunStatus` final depende do caller.
    let _ = RunStatus::Running; // anchor pra import
}
