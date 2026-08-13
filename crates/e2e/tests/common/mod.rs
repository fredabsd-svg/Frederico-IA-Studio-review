//! Helpers compartilhados pelos testes E2E em `crates/e2e/tests/`.
//!
//! Ver [`docs/modules/e2e.md`](../../../../docs/modules/e2e.md) §2
//! e [`docs/architecture/testing-strategy.md` §3](../../../../docs/architecture/testing-strategy.md).
//!
//! O ponto de entrada é [`build_orchestrator`]: monta o
//! `ChatOrchestrator` exatamente como a casca Tauri faz
//! ([`frederico_app::build_chat_orchestrator`]), com
//! `Database::open_in_memory`, `RecordingEventSink`,
//! `FileSystemJailResolver` apontando pro `TempDir` do teste,
//! `SystemClock` e o `invoker: Option<Arc<dyn WorkerInvoker>>`
//! que decide se entra o `DocumentWorkerLauncher` real ou só
//! o `FakeWorker` in-process.
//!
//! Cada teste importa via `mod common;` no topo (padrão Cargo
//! de teste de integração).

// `#[allow(dead_code)]` no nível do módulo: cada teste E2E
// compila o `common/mod.rs` em um binário separado
// (target/debug/deps/<test_name>-<hash>.exe), e o Cargo só
// reclama de `dead_code` para o que aquele binário não usa.
// Sem o allow, o `RUSTFLAGS=-D warnings` no `ci.yml` quebra
// em todo PR que adiciona helper novo. O custo é zero
// (helper morto é raro e auditado em revisão).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use frederico_app::composition::{
    build_chat_orchestrator, build_default_allowed_for_run, build_default_tools,
    build_tool_registry, initial_permission_set, initial_permission_set_for_capable_launcher,
    ChatOrchestratorParts,
};
use frederico_app::jail::FileSystemJailResolver;
use frederico_core::{EmbeddingStatus, MemoryId, ModelId, ProviderId};
use frederico_execution_engine::orchestrator::ChatOrchestrator;
use frederico_execution_engine::pipeline_orchestrator::MultimodelOrchestrator;
use frederico_memory::memory_repo::MemoryRepo;
use frederico_model_catalog::Catalog;
use frederico_process_architecture::{FakeWorkerConfig, WorkerManager, WorkerSpawnConfig};
use frederico_provider_engine::event_sink::{EventSink, RecordedEvent, RecordingEventSink};
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_provider_engine::types::{
    ChatRequest, ChatResponse, ProviderError, StreamEvent, Usage,
};
use frederico_provider_engine::ProviderAdapter;
use frederico_security::SystemClock;
use frederico_storage::{Conversation, ConversationRepo, Database, RunStatus};
use frederico_test_support::with_test_timeout_at;
use futures::stream::{self, BoxStream};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// ScriptedProvider — fake provider que devolve sequências programáveis
// ---------------------------------------------------------------------------

/// `ProviderAdapter` fake que mantém uma fila de scripts (um `Vec<StreamEvent>`
/// por chamada a `stream()`). Cada chamada a `stream()` consome o próximo
/// script. Permite simular o loop tool_call do `RunExecutor`:
/// - Round 1: `[ToolCall { ... }]` (modelo decide chamar o tool).
/// - Round 2: `[Delta { "ok" }, Done { Stop }]` (modelo responde depois
///   do `ToolResult`).
///
/// **Por que não usar o `FakeProviderAdapter` do `provider-engine::fake`
/// direto?** Ele tem um único `events: Vec<StreamEvent>` que é emitido
/// em toda chamada a `stream()` — bom pra testes que não se importam com
/// o loop, ruim pra E2E que precisam de rounds diferentes. O `ScriptedProvider`
/// é o análogo "E2E-aware".
pub struct ScriptedProvider {
    id: ProviderId,
    /// Fila de scripts (consumidos um por chamada a `stream()`).
    /// `Mutex` porque `stream()` é `&self` no trait.
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
    /// Total de vezes que `stream()` foi chamado. Útil pra asserções.
    pub call_count: AtomicU64,
    /// Modelo reportado em `known_models()`.
    pub model: ModelId,
}

