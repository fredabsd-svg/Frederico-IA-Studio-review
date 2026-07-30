//! Integration tests do `WorkerManager::spawn_external` —
//! caminho real de worker sidecar (não o fake `mpsc`).
//!
//! **Política:** **todo** teste é embrulhado em
//! [`frederico_test_support::with_test_timeout`] (5s default) —
//! deadlocks viram falha com nome do teste em 5s, não sessão
//! pendurada.
//!
//! **O que estes testes provam (Fase 5, Etapa 2B — fecha a
//! pendência "spawn_external" registrada em
//! `docs/modules/process-architecture.md` §Pendências):**
//!
//! 1. **`external_spawn_roundtrip_full_protocol`** — spawna o
//!    **stub Rust** (`worker_stub` binário em
//!    `src/bin/worker_stub.rs`, compilado via `[[bin]]` no
//!    `Cargo.toml` deste crate) que implementa o protocolo
//!    completo sobre `tokio::net::windows::named_pipe` (mesma
//!    stack do manager). Valida:
//!    - Spawn do filho via `tokio::process::Command`.
//!    - Leitura da linha `READY <name>` do stdout (handshake
//!      invertido, ADR-0017 §Decisão 2).
//!    - `connect_pipe_client` no nome anunciado.
//!    - `worker.hello` do stub com manifesto correto.
//!    - `app.ack` enviado com auth token.
//!    - `tool.invoke` roundtrip preserva payload.
//!    - `ping` retorna `worker.pong` com `status: "ok"`.
//!    - `shutdown` gracioso: app envia `app.shutdown`, worker
//!      fecha o pipe, `actor_task` detecta EOF, `child.wait()`
//!      retorna, processo termina com exit 0.
//!
//! 2. **`external_spawn_fails_when_command_missing`** — usa um
//!    binário que não existe. Espera
//!    `ProcessError::Platform` (categoria `Platform`, código
//!    `process_platform_error`). Sem child vivo nesse caso.
//!
//! 3. **`external_spawn_timeout_when_no_ready`** — usa
//!    `cmd /c timeout` que dorme sem imprimir `READY`. O
//!    `ready_timeout` do `ExternalSpawnConfig` é 500ms — espera
//!    `ProcessError::Platform` com mensagem mencionando o
//!    timeout. Valida que o **child** é morto (kill+wait) antes
//!    de retornar o erro (não fica zumbizando).
//!
//! ## Gate Windows
//!
//! O `spawn_external` é `#[cfg(windows)]` (named pipes são
//! Windows). Em outras plataformas o módulo inteiro de
//! integração fica vazio — o `cargo test` em Linux/macOS
//! compila o crate sem esses testes.

#![cfg(windows)]

use std::time::Duration;

use frederico_process_architecture::{ExternalSpawnConfig, WorkerHealth};
use frederico_test_support::{with_test_timeout, with_test_timeout_at};
use serde_json::json;

/// Path do binário `worker_stub` — stub Rust do worker
/// sidecar, compilado automaticamente pelo Cargo via
/// `[[bin]]` no `Cargo.toml` deste crate. O
/// `env!("CARGO_BIN_EXE_worker_stub")` é setado pelo Cargo
/// no momento do build dos tests, apontando pro binário
/// compilado. Substituiu o stub PowerShell
/// (`tests/stubs/worker-stub.ps1`) que era flaky no CI
/// runner (cold-start > 30s no Windows Server 2022 do GitHub
/// Actions; PR #11 runs #30541220400 / #30541686745 /
/// #30542253439 / #30542716235).
const STUB_BIN: &str = env!("CARGO_BIN_EXE_worker_stub");

const STUB_MANIFEST: &str = "tests/stubs/manifest-stub.json";

/// Budget de timeout pro test E2E. Maior que o default 5s
/// do `with_test_timeout` porque o stub Rust precisa de
/// cold-start (compilação cargo, primeira execução) +
/// handshake completo (`READY` + connect + `worker.hello`).
/// No Windows Server 2022 runner do GitHub Actions o
/// `cargo test` adiciona ~5-10s de overhead (compilação de
/// deps não cacheadas), então o budget de 30s é folgado mas
/// não exagerado. Local < 2s.
const E2E_TIMEOUT: Duration = Duration::from_secs(30);

