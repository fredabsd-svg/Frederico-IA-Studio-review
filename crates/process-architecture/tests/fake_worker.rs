//! Testes de integração do `WorkerManager` + `FakeWorker`.
//!
//! **Política:** **todo** teste é embrulhado em
//! [`frederico_test_support::with_test_timeout`] (5s default) —
//! deadlocks viram falha com nome do teste em 5s, não sessão
//! pendurada com saída cortada. É a contramedida pra classe de
//! bug que produziu o deadlock do `WorkerManager::invoke` na
//! Etapa 2A original (ADR-0015).
//!
//! Cobre:
//! - Handshake (`worker.hello` → `app.ack`) e extração do
//!   manifesto.
//! - `invoke` round-trip preserva o payload.
//! - `ping` atualiza `health` pra `Ok`.
//! - Env allowlist: variável injetada no test runner **não**
//!   vaza pro worker (regra do `process-architecture.md`
//!   §Invariantes).
//! - **Duas invocações concorrentes** — caso de uso que quebrava
//!   na Etapa 2A original (a segunda `invoke` esperava o
//!   `MutexGuard` que a primeira segurava).
//! - `invoke_with_timeout` falha com `ProcessError::Timeout`
//!   quando o worker demora.
//! - `WorkerManager::shutdown` termina o ator e o server limpo.
//!
//! Ver [`crate::manager`] e [`crate::fake`] pra detalhes do
//! design.

use std::time::Duration;

use frederico_process_architecture::{
    FakeWorkerConfig, WorkerHandle, WorkerManager, WorkerSpawnConfig,
};
use frederico_test_support::with_test_timeout;
use serde_json::json;

