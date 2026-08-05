//! Adaptador fake reutilizável pelos testes E2E do
//! `frederico-execution-engine`.
//!
//! O `ScriptedAdapter` devolve um script de eventos por chamada de
//! `stream()`. O primeiro `stream()` consome o primeiro script da
//! fila, o segundo consome o segundo, e assim por diante. É o que
//! o `RunExecutor` precisa pra simular múltiplos rounds
//! (`Delta + ToolCall + Done{ToolCalls}` no round 1, `Delta + Done{Stop}`
//! no round 2, etc.).

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::provider::{AdapterCapabilities, CostModel, RunHandle};
use frederico_provider_engine::{
    ChatRequest, ChatResponse, ProviderAdapter, ProviderError, StreamEvent,
};
use futures::stream::{self, BoxStream};

/// Adapter que devolve um script de eventos por chamada de `stream()`.
pub struct ScriptedAdapter {
    id: ProviderId,
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    call_count: Mutex<u32>,
}

impl ScriptedAdapter {
    #[allow(dead_code)] // usado em e2e.rs e recovery.rs mas não em todos os tests
    pub fn new(id: impl Into<ProviderId>, scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            id: id.into(),
            scripts: Mutex::new(scripts.into_iter().collect()),
            call_count: Mutex::new(0),
        }
    }

    #[allow(dead_code)]
    pub fn call_count(&self) -> u32 {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: false,
        }
    }

    fn cost_model(&self) -> CostModel {
        CostModel::default()
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        unimplemented!("ScriptedAdapter só faz stream")
    }

    fn stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        let next = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProviderError::network("ScriptedAdapter: sem mais scripts"))?;
        Ok(Box::pin(stream::iter(next)))
    }

    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![(ModelId::new("fake-model-v1"), "Fake Model v1")]
    }
}

/// Helper: cria um arquivo e devolve o `Tempdir`. Cada teste usa um
/// `Tempdir` novo pra não colidir com outros em paralelo.
pub struct Tempdir(std::path::PathBuf);

impl Tempdir {
    pub fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
        let unique = format!(
            "frederico-execution-engine-{label}-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            counter,
        );
        let dir = base.join(&unique);
        std::fs::create_dir(&dir).expect("criou tempdir");
        Self(dir)
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Tempdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Cria o manifesto `files.read` (idêntico ao do `e2e.rs`).
pub fn files_read_manifest() -> frederico_tool_registry::ToolManifest {
    use frederico_core::ToolId;
    use frederico_tool_registry::{JsonSchema, ToolCategory, ToolManifestBuilder};

    ToolManifestBuilder::new(ToolId::new("files.read"), "files")
        .version("0.1.0")
        .display_name("Ler arquivo")
        .description("Lê o conteúdo de um arquivo dentro do workspace.")
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
        .output_schema(JsonSchema(serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string"},
                "truncated": {"type": "boolean"},
                "bytes_read": {"type": "integer"}
            },
            "required": ["content", "truncated", "bytes_read"]
        })))
        .build()
        .expect("manifesto bem-formado")
}

/// Setup de workspace + db + registry + jail + message + run. Cada
/// teste chama isso pra ter um `Database` temp + um arquivo
/// `hello.txt` no workspace.
pub async fn setup_executor() -> (
    Tempdir,
    frederico_storage::Database,
    frederico_tool_registry::ToolRegistry,
    frederico_tool_registry::Jail,
    frederico_core::MessageId,
    frederico_core::RunId,
) {
    use frederico_core::{ModelId, ProviderId};
    use frederico_storage::{ConversationRepo, MessageRepo, RunRepo};
    use frederico_tool_registry::{Jail, ToolRegistry};

    let workspace = Tempdir::new("recovery");
    std::fs::write(workspace.path().join("hello.txt"), "Hello from fixture!").unwrap();
    let db_path = workspace.path().join("test.db");
    let db = frederico_storage::Database::open(&db_path)
        .await
        .expect("abre db");
    let mut registry = ToolRegistry::new();
    registry.register(files_read_manifest());
    let jail = Jail::new(workspace.path()).expect("jail");
    let conv = ConversationRepo::new(&db)
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .unwrap();
    let run_repo = RunRepo::new(&db);
    let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
    // Fase 6, Etapa 2: o executor usa o portão único de mudança
    // de estado (`apply_transition`). Para o `Delta` que o adapter
    // emite virar `Streaming`, o run precisa estar em
    // `CallingModel` — caso contrário o portão rejeita com
    // `InvalidTransition { from: Created }`. Em produção, a
    // transição `Created → ... → CallingModel` é feita pelo
    // orquestrador antes de chamar o executor (Etapa 4 da Fase
    // 3); em teste, simulamos manualmente no helper compartilhado.
    run_repo
        .set_state(&run.id, frederico_agent_engine::RunState::CallingModel)
        .await
        .unwrap();
    (workspace, db, registry, jail, asst.id, run.id)
}
