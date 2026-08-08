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
//! isso o app nunca abriu e a suíte seguia verde". Esse é
//! o problema que este teste resolve. Roda em todo PR
//! automaticamente via `cargo test --workspace`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Grace window antes de checar o status. 5s é mais que
/// suficiente pro setup rodar; um panic se manifesta em
/// <1s (ver doc do módulo).
const STARTUP_GRACE_SECS: u64 = 5;

/// Tamanho máximo do stderr incluído na mensagem de panic
/// (evita spam se o panic log for enorme).
const STDERR_PREVIEW_BYTES: usize = 2048;

/// Smoke test: `frederico-desktop` sobe sem panic no
/// `.setup` da casca Tauri.
///
/// **Cobre (gate contra regressão):**
/// - `recovery.rs` removido do `tokio::spawn` no setup
///   (a v1 panicava; o fix usa `tauri::async_runtime::spawn`).
/// - Qualquer outro panic introduzido no setup que
///   silencie o CI (mesma classe de bug — E2E in-process
///   não pega, só `cargo run` pega).
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

    // Spawn o binário capturando stderr (panics escrevem
    // nele). stdout descartado — o app loga em modo normal
    // mas o gate não precisa.
    let mut child = Command::new(bin_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            panic!(
                "falha ao spawnar `{bin_path}`: {e} \
                    (verifique se `apps/desktop/dist/` \
                    está construído — `npm run build` \
                    em `apps/desktop/`)"
            );
        });

    // Espera o setup completar. Se o setup panicar, o
    // processo sai em <1s; se passar, fica vivo (janela
    // aberta / event loop rodando).
    std::thread::sleep(Duration::from_secs(STARTUP_GRACE_SECS));

    match child.try_wait() {
        Ok(Some(status)) => {
            // Processo saiu durante a janela de graça —
            // captura stderr pra diagnóstico.
            let mut stderr_bytes = Vec::new();
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_end(&mut stderr_bytes);
            }
            let stderr = String::from_utf8_lossy(&stderr_bytes);
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
            // Ainda vivo — o setup passou. Mata antes do
            // test terminar pra não deixar processo zumbi
            // segurando o handle do banco.
            //
            // Ignoramos o resultado do `kill`/`wait` —
            // se o processo já morreu entre o `try_wait`
            // e o `kill`, tanto faz (vai retornar
            // `InvalidInput` ou similar).
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(e) => {
            // Erro ao checar status — improvável, mas
            // mata o processo e falha o test.
            let _ = child.kill();
            let _ = child.wait();
            panic!("erro ao verificar status do processo: {e}");
        }
    }
}