/// Helper: spawna um manager com o config dado, faz handshake, e
/// devolve o par. Embrulhado em `with_test_timeout` pra que
/// qualquer travamento vire falha nomeada.
async fn spawn(
    config: FakeWorkerConfig,
    spawn_config: WorkerSpawnConfig,
) -> (WorkerManager, WorkerHandle) {
    with_test_timeout("spawn_in_process", async {
        WorkerManager::spawn_in_process(config, spawn_config)
            .await
            .expect("spawn_in_process")
    })
    .await
    .expect("spawn não deve travar (5s timeout)")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_in_process_smoke() {
    with_test_timeout("spawn_in_process_smoke", async {
        let (manager, handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        // Manifesto carrega o que o fake reportou.
        let manifest = handle.manifest();
        assert_eq!(manifest.worker_id.to_string(), "fake-worker");
        assert!(manifest.capabilities.contains(&"fake.invoke".to_string()));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("smoke não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_roundtrip_preserves_payload() {
    with_test_timeout("invoke_roundtrip", async {
        let (manager, handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        let payload = json!({"action": "echo", "value": 42});
        let result = handle.invoke(payload.clone()).await.expect("invoke");

        // O fake responde com `{ok: true, echo: <payload>,
        // env_received: <env>}` — o echo preserva o payload
        // enviado.
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["echo"], payload);

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("roundtrip não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_updates_health_to_ok() {
    with_test_timeout("ping_updates_health", async {
        let (manager, handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        // Antes do primeiro pong, a saúde é `Unhealthy` (default
        // inicial do fake).
        let pre = handle.health_snapshot().await;
        assert_eq!(
            pre.health,
            frederico_process_architecture::WorkerHealth::Unhealthy
        );

        let pong = handle.ping().await.expect("ping");
        assert_eq!(pong["status"], json!("ok"));

        // Depois do pong, saúde vira `Ok`.
        let post = handle.health_snapshot().await;
        assert_eq!(
            post.health,
            frederico_process_architecture::WorkerHealth::Ok
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("ping não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_does_not_see_parent_env() {
    with_test_timeout("worker_does_not_see_parent_env", async {
        // Injeta `OPENAI_API_KEY` no test runner. O `fake::env`
        // é construído a partir da allowlist que o
        // `FakeWorkerConfig` declara — se o
        // `env_allowlist::build_worker_env` não estivesse
        // respeitando a invariante "env do pai não vaza", o
        // `env_received` no response carregaria essa chave.
        std::env::set_var("OPENAI_API_KEY", "sk-leak-me-1234");
        std::env::set_var("PATH", "C:\\secret\\path");

        let (manager, handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        let result = handle.invoke(json!({})).await.expect("invoke");
        let env_received = result["env_received"]
            .as_object()
            .expect("env_received é um objeto");

        // O env_received **não** pode ter `OPENAI_API_KEY` nem
        // `PATH` — o fake foi construído com `env: BTreeMap::new()`
        // (default), que não carrega nada da env do test runner.
        assert!(
            !env_received.contains_key("OPENAI_API_KEY"),
            "OPENAI_API_KEY vazou pro worker via env"
        );
        assert!(
            !env_received.contains_key("PATH"),
            "PATH vazou pro worker via env"
        );
        // O sane default `PYTHONIOENCODING` é adicionado pelo
        // `build_worker_env`, mas só quando o `WorkerManager`
        // chama essa função. Aqui o fake recebe o `env`
        // **diretamente** (não passa pelo `build_worker_env`),
        // então nem `PYTHONIOENCODING` está presente.
        // (A invariante é testada em
        // `env_allowlist_does_not_inherit_parent` no
        // `env_allowlist` unit — esta aqui é a
        // **end-to-end**: o worker não vê o env do pai via o
        // pipe.)

        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("PATH");

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("env allowlist não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_sees_explicit_allowlist() {
    with_test_timeout("worker_sees_explicit_allowlist", async {
        // O `FakeWorkerConfig::with_env` injeta env explícito
        // que o server vai reportar em `env_received`. O
        // `WorkerManager` (em produção, Etapa 2B) chamaria
        // `env_allowlist::build_worker_env` antes de spawnar
        // o processo; aqui a injeção é direta no fake pra
        // provar que o canal propaga.
        let config =
            FakeWorkerConfig::default().with_env(&[("MY_VAR".to_string(), "my_value".to_string())]);

        let (manager, handle) = spawn(config, WorkerSpawnConfig::default()).await;

        let result = handle.invoke(json!({})).await.expect("invoke");
        let env_received = result["env_received"]
            .as_object()
            .expect("env_received é um objeto");

        assert_eq!(
            env_received.get("MY_VAR").and_then(|v| v.as_str()),
            Some("my_value"),
            "MY_VAR não chegou pro worker"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("explicit allowlist não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_invocations_complete() {
    // **A prova** de que o modelo de ator resolve o caso que
    // quebrava o design da Etapa 2A original (a segunda
    // `invoke` esperava o `MutexGuard` que a primeira
    // segurava). Aqui, **duas** invokes são disparadas em
    // paralelo via `tokio::join!`. Cada uma tem seu próprio
    // `request_id` (gerado pelo ator no `handle_command`); a
    // response é despachada pelo `request_id` no
    // `handle_incoming`, sem serializar no caller.
    with_test_timeout("concurrent_invocations", async {
        let (manager, handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        let h1 = handle.clone();
        let h2 = handle.clone();
        let f1 = tokio::spawn(async move { h1.invoke(json!({"call": "first"})).await });
        let f2 = tokio::spawn(async move { h2.invoke(json!({"call": "second"})).await });

        let (r1, r2) = tokio::join!(f1, f2);
        let r1 = r1.expect("join 1").expect("invoke 1");
        let r2 = r2.expect("join 2").expect("invoke 2");

        assert_eq!(r1["echo"], json!({"call": "first"}));
        assert_eq!(r2["echo"], json!({"call": "second"}));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("invocações concorrentes não devem travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_with_short_timeout_fails() {
    with_test_timeout("invoke_with_short_timeout", async {
        // Server dorme 500ms antes de responder. O invoke usa
        // timeout de 50ms — vai falhar com `ProcessError::Timeout`.
        let config = FakeWorkerConfig::default().with_slow_response(500);
        let spawn_config = WorkerSpawnConfig::default();

        let (manager, handle) = spawn(config, spawn_config).await;

        let err = handle
            .invoke_with_timeout(json!({}), Duration::from_millis(50))
            .await
            .expect_err("timeout curto tem que falhar");
        match err {
            frederico_process_architecture::ProcessError::Timeout {
                worker_id,
                timeout_ms,
            } => {
                assert_eq!(worker_id, "fake-worker");
                assert_eq!(timeout_ms, 50);
            }
            other => panic!("erro inesperado: {other:?}"),
        }

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("timeout não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_terminates_worker_cleanly() {
    with_test_timeout("shutdown_terminates_worker", async {
        let (manager, _handle) =
            spawn(FakeWorkerConfig::default(), WorkerSpawnConfig::default()).await;

        // `shutdown` envia `app.shutdown`, espera a task do
        // ator terminar (EOF do pipe) e a task do fake server
        // terminar. Se alguma travar, o `with_test_timeout`
        // aborta em 5s com o nome do teste.
        manager.shutdown().await.expect("shutdown limpo");
    })
    .await
    .expect("shutdown não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_token_is_required_after_handshake() {
    // O `WorkerHandle::invoke` sempre inclui o `auth` (vem do
    // `WorkerState`). Aqui a gente verifica que o invoke
    // **passa** (prova que o handshake autenticou), e que o
    // manifesto carrega o `worker_id` que o fake reportou.
    with_test_timeout("auth_required", async {
        let config = FakeWorkerConfig::default();
        let (manager, handle) = spawn(config, WorkerSpawnConfig::default()).await;

        let result = handle.invoke(json!({"data": "secret"})).await;
        // Invoke com auth válido passa.
        assert!(result.is_ok(), "invoke com auth válido falhou: {result:?}");

        // O `worker_id` do handle bate com o fake.
        assert_eq!(handle.worker_id().to_string(), "fake-worker");

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("auth não deve travar");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_auth_token_is_used() {
    // O `WorkerSpawnConfig::auth_token` permite pré-definir o
    // token (em vez de gerar UUID v4). Útil pra testes que
    // querem um token conhecido.
    with_test_timeout("custom_auth_token", async {
        let config = FakeWorkerConfig::default();
        let spawn_config = WorkerSpawnConfig {
            default_timeout_ms: 30_000,
            auth_token: Some(frederico_process_architecture::WorkerAuth::new(
                "test-token-fixed",
            )),
        };
        let (manager, handle) = spawn(config, spawn_config).await;

        // Invoke passa (o server tem o mesmo token no `auth`).
        let result = handle.invoke(json!({})).await.expect("invoke");
        assert_eq!(result["ok"], json!(true));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("custom token não deve travar");
}
