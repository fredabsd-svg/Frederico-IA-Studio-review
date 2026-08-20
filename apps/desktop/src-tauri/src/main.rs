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
use frederico_execution_engine::recovery::{recover_stale_runs, DEFAULT_STALE_THRESHOLD_SECS};
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
use frederico_storage::{
    ApprovalQueueRepo, ConversationRepo, Database, MessageRepo, MigrateError, RunEventRepo,
    RunRepo, StorageError,
};
use tauri::{Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

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
    /// Catálogo efetivo: o embutido, fundido com o que os provedores
    /// responderam no boot ([ADR-0052]).
    ///
    /// Começa igual ao embutido e é substituído quando (e se) a
    /// tarefa de fundo terminar. A janela nunca espera por ele — é o
    /// que separa "dispara rede no boot" de "depende de rede para
    /// abrir", e foi a distinção que o ADR-0043 não fez.
    ///
    /// [ADR-0052]: ../../../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
    catalogo_efetivo: Arc<frederico_model_catalog::CatalogHandle>,
    /// Bundle de especialistas (Fase 6, Etapa 3, ADR-0030).
    /// Consumido pelo Tauri command `ListSpecialists` (e
    /// futuramente pelo `SubagentRunner` da Etapa 4). Carrega
    /// bundled + override via `build_specialist_registry`.
    specialist_bundle: Arc<frederico_app::composition::SpecialistBundle>,
}

/// Diretório local de dados do aplicativo (Windows: `%LOCALAPPDATA%\studio\frederico\ia`).
/// Resolvido uma vez por chamada — `ProjectDirs::from` é barato,
/// mas é guard de invariante (`expect` em vez de `unwrap_or`).
///
/// **Override via `FREDERICO_DATA_DIR`:** se a env var estiver
/// setada, **retornamos o valor dela** (criado se necessário).
/// Caso contrário, caímos no `ProjectDirs`. O override é
/// necessário pro smoke test (`apps/desktop/src-tauri/tests/
/// smoke_startup.rs`) **nunca** tocar o banco de produção do
/// usuário — o test cria um `tempdir()`, seta a env var, e o
/// binário abre o banco **dentro do tempdir**, não em
/// `%LOCALAPPDATA%`. Sem isso, qualquer `cargo test` destruiria
/// conversas/memórias/runs do usuário real (lição da sessão
/// 2026-08-10 — o smoke test truncateou um `.db` de produção
/// durante a investigação do `Migrate(VersionMismatch)`).
///
/// **Convenção de nomenclatura:** env vars com prefixo
/// `FREDERICO_` são o ponto de override público da casca. Já
/// existia `FREDERICO_DOCUMENT_WORKER_RUNTIME` (Etapa 2.A da
/// fase de Ligação) com o mesmo papel pro path do `document-worker`.
/// O `FREDERICO_DATA_DIR` segue o mesmo padrão.
fn data_local_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("FREDERICO_DATA_DIR") {
        let path = PathBuf::from(custom);
        // Cria o diretório se não existir. O `Database::open` faz
        // isso também, mas queremos que `data_local_dir()` retorne
        // um path utilizável mesmo antes do `Database::open` (ex.:
        // pra criar `workspaces/`).
        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        return path;
    }
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

/// Dispara o recovery de crash no startup em background.
///
/// **Por que este helper existe:** a `recover_stale_runs` do
/// `frederico-execution-engine` é `async` pura — não spawna nada.
/// A casca (Tauri) é quem decide como disparar. Usar
/// `tauri::async_runtime::spawn` aqui (e não `tokio::spawn`
/// direto) é o que evita o panic "there is no reactor running"
/// que aconteceu na v1 (ver `recovery.rs` §"Spawn é
/// responsabilidade do caller" e o smoke test
/// `apps/desktop/src-tauri/tests/smoke_startup.rs`).
///
/// **O que acontece dentro do task:** abre o `Database` clonado
/// (cheap, `Arc<SqlitePool>`), constrói `RunRepo` + `RunEventRepo`
/// (borrow do `Database` que vive só dentro do `async move`),
/// chama `recover_stale_runs` e loga o resultado. O `JoinHandle`
/// é descartado (`spawn` retorna `JoinHandle`, não esperamos) — o
/// recovery é best-effort: se falhar, logamos `warn` e seguimos
/// (a próxima inicialização tenta de novo).
fn spawn_startup_recovery(db: &Arc<Database>, threshold: std::time::Duration) {
    let recovery_db = (*db).clone();
    tauri::async_runtime::spawn(async move {
        let run_repo = RunRepo::new(&recovery_db);
        let run_event_repo = RunEventRepo::new(&recovery_db);
        match recover_stale_runs(&run_repo, &run_event_repo, threshold).await {
            Ok(marked) if marked > 0 => {
                tracing::info!(
                    recovered = marked,
                    "startup recovery: runs stale marcados como interrupted"
                );
            }
            Ok(_) => {
                tracing::debug!("startup recovery: nenhum run stale");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "startup recovery falhou — runs stale serão revisados na próxima inicialização"
                );
            }
        }
    });
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

