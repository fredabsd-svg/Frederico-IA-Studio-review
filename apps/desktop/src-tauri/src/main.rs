// Frederico IA Studio — casca Tauri + comandos IPC.
//
// Esta é a **casca** do app (ver ADR-0003). A casca:
//   1. Inicializa logs.
//   2. Resolve o caminho do banco de dados via diretórios do Windows.
//   3. Abre o banco SQLite rodando as migrações.
//   4. Monta o `ChatOrchestrator` (catálogo + adapters + RunRegistry
//      + EventSink + Clock + DB).
//   5. Expõe operações ao frontend via `tauri::command`.
//
// Toda a lógica de negócio vive nos crates de `crates/` — esta casca
// não a duplica.

use std::path::PathBuf;
use std::sync::Arc;

use frederico_core::{MemoryHit, MemoryScopeType, MemorySourceType, WorkerInvoker};
use frederico_diagnostics as diagnostics;
use frederico_execution_engine::orchestrator::ChatOrchestrator;
use frederico_execution_engine::recovery::{
    spawn_recover_stale_runs, DEFAULT_STALE_THRESHOLD_SECS,
};
use frederico_memory::retriever::{HybridRetriever, Retriever};
use frederico_memory::MemoryRepo;
use frederico_model_catalog::Catalog;
use frederico_provider_engine::openai_compat::OpenAiCompatAdapter;
use frederico_provider_engine::{EventSink, ProviderMap, RunRegistry};
use frederico_security::windows::WindowsCredentialStore;
use frederico_security::{Clock, CredentialStore, SystemClock};
use frederico_shared_contracts::{
    AppOp, ConversationView, CorrectionResultView, IpcRequest, IpcResponse, MemoryHitView,
    MemoryView, MessageEventView, MessageSendResult, MessageView, ModelDescriptorView,
    ProviderConfigView, ScoreBreakdownView,
};
use frederico_storage::{ApprovalQueueRepo, ConversationRepo, Database, MessageRepo};
use tauri::{Manager, State};

mod sink;

// ADR-0023 — Etapa 2.A da fase-ligação. O launcher do
// `document-worker` é um `Option` no `AppState`: quando o
// runtime é resolvido (em dev, com o `bootstrap.ps1` rodado),
// o launcher está disponível e o frontend React pode invocar
// `docs.generate` via `DocumentWorkerInvoke` (caminho de
// invoke direto, sem passar pelo `ChatOrchestrator`). Quando
// o runtime está indisponível, o `Option` é `None` e a UI
// mostra "indisponível" no diagnóstico. Ver ADR-0023 §D3.
use frederico_app::launcher::DocumentWorkerLauncher;

/// Estado compartilhado passado aos comandos Tauri.
///
/// O `credentials` é a instância **real** de
/// `WindowsCredentialStore` (DPAPI / Credential Manager). Os adapters
/// a recebem via `build_provider_map` e os comandos IPC
/// `ProviderSetCredential`/`ProviderDeleteCredential` a usam
/// diretamente — tudo passa por DPAPI, nunca por shim em memória.
struct AppState {
    db: Arc<Database>,
    orch: Arc<ChatOrchestrator>,
    credentials: Arc<WindowsCredentialStore>,
    /// Launcher do `document-worker` (ADR-0023, Etapa 2.A).
    /// `None` quando o runtime está indisponível (em produção
    /// sem `bundle.resources` populado, ou em dev sem
    /// `bootstrap.ps1` rodado). A UI mostra o status via
    /// `DocumentWorkerStatus` e pode chamar
    /// `DocumentWorkerInvoke` quando `Some`.
    document_worker: Option<Arc<DocumentWorkerLauncher>>,
    /// Provider de embedding (Fase de Ligação, Etapa 3).
    /// `OpenRouterEmbeddingAdapter` quando a key do OpenRouter
    /// está disponível (DPAPI ou env var); `NoopEmbeddingAdapter`
    /// caso contrário (retriever vira lexical-only). O
    /// `build_retriever` helper local lê daqui.
    embedding_provider: Arc<dyn frederico_memory::embedding::EmbeddingProvider>,
}

/// Diretório local de dados do aplicativo (Windows: `%LOCALAPPDATA%\studio\frederico\ia`).
/// Resolvido uma vez por chamada — `ProjectDirs::from` é barato,
/// mas é guard de invariante (`expect` em vez de `unwrap_or`).
fn data_local_dir() -> PathBuf {
    directories::ProjectDirs::from("studio", "frederico", "ia")
        .expect("diretórios do projeto resolvem em Windows")
        .data_local_dir()
        .to_path_buf()
}

/// Resolve o caminho do banco de dados.
fn resolve_db_path() -> PathBuf {
    data_local_dir().join("frederico.db")
}

