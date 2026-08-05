//! Teste E2E do Passo 10 de auditoria (Fase 3, Etapa 5.x).
//!
//! Valida que o `RunExecutor` grava uma entrada em `tool_audit`
//! (via `DbAuditSink`) toda vez que uma `tool_call` é processada
//! — aprovada + executada, ou rejeitada pelo validador.
//!
//! ## Cenário
//!
//! 1. `ScriptedAdapter` emite `Delta + ToolCall("files.read") + Done{ToolCalls}`
//!    no round 1, e `Delta + Done{Stop}` no round 2.
//! 2. O `RunExecutor` valida + executa a tool_call.
//! 3. O `DbAuditSink` grava em `tool_audit`.
//! 4. O test lê via `ToolAuditRepo::list_for_run` e valida 1 entrada.

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
use frederico_storage::{ConversationRepo, Database, MessageRepo, RunRepo, ToolAuditRepo};
use frederico_tool_registry::{
    FileReadPermission, FilesReadTool, Jail, JsonSchema, PermissionSet, RiskLevel, Tool,
    ToolCategory, ToolManifest, ToolManifestBuilder, ToolRegistry,
};
use futures::stream;
use tokio_util::sync::CancellationToken;

static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let unique = format!(
        "frederico-exec-audit-{}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0),
        n,
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `ScriptedAdapter` que emite: round 1 = Delta + ToolCall + Done{ToolCalls};
/// round 2 = Delta + Done{Stop}.
struct TwoRoundAdapter;

#[async_trait]
impl ProviderAdapter for TwoRoundAdapter {
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
        request: ChatRequest,
    ) -> Result<futures::stream::BoxStream<'static, StreamEvent>, ProviderError> {
        let has_tool_results = request
            .messages
            .iter()
            .any(|m| matches!(m.role, frederico_provider_engine::types::Role::Tool));
        let events: Vec<StreamEvent> = if has_tool_results {
            vec![
                StreamEvent::Delta {
                    content: "ok".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ]
        } else {
            vec![
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
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        Vec::new()
    }
}

fn make_files_read_manifest() -> ToolManifest {
    ToolManifestBuilder::new(frederico_core::ToolId::new("files.read"), "files")
        .display_name("Ler arquivo")
        .description("Lê arquivo do workspace.")
        .risk_level(RiskLevel::Safe)
        .category(ToolCategory::Files)
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
async fn audit_records_files_read_execution() {
    let dir = tempdir();
    let db = Database::open(&dir.join("audit.db")).await.unwrap();

    // Cria um arquivo de teste no workspace.
    let workspace = tempdir();
    std::fs::write(workspace.join("hello.txt"), "Hello, audit!").unwrap();
    let jail = Jail::new(&workspace).unwrap();

    // Tooling: registry com `files.read`, Jail compartilhado, tool concreta.
    let mut registry = ToolRegistry::new();
    registry.register(make_files_read_manifest());
    let files_read: Arc<dyn Tool> = Arc::new(FilesReadTool::new());

    // Cria conv + msg + run.
    let conv = ConversationRepo::new(&db)
        .create(&ProviderId::new("scripted"), &ModelId::new("fake"), None)
        .await
        .unwrap();
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .unwrap();
    let run_repo = RunRepo::new(&db);
    let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
    // Fase 6, Etapa 2: portão único exige `CallingModel` para
    // o `Delta` virar `Streaming`. Simula o orquestrador.
    run_repo
        .set_state(&run.id, frederico_agent_engine::RunState::CallingModel)
        .await
        .unwrap();

    let perms = PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        ..Default::default()
    };

    // Constrói o executor com TwoRoundAdapter + DbAuditSink.
    let adapter: Arc<dyn ProviderAdapter> = Arc::new(TwoRoundAdapter);
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
    )
    .with_audit_sink(Arc::new(
        frederico_execution_engine::audit_sink::DbAuditSink::new(db.clone(), run.id),
    ));

    // Roda o loop. Espera 1 round de tool_call + 1 round final.
    let outcome = executor
        .run(
            asst.id,
            run.id,
            ModelId::new("fake"),
            vec![ChatMessage::user("lê o hello.txt")],
        )
        .await
        .unwrap();

    // 1 tool_call executada.
    assert_eq!(outcome.tool_calls.len(), 1, "esperava 1 tool_call");
    assert!(outcome.tool_calls[0].ok, "files.read deveria ter sucesso");

    // Valida que o audit gravou 1 entrada.
    let audit_repo = ToolAuditRepo::new(&db);
    let entries = audit_repo.list_for_run(&run.id).await.unwrap();
    assert_eq!(
        entries.len(),
        1,
        "esperava 1 entrada de audit, veio {entries:?}"
    );
    assert_eq!(entries[0].tool_id, "files.read");
    assert!(entries[0].result_ok);
    assert!(entries[0].arguments_json.contains("hello.txt"));
    assert!(entries[0].duration_micros >= 0);

    // Limpa o run + message events pra deixar o db consistente.
    drop(executor);
}