impl ScriptedProvider {
    /// Constrói o provider com uma fila de scripts. O último script
    /// é re-emitido indefinidamente se `stream()` for chamado mais
    /// vezes do que scripts na fila (fallback pra loops que rodam
    /// mais rounds do que o teste prevê).
    pub fn new(
        id: impl Into<ProviderId>,
        model: impl Into<ModelId>,
        scripts: Vec<Vec<StreamEvent>>,
    ) -> Self {
        Self {
            id: id.into(),
            scripts: Mutex::new(scripts),
            call_count: AtomicU64::new(0),
            model: model.into(),
        }
    }

    /// Atalho: devolve `[Delta, Done]` no primeiro round e encerra o loop
    /// (sem tool call). Útil pra testes que só querem ver o caminho
    /// "modelo responde sem chamar tool".
    #[allow(dead_code)] // nem todo teste usa; alguns constroem o provider com vetor de scripts
    pub fn plain_reply(
        id: impl Into<ProviderId>,
        model: impl Into<ModelId>,
        content: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            model,
            vec![vec![
                StreamEvent::Delta {
                    content: content.into(),
                },
                StreamEvent::Done {
                    stop_reason: frederico_provider_engine::types::StopReason::Stop,
                },
            ]],
        )
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> frederico_provider_engine::provider::AdapterCapabilities {
        frederico_provider_engine::provider::AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: true,
        }
    }

    fn cost_model(&self) -> frederico_provider_engine::provider::CostModel {
        frederico_provider_engine::provider::CostModel::default()
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        Ok(ChatResponse {
            content: "ok".to_string(),
            stop_reason: frederico_provider_engine::types::StopReason::Stop,
            usage: Usage::default(),
        })
    }

    fn stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut scripts = self.scripts.lock().unwrap();
        let events = if scripts.is_empty() {
            // Sem mais scripts: devolve Done pra encerrar o loop.
            vec![StreamEvent::Done {
                stop_reason: frederico_provider_engine::types::StopReason::Stop,
            }]
        } else if n as usize >= scripts.len() {
            // Mais chamadas do que scripts: re-emite o último.
            scripts.last().unwrap().clone()
        } else {
            scripts.swap_remove(0)
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn cancel(
        &self,
        _handle: frederico_provider_engine::provider::RunHandle,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![(self.model.clone(), "E2E scripted model")]
    }
}

// ---------------------------------------------------------------------------
// WorkspaceTempdir — tempdir pra workspaces de conversa + cleanup
// ---------------------------------------------------------------------------

/// Tempdir que serve de raiz pros workspaces das conversas no
/// `FileSystemJailResolver`. Cada teste ganha o seu; cleanup
/// automático no `Drop`.
pub struct WorkspaceTempdir {
    pub dir: TempDir,
}

impl WorkspaceTempdir {
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("cria tempdir raiz"),
        }
    }

    /// Path raiz (`<tempdir>/workspaces/`). O `FileSystemJailResolver`
    /// cria `<workspaces>/<cid>/` por conversa.
    pub fn workspaces_root(&self) -> PathBuf {
        self.dir.path().join("workspaces")
    }
}

// ---------------------------------------------------------------------------
// fake_invoker — WorkerHandle in-process (Fase 5, ADR-0024 §D3)
// ---------------------------------------------------------------------------