/// Constrói o `RuntimeContext` (ADR-0023 §D1) com os 3
/// candidatos de runtime do `document-worker`. Função pura
/// em relação ao `document-worker` em si — só lê env,
/// `tauri::AppHandle::path()`, e o filesystem. Não spawna
/// nada.
///
/// **Precedência** (delegada ao `resolve_document_worker_runtime`):
/// 1. `FREDERICO_DOCUMENT_WORKER_RUNTIME` (env var) — overrides
///    pra testes e setups não-padrão.
/// 2. `app.path().resolve("document-worker", Resource)` —
///    bundle.resources do Tauri. Em dev, o Tauri pode
///    retornar um path que não existe; em produção, depois
///    da Fase 9 do PROMPT MESTRE, vai retornar o path
///    empacotado.
/// 3. `CARGO_MANIFEST_DIR/../workers/document-worker/runtime`
///    — caminho de dev no repositório. Só existe se o
///    `bootstrap.ps1` rodou.
///
/// **Por que essa função é separada:** o `frederico-app` é
/// puro (sem `tauri`), então a lógica de construir o
/// `RuntimeContext` (que precisa de `AppHandle`) mora aqui
/// na casca. O `frederico-app` recebe o `RuntimeContext` já
/// materializado e só itera sobre os 3 candidatos.
fn resolve_runtime_context(app: &tauri::AppHandle) -> frederico_app::runtime::RuntimeContext {
    // Opção 1: env var.
    let env_override = std::env::var("FREDERICO_DOCUMENT_WORKER_RUNTIME")
        .ok()
        .map(PathBuf::from);

    // Opção 2: recursos do app (bundle.resources do Tauri).
    // Em dev, `Resource` pode apontar pra um diretório que
    // não existe; nesse caso, `try_exists` no resolvedor
    // rejeita silenciosamente e cai pra próxima opção. Por
    // isso usamos `Option` em vez de `Result` aqui.
    let app_resources = app
        .path()
        .resolve("document-worker", tauri::path::BaseDirectory::Resource)
        .ok();

    // Opção 3: caminho de dev no repositório. Em produção
    // (`.exe` instalado), `CARGO_MANIFEST_DIR` aponta pro
    // diretório de instalação, não pro repo — nesse caso o
    // caminho `<manifest>/../workers/document-worker/runtime`
    // não existe, e o resolvedor rejeita.
    //
    // **Atenção:** `CARGO_MANIFEST_DIR` é o diretório do
    // `Cargo.toml` do crate `frederico-desktop`, que é
    // `apps/desktop/src-tauri/`. O caminho
    // `<...>/../workers/document-worker/runtime` é relativo
    // ao `apps/desktop/src-tauri/`, que é
    // `apps/desktop/src-tauri/../workers/...` = `apps/workers/...`.
    // **Errado.** O correto é subir mais um nível:
    // `<...>/../../workers/document-worker/runtime` =
    // `apps/../../workers/...` = `workers/...` (correto, raiz
    // do repo).
    let dev_repo = std::env::var("CARGO_MANIFEST_DIR").ok().map(|d| {
        PathBuf::from(d)
            .parent() // apps/desktop/src-tauri → apps/desktop
            .and_then(|p| p.parent()) // apps/desktop → apps
            .and_then(|p| p.parent()) // apps → repo root
            .map(|p| p.join("workers").join("document-worker").join("runtime"))
    });
    let dev_repo = dev_repo.flatten();

    frederico_app::runtime::RuntimeContext {
        env_override,
        app_resources,
        dev_repo,
    }
}

/// Constrói o `ProviderMap` com adapters pré-registrados. O
/// `simulated` está sempre disponível (testes + modo demo). Os
/// reais (OpenAI, Anthropic) dependem de credencial cadastrada
/// — o adapter retorna `ProviderErrorKind::Auth` até lá.
fn build_provider_map(credentials: Arc<dyn CredentialStore>) -> Arc<ProviderMap> {
    let mut map = ProviderMap::new();
    // simulated — sempre presente.
    map.insert(Arc::new(
        frederico_provider_engine::fake::trait_level::FakeProviderAdapter::new("simulated"),
    ));
    // OpenAI
    map.insert(Arc::new(OpenAiCompatAdapter::with_bearer_auth(
        "openai",
        "https://api.openai.com/v1",
        credentials.clone(),
    )));
    // OpenRouter — com `HTTP-Referer`/`X-Title` para atribuição.
    map.insert(Arc::new(OpenAiCompatAdapter::with_openrouter_auth(
        "openrouter",
        "https://openrouter.ai/api/v1",
        credentials.clone(),
    )));
    // DeepSeek
    map.insert(Arc::new(OpenAiCompatAdapter::with_bearer_auth(
        "deepseek",
        "https://api.deepseek.com/v1",
        credentials.clone(),
    )));
    // Mistral
    map.insert(Arc::new(OpenAiCompatAdapter::with_bearer_auth(
        "mistral",
        "https://api.mistral.ai/v1",
        credentials.clone(),
    )));
    // NVIDIA NIM
    map.insert(Arc::new(OpenAiCompatAdapter::with_bearer_auth(
        "nvidia",
        "https://integrate.api.nvidia.com/v1",
        credentials.clone(),
    )));
    // Ollama (local, sem auth)
    map.insert(Arc::new(OpenAiCompatAdapter::without_auth(
        "ollama",
        "http://localhost:11434/v1",
        credentials.clone(),
    )));
    // LM Studio (local, sem auth)
    map.insert(Arc::new(OpenAiCompatAdapter::without_auth(
        "lmstudio",
        "http://localhost:1234/v1",
        credentials.clone(),
    )));
    // Anthropic
    map.insert(Arc::new(
        frederico_provider_engine::anthropic::AnthropicAdapter::new(credentials),
    ));
    Arc::new(map)
}