/// Mostra um diálogo nativo do Windows descrevendo por que
/// `Database::open` falhou no startup e o que o usuário pode
/// fazer pra recuperar. Chamado quando o `.setup` da casca
/// não consegue abrir o banco — substitui o `.expect("abre o
/// banco SQLite")` que gerava pânico sem mensagem.
///
/// **Por que existe:** `Database::open` faz duas coisas —
/// cria a pool SQLite e roda `sqlx::migrate!` no diretório
/// `crates/storage/migrations/`. As falhas mais comuns são:
///
/// 1. `Migrate(VersionMismatch(N))` — o arquivo de migração
///    `N` foi editado **depois de aplicado** (regra do
///    `sqlx::migrate!`: migração aplicada é imutável; checksum
///    SHA-384 do arquivo no disco tem que bater com o que está
///    gravado em `_sqlx_migrations.checksum`). Costuma acontecer
///    quando o usuário roda uma build de commit X com um
///    banco criado por uma build de commit Y — o schema
///    divergiu. **Recovery:** resetar o banco (perde
///    conversas/memórias/runs) ou restaurar de backup.
/// 2. `Migrate(Dirty(N))` — uma migração anterior falhou no
///    meio, deixando `_sqlx_migrations.success = 0` pra
///    versão `N`. **Recovery:** investigar o estado da
///    tabela; nunca é problema do usuário.
/// 3. `Migrate(MissingVersion(N))` — o arquivo de migração
///    sumiu do diretório. **Recovery:** reinstalar a build
///    correta.
/// 4. Erro de I/O do SQLite (arquivo trancado, permissão,
///    disco cheio) — variantes `Open { path, source }` /
///    `Query`.
///
/// O diálogo mostra a causa específica + caminho do banco +
/// passos de recuperação. O processo sai logo depois com
/// código não-zero — o smoke test detecta isso como falha
/// de startup **legível** (não mais pânico genérico).
///
/// **Por que `String` (não `Box<dyn Error>` direto):** o
/// `.setup` retorna `Result<(), Box<dyn Error>>` mas queremos
/// também logar a mensagem com `tracing::error!` antes de
/// mostrar o diálogo. Logar e mostrar a mesma string é mais
/// fácil de auditar do que encadear erros.
fn handle_startup_db_error(
    handle: &tauri::AppHandle,
    db_path: &std::path::Path,
    err: &StorageError,
) -> String {
    // Log sempre — `frederico-mind.log` no data dir guarda
    // pra diagnóstico posterior mesmo se o usuário só
    // fechar o diálogo sem anotar.
    tracing::error!(
        error = %err,
        db_path = %db_path.display(),
        "falha fatal ao abrir banco SQLite no startup"
    );

    // Texto do diálogo: 2 partes — o que aconteceu + o que
    // fazer. Linguagem direta, PT-BR (a UI do app é PT-BR;
    // o sistema de mensagens de erro segue o mesmo padrão
    // do `ProviderErrorView`).
    let (cause_line, recovery_lines) = match err {
        StorageError::Migrate(migrate_err) => {
            // `sqlx::migrate::MigrateError` tem variantes
            // como `VersionMismatch`, `Dirty`, `MissingVersion`
            // etc. A `Display` já produz algo útil (ex.:
            // "migration 1 was previously applied but its
            // file has been modified"); usamos isso na
            // mensagem mas complementamos com o caminho de
            // recuperação específico.
            match migrate_err {
                MigrateError::VersionMismatch(_) => (
                    "o arquivo de migração mudou desde que a \
                     migração foi aplicada ao banco (regra do \
                     `sqlx`: migração aplicada é imutável)."
                        .to_string(),
                    vec![
                        "1. Feche este diálogo.".to_string(),
                        format!("2. Faça backup do banco: copie `{}` para um lugar seguro.", db_path.display()),
                        "3. Apague o arquivo `frederico.db` (o app recria do zero com o schema atual).".to_string(),
                        "4. Reinicie o app.".to_string(),
                        "ATENÇÃO: conversas, memórias e runs serão perdidos. \
                         Se você tem dados importantes, abra o `frederico-mind.log` \
                         no mesmo diretório e procure ajuda antes de apagar."
                            .to_string(),
                    ],
                ),
                MigrateError::VersionMissing(_) => (
                    "um arquivo de migração esperado \
                     não está no diretório da build."
                        .to_string(),
                    vec![
                        "A build está corrompida ou incompleta. \
                         Reinstale o app (ou rode `cargo install` \
                         de novo se for dev).".to_string(),
                    ],
                ),
                MigrateError::VersionNotPresent(_) => (
                    "uma migração aplicada está ausente da build \
                     atual (a build tem versão mais antiga que \
                     o banco)."
                        .to_string(),
                    vec![
                        "A build é mais antiga que a versão que \
                         criou este banco. Atualize o app para a \
                         versão mais recente e reabra.".to_string(),
                    ],
                ),
                MigrateError::VersionTooOld(_, _) | MigrateError::VersionTooNew(_, _) => (
                    "a ordem das migrações na build não \
                     corresponde ao histórico do banco."
                        .to_string(),
                    vec![
                        "A build está fora de ordem com o banco. \
                         Atualize o app para a versão correta ou \
                         apague o banco (perde dados).".to_string(),
                    ],
                ),
                MigrateError::Dirty(_) => (
                    "uma migração anterior foi marcada como \
                     parcial (sucesso=0). Em SQLite isso é \
                     raro mas pode acontecer após uma \
                     interrupção."
                        .to_string(),
                    vec![
                        format!("Abra o banco `{}` com `sqlite3`, marque a migração como sucesso (UPDATE _sqlx_migrations SET success=1) ou apague a linha, depois reabra o app.", db_path.display()),
                    ],
                ),
                MigrateError::ExecuteMigration(_, _) | MigrateError::Execute(_) => (
                    "uma migração falhou ao executar (erro de SQL ou violação de constraint)."
                        .to_string(),
                    vec![
                        format!("Reporte o problema incluindo o arquivo `{}` e o `frederico-mind.log`.", db_path.display()),
                    ],
                ),
                MigrateError::Source(_) => (
                    "não consegui ler o diretório de migrações \
                     embutido no binário."
                        .to_string(),
                    vec![
                        "A build está corrompida. Reinstale o app.".to_string(),
                    ],
                ),
                other => (
                    format!("falha de migração: {other}"),
                    vec![
                        "Verifique o `frederico-mind.log` no \
                         mesmo diretório do banco para mais detalhes."
                            .to_string(),
                    ],
                ),
            }
        }
        StorageError::Open { path, source } => (
            format!("não consegui abrir/criar o banco SQLite: {source}"),
            vec![
                format!("Caminho: {}", path.display()),
                "Verifique se o diretório existe e é gravável, \
                 se o disco não está cheio, e se outro processo \
                 não está segurando o arquivo."
                    .to_string(),
            ],
        ),
        StorageError::Query(source) => (
            format!("query falhou ao abrir o banco: {source}"),
            vec!["Verifique o `frederico-mind.log` para mais detalhes.".to_string()],
        ),
        other => (
            format!("erro de storage: {other}"),
            vec!["Verifique o `frederico-mind.log` para mais detalhes.".to_string()],
        ),
    };

    let mut message = String::new();
    message.push_str("O Frederico IA Studio não conseguiu abrir o banco de dados.\n\n");
    message.push_str("Causa: ");
    message.push_str(&cause_line);
    message.push_str("\n\nCaminho do banco:\n  ");
    message.push_str(&db_path.display().to_string());
    message.push_str("\n\nO que fazer:\n");
    for line in &recovery_lines {
        message.push('\n');
        message.push_str(line);
    }
    message.push_str(
        "\n\n(O diagnóstico completo está no stderr do app — capture-o com `cargo run 2> erro.log` ou via Event Viewer do Windows.)\n",
    );

    // Mostra o diálogo com **timeout de 3s** (caminho não
    // interativo pro CI / serviços / desktop sem sessão).
    // Justificativa: `blocking_show` original segurava o
    // processo indefinidamente esperando o usuário fechar a
    // janela. Em ambiente headless (runner de CI, Windows
    // session 0, container sem desktop), `blocking_show` ou
    // pendura ou retorna erro silencioso — em ambos os casos
    // o processo não sai. O CI do `frederico-process-architecture`
    // roda em headless e ficaria travado.
    //
    // Estratégia:
    // 1. Spawn uma thread que chama `blocking_show`. A
    //    thread sinaliza via `mpsc::channel` quando o
    //    dialog fecha.
    // 2. O thread principal espera até 3s pelo sinal.
    // 3. Se o sinal chegou: usuário fechou, retorna mensagem
    //    (normal).
    // 4. Se timeoutou: ambiente headless — loga warning,
    //    segue. O dialog continua aberto na thread em
    //    background e é morto quando o processo sair (Err do
    //    setup propaga via `Builder::run`).
    //
    // Por que 3s e não mais: smoke test espera 5s no grace
    // window. Se o dialog levasse 5s+ pra timeoutar, o test
    // mataria o processo antes do timeout — desperdício. 3s
    // dá folga pra usuário real (que tipicamente clica em
    // <1s) sem prender o CI.
    //
    // Por que `mpsc::channel` (não `Condvar`): o `recv_timeout`
    // é direto, sem lock manual. O canal é consumido uma vez
    // (ou pelo recv com timeout, ou por `Drop` quando a
    // thread morre com o processo — `send` em canal fechado
    // é no-op).
    let (dialog_done_tx, dialog_done_rx) = std::sync::mpsc::channel::<()>();
    let dialog_handle = handle.clone();
    let message_for_thread = message.clone();
    std::thread::spawn(move || {
        // **`catch_unwind` é necessário aqui:** o
        // `tauri-plugin-dialog::blocking_show` (via `rfd` no
        // Windows) **panica** em vez de retornar `Err` quando
        // não há GUI disponível (headless, Windows session 0,
        // runner de CI sem desktop). Verificado na sessão
        // 2026-08-10: `thread '<unnamed>' panicked at
        // ...tauri-plugin-dialog-2.7.2/src/lib.rs:358:9: called
        // \`Result::unwrap()\` on an \`Err\` value`. Sem o
        // `catch_unwind`, o panic da thread do dialog
        // contaminaria o stderr do test (que procura
        // `panicked at` pra detectar pânico do `.setup`) e
        // quebraria a distinção entre "panic genuíno" e
        // "recovery gracioso".
        //
        // O `catch_unwind` captura o panic, loga, e
        // prossegue. O diagnóstico completo já está no
        // stderr via `tracing::error!` lá em cima — o
        // dialog é uma camada opcional de UX, não a
        // fonte de verdade do erro.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dialog_handle
                .dialog()
                .message(&message_for_thread)
                .title("Frederico IA Studio — falha ao abrir o banco")
                .kind(MessageDialogKind::Error)
                .blocking_show();
        }));
        if let Err(panic_payload) = result {
            // Converte o payload (geralmente `&str` ou `String`)
            // em uma string logável. `downcast_ref::<&str>` é
            // o caso comum; o fallback cobre outros tipos.
            let msg: &str = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.as_str()
            } else {
                "<payload não-string>"
            };
            tracing::warn!(
                panic = msg,
                "blocking_show panico — provavelmente headless (sem GUI). \
                 Diagnóstico já está no stderr via tracing::error! acima."
            );
        }
        // Sinaliza que o dialog fechou. Se o canal já estiver
        // fechado (main thread saiu após timeout), o `send`
        // retorna Err — ignoramos.
        let _ = dialog_done_tx.send(());
    });

    match dialog_done_rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(()) => {
            // Usuário fechou o dialog. Normal.
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Ambiente headless (ou usuário distraído). Loga
            // warning — o diagnóstico completo já está no
            // stderr via `tracing::error!` lá em cima.
            tracing::warn!(
                timeout_secs = 3,
                "dialog de erro não foi fechado em 3s — provavelmente headless; prosseguindo para shutdown"
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            // Thread do dialog morreu sem sinalizar (não
            // deveria acontecer, mas o tipo cobre).
            tracing::warn!("thread do dialog de erro desconectou inesperadamente");
        }
    }

    message
}

