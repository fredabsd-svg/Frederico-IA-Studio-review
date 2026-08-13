//! E2E — proxy de rede wireado no `exec.node` (Fase 7, Etapa 6+1).
//!
//! Espelha `e2e_network_proxy_wired_into_exec_python.rs`, mas
//! pro `exec.node`. **Não repete a investigação inteira** (env
//! block, `CREATE_UNICODE_ENVIRONMENT`, `SystemRoot`) — esses
//! bugs viviam em `crates/security/src/jail.rs::spawn_windows`,
//! camada **compartilhada** entre `exec.python` e `exec.node`
//! (`FilesExecToolBase::start_network_proxy` +
//! `SecurityJailResolver::spawn`, ambos comuns aos dois tools).
//! O que este arquivo prova é o pedaço que **não** é
//! compartilhado: o `FilesExecNodeTool::execute` em
//! `crates/tool-registry/src/exec/node.rs` de fato chama
//! `start_network_proxy` e injeta `extra_env` do jeito certo —
//! a suposição de "deve herdar o conserto por compartilhar
//! `jail.rs`" vira fato observado, não suposição.
//!
//! Mesmo padrão do arquivo do Python: hard-fail se o bootstrap
//! do runtime falhar (teste de segurança que pula é fail-open
//! com outra roupa).

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeId, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_security::network::{NetworkAllowlist, NoopNetworkAuditSink};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

/// Bootstrap do `node-20.16.0` embeddable. Hard-fail se falhar
/// (mesma regra do `build_registry` do arquivo do Python — um
/// teste de segurança que pula por ausência do runtime não prova
/// nada).
async fn build_registry() -> Arc<RuntimeRegistry> {
    let tmp: &'static TempDir = Box::leak(Box::new(TempDir::new().expect("tempdir")));
    let cfg = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: true,
        mirror_url: None,
        download_timeout: Duration::from_secs(300),
    };
    let registry = RuntimeRegistry::new(cfg).expect("RuntimeRegistry::new");
    let report = registry
        .bootstrap_all()
        .await
        .expect("RuntimeRegistry::bootstrap_all falhou");

    if !report.failed.is_empty() {
        for (id, err) in &report.failed {
            eprintln!("[e2e_network_proxy_wired_node/setup] {} bootstrap falhou: {:?}", id, err);
        }
        panic!(
            "runtime node-20.16.0 indisponível: bootstrap falhou. \
             Teste de segurança não pode pular."
        );
    }

    let node_id = RuntimeId::new("node-20.16.0");
    let runtime = registry
        .get(&node_id)
        .expect("node-20.16.0 no registry (hard-coded na Etapa 3)");
    let exe = runtime.executable();
    if !exe.exists() {
        panic!(
            "node-20.16.0 bootstrapped (report OK) mas exe não existe em {}. \
             Bug do bootstrap — abrir issue.",
            exe.display()
        );
    }

    Arc::new(registry)
}

async fn setup() -> (Arc<dyn Tool>, ToolContext, TempDir) {
    let runtimes = build_registry().await;
    let workdir = TempDir::new().expect("workdir tempdir");
    let jail = Jail::new(workdir.path()).expect("Jail::new em workspace tempdir");
    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    // Allowlist vazia = deny-by-default (ADR-0033 D3).
    let network_allowlist = NetworkAllowlist::new();
    let network_audit = Arc::new(NoopNetworkAuditSink);
    let tools = build_default_exec_tools(resolver, runtimes, audit, network_allowlist, network_audit);
    let node_tool = tools
        .iter()
        .find(|t| t.manifest().id == frederico_core::ToolId::new("exec.node"))
        .expect("exec.node no Vec")
        .clone();

    let ctx = ToolContext::new(ConversationId::new(), RunId::new(), MessageId::new(), jail);

    (node_tool, ctx, workdir)
}

/// **O teste que importa:** `exec.node` real, o filho Node fala
/// **diretamente com o proxy** (host/port = `HTTP_PROXY`, `path`
/// = URI absoluta `http://example.com/` — é assim que um cliente
/// HTTP fala com um proxy, RFC 7230 §5.3.2; é o que
/// `network.rs::parse_http` do lado do proxy espera). O proxy
/// (deny-by-default) recusa com 502.
///
/// **Por que não `http.get('http://example.com/', ...)` puro:**
/// o módulo `http` nativo do Node, ao contrário do `urllib` do
/// Python, **não** honra `HTTP_PROXY`/`HTTPS_PROXY`
/// automaticamente — isso é comportamento de browser/curl/git,
/// não do runtime. A primeira versão deste teste usava
/// `http.get` direto e o request ia **sem passar pelo proxy**,
/// batendo em `example.com` de verdade e voltando com um status
/// normal — o teste falhava, mas por um motivo que não tinha
/// nada a ver com o sandbox (bug do teste, não do wiring).
///
/// **Sempre sai com código != 0** (evidência sempre no stderr,
/// capturado em `error_message`) — mesmo padrão do teste do
/// Python: nenhum caminho (bloqueado, erro de request, timeout,
/// `HTTP_PROXY` ausente) deveria terminar em exit 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_node_blocked_by_network_proxy_deny_by_default() {
    let (tool, ctx, _workdir) = setup().await;

    let code = r#"
const http = require('http');
const httpProxy = process.env.HTTP_PROXY || '<unset>';
process.stderr.write(`HTTP_PROXY=${httpProxy}\n`);

if (httpProxy === '<unset>') {
    process.stderr.write('NO_PROXY_SET\n');
    process.exit(50);
}

const proxyUrl = new URL(httpProxy);
const options = {
    host: proxyUrl.hostname,
    port: proxyUrl.port,
    // Request-target em forma absoluta: é assim que um cliente
    // fala com um proxy HTTP (não com o path relativo que usaria
    // falando direto com o origin server).
    path: 'http://example.com/',
    headers: { Host: 'example.com' },
    timeout: 5000,
};

const req = http.get(options, (res) => {
    process.stderr.write(`RESPONSE: status=${res.statusCode}\n`);
    if (res.statusCode === 502) {
        process.stderr.write('BLOCKED_502\n');
    } else {
        process.stderr.write('UNEXPECTED_STATUS\n');
    }
    process.exit(42);
});
req.on('error', (e) => {
    process.stderr.write(`REQUEST_ERROR: ${e.message}\n`);
    process.exit(43);
});
req.on('timeout', () => {
    process.stderr.write('REQUEST_TIMEOUT\n');
    req.destroy();
    process.exit(44);
});
"#;
    let args = json!({"code": code, "max_wall_clock_ms": 10000});

    let result = tool.execute(&ctx, &args).await;
    let err_msg = result.error_message.as_deref().unwrap_or("");
    eprintln!("[test] tool.execute ok={} err_msg={err_msg:?}", result.ok);

    // **Prova do meio:** HTTP_PROXY chegou no filho Node com
    // valor real. Mesma razão do teste do Python — sem isso, o
    // resto não prova nada sobre o wiring.
    assert!(
        err_msg.contains("HTTP_PROXY=http://127.0.0.1:"),
        "HTTP_PROXY não chegou ao processo filho Node (wiring quebrado?). err_msg: {err_msg:?}"
    );

    // **Prova do fim:** a recusa veio do proxy (502), não de
    // qualquer outro erro de rede genérico do lado do Node.
    assert!(
        err_msg.contains("BLOCKED_502") || err_msg.contains("RESPONSE: status=502"),
        "Esperava resposta 502 do proxy. err_msg: {err_msg:?}"
    );
}