/// Spawna o `FakeWorker` in-process (`WorkerManager::spawn_in_process`
/// com `FakeWorkerConfig::default()`) e devolve o `Arc<dyn WorkerInvoker>`
/// + o `WorkerManager` (que precisa ficar vivo no escopo do teste —
///   quando o `WorkerManager` é droppado, a task do ator termina via
///   `command_rx` fechar; o `WorkerHandle` sozinho não segura o ator).
///
/// **Por que devolver o manager junto:** o helper original em
/// `frederico_app::composition::tests:360-368` faz
/// `let (_manager, handle) = ...` — o `_manager` é descartado no fim
/// do escopo da função, e o ator morre junto. Pra testes que usam
/// o handle **após** o helper retornar (todo E2E), o ator já caiu
/// e o `invoke` falha com `ProcessError::Transport`. Esta versão
/// devolve o manager pro caller segurar.
#[allow(dead_code)] // nem todo teste usa (alguns constroem o launcher real)
pub async fn fake_invoker() -> (Arc<dyn frederico_core::WorkerInvoker>, WorkerManager) {
    let (manager, handle) =
        WorkerManager::spawn_in_process(FakeWorkerConfig::default(), WorkerSpawnConfig::default())
            .await
            .expect("spawn fake worker in-process");
    (Arc::new(handle), manager)
}

// ---------------------------------------------------------------------------
// build_orchestrator — ponto único de montagem (mesma função que a casca)
// ---------------------------------------------------------------------------

/// Resultado da montagem. Devolve os handles pros componentes
/// que o caller precisa asserir. `#[allow(dead_code)]` nos campos
/// porque cada teste consome um subconjunto diferente — manter
/// a struct uniforme é mais ergonômico que derivar N variantes.
#[allow(dead_code)]
pub struct OrchestratorHandle {
    pub orchestrator: Arc<ChatOrchestrator>,
    pub db: Arc<Database>,
    pub sink: Arc<RecordingEventSink>,
    pub provider: Arc<ScriptedProvider>,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub workspace: WorkspaceTempdir,
    /// `WorkerManager` do `FakeWorker` in-process. Fica vivo no
    /// `OrchestratorHandle` — quando o handle é droppado, o manager
    /// também é, terminando o ator limpo. Sem isso, o `invoke` falha
    /// com `ProcessError::Transport` porque o ator já caiu.
    ///
    /// `None` quando `invoker` é `None` (degradação declarada).
    pub worker_manager: Option<WorkerManager>,
}