fn main() {
    diagnostics::init();
    tracing::info!("Frederico IA Studio iniciando…");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let db_path = resolve_db_path();
            tracing::info!(?db_path, "abrindo banco SQLite");

            // **Caminho de erro do startup (Etapa 6+):** o
            // `Database::open` pode falhar por várias razões
            // (migrations incompatíveis, I/O, etc.). A v1 do
            // código tinha `.expect("abre o banco SQLite")` que
            // panicava no stderr sem mostrar nada pro usuário.
            // A Etapa 6+ substitui por:
            //
            // 1. `handle_startup_db_error` mostra um dialog
            //    nativo do Windows com a causa específica + o
            //    caminho de recuperação, e loga via
            //    `tracing::error!` pro stderr. Aguarda até 3s
            //    pelo dialog fechar (caminho não interativo
            //    cai pro `tracing::warn!` + segue).
            // 2. `std::process::exit(1)` mata o processo
            //    **sem** passar pelo Tauri runtime (sem
            //    `App::exit`, sem `Builder::run` retornando
            //    Err) — o que evita o "Failed to setup app"
            //    panic do main thread (`tauri-2.11.5/src/app.rs:
            //    1425` verificado em 2026-08-10). Sem essa
            //    distinção, o smoke test
            //    (`apps/desktop/src-tauri/tests/smoke_startup.rs`)
            //    confunde o startup recovery com a regressão
            //    do `tokio::spawn` (v1 — Etapa 5.x Fase 3).
            //
            // **Por que não `return Err(...)`:** Tauri trata
            // como panic (verificado). **Por que não
            // `handle.exit(1)`:** também passa pelo panic do
            // runtime. **`std::process::exit(1)` é a única
            // saída que mata o processo sem o runtime panicar.
            //
            // **Trade-off:** destructors não rodam (DB não
            // fecha, sockets não fecham). Aceitável porque o
            // DB está em estado inconsistente de qualquer
            // jeito (o `Database::open` falhou). A sessão
            // 2026-08-10 confirmou que o Tauri panic é pior
            // (ruído no stderr, falso positivo no smoke).
            if let Err(err) = tauri::async_runtime::block_on(async {
                Database::open(&db_path).await
            }) {
                handle_startup_db_error(&handle, &db_path, &err);
                std::process::exit(1);
            }
            let db = Arc::new(
                tauri::async_runtime::block_on(async { Database::open(&db_path).await })
                    .expect("abre o banco SQLite (após verificação de erro acima)"),
            );

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
            // **Catálogo efetivo** (ADR-0052): um handle que o
            // refresh de boot substitui e que o motor de execução
            // lê a cada envio. A UI e o motor precisam enxergar a
            // *mesma* lista — quando divergiram, a lista suspensa
            // oferecia modelos que o motor rejeitava com
            // `ModelNotFound`.
            let catalogo_efetivo = Arc::new(frederico_model_catalog::CatalogHandle::new(
                catalog.clone(),
            ));
            // Specialist bundle (Fase 6, Etapa 3, ADR-0030):
            // carrega bundled + override + pareia com o catálogo pra
            // resolver capabilities por `default_model`. Mesmo
            // `Arc<Catalog>` que o orchestrator usa — se a UI trocar
            // o catálogo em runtime (Etapa futura), o `ListSpecialists`
            // e o orchestrator enxergam a mudança junto.
            let specialist_bundle = Arc::new(
                frederico_app::composition::build_specialist_registry(catalog.clone()),
            );

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
            // aborta o startup com mensagem legível. **Conceito
            // separado** do `SecurityJailResolver` da Etapa 2 da
            // Fase 7 (ADR-0036) — este orquestra spawn isolado
            // (Job Object + Restricted Token + Env Filter); o
            // `FileSystemJailResolver` resolve o `Jail` (path
            // safety) por `ConversationId`. A integração com o
            // `RunExecutor` (executar `exec.python`/`exec.node`
            // sob o `SecurityJailResolver`) entra na **Etapa 4 da
            // Fase 7**. O `ToolRegistry` (acima) ainda é vazio
            // nesta Etapa 1; o registro dos manifestos entra no
            // commit 4b (`build_tool_registry`).
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
            // comum** (1 tool: `FilesReadTool`) quando ambos
            // `invoker` e `exec_deps` são `None`, e vai
            // bumpando os subsistemas atômicos. O
            // `ToolRegistry` é construído a partir dessas
            // tools concretas em
            // `frederico_app::build_tool_registry` — não há
            // mais `ToolRegistry::new()` solto.
            //
            // **Bump atômico capability + permission** (Etapa
            // 4 da Fase 7 + ADR-0020 §3 D3, ADR-0024 §D2):
            // mesmas `Option`s passadas pra
            // `build_default_tools`, `build_default_allowed_for_run`
            // e pro ternário do `permission_set` — quando
            // `Some`, o subsistema aparece no `ToolRegistry`
            // + na allowlist + com a permissão bumpada;
            // quando `None`, em nenhum dos três lugares. A
            // simetria é o que garante que o modelo **nunca**
            // vê um tool que não consegue invocar (degradação
            // declarada, não substituição silenciosa).
            //
            // (SpecialistRegistry + PermissionLoader são
            // construídos mais abaixo, perto de onde o
            // `ChatOrchestratorParts` os consome — ver bloco
            // `ChatOrchestratorParts::new`.)

            // `SecurityJailResolver` (Etapa 2 da Fase 7,
            // ADR-0031 + ADR-0036). Orquestrador do sandbox
            // Windows. Em Linux, retorna `platform_supported =
            // false` e o `spawn` retorna `Err(Unsupported)`
            // (degradação declarada). Construtor é **sync**
            // (não tem I/O), pode rodar direto na `setup` da
            // casca.
            //
            // **Etapa 5+ (2026-08-10) — reativado:** a
            // Etapa 5+ fechou (PR #48 — raw CreateProcessAsUserW
            // + Mandatory Label\Low via SetFileSecurityW no
            // workdir). O `RestrictedToken` agora É aplicado
            // no spawn (TokenIntegrityLevel=Low + drop dos 6
            // privilégios). O `exec_deps` volta ao catálogo.
            //
            // `new()` já retorna `Arc<SecurityJailResolver>`
            // — não envolver em outro `Arc::new` (causaria
            // `Arc<Arc<...>>`).
            let security_jail_resolver: Arc<frederico_security::jail::SecurityJailResolver> =
                frederico_security::jail::SecurityJailResolver::new(
                    frederico_security::jail::SecurityJailConfig::secure_default(),
                )
                .expect("SecurityJailResolver::new");

            // `RuntimeRegistry` (Etapa 3 da Fase 7). Hard-coda
            // Python 3.12.4 + Node 20.16.0. Construtor sync
            // (cria `install_root` se não existir). O
            // `bootstrap_all` (download + extract + validate)
            // é **async** — rodamos em background task pra
            // não bloquear a abertura do app (pode levar
            // minutos em primeira execução).
            //
            // **Fail-soft:** se o bootstrap falhar (sem rede,
            // disco cheio, etc.), as tools `exec.python` /
            // `exec.node` são registradas mesmo assim (o
            // modelo as vê no schema), mas `execute` retorna
            // erro `"runtime 'python-3.12.4' nao registrado"`
            // — degradação declarada, não substituição
            // silenciosa.
            let runtime_registry = Arc::new(
                frederico_runtimes::RuntimeRegistry::new(
                    frederico_runtimes::RuntimeConfig::secure_default(),
                )
                .expect("RuntimeRegistry::new"),
            );
            // Spawn do bootstrap em background. A task é
            // fire-and-forget; o log mostra progresso
            // (bootstrapped vs cached vs failed). O
            // `tokio::spawn` no `tauri::async_runtime` usa
            // o runtime do Tauri (criado no `Builder`).
            let runtimes_for_bootstrap = runtime_registry.clone();
            tauri::async_runtime::spawn(async move {
                match runtimes_for_bootstrap.bootstrap_all().await {
                    Ok(report) => {
                        tracing::info!(
                            python_3_12_4 = ?report.cached.contains(&frederico_runtimes::RuntimeId::new("python-3.12.4")),
                            node_20_16_0 = ?report.cached.contains(&frederico_runtimes::RuntimeId::new("node-20.16.0")),
                            bootstrapped_count = report.bootstrapped.len(),
                            cached_count = report.cached.len(),
                            failed_count = report.failed.len(),
                            duration_ms = report.total_duration.as_millis() as u64,
                            "runtimes: bootstrap_all completou"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "runtimes: bootstrap_all falhou (degracao declarada; \
                             exec.python/exec.node vao falhar ate reiniciar)"
                        );
                    }
                }
            });

            // `AuditSink` (Etapa 1 da Fase 3, Passo 10 do
            // `validate_tool_call`). v1 da Etapa 4 da Fase 7
            // usa `NoopAuditSink` (a implementação concreta
            // `DbAuditSink` que grava em `tool_audit` é
            // trabalho da Etapa 5+ — Passo 10 do validador é
            // o lugar natural, e o `Tool::execute` não tem
            // `run_id`).
            //
            // **Etapa 5+ (2026-08-10) — reativado:** `NoopAuditSink`
            // entra no `ExecDeps` (a `DbAuditSink` real é trabalho
            // da Etapa 5+ da Fase 3, depois).
            let audit_sink: Arc<dyn frederico_tool_registry::AuditSink> =
                Arc::new(frederico_tool_registry::NoopAuditSink);

            // `exec_deps` da Etapa 4 da Fase 7 (Python + Node sob
            // SecurityJailResolver) **reativado** pela Etapa 5+
            // (2026-08-10). A path safety enforcement fechou:
            //   1. `SetFileSecurityW(workdir, LABEL_SECURITY_INFORMATION)`
            //      aplica `Mandatory Label\Low` no workdir
            //      (SACL com SYSTEM_MANDATORY_LABEL_ACE_TYPE
            //      + policy NO_WRITE_UP).
            //   2. Token do child = `RestrictedToken` (drop 6
            //      privilégios) + `TokenIntegrityLevel = Low`
            //      via `duplicate_as_primary()`.
            //   3. Spawn via `CreateProcessAsUserW` raw com
            //      `CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT`.
            //   4. `AssignProcessToJobObject` per-invocation
            //      (KILL_ON_JOB_CLOSE) + `ResumeThread`.
            //   5. Pipes stdout/stderr SEM label (Medium) — child
            //      (Low) consegue escrever porque pipes anônimos
            //      sem label passam o Mandatory Label check.
            //      (Pendência Fase 8: rótulo nos pipes exigiria
            //      SeSecurityPrivilege no caller; trade-off
            //      documentado em SECURITY.md §"O que essa
            //      combinação NÃO protege".)
            //
            // **Bump atômico (Etapa 5+):** o `exec_deps` é
            // `Some(exec_deps)` em `build_default_tools` e
            // `build_default_allowed_for_run` — `exec.python`
            // e `exec.node` voltam ao catálogo. Validação
            // garantida pelos 3 tests em
            // `crates/e2e/tests/e2e_exec_python_under_sandbox.rs`
            // (`child_cannot_write_outside_workspace`,
            // `exec_python_simple_hello_world`,
            // `wall_clock_kills_long_running_process`).
            // Etapa 6+1 da Fase 7: `DbNetworkAuditSink` real,
            // não `Noop`. `new_unbound` porque o sink é
            // compartilhado entre **todas** as invocações de
            // `exec.python`/`exec.node` do processo — cada
            // `NetworkAccessEntry` já carrega o `run_id` da
            // chamada específica (estampado por
            // `start_network_proxy` a cada `execute()`), então o
            // sink não precisa (e não deve) ficar vinculado a um
            // único run. Ver `network_audit_sink.rs::append_sync`.
            let network_audit_sink: Arc<
                dyn frederico_security::network::NetworkAuditSink,
            > = Arc::new(frederico_security::network_audit_sink::DbNetworkAuditSink::new_unbound(
                (*db).clone(),
            ));

            // `PermissionLoader` (Etapa 3 PR 2 da Fase 6) —
            // construído aqui (antes do `ExecDeps`) porque a
            // Etapa 7 da Fase 7 precisa dele pra carregar a
            // `network_allowlist` do proxy de rede antes do
            // `exec_deps` existir. Mesma instância é reusada
            // mais abaixo no `ChatOrchestratorParts.permission_loader`
            // (sem re-parse redundante — o cache é em memória,
            // chaveado por `(path, content_hash)`).
            let permission_loader =
                std::sync::Arc::new(frederico_tool_registry::PermissionLoader::new());

            // Etapa 7 da Fase 7 (ADR-0033): allowlist do proxy
            // de rede carregada do perfil TOML do usuário ∩
            // projeto (`~/.config/frederico/profiles/default.toml`
            // + `./.frederico/project.toml`). **Só user+project**
            // — o layer de assistant exige um `assistant_id` que
            // não existe neste ponto do boot (o `ExecDeps` é
            // process-wide, construído uma vez, não por
            // conversa/assistant escolhido). Refinar por
            // assistant é Fase 8, quando o proxy virar per-run.
            // Path ausente (primeiro launch, sem profile
            // configurado) → `PermissionSet::default()` via
            // `load_profile` → `network_allowlist` vazio →
            // deny-by-default (mesmo comportamento pré-Etapa-7,
            // não uma regressão).
            let network_allowlist_hosts: Vec<String> =
                match frederico_tool_registry::PermissionLoader::default_user_profile_path() {
                    Some(user_path) => {
                        let project_path =
                            frederico_tool_registry::PermissionLoader::default_project_profile_path();
                        let user_ps = permission_loader.load_profile(&user_path);
                        let project_ps = permission_loader.load_profile(&project_path);
                        let merged_ps = user_ps.merge(&project_ps);
                        frederico_app::composition::effective_network_allowlist_hosts(&merged_ps)
                    }
                    None => Vec::new(),
                };
            let network_allowlist = frederico_security::network::NetworkAllowlist::new()
                .with_allowed(network_allowlist_hosts);

            let exec_deps = frederico_app::composition::ExecDeps {
                resolver: security_jail_resolver.clone(),
                runtimes: runtime_registry.clone(),
                audit: audit_sink,
                network_allowlist,
                network_audit: network_audit_sink,
            };

            // Tools concretas. A Etapa 6 (UI de configuração)
            // permite ligar/desligar; aqui vem do
            // `build_default_tools`, que retorna o **mínimo
            // comum** (1 tool: `FilesReadTool`) quando ambos
            // `invoker` e `exec_deps` são `None`, e vai
            // bumpando os subsistemas atômicos. O
            // `ToolRegistry` é construído a partir dessas
            // tools concretas em
            // `frederico_app::build_tool_registry` — não há
            // mais `ToolRegistry::new()` solto.
            //
            // **Bump atômico capability + permission** (Etapa
            // 4 da Fase 7 + ADR-0020 §3 D3, ADR-0024 §D2):
            // mesmas `Option`s passadas pra
            // `build_default_tools`, `build_default_allowed_for_run`
            // e pro ternário do `permission_set` — quando
            // `Some`, o subsistema aparece no `ToolRegistry`
            // + na allowlist + com a permissão bumpada;
            // quando `None`, em nenhum dos três lugares. A
            // simetria é o que garante que o modelo **nunca**
            // vê um tool que não consegue invocar (degradação
            // declarada, não substituição silenciosa).
            //
            // **Etapa 5+ da Fase 7 (2026-08-10) — reativado:**
            // `exec_deps` é `Some(exec_deps)` (path safety
            // enforcement fechou, ver bloco acima). O catálogo
            // volta a incluir `exec.python` + `exec.node` (com
            // `Mandatory Label\Low` no workdir + token restrito
            // no child).
            // Flag de subsistema exec disponível (a Etapa 5+
            // sempre constrói `exec_deps` aqui, então é `true`
            // no caminho feliz). Mantida como `let` separado
            // pra documentar a simetria com o match 4-estados
            // abaixo — facilita Etapas futuras onde o
            // `exec_deps` pode falhar em `Some` (ex.: bootstrap
            // da runtime falha, ou sandbox init falha). Por
            // ora, hard-coded `true` (mesma regra do `Some(exec_deps)`
            // acima).
            let exec_deps_available = true;
            // **Marcos de projeto (Etapa 4 da Fase 8, ADR-0048 §D2).**
            // Dependem só do banco, que já está aberto aqui.
            let marco_deps = Some(frederico_tool_registry::MarcoDeps {
                pool: std::sync::Arc::new(db.pool().clone()),
            });

            // **GitHub (ADR-0049 §D4): duas condições independentes.**
            //
            // Token no cofre **e** matriz não-vazia no perfil efetivo.
            // Faltando qualquer uma, `github.push` e
            // `github.create_pr` ficam fora do catálogo e da allowlist
            // — bump atômico (ADR-0020 §3 D3).
            //
            // A conta lida é a primeira cadastrada no serviço
            // `github`. Multi-conta é decisão de UI que ainda não
            // existe; escolher aqui uma regra silenciosa ("a mais
            // recente", "a alfabética") criaria comportamento que
            // ninguém pediu e que o usuário não consegue prever.
            let token_github = tauri::async_runtime::block_on(async {
                let contas = frederico_security::ServiceCredentialStore::list_accounts(&*credentials, "github")
                    .await
                    .unwrap_or_default();
                let conta = contas.first()?;
                let chave = frederico_security::ServiceCredentialKey::new("github", conta).ok()?;
                frederico_security::ServiceCredentialStore::get_secret(&*credentials, &chave)
                    .await
                    .ok()
                    .flatten()
            });
            // A matriz vem do perfil efetivo (usuário ∩ projeto),
            // pelo mesmo caminho do `network_allowlist` acima. Sem
            // perfil, `load_profile` cai no `PermissionSet::default()`
            // → matriz vazia → ferramentas fora do catálogo, que é o
            // deny-by-default do ADR-0049 §D4.
            let github_repos: Vec<frederico_tool_registry::RegraGithubPerfil> =
                match frederico_tool_registry::PermissionLoader::default_user_profile_path() {
                    Some(user_path) => {
                        let project_path =
                            frederico_tool_registry::PermissionLoader::default_project_profile_path();
                        let user_ps = permission_loader.load_profile(&user_path);
                        let project_ps = permission_loader.load_profile(&project_path);
                        user_ps.merge(&project_ps).github_repos
                    }
                    None => Vec::new(),
                };
            let github_deps =
                frederico_app::composition::build_github_deps(token_github, &github_repos);

            let tools = frederico_app::composition::build_default_tools(
                document_worker_invoker.clone(),
                Some(exec_deps.clone()),
                marco_deps.clone(),
                github_deps.clone(),
            );
            let allowed_for_run = frederico_app::composition::build_default_allowed_for_run(
                document_worker_invoker.clone(),
                Some(&exec_deps),
                marco_deps.is_some(),
                github_deps.is_some(),
            );
            // **Permission set 4-estados (Etapa 5+):** os 2
            // subsistemas (document-worker + exec) são
            // independentes — o document-worker pode estar
            // disponível (runtime resolvido) sem o exec, e
            // vice-versa. Combinamos em 4 ramos:
            //   - ambos Some → `capable_launcher_and_exec`
            //     (documents: Full + python/node: Sandboxed).
            //   - só invoker Some → `for_capable_launcher`
            //     (documents: Full só).
            //   - só exec_deps Some → `for_exec` (python/node:
            //     Sandboxed só).
            //   - nenhum Some → `initial_permission_set` (deny
            //     default, Etapa 1).
            //
            // **Bump atômico capability + permission** (ADR-0020
            // §3 D3): a mesma `Option` que foi pra
            // `build_default_tools` / `build_default_allowed_for_run`
            // é refletida no `PermissionSet` — sem meia-medida.
            let permission_set = match (
                document_worker_invoker.is_some(),
                exec_deps_available,
            ) {
                (true, true) => frederico_app::composition::initial_permission_set_for_capable_launcher_and_exec(),
                (true, false) => frederico_app::composition::initial_permission_set_for_capable_launcher(),
                (false, true) => frederico_app::composition::initial_permission_set_for_exec(),
                (false, false) => frederico_app::composition::initial_permission_set(),
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

            // SpecialistRegistry (Etapa 3 PR 1 da Fase 6, ADR-0030)
            // + PermissionLoader (Etapa 3 PR 2). O
            // `ChatOrchestrator` consome ambos pra montar o
            // `SubagentRunner` interno (Etapa 4 PR 2,
            // ADR-0027). Reusamos o `specialist_bundle` que o
            // Tauri command `list_specialists` já consome
            // (criado na Etapa 3 — mesma `Arc<Catalog>` que o
            // orchestrator).
            let specialist_registry = specialist_bundle.registry.clone();
            // `permission_loader` já foi construído acima (antes
            // do `exec_deps`, pra carregar a `network_allowlist`)
            // — reusado aqui, sem re-parse.

            // `MultimodelOrchestrator` (Etapa 5 PR 2 da Fase 6,
            // ADR-0028). Mesmo factory que os E2E da raiz
            // chamam (`crates/e2e/tests/e2e_pipeline_sequencial_e2e.rs`).
            // O `ChatOrchestrator::start_pipeline` /
            // `cancel_pipeline` delegam pra ele.
            let tool_registry_for_orchestrator =
                frederico_app::composition::build_tool_registry(&tools);
            let multimodel_orchestrator = std::sync::Arc::new(
                frederico_execution_engine::pipeline_orchestrator::MultimodelOrchestrator::new(
                    db.clone(),
                    runs.clone(),
                    sink.clone(),
                    catalog.clone(),
                    clock.clone(),
                    providers.clone(),
                    tool_registry_for_orchestrator.clone(),
                    jail_resolver.clone(),
                    tools.clone(),
                    allowed_for_run.clone(),
                    permission_set.clone(),
                ),
            );

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
                catalog: catalogo_efetivo.clone(),
                tool_registry: tool_registry_for_orchestrator,
                jail_resolver: jail_resolver.clone(),
                tools,
                allowed_for_run,
                permission_set,
                memory_extractor: memory_extractor_handle,
                specialist_registry,
                permission_loader,
                multimodel_orchestrator: Some(multimodel_orchestrator),
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
            // **Por que `tauri::async_runtime::spawn` (não
            // `tokio::spawn`):** o `.setup` do Tauri é **síncrono**
            // (não `async`) — `tokio::spawn` panica com "there is
            // no reactor running, must be called from the context
            // of a Tokio 1.x runtime" quando invocado de fora de
            // um contexto de runtime. O wrapper do Tauri usa o
            // runtime que ele próprio configurou (tokio por
            // default) e o `.setup` já é chamado com o runtime
            // ativo. Ver `crates/execution-engine/src/recovery.rs`
            // §"Spawn é responsabilidade do caller" e o smoke
            // test `apps/desktop/src-tauri/tests/smoke_startup.rs`
            // que prova que o setup passa sem panic.
            //
            // O `Database` é `Arc<SqlitePool>` internamente — clonar
            // é barato. O `RunRepo` é construído dentro do closure
            // da task (o borrow do `&Database` não pode escapar
            // pra um `Future + 'static`).
            spawn_startup_recovery(
                &db,
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

            // **Refresh de catálogo no boot** (ADR-0052 §D1). Começa
            // com o embutido inteiro, para a janela abrir com lista
            // completa antes de qualquer resposta de rede.
            // A tarefa de fundo. Nada aqui bloqueia a abertura: se a
            // rede estiver fora, se o provedor demorar ou se a
            // resposta for inválida, o catálogo embutido continua no
            // lugar e o app segue utilizável.
            {
                let providers_para_refresh = providers.clone();
                let destino = catalogo_efetivo.clone();
                tauri::async_runtime::spawn(async move {
                    let mut respostas = Vec::new();
                    for id in providers_para_refresh.providers() {
                        let Some(adapter) = providers_para_refresh.get(&id) else {
                            continue;
                        };
                        match adapter.listar_modelos().await {
                            Ok(modelos) if !modelos.is_empty() => {
                                tracing::info!(
                                    provider = id.as_str(),
                                    total = modelos.len(),
                                    "catálogo do provedor atualizado"
                                );
                                respostas.push(frederico_model_catalog::RespostaDoProvedor {
                                    provider: id.clone(),
                                    modelos: modelos
                                        .into_iter()
                                        .map(|m| frederico_model_catalog::ModeloRemotoNormalizado {
                                            id: m.id,
                                            nome: m.nome,
                                            janela_de_contexto: m.janela_de_contexto,
                                            entrada: m.entrada,
                                            saida: m.saida,
                                        })
                                        .collect(),
                                });
                            }
                            // **Lista vazia é tratada como falha.** Um
                            // provedor que responde `[]` faria a fusão
                            // apagar todos os modelos embutidos dele —
                            // e "não consegui listar" é bem mais
                            // provável que "este provedor não tem
                            // modelo nenhum".
                            Ok(_) => tracing::warn!(
                                provider = id.as_str(),
                                "o provedor devolveu lista vazia; mantendo o catálogo embutido"
                            ),
                            Err(e) => tracing::info!(
                                provider = id.as_str(),
                                erro = %e,
                                "sem refresh de catálogo para este provedor"
                            ),
                        }
                    }
                    if respostas.is_empty() {
                        return;
                    }
                    let fundido = frederico_model_catalog::fundir(Catalog::load(), &respostas);
                    destino.replace(std::sync::Arc::new(
                        frederico_model_catalog::Catalog::from_models(
                            fundido.into_iter().map(|m| m.descritor).collect(),
                        ),
                    ));
                });
            }

            app.manage(AppState {
                db,
                orch,
                credentials,
                document_worker,
                embedding_provider,
                specialist_bundle,
                catalogo_efetivo,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc_dispatch,
            app_version,
            document_worker_status,
            document_worker_invoke,
            document_worker_reset,
            list_specialists,
            start_pipeline,
            cancel_pipeline,
            list_resumable_pipelines,
            list_pipeline_stages,
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
            // Lê o catálogo **efetivo** (ADR-0052): o embutido, já
            // fundido com o que os provedores responderam neste boot.
            // Enquanto a tarefa de fundo não termina, isto é
            // exatamente o embutido — nunca uma lista vazia.
            let list: Vec<ModelDescriptorView> = state
                .catalogo_efetivo
                .current()
                .list_all()
                .into_iter()
                .map(model_to_view)
                .collect();
            Ok(IpcResponse::ok(list).unwrap_or_else(|e| IpcResponse::err(e.to_string())))
        }
        AppOp::ModelCatalogForProvider { provider } => {
            // Mesma fonte do `ModelCatalogList`: o catálogo efetivo,
            // filtrado. É esta a chamada que o seletor de modelo do
            // formulário de criação usa.
            let list: Vec<ModelDescriptorView> = state
                .catalogo_efetivo
                .current()
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

// --- Etapa 3 da Fase 6: registro de especialistas (ADR-0030) ---

/// `tauri::command` que devolve a lista de especialistas
/// disponíveis (bundled + override) com as capabilities do
/// `default_model` já resolvidas via catálogo. Consumido pelo
/// `<SpecialistPicker>` (Etapa 3, ADR-0030 §D5) e pelo Modo
/// Equipe (Etapa 6, sidebar).
///
/// **Por que separado do `ipc_dispatch`:** a UI do Modo Equipe
/// faz polling de 1 em 1s enquanto o picker está aberto
/// (mostra spinner "carregando"). O `ipc_dispatch` enfileira
/// no mesmo canal do orchestrator; um comando dedicado é mais
/// barato e não disputa com o `MessageSend`. Mesma justificativa
/// do `document_worker_status` (Etapa 2.A da Fase de Ligação).
///
/// **Por que `Vec<SpecialistSummary>` direto e não view do
/// `shared-contracts`:** o `SpecialistSummary` é o tipo da
/// camada de catálogo (sem paths internos, sem custos — só o
/// que a UI precisa). Promover a view no `shared-contracts` é
/// trabalho da Etapa 6 (quando o Modo Equipe consumir o tipo
/// também) — pra Etapa 3 o comando já é consumido pelo
/// frontend via `dispatch<ListSpecialistsView[]>`.
#[tauri::command]
async fn list_specialists(
    state: State<'_, AppState>,
) -> Result<Vec<frederico_model_catalog::SpecialistSummary>, String> {
    Ok(state.specialist_bundle.list_summaries())
}

// ============================================================================
// Pipeline Sequencial (Fase 6, Etapa 5/6, ADR-0028)
// ============================================================================

/// `tauri::command` que inicia um pipeline multimodelo
/// sequencial (Etapa 6 do Modo Equipe). Recebe uma lista de
/// `StageSpec` (model_id, provider_id, input) e o
/// `parent_run_id` (= `RunId` da conversa atual), delega pro
/// `ChatOrchestrator::start_pipeline` e devolve o `pipeline_id`
/// (= `MultimodelRun.id`).
///
/// **Por que separado do `ipc_dispatch`:** o pipeline é uma
/// operação assíncrona (executa em background via `tokio::spawn`).
/// O `ipc_dispatch` enfileira no mesmo canal do orchestrator;
/// um comando dedicado evita disputa com o `MessageSend` e
/// permite polling simples (a UI faz `list_resumable_pipelines`
/// no startup).
///
/// **Por que `Result<String, String>` e não um tipo de
/// `shared-contracts`:** o erro é propagado como string
/// (legível pela UI). A UI discrimina por substring
/// ("não encontrado", "provider", "modelo"). A Etapa 6 (UI)
/// pluga uma view estruturada se necessário.
#[tauri::command]
async fn start_pipeline(
    parent_run_id: String,
    stages: Vec<frederico_execution_engine::pipeline_orchestrator::StageSpec>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .orch
        .start_pipeline(&parent_run_id, stages)
        .map_err(|e| format!("{e}"))
}

/// `tauri::command` que cancela um pipeline em curso (D7 do
/// ADR-0028). Cascateia o `CancellationToken` pro `RunExecutor`
/// do stage em curso; stages futuros são marcados `Cancelled`
/// direto pelo loop.
#[tauri::command]
async fn cancel_pipeline(pipeline_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .orch
        .cancel_pipeline(&pipeline_id)
        .map_err(|e| format!("{e}"))
}

/// `tauri::command` que lista os `MultimodelRun`s em estado
/// `Running` ou `PartiallyCompleted` (D5 do ADR-0028: a UI
/// carrega esses no startup e oferece "retomar pipeline
/// interrompido"). Devolve `Vec<MultimodelRun>` direto (sem
/// view no `shared-contracts` — a Etapa 6 consome no
/// frontend via `dispatch<ListResumablePipelinesView[]>`).
///
/// **Por que separado do `ipc_dispatch`:** o `ipc_dispatch`
/// enfileira no canal do orchestrator; um comando dedicado é
/// mais barato e não disputa com `MessageSend` no startup
/// (que já chama `list_conversations` + `list_specialists` +
/// agora `list_resumable_pipelines` em paralelo).
#[tauri::command]
async fn list_resumable_pipelines(
    state: State<'_, AppState>,
) -> Result<Vec<frederico_storage::MultimodelRun>, String> {
    use frederico_storage::PipelineRepo;
    PipelineRepo::new(&state.db)
        .list_resumable()
        .await
        .map_err(|e| format!("{e}"))
}

/// `tauri::command` que lista os `MultimodelStage`s de um
/// pipeline (Etapa 7 do Modo Equipe, UI/Polish). Devolve
/// `Vec<MultimodelStage>` ordenado por `seq` ASC (mesma ordem
/// do `PipelineRepo::list_stages`).
///
/// **Por que separado do `ipc_dispatch`:** o `list_resumable_pipelines`
/// devolve só cabeçalhos (`MultimodelRun`). Pra UI mostrar o
/// progresso de cada stage, ela precisa dos stages
/// individuais. Um comando dedicado é mais barato que passar
/// um struct gordo pelo `ipc_dispatch`.
///
/// **Erros:** retorna `Err(MultimodelError::RunNotFound)` se
/// o `run_id` não existe (a UI mostra "pipeline não
/// encontrado"). `Sqlite` errors propagam como `String`
/// legível.
#[tauri::command]
async fn list_pipeline_stages(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<frederico_storage::MultimodelStage>, String> {
    use frederico_storage::PipelineRepo;
    PipelineRepo::new(&state.db)
        .list_stages(&run_id)
        .await
        .map_err(|e| format!("{e}"))
}