fn main() {
    diagnostics::init();
    tracing::info!("Frederico IA Studio iniciando…");

    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let db_path = resolve_db_path();
            tracing::info!(?db_path, "abrindo banco SQLite");

            let db = tauri::async_runtime::block_on(async { Database::open(&db_path).await })
                .expect("abre o banco SQLite");
            let db = Arc::new(db);

            // `purge_expired` na inicialização (Etapa 4 da Fase 4,
            // `ADR-0014 §3` — coleta preguiçosa na leitura, com
            // purge manual no startup). Roda DEPOIS das migrations
            // (o `Database::open` aplica) e ANTES do `ChatOrchestrator`
            // ser construído. Best-effort: se o banco estiver em uso
            // por outro processo (`SQLITE_BUSY`), loga e segue —
            // a próxima inicialização tenta de novo. Custo: 1
            // `DELETE` que lista os IDs deletados pra log.
            //
            // O `SystemClock` ainda não foi construído aqui
            // (declarado mais abaixo), então usamos `chrono::Utc::now()`
            // direto pra este ponto de boot — o custo de não ter
            // um `Clock` injetado aqui é zero (não é caminho
            // crítico, falha silenciosa).
            //
            // O `setup` do Tauri não é `async`, então usamos
            // `tauri::async_runtime::block_on` (mesma estratégia
            // do `Database::open` logo acima).
            tauri::async_runtime::block_on(async {
                let memory_repo = MemoryRepo::new(&db);
                match memory_repo.purge_expired(chrono::Utc::now()).await {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(
                            memory.purge_expired = deleted,
                            "memórias expiradas/superseded purgadas na inicialização"
                        );
                    }
                    Ok(_) => {
                        tracing::debug!("nenhuma memória expirada/superseded pra purgar");
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "falha ao purgar memórias expiradas na inicialização — seguindo adiante"
                        );
                    }
                }
            });

            let clock: Arc<dyn Clock> = Arc::new(SystemClock);

            // `WindowsCredentialStore` real (DPAPI / Credential
            // Manager). Esta é a **única** instância no processo:
            // os adapters a recebem via `build_provider_map` e os
            // comandos IPC `ProviderSetCredential`/
            // `ProviderDeleteCredential` a usam diretamente. Tudo
            // passa por DPAPI — nunca por shim em memória. Ver
            // ADR-0007 §Decisão.
            let credentials = Arc::new(WindowsCredentialStore::new());
            let credentials_dyn: Arc<dyn CredentialStore> = credentials.clone();

            let providers = build_provider_map(credentials_dyn);
            let runs = RunRegistry::new();
            let catalog = Arc::new(Catalog::load().clone());

            // Sink: TauriEventSink emite via `Window::emit`. Se a
            // janela estiver fechada, `emit` falha silenciosa —
            // o journal no SQLite é a fonte de verdade.
            let sink: Arc<dyn EventSink> = Arc::new(sink::TauriEventSink::new(handle));

            // Tooling (Etapa 4.x.y): `ToolRegistry` + Jail + tools
            // concretas. A Etapa 6 carrega o `PermissionSet` real do
            // assistente/projeto. Aqui o catálogo inicial tem só
            // `FilesReadTool` (in-process, sem worker sidecar). O
            // `ToolRegistry` é construído a partir das tools
            // concretas em `frederico_app::build_tool_registry`
            // (commit 4b) — não há mais `ToolRegistry::new()` solto.
            // `JailResolver` para workspace per-conversa
            // (Etapa 1 da Fase de Ligação, ADR-0022 §D2/D3). Cria
            // `<data_local_dir>/workspaces/<conversation_id>/` sob
            // demanda. **Erro duro**: se o `mkdir` falhar, o app
            // aborta o startup com mensagem legível. A Etapa 7
            // (modo desenvolvedor) substitui por `SecurityJailResolver`
            // via `frederico-security`. O `ToolRegistry` (acima)
            // ainda é vazio nesta Etapa 1; o registro dos
            // manifestos entra no commit 4b (`build_tool_registry`).
            let workspaces_root = data_local_dir().join("workspaces");
            let jail_resolver: Arc<dyn frederico_tool_registry::JailResolver> = Arc::new(
                frederico_app::jail::FileSystemJailResolver::new(workspaces_root),
            );

            // ADR-0023 §D1+D2 — Etapa 2.A da fase-ligação.
            // Resolve o runtime do `document-worker` e instancia
            // o `DocumentWorkerLauncher` (lazy + restart on
            // death + kill tree no app exit). **Degradação
            // declarada**: se nenhum dos 3 candidatos do
            // `RuntimeContext` (env, app_resources, dev_repo)
            // tem o runtime completo, `document_worker` fica
            // `None` e a UI mostra "indisponível" via
            // `DocumentWorkerStatus`. Sem fallback pro
            // `FakeWorker` (que retornaria `handler_stub` —
            // "documento falso entregue como verdadeiro" é a
            // falha mais cara).
            let runtime_ctx = resolve_runtime_context(app.handle());
            let document_worker =
                match frederico_app::runtime::resolve_document_worker_runtime(&runtime_ctx) {
                    Some(location) => {
                        tracing::info!(
                            runtime_root = %location.root.display(),
                            source = ?location.source,
                            "document-worker runtime resolvido — DocumentWorkerLauncher disponível"
                        );
                        Some(Arc::new(DocumentWorkerLauncher::new(
                            location,
                            frederico_app::launcher::LauncherConfig::default(),
                        )))
                    }
                    None => {
                        tracing::warn!(
                            "document-worker runtime indisponível: \
                         docs.generate/docs.inspect NÃO estarão acessíveis. \
                         Para habilitar: execute workers/document-worker/bootstrap.ps1 (dev) \
                         ou popule bundle.resources do Tauri (produção — Fase 9)."
                        );
                        None
                    }
                };

            // **Etapa 2.B (ADR-0024):** o mesmo launcher vira
            // um `Arc<dyn WorkerInvoker>` — o contrato genérico
            // que `build_default_tools` / `build_default_allowed_for_run`
            // esperam. **Bump atômico** com o `documents: None →
            // Full` (ADR-0020 §3 D3) e o registro de
            // `DocsGenerateTool` + `DocsInspectTool` no
            // `ToolRegistry`: a casca chama as 3 funções de
            // composição (`build_default_tools`,
            // `build_default_allowed_for_run`,
            // `initial_permission_set*`) com a **mesma**
            // `Option<Arc<dyn WorkerInvoker>>` — quando `Some`,
            // tudo aparece; quando `None`, nada aparece. Sem
            // meia-medida.
            //
            // O launcher continua existindo como
            // `Option<Arc<DocumentWorkerLauncher>>` (campo
            // `document_worker` do `AppState`) porque os
            // commands Tauri `document_worker_status` /
            // `document_worker_invoke` / `document_worker_reset`
            // precisam do tipo concreto (o trait
            // `WorkerInvoker` **não** expõe `status()` nem
            // `reset()` — `WorkerError::PermanentlyDead` pede
            // `reset()` da UI). **Dois papéis, mesmo objeto.**
            //
            // **Por que `(**launcher_arc).clone()`:** o campo
            // `document_worker` é `Option<Arc<DocumentWorkerLauncher>>`
            // (o `Arc` permite que o `DocumentWorkerLauncher`
            // viva no `AppState` E seja clonado pra cá). Para
            // construir o `Arc<dyn WorkerInvoker>`, preciso
            // de um `DocumentWorkerLauncher` **dono** (não
            // outro `Arc`) — então extraio o conteúdo via
            // `(**launcher_arc).clone()` e reempacoto. O
            // `state` interno é `Arc<Mutex<...>>` (custo do
            // clone: refcount + 1) e `location`/`config` são
            // `Clone` barato — o clone é leve.
            let document_worker_invoker: Option<Arc<dyn WorkerInvoker>> =
                document_worker.as_ref().map(|launcher_arc| {
                    let launcher: DocumentWorkerLauncher = (**launcher_arc).clone();
                    Arc::new(launcher) as Arc<dyn WorkerInvoker>
                });

            // Tools concretas. A Etapa 6 (UI de configuração)
            // permite ligar/desligar; aqui vem do
            // `build_default_tools`, que retorna o **mínimo
            // comum** (1 tool: `FilesReadTool`) quando o
            // invoker é `None`, e `FilesReadTool +
            // DocsGenerateTool + DocsInspectTool` quando o
            // invoker é `Some`. O `ToolRegistry` é construído
            // a partir dessas tools concretas em
            // `frederico_app::build_tool_registry` (commit 4b)
            // — não há mais `ToolRegistry::new()` solto.
            //
            // **Bump atômico capability + permission**
            // (ADR-0020 §3 D3, ADR-0024 §D2): mesma
            // `Option<Arc<dyn WorkerInvoker>>` passada pra
            // `build_default_tools`, `build_default_allowed_for_run`
            // e pro ternário do `permission_set` — quando
            // `Some`, as 2 tools do `document-worker`
            // aparecem no `ToolRegistry`, os 2 `ToolId`s
            // aparecem na allowlist do `RunExecutor`, e
            // `documents` vira `Full`; quando `None`, em
            // nenhum dos três lugares. A simetria é o que
            // garante que o modelo **nunca** vê um tool que
            // não consegue invocar (degradação declarada, não
            // substituição silenciosa).
            let tools =
                frederico_app::composition::build_default_tools(document_worker_invoker.clone());
            let allowed_for_run = frederico_app::composition::build_default_allowed_for_run(
                document_worker_invoker.clone(),
            );
            let permission_set = if document_worker_invoker.is_some() {
                frederico_app::composition::initial_permission_set_for_capable_launcher()
            } else {
                frederico_app::composition::initial_permission_set()
            };

            // `MemoryExtractor` (Fase 4, Etapa 5; Fase de Ligação
            // Etapa 3 — bump de default). Constrói via
            // `frederico_app::composition::build_memory_extractor`
            // (mesma função que os E2E consomem). Se a key do
            // OpenRouter está disponível (DPAPI ou env var), o
            // `LlmMemoryClassifier` usa `OpenRouterCompletionProvider`
            // (gpt-4o-mini); senão, cai pra `NoopCompletionProvider`
            // com warning logado (degradação declarada). O
            // extractor roda em background via `tokio::spawn` e
            // processa jobs do canal mpsc (256, sem
            // `tokio::time::interval` — ADR-0014 §1).
            //
            // Custo por run concluído: 1 chamada LLM (cota
            // 5/min default, regra do ADR-0012 §2).
            let memory_extractor_handle = tauri::async_runtime::block_on(async {
                let cfg = frederico_app::composition::MemoryConfig::default();
                let key = lookup_openrouter_key(&credentials).await;
                frederico_app::composition::build_memory_extractor(&db, &cfg, key)
            });

            // Composição centralizada no `frederico-app` (Etapa 1
            // da Fase de Ligação, ADR-0022 §D4). Esta é a
            // **mesma função** que os E2E da raiz chamam
            // (`tests/e2e/`, Etapa 5 da fase). O `ChatOrchestratorParts`
            // agrupa os 12 args que antes eram posicionais.
            let parts = frederico_app::composition::ChatOrchestratorParts {
                providers: providers.clone(),
                runs: runs.clone(),
                sink: sink.clone(),
                db: db.clone(),
                clock: clock.clone(),
                catalog: catalog.clone(),
                tool_registry: frederico_app::composition::build_tool_registry(&tools),
                jail_resolver: jail_resolver.clone(),
                tools,
                allowed_for_run,
                permission_set,
                memory_extractor: memory_extractor_handle,
            };
            let orch = Arc::new(frederico_app::composition::build_chat_orchestrator(parts));

            // Recovery de crash (Etapa 5.x). Spawna uma task em
            // background que lista runs não-terminais com
            // `last_heartbeat_at` mais velho que 120s e marca como
            // `interrupted` (a view `runs_with_status` mapeia
            // `interrupted → timeout`). Não bloqueia o startup —
            // roda em paralelo com a abertura da janela. O threshold
            // é maior que o `event_timeout` do executor (60s)
            // porque o executor pode estar esperando o delta final
            // do provider.
            //
            // O `Database` é `Arc<SqlitePool>` internamente — clonar
            // é barato. O `spawn_recover_stale_runs` recebe o
            // `Database` e constrói o `RunRepo` dentro do closure
            // (sem `unsafe`).
            let _recovery_handle = spawn_recover_stale_runs(
                (*db).clone(),
                std::time::Duration::from_secs(DEFAULT_STALE_THRESHOLD_SECS),
            );

            // Provider de embedding (Fase de Ligação, Etapa 3):
            // OpenRouterEmbeddingAdapter se a key está
            // disponível, NoopEmbeddingAdapter caso contrário
            // (degradação declarada). Construído **antes** do
            // `app.manage` porque precisa de `credentials` (que é
            // movido pro `AppState`).
            let embedding_provider: Arc<dyn frederico_memory::embedding::EmbeddingProvider> =
                tauri::async_runtime::block_on(async {
                    let cfg = frederico_app::composition::MemoryConfig::default();
                    let key = lookup_openrouter_key(&credentials).await;
                    frederico_app::composition::build_embedding_provider(&cfg, key)
                });

            app.manage(AppState {
                db,
                orch,
                credentials,
                document_worker,
                embedding_provider,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc_dispatch,
            app_version,
            document_worker_status,
            document_worker_invoke,
            document_worker_reset,
        ])
        .run(tauri::generate_context!())
        .expect("falha ao rodar app Tauri");
}

