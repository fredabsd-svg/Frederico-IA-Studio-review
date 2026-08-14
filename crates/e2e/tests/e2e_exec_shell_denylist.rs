//! E2E — `exec.shell` recusa comandos destrutivos antes do spawn
//! (Etapa 7 da Fase 7, ADR-0034 D3).
//!
//! **Teste de negação principal** (regra do user, 2026-08-08:
//! "pelo menos um teste de negação por etapa" — sandbox se prova
//! impedindo, não funcionando). Nomeado conforme
//! `docs/architecture/exec-tools-specification.md` §"Mapa de E2E":
//! `e2e_exec_shell_denylist.rs::rm_rf_is_rejected_by_denylist`.
//!
//! **Por que não precisa de `RuntimeRegistry` bootstrapped** (ao
//! contrário de `e2e_exec_python_under_sandbox.rs`): `exec.shell`
//! usa `cmd.exe` (built-in do SO via `%SystemRoot%`), não um
//! runtime portátil. O `RuntimeRegistry` passado pra
//! `build_default_exec_tools` fica vazio (sem bootstrap) — shell
//! não o consulta (`resolve_runtime_id` retorna um valor
//! informativo, não usado pra lookup).
//!
//! **Por que a recusa é provada sem spawn:** `denylist_hit` roda
//! dentro de `build_args`, chamado **antes** de qualquer
//! `SecurityJailResolver::spawn`. O teste prova isso indiretamente
//! — o erro retornado é o `ExecError::CommandDenied` (mensagem
//! "recusado pela denylist"), não um erro de spawn/exit-code, e a
//! resposta chega instantânea (sem esperar processo nenhum rodar).

#![cfg(windows)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

/// `RuntimeRegistry` vazio (sem bootstrap) — `exec.shell` não usa
/// runtimes portáteis, só `build_default_exec_tools` exige o tipo.
fn empty_registry() -> Arc<RuntimeRegistry> {
    let tmp: &'static TempDir = Box::leak(Box::new(TempDir::new().expect("tempdir")));
    let cfg = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: false,
        mirror_url: None,
        download_timeout: Duration::from_secs(1),
    };
    Arc::new(RuntimeRegistry::new(cfg).expect("RuntimeRegistry::new"))
}

fn build_exec_tools() -> Vec<Arc<dyn Tool>> {
    let runtimes = empty_registry();
    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    let network_allowlist = frederico_security::network::NetworkAllowlist::new();
    let network_audit: Arc<dyn frederico_security::network::NetworkAuditSink> =
        Arc::new(frederico_security::network::NoopNetworkAuditSink);
    build_default_exec_tools(resolver, runtimes, audit, network_allowlist, network_audit)
}

fn find_shell_tool(tools: &[Arc<dyn Tool>]) -> Arc<dyn Tool> {
    tools
        .iter()
        .find(|t| t.manifest().id == frederico_core::ToolId::new("exec.shell"))
        .expect("exec.shell no Vec retornado por build_default_exec_tools")
        .clone()
}

fn make_ctx(workspace: &std::path::Path) -> ToolContext {
    let jail = Jail::new(workspace).expect("Jail::new em workspace tempdir");
    ToolContext::new(ConversationId::new(), RunId::new(), MessageId::new(), jail)
}

/// **Teste de negação principal.** `rm -rf /` é recusado pela
/// denylist antes de qualquer processo ser criado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rm_rf_is_rejected_by_denylist() {
    let tools = build_exec_tools();
    let tool = find_shell_tool(&tools);
    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let start = Instant::now();
    let result = tool.execute(&ctx, &json!({ "command": "rm -rf /" })).await;
    let elapsed = start.elapsed();

    eprintln!(
        "[e2e_exec_shell/denylist] ok={} err={:?} elapsed={:?}",
        result.ok, result.error_message, elapsed
    );
    assert!(!result.ok, "esperava recusa, tool retornou ok");
    let err = result.error_message.unwrap_or_default();
    assert!(
        err.contains("recusado pela denylist"),
        "esperava erro de denylist, veio: {err:?}"
    );
    // Recusa acontece em `build_args`, antes do spawn — deve ser
    // essencialmente instantânea (sem esperar Job Object/processo).
    assert!(
        elapsed < Duration::from_secs(2),
        "recusa da denylist demorou {elapsed:?} — suspeita de spawn real antes da checagem"
    );
}

/// **Cobertura adicional:** `del /f /s /q` (variante Windows do
/// `rm -rf`) também é recusado — prova que a denylist não é
/// específica de um único padrão POSIX.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn del_f_s_q_is_rejected_by_denylist() {
    let tools = build_exec_tools();
    let tool = find_shell_tool(&tools);
    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(&ctx, &json!({ "command": "del /f /s /q C:\\Windows" }))
        .await;
    assert!(!result.ok, "esperava recusa, tool retornou ok");
    let err = result.error_message.unwrap_or_default();
    assert!(
        err.contains("recusado pela denylist"),
        "esperava erro de denylist, veio: {err:?}"
    );
}