/// Monta o `ChatOrchestrator` exatamente como a casca Tauri faz
/// ([`frederico_app::build_chat_orchestrator`]). Reuso o ponto único
/// — não monta `ChatOrchestrator::new` direto. Isso é o que garante
/// a regra "os E2E chamam a mesma função que a casca"
/// ([`docs/architecture/testing-strategy.md` §3 "Regra da composição
/// compartilhada")].
///
/// ## Parâmetros
///
/// - `invoker`: `Some(h)` = o `WorkerHandle`/`DocumentWorkerLauncher`
///   entra no `ToolRegistry` (3 tools) e na allowlist (3 ids);
///   `documents` permission vira `Full`. `None` = degradação
///   declarada (1 tool, 1 id, `documents: None`).
/// - `provider`: o `ScriptedProvider` (fila de scripts por round).
///   Já é o `Arc<dyn ProviderAdapter>` que o `ProviderMap` carrega.
/// - `provider_id`, `model_id`: id do provedor e do modelo. Têm que
///   bater com o que o `provider` reporta em `id()` e `known_models()`.
pub async fn build_orchestrator(
    invoker: Option<Arc<dyn frederico_core::WorkerInvoker>>,
    worker_manager: Option<WorkerManager>,
    provider: Arc<ScriptedProvider>,
    provider_id: ProviderId,
    model_id: ModelId,
    db: Option<Arc<Database>>,
) -> OrchestratorHandle {
    let db = match db {
        Some(d) => d,
        None => Arc::new(Database::open_in_memory().await.expect("open in-memory db")),
    };
    let sink: Arc<RecordingEventSink> = Arc::new(RecordingEventSink::new());
    let mut provider_map = ProviderMap::new();
    provider_map.insert(provider.clone());
    let providers = Arc::new(provider_map);
    let runs = RunRegistry::new();
    let clock: Arc<dyn frederico_security::Clock> = Arc::new(SystemClock);
    let catalog: Arc<Catalog> = Arc::new(Catalog::load().clone());

    let has_invoker = invoker.is_some();
    let invoker_for_tools = invoker.clone();
    // Etapa 4 da Fase 7: os E2E não precisam do subsistema exec
    // (eles testam outras fases). Passamos `None` pro `exec_deps`
    // pra ficar com o shape antigo (`[FilesReadTool]` ou
    // `[FilesReadTool, DocsGenerateTool, DocsInspectTool]`).
    let tools = build_default_tools(invoker_for_tools, None);
    let tool_registry = build_tool_registry(&tools);
    let allowed_for_run = build_default_allowed_for_run(invoker, None);
    let permission_set = if has_invoker {
        initial_permission_set_for_capable_launcher()
    } else {
        initial_permission_set()
    };

    let workspace = WorkspaceTempdir::new();
    let jail_resolver: Arc<dyn frederico_tool_registry::JailResolver> =
        Arc::new(FileSystemJailResolver::new(workspace.workspaces_root()));

    // `MultimodelOrchestrator` (Etapa 5 PR 2 da Fase 6,
    // ADR-0028). Mesmo factory que a casca Tauri chama.
    // Os E2E de pipeline (`e2e_pipeline_sequencial_e2e.rs`)
    // consomem via `orchestrator.start_pipeline(...)`.
    let multimodel_orchestrator = Arc::new(MultimodelOrchestrator::new(
        db.clone(),
        runs.clone(),
        sink.clone() as Arc<dyn EventSink>,
        catalog.clone(),
        clock.clone(),
        providers.clone(),
        tool_registry.clone(),
        jail_resolver.clone(),
        tools.clone(),
        allowed_for_run.clone(),
        permission_set.clone(),
    ));

    let parts = ChatOrchestratorParts {
        providers,
        runs,
        sink: sink.clone() as Arc<dyn EventSink>,
        db: db.clone(),
        clock,
        catalog: catalog.clone(),
        tool_registry,
        jail_resolver,
        tools,
        allowed_for_run,
        permission_set,
        memory_extractor: None,
        // SpecialistRegistry (Etapa 3 PR 1) + PermissionLoader
        // (Etapa 3 PR 2) — Etapa 4 PR 2 fecha o portão do
        // subagente consumindo esses dois via
        // `SubagentRunner`. Mesma factory que a casca Tauri
        // chama (`build_specialist_registry`) e o Tauri
        // command `list_specialists` consome.
        specialist_registry: frederico_app::composition::build_specialist_registry(catalog)
            .registry,
        permission_loader: Arc::new(frederico_tool_registry::PermissionLoader::new()),
        // `MultimodelOrchestrator` (Etapa 5 PR 2). O
        // `ChatOrchestrator::start_pipeline` / `cancel_pipeline`
        // delegam pra ele.
        multimodel_orchestrator: Some(multimodel_orchestrator),
        // Etapa 6 da Fase 7 (ADR-0033): allowlist do proxy
        // de rede do sandbox. Vazio = deny-by-default nos
        // E2E (os tests existentes não exercitam rede).
        network_allowlist:
            frederico_security::network::NetworkAllowlist::new(),
    };

    let orchestrator = Arc::new(build_chat_orchestrator(parts));

    OrchestratorHandle {
        orchestrator,
        db,
        sink,
        provider,
        provider_id,
        model_id,
        workspace,
        worker_manager,
    }
}

// ---------------------------------------------------------------------------
// create_test_conversation — wrapper trivial
// ---------------------------------------------------------------------------

/// Cria uma conversa de teste. `title = Some("E2E test")` por default.
pub async fn create_test_conversation(
    db: &Database,
    provider_id: &ProviderId,
    model_id: &ModelId,
    title: Option<&str>,
) -> Conversation {
    ConversationRepo::new(db)
        .create(provider_id, model_id, title)
        .await
        .expect("cria conversa de teste")
}

// ---------------------------------------------------------------------------
// wait_for_run_completion — poll o sink até RunStatus final
// ---------------------------------------------------------------------------