/// Dispatcher IPC. Despacha o `AppOp` para o orquestrador / storage.
#[tauri::command]
async fn ipc_dispatch(
    request: IpcRequest,
    state: State<'_, AppState>,
) -> Result<IpcResponse, String> {
    match request.op {
        AppOp::Ping => Ok(IpcResponse::ok(serde_json::json!({ "pong": true })).unwrap()),
        AppOp::GetAppInfo => match state.db.app_info().await {
            Ok(info) => {
                Ok(IpcResponse::ok(info).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
            }
            Err(e) => Ok(IpcResponse::err(e.to_string())),
        },

        // --- Etapa 1: Provedores (credenciais) ---
        AppOp::ProviderList => {
            // Lista provedores conhecidos do storage; por enquanto
            // a tabela está vazia até o usuário cadastrar o primeiro.
            let repo = frederico_storage::ProviderConfigRepo::new(&state.db);
            let list: Vec<ProviderConfigView> = repo
                .list()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|c| ProviderConfigView {
                    provider: c.provider_id,
                    display_name: c.display_name,
                    configured: c.configured,
                    last_ok_at: c.last_ok_at,
                    last_error_at: c.last_error_at,
                    last_error: c.last_error,
                })
                .collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ProviderSetCredential { provider, value } => {
            // DPAPI real: grava no Windows Credential Manager. A
            // mesma instância configurada no `setup` é usada —
            // nada de shim de memória.
            let sec = secrecy::SecretString::new(value.into());
            state
                .credentials
                .set(&provider, &sec)
                .await
                .map_err(|e| e.to_string())?;
            // Marca o provider como configurado no storage.
            frederico_storage::ProviderConfigRepo::new(&state.db)
                .upsert(&provider, provider.as_str(), true)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ProviderDeleteCredential { provider } => {
            // DPAPI real: remove do Windows Credential Manager.
            // Delete é idempotente — ver `WindowsCredentialStore::delete`.
            state
                .credentials
                .delete(&provider)
                .await
                .map_err(|e| e.to_string())?;
            frederico_storage::ProviderConfigRepo::new(&state.db)
                .upsert(&provider, provider.as_str(), false)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }

        // --- Leva 2: Catálogo ---
        AppOp::ModelCatalogList => {
            let cat = Catalog::load();
            let list: Vec<ModelDescriptorView> =
                cat.list_all().into_iter().map(model_to_view).collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ModelCatalogForProvider { provider } => {
            let cat = Catalog::load();
            let list: Vec<ModelDescriptorView> = cat
                .list_for_provider(&provider)
                .into_iter()
                .map(model_to_view)
                .collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }

        // --- Leva 3: Conversas ---
        AppOp::ConversationCreate {
            provider,
            model,
            title,
        } => {
            let conv = ConversationRepo::new(&state.db)
                .create(&provider, &model, title.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(conv_to_view(&conv))
                .unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ConversationList => {
            let list: Vec<ConversationView> = ConversationRepo::new(&state.db)
                .list_recent(100)
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .map(conv_to_view)
                .collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ConversationGet { id } => {
            let cid = uuid::Uuid::parse_str(&id)
                .map(frederico_core::ConversationId)
                .map_err(|e| e.to_string())?;
            let conv = ConversationRepo::new(&state.db)
                .get(&cid)
                .await
                .map_err(|e| e.to_string())?;
            let msgs: Vec<MessageView> = MessageRepo::new(&state.db)
                .list_for_conversation(&cid)
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .map(message_to_view)
                .collect();
            let payload = serde_json::json!({
                "conversation": conv_to_view(&conv),
                "messages": msgs,
            });
            Ok(IpcResponse::ok(payload).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ConversationRename { id, title } => {
            let cid = uuid::Uuid::parse_str(&id)
                .map(frederico_core::ConversationId)
                .map_err(|e| e.to_string())?;
            ConversationRepo::new(&state.db)
                .rename(&cid, title.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ConversationSetModel {
            id,
            provider,
            model,
        } => {
            let cid = uuid::Uuid::parse_str(&id)
                .map(frederico_core::ConversationId)
                .map_err(|e| e.to_string())?;
            ConversationRepo::new(&state.db)
                .set_model(&cid, &provider, &model)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ConversationDelete { id } => {
            let cid = uuid::Uuid::parse_str(&id)
                .map(frederico_core::ConversationId)
                .map_err(|e| e.to_string())?;
            ConversationRepo::new(&state.db)
                .delete(&cid)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }

        // --- Leva 3: Mensagem + Run ---
        AppOp::MessageSend {
            conversation_id,
            content,
        } => {
            let cid = uuid::Uuid::parse_str(&conversation_id)
                .map(frederico_core::ConversationId)
                .map_err(|e| e.to_string())?;
            let (user_msg, run_id) = state
                .orch
                .send_message(cid, content)
                .await
                .map_err(|e| format!("{e:?}"))?;
            let result = MessageSendResult {
                user_message: message_to_view(&user_msg),
                run_id: run_id.0.to_string(),
            };
            Ok(IpcResponse::ok(result).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::RunGetEvents {
            message_id,
            since_seq,
        } => {
            let mid = uuid::Uuid::parse_str(&message_id)
                .map(frederico_core::MessageId)
                .map_err(|e| e.to_string())?;
            let events: Vec<MessageEventView> = state
                .orch
                .get_events(mid, since_seq)
                .await
                .map_err(|e| format!("{e:?}"))?
                .into_iter()
                .map(|e| MessageEventView {
                    id: e.id,
                    message_id: e.message_id.0.to_string(),
                    seq: e.seq,
                    kind: e.kind,
                    data: e.data,
                    created_at: e.created_at,
                })
                .collect();
            Ok(IpcResponse::ok(events).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::RunCancel { run_id } => {
            let rid = uuid::Uuid::parse_str(&run_id)
                .map(frederico_core::RunId)
                .map_err(|e| e.to_string())?;
            state
                .orch
                .cancel_run(rid)
                .await
                .map_err(|e| format!("{e:?}"))?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }

        // --- Etapa 6: fila de aprovação ---
        AppOp::ApprovalList => {
            let repo = ApprovalQueueRepo::new(&state.db);
            let list: Vec<frederico_shared_contracts::ApprovalEntryView> = repo
                .list_pending()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|e| frederico_shared_contracts::ApprovalEntryView {
                    id: e.id,
                    run_id: e.run_id.0.to_string(),
                    tool_id: e.tool_id,
                    request_json: e.request_json,
                    created_at: e.created_at,
                })
                .collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ApprovalRespond {
            approval_id,
            decision,
        } => {
            let approved = decision
                .get("approved")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let decision_json = serde_json::to_string(&decision)
                .map_err(|e| format!("erro ao serializar decision: {e}"))?;
            let repo = ApprovalQueueRepo::new(&state.db);
            repo.resolve(&approval_id, &decision_json, approved)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }

        // --- Etapa 5 (Fase 4): painel de memória ---
        AppOp::MemoryList {
            scope_type,
            scope_id,
            include_pending,
        } => {
            let parsed = parse_scope_type(&scope_type).map_err(|e| e.to_string())?;
            let repo = MemoryRepo::new(&state.db);
            let now = chrono::Utc::now();
            // O `list_by_scope` filtra `pending_review = false`
            // por padrão (regra do `MemoryRecord::is_visible`).
            // Quando `include_pending = true`, queremos ver
            // também a fila de revisão de ExternalContent —
            // então listamos via `list_pending_review` (helper
            // novo) OU listamos tudo e filtramos no caller.
            // Implementação: lista via `search_lexical("*")`
            // para incluir `pending_review = true`? Não — mais
            // limpo: listar `list_by_scope` + um segundo
            // `list_by_pending_review` quando aplicável.
            // Para v1, mantemos simples: a UI chama com
            // `include_pending = true` no painel de revisão
            // e o backend concatena as duas listas.
            let mut records = repo
                .list_by_scope(parsed, &scope_id, now)
                .await
                .map_err(|e| e.to_string())?;
            if include_pending {
                // Pending review lista todas as memórias com
                // `pending_review = true` independente de
                // escopo — o caller filtra no cliente. Por
                // ora, deixa como a Etapa 5 decidir; se
                // precisar, adicionamos um `list_pending()`
                // puro no repo.
                let pending = repo
                    .list_pending_review(now)
                    .await
                    .map_err(|e| e.to_string())?;
                records.extend(pending);
            }
            let view: Vec<MemoryView> = records.iter().map(record_to_view).collect();
            Ok(IpcResponse::ok(view).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::MemoryRetrieve {
            scope_type,
            scope_id,
            query,
            k,
        } => {
            let parsed = parse_scope_type(&scope_type).map_err(|e| e.to_string())?;
            let retriever = build_retriever(&state);
            let req = frederico_core::RetrievalRequest {
                scope_type: parsed,
                scope_id: scope_id.clone(),
                query,
                k: k as usize,
                token_budget: 1500,
                recency_epsilon: 0.01,
            };
            let result = retriever.retrieve(req).await.map_err(|e| e.to_string())?;
            let view: Vec<MemoryHitView> = result.hits.iter().map(hit_to_view).collect();
            Ok(IpcResponse::ok(view).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::MemoryApplyCorrection {
            old_id,
            replacement,
        } => {
            let parsed_id = uuid::Uuid::parse_str(&old_id)
                .map(frederico_core::MemoryId)
                .map_err(|e| e.to_string())?;
            // Converte `NewMemoryInputView` (do IPC, com
            // `Deserialize`) pra `NewMemoryInput` (do
            // `frederico-memory`, só `Clone` por design).
            let input = input_view_to_input(replacement)
                .map_err(|e| format!("replacement inválido: {e}"))?;
            let repo = MemoryRepo::new(&state.db);
            let result = repo
                .apply_correction(&parsed_id, input, chrono::Utc::now())
                .await
                .map_err(|e| e.to_string())?;
            let view = CorrectionResultView {
                old_id: result.old_id.0.to_string(),
                new_record: record_to_view(&result.new_record),
                superseded_at: result.superseded_at.to_rfc3339(),
            };
            Ok(IpcResponse::ok(view).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::MemoryConfirmPending { id } => {
            let parsed_id = uuid::Uuid::parse_str(&id)
                .map(frederico_core::MemoryId)
                .map_err(|e| e.to_string())?;
            let repo = MemoryRepo::new(&state.db);
            repo.confirm_pending(&parsed_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::MemoryRejectPending { id } => {
            let parsed_id = uuid::Uuid::parse_str(&id)
                .map(frederico_core::MemoryId)
                .map_err(|e| e.to_string())?;
            let repo = MemoryRepo::new(&state.db);
            repo.reject_pending(&parsed_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(IpcResponse::ok(()).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::MemoryPurgeExpired => {
            let repo = MemoryRepo::new(&state.db);
            let deleted = repo
                .purge_expired(chrono::Utc::now())
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(
                memory.purge_expired = deleted,
                "memórias expiradas/superseded purgadas via IPC"
            );
            Ok(IpcResponse::ok(deleted).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
    }
}

#[tauri::command]
fn app_version() -> String {
    frederico_core::APP_VERSION.to_string()
}

// --- helpers de conversão -----------------------------------------------

fn conv_to_view(c: &frederico_storage::Conversation) -> ConversationView {
    ConversationView {
        id: c.id.0.to_string(),
        title: c.title.clone(),
        provider_id: c.provider_id.as_str().to_string(),
        model_id: c.model_id.as_str().to_string(),
        created_at: c.created_at.clone(),
        updated_at: c.updated_at.clone(),
        total_cost_microcents: c.total_cost_microcents,
    }
}

fn message_to_view(m: &frederico_storage::Message) -> MessageView {
    MessageView {
        id: m.id.0.to_string(),
        conversation_id: m.conversation_id.0.to_string(),
        role: m.role.clone(),
        content: m.content.clone(),
        status: m.status.clone(),
        run_id: m.run_id.map(|r| r.0.to_string()),
        prompt_tokens: m.prompt_tokens,
        completion_tokens: m.completion_tokens,
        cost_microcents: m.cost_microcents,
        error: m.error.clone(),
        created_at: m.created_at.clone(),
        finished_at: m.finished_at.clone(),
    }
}

fn model_to_view(m: &frederico_model_catalog::ModelDescriptor) -> ModelDescriptorView {
    ModelDescriptorView {
        provider: m.provider.clone(),
        model: m.model.clone(),
        display_name: m.display_name.clone(),
        context_window: m.context_window,
        modalities: serde_json::to_value(&m.modalities).unwrap_or(serde_json::Value::Null),
        capabilities: serde_json::to_value(&m.capabilities).unwrap_or(serde_json::Value::Null),
        pricing_input_microcents_per_million: m.pricing_per_million.input_microcents,
        pricing_output_microcents_per_million: m.pricing_per_million.output_microcents,
    }
}

// --- Etapa 5 (Fase 4): helpers de memória ---------------------------

/// Parseia o `scope_type` do IPC (string) em `MemoryScopeType`.
/// Erro: retorna `String` com mensagem PT-BR.
fn parse_scope_type(s: &str) -> Result<MemoryScopeType, String> {
    use std::str::FromStr;
    MemoryScopeType::from_str(s).map_err(|_| format!("scope_type inválido: {s}"))
}

/// Constrói o retriever padrão da casca. Usa `HybridRetriever`
/// com o `embedding_provider` do `AppState` — que é o
/// `OpenRouterEmbeddingAdapter` se a key do OpenRouter está
/// disponível (DPAPI ou env var), ou `NoopEmbeddingAdapter`
/// caso contrário (degradação declarada, retriever vira
/// lexical-only — o `HybridRetriever` é transparente: se o
/// provider é `NoopEmbeddingAdapter`, age como o
/// `LexicalRetriever`).
///
/// **Por que helper local e não no `composition.rs`:** o
/// `HybridRetriever` é emprestado (`'a`) e a fonte é o
/// `&state.db` (referência do Tauri). Mover pro `composition.rs`
/// exigiria lifting do lifetime ou mudança de design
/// (e.g.owned hybrid retriever). Por ora fica aqui; a
/// composição do `embedding_provider` (que é o que importa
/// pra Etapa 3) mora em `frederico_app::composition::build_embedding_provider`.
fn build_retriever<'a>(state: &'a State<'_, AppState>) -> HybridRetriever<'a> {
    use frederico_memory::config::ScoringWeights;
    HybridRetriever::new(
        &state.db,
        state.embedding_provider.clone(),
        ScoringWeights::default(),
    )
}

/// Converte `MemoryRecord` em `MemoryView` (serializável pra UI).
fn record_to_view(r: &frederico_core::MemoryRecord) -> MemoryView {
    MemoryView {
        id: r.id.0.to_string(),
        scope_type: r.scope_type.as_str().to_string(),
        scope_id: r.scope_id.clone(),
        type_: r.type_.as_str().to_string(),
        content: r.content.clone(),
        origin: r.origin.as_str().to_string(),
        source_type: r.source_type.as_str().to_string(),
        source_id: r.source_id.clone(),
        confidence: r.confidence,
        importance: r.importance,
        embedding_status: r.embedding_status.as_str().to_string(),
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
        expires_at: r.expires_at.map(|d| d.to_rfc3339()),
        superseded_by: r.superseded_by.map(|id| id.0.to_string()),
        superseded_at: r.superseded_at.map(|d| d.to_rfc3339()),
        user_confirmed: r.user_confirmed,
        user_pinned: r.user_pinned,
        pending_review: r.pending_review,
    }
}

/// Converte `MemoryHit` (com `ScoreBreakdown`) em `MemoryHitView`.
fn hit_to_view(h: &MemoryHit) -> MemoryHitView {
    MemoryHitView {
        record: record_to_view(&h.record),
        score: h.score,
        score_breakdown: ScoreBreakdownView {
            lexical: h.score_breakdown.lexical,
            recency: h.score_breakdown.recency,
            semantic: h.score_breakdown.semantic,
            importance: h.score_breakdown.importance,
            confirmation: h.score_breakdown.confirmation,
            scope_match: h.score_breakdown.scope_match,
        },
        explanation: h.explanation.clone(),
    }
}

/// Converte `NewMemoryInputView` (do IPC) em
/// `frederico_memory::NewMemoryInput`. Erros de parsing
/// (escopo/tipo/origem inválidos, `expires_at` malformado)
/// viram `String` PT-BR.
fn input_view_to_input(
    v: frederico_shared_contracts::NewMemoryInputView,
) -> Result<frederico_memory::NewMemoryInput, String> {
    use std::str::FromStr;
    let scope_type = MemoryScopeType::from_str(&v.scope_type)
        .map_err(|_| format!("scope_type inválido: {}", v.scope_type))?;
    let type_ = frederico_core::MemoryType::from_str(&v.type_)
        .map_err(|_| format!("type inválido: {}", v.type_))?;
    let origin = frederico_core::MemoryOrigin::from_str(&v.origin)
        .map_err(|_| format!("origin inválido: {}", v.origin))?;
    let expires_at = match v.expires_at {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(&s)
                .map_err(|e| format!("expires_at malformado: {e}"))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    Ok(frederico_memory::NewMemoryInput {
        scope_type,
        scope_id: v.scope_id,
        type_,
        content: v.content,
        origin,
        source_type: MemorySourceType::new(v.source_type),
        source_id: v.source_id,
        confidence: v.confidence,
        importance: v.importance,
        expires_at,
        // A Etapa 5 não tem UI pra "user_confirmed" / "user_pinned"
        // no formulário de correção — default false. A UI pode
        // expor flags depois se precisar.
        user_confirmed: false,
        user_pinned: false,
    })
}

// --- Etapa 2.A da Fase de Ligação (ADR-0023) ---

/// `tauri::command` que devolve o status do launcher do
/// `document-worker`. UI consome pra mostrar "document-worker:
/// disponível/indisponível, caminho resolvido" no diagnóstico
/// (ADR-0023 §D2 — degradação declarada). Retorna
/// `LauncherStatus::message` PT-BR (regra do projeto).
///
/// **Quando o launcher é `None`** (runtime indisponível em
/// produção sem `bundle.resources`, ou em dev sem
/// `bootstrap.ps1`), devolve um `LauncherStatus` com
/// `alive: false` e `message` explicando o que fazer. A UI
/// mostra a mensagem diretamente — não tem que traduzir.
#[tauri::command]
async fn document_worker_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    match state.document_worker.as_ref() {
        Some(launcher) => {
            let s = launcher.status().await;
            Ok(serde_json::json!({
                "available": s.alive,
                "runtime_source": s.runtime_source,
                "runtime_path": s.runtime_path,
                "message": s.message,
            }))
        }
        None => Ok(serde_json::json!({
            "available": false,
            "runtime_source": null,
            "runtime_path": null,
            "message": "document-worker indisponível: nenhum dos 3 candidatos do \
                        RuntimeContext (env, app_resources, dev_repo) tem runtime \
                        completo. Execute workers/document-worker/bootstrap.ps1 (dev) \
                        ou popule bundle.resources do Tauri (produção — Fase 9 do \
                        PROMPT MESTRE)."
        })),
    }
}

/// `tauri::command` que invoca o launcher do `document-worker`
/// com um payload JSON. **Caminho de invoke direto**, sem
/// passar pelo `ChatOrchestrator` / `ToolRegistry` (essa
/// integração é Etapa 2.B). Frontend React usa isso quando o
/// usuário clica em "gerar documento" no botão da UI.
///
/// Devolve o resultado do `invoke` (JSON arbitrário — o
/// `document-worker` define o schema por capability) ou uma
/// mensagem de erro PT-BR (regra do projeto) em caso de
/// `RuntimeUnavailable` / `PermanentlyDead` / `InvokeFailed`.
#[tauri::command]
async fn document_worker_invoke(
    payload: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let launcher = state.document_worker.as_ref().ok_or_else(|| {
        "document-worker indisponível: execute workers/document-worker/bootstrap.ps1 \
             (dev) ou popule bundle.resources do Tauri (produção)."
            .to_string()
    })?;
    launcher.invoke(payload).await.map_err(|e| e.to_string())
}

/// `tauri::command` que reseta o state do launcher (botão
/// "tentar reiniciar" no diagnóstico após `PermanentlyDead`).
/// Não faz nada se o launcher é `None` (idempotente).
#[tauri::command]
async fn document_worker_reset(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    if let Some(launcher) = state.document_worker.as_ref() {
        launcher.reset().await;
        Ok(serde_json::json!({ "ok": true, "message": "launcher resetado" }))
    } else {
        Ok(serde_json::json!({ "ok": false, "message": "launcher indisponível" }))
    }
}

// --- Etapa 3 da Fase de Ligação: lookup de key do OpenRouter ---

/// Busca a API key do OpenRouter pro embedding/classificador
/// de memória. Ordem de precedência (degradação declarada):
///
/// 1. **DPAPI** via `WindowsCredentialStore` (chave cadastrada
///    pelo usuário no Settings, Etapa 6 da Fase 3 vai expor
///    isso). O `target_name` é
///    `Frederico-IA-Studio:provider:openrouter` (formato
///    `target_name_for(provider)` em `crates/security/src/windows.rs`).
/// 2. **Env var** `OPENROUTER_API_KEY` (fallback pra dev local
///    e CI). Útil pra E2E com `verify-external.ps1`.
///
/// Se ambas ausentes, retorna `None` — o
/// `build_completion_provider` / `build_embedding_provider`
/// logam warning e caem pra `Noop*` (degradação declarada).
async fn lookup_openrouter_key(
    credentials: &Arc<WindowsCredentialStore>,
) -> Option<secrecy::SecretString> {
    use frederico_core::ProviderId;
    use secrecy::SecretString;

    // 1. DPAPI (mesmo `ProviderId` que `build_provider_map` usa
    //    pra registrar o `OpenAiCompatAdapter` OpenRouter).
    if let Ok(Some(secret)) = credentials.get(&ProviderId::new("openrouter")).await {
        return Some(secret);
    }

    // 2. Env var.
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .map(|s| SecretString::new(s.into_boxed_str()))
}