/// `ready_timeout` do `ExternalSpawnConfig` pro test E2E.
/// Maior que o default 10s porque o `cargo test` overhead
/// no CI pode adicionar latência. 20s é folgado (stub
/// local < 100ms).
const E2E_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Helper: monta o `ExternalSpawnConfig` apontando pro stub
/// Rust (`worker_stub` binário). Embrulhado em
/// `with_test_timeout` no nível de cada teste.
fn stub_config(
    extra: impl FnOnce(ExternalSpawnConfig) -> ExternalSpawnConfig,
) -> ExternalSpawnConfig {
    let mut cfg = ExternalSpawnConfig::new(STUB_BIN)
        .with_args(vec!["--manifest".to_string(), STUB_MANIFEST.to_string()])
        .with_auth_token("integration-test-token");
    // Working dir do filho = o do test runner (que é `cwd` da
    // workspace quando o `cargo test` roda). Garante que o
    // caminho relativo `tests/stubs/manifest-stub.json` resolve.
    cfg = extra(cfg);
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_spawn_roundtrip_full_protocol() {
    with_test_timeout_at(
        "external_spawn_roundtrip_full_protocol",
        E2E_TIMEOUT,
        async {
            let cfg = stub_config(|c| {
                c.with_cwd(env!("CARGO_MANIFEST_DIR"))
                    .with_ready_timeout(E2E_READY_TIMEOUT)
            });
            let (manager, handle) =
                frederico_process_architecture::WorkerManager::spawn_external(cfg)
                    .await
                    .expect("spawn_external deve succeed");

            // 1. Manifesto carrega o que o stub reportou.
            let manifest = handle.manifest();
            assert_eq!(manifest.worker_id.to_string(), "stub-worker");
            assert!(manifest.capabilities.contains(&"stub.echo".to_string()));

            // 2. `invoke` roundtrip — o stub faz echo do payload.
            let payload = json!({"action": "echo", "value": 42, "list": [1, 2, 3]});
            let result = handle.invoke(payload.clone()).await.expect("invoke");
            assert_eq!(result["ok"], json!(true));
            assert_eq!(result["echo"], payload);

            // 3. `ping` retorna `worker.pong`.
            let pong = handle.ping().await.expect("ping");
            assert_eq!(pong["status"], json!("ok"));

            // 4. `health` virou `Ok` depois do pong.
            let snap = handle.health_snapshot().await;
            assert_eq!(snap.health, WorkerHealth::Ok);

            // 5. `shutdown` gracioso: app.shutdown → worker fecha
            //    pipe → actor_task EOF → child.wait() retorna.
            manager.shutdown().await.expect("shutdown");
        },
    )
    .await
    .expect("E2E roundtrip não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_spawn_fails_when_command_missing() {
    with_test_timeout("external_spawn_fails_when_command_missing", async {
        // Binário que **certamente** não existe. PATH é
        // configurado no test runner mas o nome é único o
        // suficiente pra não colidir com nada real.
        let cfg = ExternalSpawnConfig::new("frederico-nonexistent-binary-12345-xyz")
            .with_args(vec!["--doesnt-matter".to_string()])
            .with_ready_timeout(Duration::from_secs(2));

        let result = frederico_process_architecture::WorkerManager::spawn_external(cfg).await;

        match result {
            Ok(_) => panic!("binário inexistente deveria ter falhado"),
            Err(frederico_process_architecture::ProcessError::Platform { .. }) => {
                // Esperado.
            }
            Err(other) => panic!("esperava Platform, veio: {other}"),
        }
    })
    .await
    .expect("fail-on-missing não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_spawn_timeout_when_no_ready() {
    with_test_timeout("external_spawn_timeout_when_no_ready", async {
        // `cmd /c timeout /t 5 /nobreak >nul` no Windows:
        // processo que dorme 5s sem imprimir nada. O
        // `ready_timeout` 500ms do `ExternalSpawnConfig`
        // dispara antes.
        //
        // Validamos que o `child` é **morto** (kill+wait) antes
        // do erro retornar — o integration test não pode ficar
        // com zumbi.
        let cfg = ExternalSpawnConfig::new("cmd")
            .with_args(vec![
                "/c".to_string(),
                "timeout".to_string(),
                "/t".to_string(),
                "5".to_string(),
                "/nobreak".to_string(),
                ">nul".to_string(),
            ])
            .with_ready_timeout(Duration::from_millis(500));

        let start = std::time::Instant::now();
        let result = frederico_process_architecture::WorkerManager::spawn_external(cfg).await;
        let elapsed = start.elapsed();

        match result {
            Ok(_) => panic!("processo sem READY deveria ter falhado"),
            Err(frederico_process_architecture::ProcessError::Platform { .. }) => {
                // Esperado.
            }
            Err(other) => panic!("esperava Platform, veio: {other}"),
        }
        // Tem que falhar perto do timeout (não dos 5s do
        // `timeout /t 5`). Tolerância: < 2s.
        assert!(
            elapsed < Duration::from_secs(2),
            "demorou demais ({:?}) — child não foi morto?",
            elapsed
        );
    })
    .await
    .expect("timeout-no-ready não deve travar");
}
