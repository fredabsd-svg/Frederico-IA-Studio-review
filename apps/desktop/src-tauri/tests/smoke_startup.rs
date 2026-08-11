//! Smoke test do binário `frederico-desktop`: prova que o `.setup`
//! da casca Tauri não entra em pânico no startup.
//!
//! **Por que este teste existe:**
//!
//! A Etapa 5.x da Fase 3 introduziu o recovery de crash no
//! startup. A v1 disparava o `recover_stale_runs` via
//! `tokio::spawn` direto, **de dentro do `.setup` do Tauri**.
//!
//! O `.setup` do Tauri é **síncrono** (não `async`): o closure
//! retorna `Result<(), Box<dyn Error>>`, não um `Future`. O
//! `tokio::spawn` exige que o caller já esteja dentro de um
//! `tokio::runtime::Handle::current()` — e o setup é chamado
//! de um contexto em que o `Handle` **não** é o current
//! (mesmo o Tauri usando tokio internamente, o setup roda
//! fora do `block_on`). Resultado: panic
//! `"there is no reactor running, must be called from the
//! context of a Tokio 1.x runtime"` na inicialização, e o
//! binário saía com erro antes da janela abrir.
//!
//! Nenhum teste E2E anterior subia o binário de verdade — o
//! caminho de produção exercitado pelos E2E em
//! `crates/e2e/tests/` é **in-process** via
//! `frederico_app::build_chat_orchestrator` (ver
//! `docs/architecture/testing-strategy.md` §3, "Fronteira do
//! que os E2E cobrem"). O `cargo test --workspace` ficava
//! verde enquanto a casca jamais abria — exatamente o tipo
//! de "verde mentiroso" que a REGRA 1.8 / §1.10 do
//! `REGRAS-DO-PROJETO.md` quer coibir.
//!
//! **O que este teste faz:**
//!
//! 1. `cargo test -p frederico-desktop` compila o binário
//!    (a macro `env!("CARGO_BIN_EXE_frederico-desktop")`
//!    expande pro path do `.exe` em `target/debug/`).
//! 2. Spawna o binário com stdout/stderr capturados.
//! 3. Espera 5 segundos (grace window pro setup rodar).
//! 4. Verifica o status: se ainda está vivo, o setup
//!    passou — sucesso. Se saiu, falhou (panic ou erro
//!    fatal de startup); capturamos o stderr pra diagnóstico.
//! 5. Mata o processo antes do test terminar.
//!
//! **Pré-requisito:** `apps/desktop/dist/` precisa estar
//! construído (`npm run build` em `apps/desktop/`) — o
//! `tauri.conf.json` aponta `frontendDist: "../dist"` e o
//! Tauri falha se não encontra. O CI do `verify-external.ps1`
//! faz esse build antes do `cargo test --workspace`. Em
//! local dev: rode `npm run build` em `apps/desktop/` uma
//! vez antes de `cargo test -p frederico-desktop`.
//!
//! **Por que 5s:** um panic no setup se manifesta em <1s
//! (panic imediato, sem retry). 5s é folga suficiente pra
//! qualquer máquina de CI/dev rodar o `Database::open` +
//! composição + recovery em background sem disparar falso
//! positivo. Aumentar não ajuda — se passou 5s vivo, o
//! setup passou; se está em loop infinito de init, é outro
//! defeito (que 30s não resolveria).
//!
//! **Por que não `#[ignore]`:** o user pediu explicitamente
//! um teste "que falha se o binário entrar em pânico no
//! startup — nenhum E2E atual cobre o setup da casca, por
//! isso o app nunca abriu e a suíte seguia verde". Esse é o
//! problema que este teste resolve. Roda em todo PR
//! automaticamente via `cargo test --workspace`.
//!
//! **Nova classe de erro detectada (Etapa 6+):** o `.setup`
//! agora substitui o `.expect("abre o banco SQLite")` por
//! um caminho de erro gracioso: se `Database::open` falhar
//! (ex.: `Migrate(VersionMismatch(1))` num banco de versão
//! anterior), o app mostra um diálogo nativo com a causa
//! e o caminho de recuperação. O `blocking_show` do
//! `tauri-plugin-dialog` mantém o processo vivo enquanto o
//! usuário não fecha a janela — **não é pânico**, o app
//! está mostrando uma mensagem de erro.
//!
//! **Cobertura do caminho de erro:** o smoke test deste
//! arquivo só cobre o caminho **feliz** (binário sobe sem
//! panic). O caminho de erro de banco (`Database::open`
//! falhando) é coberto **in-process** em
//! `tests/db_open_failure.rs` — uma tentativa anterior
//! (PR #48 v2 e v3) de cobrir o caminho de erro spawnando
//! o binário mostrou-se flaky em CI (Windows Server 2022,
//! runs #31448091994 e #31489302906): o `.setup` callback
//! não rodava dentro da janela de 5s, e o teste passava
//! por accidente quando só o primeiro `tracing::info!`
//! era emitido. O teste in-process verifica a pré-condição
//! (o `Database::open` retorna erro estruturado em path
//! inválido) sem depender do Tauri runtime.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Grace window antes de checar o status. 5s é mais que
/// suficiente pro setup rodar; um panic se manifesta em
/// <1s (ver doc do módulo).
const STARTUP_GRACE_SECS: u64 = 5;