/// Espera o `RunStatus` final do `run_id` aparecer no `RecordingEventSink`.
/// Retorna o `RunStatus`. Estoura `panic!` se o timeout estourar sem
/// status final — o teste fica vermelho com mensagem clara.
///
/// **Por que polling e não `recv` num channel?** O `RecordingEventSink`
/// é `Mutex<Vec<...>>` (sem semântica de notificação). Polling a cada
/// 10ms é suficiente pro E2E (o loop inteiro leva < 100ms com FakeWorker).
pub async fn wait_for_run_completion(
    sink: &RecordingEventSink,
    run_id: frederico_core::RunId,
    timeout: Duration,
) -> RunStatus {
    let final_statuses = [
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Timeout,
    ];

    let result = with_test_timeout_at("wait_for_run_completion", timeout, async move {
        loop {
            for ev in sink.events_for(run_id) {
                if let RecordedEvent::Status(s) = ev {
                    if final_statuses.contains(&s) {
                        return s;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    result.unwrap_or_else(|_| {
        panic!(
            "timeout esperando RunStatus final do run {run_id} (sink tem {} eventos no total)",
            sink.events.lock().unwrap().len()
        )
    })
}

// ---------------------------------------------------------------------------
// Helpers de pipeline de memória (Etapa 3 da Fase de Ligação)
// ---------------------------------------------------------------------------

/// Espera o `MemoryExtractor` (em background) classificar o job
/// e persistir uma memória. Retorna o `MemoryId` da primeira
/// memória persistida. Usado pelos E2E determinístico
/// (CI de PR) e real (noturno).
pub async fn wait_for_classified_memory(db: &Database, timeout: Duration) -> MemoryId {
    let start = std::time::Instant::now();
    loop {
        let repo = MemoryRepo::new(db);
        match repo.list_pending_embeddings(100).await {
            Ok(pending) if !pending.is_empty() => return pending[0].id,
            Ok(_) => {}
            Err(e) => {
                eprintln!("list_pending_embeddings errou (transient?): {e}");
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "timeout ({}s) esperando classificador processar o job. \
                 Possiveis causas: provider indisponivel, classificador \
                 decidiu descartar (confidence < threshold), ou bug no \
                 pipeline. Verifique os logs do MemoryExtractor.",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Espera o `EmbeddingWorker` (em background) calcular o
/// embedding da memória e marcar `embedding_status = Ready`.
pub async fn wait_for_embedding(db: &Database, id: MemoryId, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        let repo = MemoryRepo::new(db);
        if let Ok(Some(m)) = repo.get(&id).await {
            if m.embedding_status == EmbeddingStatus::Ready {
                return;
            }
            if m.embedding_status == EmbeddingStatus::Failed {
                panic!(
                    "EmbeddingWorker marcou memoria {id} como Failed. \
                     Causas provaveis: timeout do provider, erro HTTP, \
                     ou dim mismatch. Verifique os logs."
                );
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "timeout ({}s) esperando embedding da memoria {id}. \
                 Verifique se o EmbeddingWorker foi spawnado.",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Cria uma conversa de teste com `provider_id` e `model_id`
/// fixos. Retorna `(Conversation, conversation_id_string)`.
/// O `conversation_id_string` é o formato que o
/// `MemoryExtractionJob` espera.
pub async fn create_memory_test_conversation(
    db: &Database,
) -> (frederico_storage::Conversation, String) {
    let conv_repo = ConversationRepo::new(db);
    let conv = conv_repo
        .create(
            &ProviderId::new("openai"),
            &ModelId::new("gpt-4o-mini"),
            Some("e2e memory"),
        )
        .await
        .expect("cria conversa de teste");
    let id_str = conv.id.as_uuid().to_string();
    (conv, id_str)
}

/// Helper de timestamp "agora" — usa `Utc::now()`.
pub fn memory_job_now() -> chrono::DateTime<Utc> {
    Utc::now()
}