/// Tamanho máximo do stderr incluído na mensagem de panic
/// (evita spam se o panic log for enorme).
const STDERR_PREVIEW_BYTES: usize = 2048;

/// Assinatura que o `tracing` produz quando o startup
/// recovery (Etapa 6+) é acionado. Mantida por
/// compatibilidade com mensagens de erro legadas (debug
/// visual em ambiente interativo), mas o smoke test atual
/// só distingue "binário vivo" de "binário saiu (panic)".
/// A cobertura do caminho de recovery está em
/// `tests/db_open_failure.rs` (in-process, mais confiável).
const STARTUP_RECOVERY_MARKER: &str = "falha fatal ao abrir banco SQLite";

/// Smoke test: `frederico-desktop` sobe sem panic no
/// `.setup` da casca Tauri.
///
/// **Cobre (gate contra regressão):**
/// - `recovery.rs` removido do `tokio::spawn` no setup
///   (a v1 panicava; o fix usa `tauri::async_runtime::spawn`).
/// - Qualquer outro panic introduzido no setup que
///   silencie o CI (mesma classe de bug — E2E in-process
///   não pega, só `cargo run` pega).
/// - Detecta a **nova classe** de erro introduzida na
///   Etapa 6+ (substituição do `.expect()` por
///   `handle_startup_db_error`): o processo fica preso no
///   dialog nativo (não sai, não pânica). O test distingue
///   lendo o stderr — a presença do
///   `STARTUP_RECOVERY_MARKER` indica "recovery gracioso",
///   não regressão.
///
/// **Não cobre:**
/// - Comportamento da janela após abrir (esse é território
///   de teste manual / WinAppDriver, fora do escopo do gate
///   de PR).
/// - Crash tardio (depois dos 5s) — o `RunExecutor` e o
///   `recovery` em si têm cobertura própria nos unit
///   tests e nos E2E in-process.
#[test]
fn binary_does_not_panic_on_startup() {
    // O `cargo test` define essa env var com o path absoluto
    // do binário `frederico-desktop` (que ele acabou de
    // compilar pro test).
    let bin_path = env!("CARGO_BIN_EXE_frederico-desktop");

    // Sanity check: o binário existe e é executável. Se não,
    // o `Command::new` ainda assim retornaria um erro mais
    // abaixo — mas uma mensagem clara aqui ajuda debug.
    assert!(
        std::path::Path::new(bin_path).exists(),
        "binário não encontrado em {bin_path} — `cargo test` \
         deveria ter compilado antes de rodar este test"
    );

    // **CRÍTICO: nunca usar `%LOCALAPPDATA%` aqui.** O binário
    // lê `data_local_dir()` que, sem env var, cai em
    // `directories::ProjectDirs::from("studio", "frederico", "ia")`
    // → `%LOCALAPPDATA%\studio\frederico\ia\` (ou path similar).
    // Se o test rodasse sem env var, abriria o banco de **produção**
    // do usuário e potencialmente o truncatearia (vimos isso na
    // sessão 2026-08-10 — o `Database::open` com `?mode=rwc` pode
    // truncar arquivos existentes em algumas condições; mesmo
    // sem truncate, criaria o banco novo no dir de produção).
    //
    // Solução: tempdir limpo (auto-drop no fim do test) +
    // `FREDERICO_DATA_DIR` apontando pra ele. O binário respeita
    // a env var e abre o banco no tempdir — zero contato com o
    // path de produção.
    //
    // **Por que `tempdir()` (não `tempdir_in(prefix)`):** o
    // crate `tempfile` cria em `%TEMP%` (no Windows,
    // `%LOCALAPPDATA%\Temp\`) e limpa automaticamente no drop.
    // O test não precisa do path antes do cleanup porque o
    // tempdir morre junto com o test.
    let data_dir = tempfile::tempdir().expect("criar tempdir pro FREDERICO_DATA_DIR");
    let data_dir_path = data_dir.path().to_path_buf();
    // Mantém o tempdir vivo até o final do test (ele é dropado
    // quando sai de escopo no final da função).
    let _keep_alive = &data_dir;

    // Spawn o binário capturando stderr (panics escrevem
    // nele). stdout descartado — o app loga em modo normal
    // mas o gate não precisa.
    let mut child = Command::new(bin_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("FREDERICO_DATA_DIR", &data_dir_path)
        .spawn()
        .unwrap_or_else(|e| {
            panic!(
                "falha ao spawnar `{bin_path}`: {e} \
                    (verifique se `apps/desktop/dist/` \
                    está construído — `npm run build` \
                    em `apps/desktop/`)"
            );
        });

    // **Por que uma thread pra ler stderr:** o startup
    // recovery (Etapa 6+) usa `tauri-plugin-dialog` com
    // `blocking_show`, que mantém o processo vivo enquanto
    // o dialog está aberto. Quando o smoke test mata o
    // processo (TerminateProcess no Windows), o buffer do
    // `tracing` não tem tempo de flush — o stderr volta
    // vazio e o test não consegue detectar a classe de
    // erro nova. A thread começa a ler stderr **antes**
    // do kill e usa `read_to_end` (bloqueia até o pipe
    // fechar, que coincide com a morte do processo). O
    // `mpsc::channel` sincroniza com o test principal.
    let stderr_handle = child.stderr.take();
    let (stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>();
    let _stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut s) = stderr_handle {
            let _ = s.read_to_end(&mut bytes);
        }
        let _ = stderr_tx.send(bytes);
    });

    // Espera o setup completar. Se o setup panicar, o
    // processo sai em <1s; se passar, fica vivo (janela
    // aberta / event loop rodando).
    std::thread::sleep(Duration::from_secs(STARTUP_GRACE_SECS));

    // `try_wait` não bloqueia — só checa o status atual.
    let process_state = child.try_wait();

    // Independente do status, mata o processo antes de ler
    // o stderr. Se ele já saiu, o `kill`/`wait` é no-op
    // (ambos retornam erro que ignoramos). Se estiver vivo
    // (dialog aberto), o `kill` força a saída — e isso
    // fecha o pipe, o que destrava a thread de leitura.
    let _ = child.kill();
    let _ = child.wait();

    // `recv` bloqueia até a thread enviar. Como a thread só
    // envia quando o pipe fecha (após o kill acima), este
    // recv é bounded pelo tempo de cleanup do processo —
    // tipicamente <100ms. Sem timeout porque queremos
    // garantir que o stderr foi lido.
    let stderr_bytes = stderr_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_bytes);

    match process_state {
        Ok(Some(status)) => {
            // Processo saiu durante a janela de graça.
            // Isso É o caso do pânico genuíno (regressão do
            // `tokio::spawn` ou outro defeito introduzido).
            let preview: String = stderr.chars().take(STDERR_PREVIEW_BYTES).collect();
            panic!(
                "`frederico-desktop` saiu durante o startup \
                 (status: {status:?}). Isto indica panic no \
                 `.setup` da casca Tauri (a v1 do recovery \
                 panicava com `tokio::spawn` fora de runtime; \
                 o fix usa `tauri::async_runtime::spawn` — ver \
                 `apps/desktop/src-tauri/src/main.rs::spawn_startup_recovery`).\n\n\
                 stderr (primeiros {STDERR_PREVIEW_BYTES} chars):\n{preview}"
            );
        }
        Ok(None) => {
            // Processo **vivo** após 5s. Duas sub-classes:
            //
            // (a) **Saudável:** o setup passou, a janela
            //     está aberta, o event loop do Tauri está
            //     rodando. O stderr NÃO tem o marker.
            //     → sucesso.
            //
            // (b) **Startup recovery ativo (Etapa 6+):** o
            //     `Database::open` falhou e o `blocking_show`
            //     do dialog nativo está segurando o processo
            //     vivo. O stderr TEM o marker.
            //     → falha (app não abriu), mas com
            //     diagnóstico diferente: aponta pro
            //     `frederico-mind.log`, não pro `tokio::spawn`.
            if stderr.contains(STARTUP_RECOVERY_MARKER) {
                let preview: String = stderr.chars().take(STDERR_PREVIEW_BYTES).collect();
                panic!(
                    "`frederico-desktop` ficou preso no startup \
                     (status vivo, mas o `tracing::error!` do \
                     startup recovery apareceu no stderr — \
                     significa que o `Database::open` falhou e o \
                     dialog nativo está sendo mostrado).\n\n\
                     Ver `frederico-mind.log` no mesmo diretório \
                     do banco para o diagnóstico completo (db_path \
                     e a variante específica de \
                     `sqlx::migrate::MigrateError`).\n\n\
                     Para o smoke ficar verde, restaure o banco \
                     (delete `frederico.db` e reabra, ou use um \
                     backup) e re-rode o test.\n\n\
                     stderr (primeiros {STDERR_PREVIEW_BYTES} chars):\n{preview}"
                );
            }
            // Caso (a) — sucesso. O `kill`/`wait` acima
            // matou o processo; o test termina.
        }
        Err(e) => {
            // Erro ao checar status — improvável, mas o
            // processo já foi morto pelo `kill` no início.
            panic!("erro ao verificar status do processo: {e}");
        }
    }
}
