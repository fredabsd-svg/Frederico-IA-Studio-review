//! E2E — proxy de rede **wireado no `exec.python`** (Fase 7, Etapa 6).
//!
//! Fecha o pedaço que faltava no PR #51 antes do user pedir:
//! o proxy existe, é testado em isolamento
//! (`e2e_network_proxy.rs`), mas se ninguém o **usa** no caminho
//! de produção (sandbox), o `SECURITY.md` promete barreira que
//! o binário não aplica. Este teste prova o wiring completo:
//!
//! 1. `exec.python` real (com Python 3.12.4 embeddable) é
//!    invocado via o `FilesExecPythonTool::execute`.
//! 2. O tool sobe o `start_proxy` automaticamente, escreve
//!    `<workdir>/.frederico/proxy.port`, e injeta
//!    `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` no `extra_env`.
//! 3. O Python filho roda `urllib.request.urlopen("http://blocked...")`.
//! 4. O proxy retorna `502 Bad Gateway` (allowlist vazia = deny).
//! 5. O Python vê `urllib.error.HTTPError` e o stdout/stderr
//!    carrega a mensagem.
//! 6. O `exec.python` retorna `ToolResult::err` (exit code != 0).
//!
//! **Por que este teste é o pedaço que faltava:** sem ele, o
//! PR #51 fecha com "rede documentada mas desligada" — o
//! `SECURITY.md` cita proteção de rede que o binário não
//! aplica. Este teste é o que torna a frase do doc
//! observável: o filho do sandbox tenta acessar, o proxy
//! recusa, o Python vê o erro.
//!
//! **Setup:** mesmo padrão de
//! `e2e_exec_python_under_sandbox.rs` (bootstrap do
//! `python-3.12.4` via `frederico-runtimes`). Hard-fail se
//! o bootstrap não consegue entregar um Python rodando —
//! teste de segurança que pula por ausência do runtime que
//! ele deveria testar é fail-open com outra roupa (o test
//! passa sem ter provado nada).
//!
//! **`DbNetworkAuditSink` real (Etapa 6+1):** o teste
//! `exec_python_network_deny_is_persisted_via_db_network_audit_sink`
//! injeta um `DbNetworkAuditSink::new_unbound` de verdade (não
//! `Noop`) e confirma, via `NetworkAuditRepo::list_for_run`, que
//! a entrada persistida tem o `run_id` **certo** — não `NULL`.
//! Isso prova dois pontos que um teste com `Noop` não prova: (1)
//! o sink real está de fato injetado no caminho de produção, não
//! só compilando; (2) o `run_id` sobrevive ao round-trip
//! String→Uuid sem virar `NULL` silenciosamente — o formato
//! `Display` de `RunId` é `"RunId(<uuid>)"` (pensado pra log, não
//! pra round-trip), então passar a string errada nesse ponto faz
//! o `Uuid::parse_str` do lado do sink falhar sem panic, sem erro
//! visível — só um `run_id` faltando na tabela depois.
//!
//! **Limitações documentadas:**
//! - Não testamos bypass via raw socket (coberto pelo
//!   `e2e_network_raw_socket_bypasses_proxy_documented` em
//!   `e2e_network_proxy.rs`).
//! - O test do feature flag `FREDERICO_NETWORK_PROXY_V1=0` (D7)
//!   **não** está aqui porque `frederico-e2e` tem
//!   `unsafe_code = "forbid"` e `std::env::set_var` é `unsafe`
//!   desde Rust 1.86+. A função `is_network_proxy_v1_enabled`
//!   tem unit test próprio em `crates/security/src/env_filter.rs`
//!   (que aceita input explícito, sem mexer no env).

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeId, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_security::network::{NetworkAllowlist, NetworkAuditSink, NoopNetworkAuditSink};
use frederico_security::network_audit_sink::DbNetworkAuditSink;
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

// ============================================================================
// Setup helpers (mesmo padrão de e2e_exec_python_under_sandbox.rs)
// ============================================================================

/// Constrói um `RuntimeRegistry` com bootstrap do `python-3.12.4`
/// embeddable. Hard-fail se o bootstrap falha.
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
            eprintln!(
                "[e2e_network_proxy_wired/setup] {} bootstrap falhou: {:?}",
                id, err
            );
        }
        panic!(
            "runtime python-3.12.4 indisponível: bootstrap falhou. \
             Teste de segurança não pode pular."
        );
    }

    let py_id = RuntimeId::new("python-3.12.4");
    let runtime = registry
        .get(&py_id)
        .expect("python-3.12.4 no registry (hard-coded na Etapa 3)");
    let exe = runtime.executable();
    if !exe.exists() {
        panic!(
            "python-3.12.4 bootstrapped (report OK) mas exe não existe em {}. \
             Bug do bootstrap — abrir issue.",
            exe.display()
        );
    }

    Arc::new(registry)
}

/// Setup compartilhado: constrói as tools (com `NetworkAllowlist`
/// e `NetworkAuditSink` injetados), o `Jail` (apontando pro
/// workdir tempdir), e o `ToolContext`. Caller usa isso pra
/// chamar `tool.execute(ctx, &args)`.
///
/// **`network_audit` é parâmetro** (não hardcoded `Noop`): os
/// testes de wiring do proxy (deny/allow) não se importam com
/// persistência e passam `NoopNetworkAuditSink`; o teste de
/// persistência (`..._is_persisted_via_db_network_audit_sink`)
/// passa um `DbNetworkAuditSink` real apontado pra um DB
/// in-memory que ele mesmo consulta depois.
///
/// **`run_id` também é parâmetro** (não gerado internamente):
/// `network_audit` grava com FK pra `runs.id` (CASCADE). Os
/// testes que não se importam com persistência passam
/// `RunId::new()` solto (sem `runs` row — tudo bem, `Noop`
/// nunca toca o banco). O teste de persistência precisa que o
/// `run_id` do `ToolContext` seja o `id` de uma linha real em
/// `runs` (senão a `INSERT` em `network_audit` falha com
/// `FOREIGN KEY constraint failed` — achado desta investigação:
/// sem isso, `DbNetworkAuditSink::record` falha silenciosamente
/// pra QUALQUER run que não passou pelo `RunExecutor` de
/// verdade).
async fn setup(
    network_allowlist: NetworkAllowlist,
    network_audit: Arc<dyn NetworkAuditSink>,
    run_id: RunId,
) -> (Arc<dyn Tool>, ToolContext, TempDir) {
    let runtimes = build_registry().await;

    // Workdir = tempdir owned pelo user (Etapa 5+ integrity
    // label precisa disso).
    let workdir = TempDir::new().expect("workdir tempdir");

    // Jail resolveado pra esse workdir. O `Jail::new` valida
    // que o path existe e é writable; o workdir tempdir é
    // ambos.
    let jail = Jail::new(workdir.path()).expect("Jail::new em workspace tempdir");

    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");

    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    let tools =
        build_default_exec_tools(resolver, runtimes, audit, network_allowlist, network_audit);
    let python_tool = tools
        .iter()
        .find(|t| t.manifest().id == frederico_core::ToolId::new("exec.python"))
        .expect("exec.python no Vec")
        .clone();

    let ctx = ToolContext::new(ConversationId::new(), run_id, MessageId::new(), jail);

    (python_tool, ctx, workdir)
}

// ============================================================================
// Test: exec.python com proxy deny-by-default → urllib.HTTPError 502
// ============================================================================

/// **O teste que importa:** exec.python real, Python tenta
/// `urllib.request.urlopen("http://example.com/")` (host que
/// **resolve de verdade** — propositalmente não é um domínio
/// inexistente) via HTTP_PROXY, o proxy (deny-by-default)
/// retorna 502 `not_in_allowlist`, Python vê `HTTPError`, exit
/// code != 0, `ToolResult::err`.
///
/// **Por que `example.com` e não um domínio inexistente:** a
/// primeira versão deste teste usava `blocked.example.com`
/// (não resolve). O teste passava, mas pelo motivo errado — o
/// `HTTP_PROXY` nem chegava a ser injetado no filho
/// (`CreateProcessAsUserW` falhava com `ERROR_INVALID_PARAMETER`
/// por faltar `CREATE_UNICODE_ENVIRONMENT`, e um fallback
/// silencioso reexecutava com env herdado do parent, sem
/// proxy), e o Python bloqueava sozinho por `getaddrinfo
/// failed`, sem o proxy nunca entrar em ação. Com um host que
/// resolve, esse caminho de falso-positivo fica fechado: se o
/// wiring quebrar de novo, o Python consegue resolver e
/// conectar direto, e o teste falha (em vez de passar por
/// coincidência).
///
/// **`socket.socket()` cru antes do `urlopen`:** depois de
/// corrigir o `CREATE_UNICODE_ENVIRONMENT`, esse teste ainda
/// falhava — `WSAEPROVIDERFAILEDINIT` (WinError 10106) em
/// **qualquer** `socket.socket(...)`, nem chegava a tentar
/// conectar. Causa: `SystemRoot` não estava no
/// `EnvAllowlist::REQUIRED` — sem ele, `WSAStartup` não
/// consegue expandir `%SystemRoot%\system32\mswsock.dll` (o
/// path que o catálogo Winsock guarda pros providers base), e
/// a falha de carregar o provider é indistinguível, pro
/// caller, de "proxy bloqueando". Fixado em `env_filter.rs`
/// (`SystemRoot` + `windir` agora REQUIRED). Esse
/// `socket.socket()` isolado fica como regressão: se
/// `SystemRoot` sumir de novo, esse teste falha no ponto
/// exato, em vez de produzir um 502 ambíguo.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_python_blocked_by_network_proxy_deny_by_default() {
    // Allowlist vazia = deny-by-default (ADR-0033 D3).
    let allowlist = NetworkAllowlist::new();
    let (tool, ctx, _workdir) =
        setup(allowlist, Arc::new(NoopNetworkAuditSink), RunId::new()).await;

    // Python que tenta acessar `example.com` (resolve e
    // responde de verdade). Vai passar pelo HTTP_PROXY
    // injetado pelo tool; o proxy recusa (host fora da
    // allowlist), Python vê 502.
    let code = r#"
import os
import sys
import socket
import urllib.request

http_proxy = os.environ.get("HTTP_PROXY", "<unset>")
sys.stderr.write(f"HTTP_PROXY={http_proxy}\n")
sys.stderr.flush()

try:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sys.stderr.write("SOCKET_CREATE_OK\n")
    s.close()
except OSError as e:
    sys.stderr.write(f"SOCKET_CREATE_FAILED: {e}\n")
    sys.exit(44)
sys.stderr.flush()

try:
    urllib.request.urlopen("http://example.com/", timeout=5)
    sys.stderr.write("UNEXPECTED: request succeeded\n")
    sys.exit(0)
except urllib.error.HTTPError as e:
    sys.stderr.write(f"BLOCKED_HTTP: code={e.code} reason={e.reason}\n")
    sys.exit(42)
except Exception as e:
    sys.stderr.write(f"BLOCKED_OTHER: {type(e).__name__}: {e}\n")
    sys.exit(43)
"#;
    let args = json!({"code": code, "max_wall_clock_ms": 10000});

    let result = tool.execute(&ctx, &args).await;
    // O tool retorna `ToolResult::err { output: Null, error_message }`
    // quando exit != 0. O `error_message` é `"python exit
    // code N: "` seguido do stderr. O stdout do Python vai
    // pro stderr (via `sys.stderr.write`).
    let err_msg = result.error_message.as_deref().unwrap_or("");
    eprintln!("[test] tool.execute ok={} err_msg={err_msg:?}", result.ok);

    // **Prova do meio:** HTTP_PROXY precisa ter chegado no
    // filho com um valor real (não `<unset>`). Sem isso, tudo
    // que vem depois (o 502) não prova nada sobre o proxy — o
    // Python pode ter sido bloqueado por qualquer outro motivo.
    assert!(
        err_msg.contains("HTTP_PROXY=http://127.0.0.1:"),
        "HTTP_PROXY não chegou ao processo filho (wiring quebrado?). err_msg: {err_msg:?}"
    );

    // **`socket.socket()` cru funciona.** Se isso falhar
    // (`SOCKET_CREATE_FAILED`), o problema é infraestrutura de
    // rede do sandbox (ex.: `SystemRoot` ausente do env →
    // `WSAEPROVIDERFAILEDINIT`), não o proxy — falhar aqui com
    // uma mensagem específica evita reinvestigar o mesmo bug.
    assert!(
        err_msg.contains("SOCKET_CREATE_OK"),
        "socket.socket() falhou no filho antes mesmo de tentar o proxy \
         (infra de rede do sandbox quebrada, não o proxy). err_msg: {err_msg:?}"
    );

    // **Prova do fim:** a recusa tem que ser o 502 do proxy
    // (host `example.com` resolve e responde de verdade — se
    // o proxy não tivesse recusado, a request teria sucesso).
    assert!(
        err_msg.contains("BLOCKED_HTTP")
            && (err_msg.contains("code=502") || err_msg.contains("Bad Gateway")),
        "Esperava HTTPError 502 do proxy (não_in_allowlist). err_msg: {err_msg:?}"
    );
}

// ============================================================================
// Test: exec.python com allowlist contendo o host → urllib segue,
// mas falha com DNS (host não existe). Proxy autoriza, Python
// falha no DNS — exit != 0, mas a falha é do upstream.
// ============================================================================

/// Allowlist contém o host, então o proxy **autoriza** a
/// request. Mas o host não existe de verdade (DNS falha), e
/// o proxy retorna `502 upstream_unreachable`. O Python vê
/// 502 (não 403/deny). A diferença entre este teste e o
/// anterior é o `deny_reason` interno do proxy — aqui é
/// `upstream_unreachable`, não `allowlist_empty`.
///
/// O ponto é provar que o allowlist permite quando o host
/// está na lista (mesmo que o upstream falhe depois — o que
/// é fora do escopo do proxy).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_python_allowed_by_network_proxy_when_host_in_allowlist() {
    let allowlist = NetworkAllowlist::new().with_allowed(["allowed.example.com"]);
    let (tool, ctx, _workdir) =
        setup(allowlist, Arc::new(NoopNetworkAuditSink), RunId::new()).await;

    let code = r#"
import os
import sys
import urllib.request

http_proxy = os.environ.get("HTTP_PROXY", "<unset>")
sys.stderr.write(f"HTTP_PROXY={http_proxy}\n")
sys.stderr.flush()

try:
    urllib.request.urlopen("http://allowed.example.com/", timeout=3)
    sys.stderr.write("UNEXPECTED: request succeeded\n")
    sys.exit(0)
except urllib.error.HTTPError as e:
    sys.stderr.write(f"PROXY_UPSTREAM: code={e.code} reason={e.reason}\n")
    sys.exit(42)
except Exception as e:
    sys.stderr.write(f"OTHER_ERROR: {type(e).__name__}: {e}\n")
    sys.exit(43)
"#;
    let args = json!({"code": code, "max_wall_clock_ms": 10000});

    let result = tool.execute(&ctx, &args).await;
    let err_msg = result.error_message.as_deref().unwrap_or("");

    // **Prova do meio** (mesma razão do teste acima): sem isso,
    // um `getaddrinfo failed` direto do Python (proxy nunca
    // usado) fica indistinguível de um `upstream_unreachable`
    // do proxy — os dois batem na mesma string.
    assert!(
        err_msg.contains("HTTP_PROXY=http://127.0.0.1:"),
        "HTTP_PROXY não chegou ao processo filho (wiring quebrado?). err_msg: {err_msg:?}"
    );

    let saw_proxy_or_dns = err_msg.contains("PROXY_UPSTREAM")
        || err_msg.contains("OTHER_ERROR")
        || err_msg.contains("502")
        || err_msg.contains("getaddrinfo")
        || err_msg.contains("nodename");
    assert!(
        saw_proxy_or_dns,
        "Esperava marca de upstream-unreachable no stderr. err_msg: {err_msg:?}"
    );
}

// ============================================================================
// Test: proxy.port file é criado e removido pelo RAII guard
// ============================================================================

/// Sanity check do lifecycle do guard: o tool escreve
/// `<workdir>/.frederico/proxy.port` durante `execute()` e
/// remove no Drop (após `collect_output`). Verifica que o
/// arquivo **não** existe depois do `execute()` retornar.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_port_file_is_cleaned_up_after_exec() {
    let allowlist = NetworkAllowlist::new();
    let (tool, ctx, workdir) = setup(allowlist, Arc::new(NoopNetworkAuditSink), RunId::new()).await;

    let code = r#"print("hello from python")"#;
    let args = json!({"code": code, "max_wall_clock_ms": 5000});

    let proxy_dir = workdir.path().join(".frederico");
    assert!(
        !proxy_dir.exists(),
        ".frederico/ não deveria existir antes do execute (workdir: {:?})",
        workdir.path()
    );

    let result = tool.execute(&ctx, &args).await;
    assert!(result.ok, "hello world deveria passar: {:?}", result.output);

    // **Depois do execute, o guard dropou.** O `proxy.port`
    // deveria ter sido removido.
    let proxy_port = workdir.path().join(".frederico").join("proxy.port");
    assert!(
        !proxy_port.exists(),
        "proxy.port deveria ter sido removido pelo Drop do guard. \
         Path: {:?} (workdir ainda existe: {})",
        proxy_port,
        workdir.path().exists()
    );
}

// ============================================================================
// Test: DbNetworkAuditSink real persiste a entrada com o run_id certo
// ============================================================================

/// **O teste da Etapa 6+1:** injeta um `DbNetworkAuditSink` real
/// (não `Noop`) apontado pra um `Database` in-memory, roda
/// `exec.python` batendo em `example.com` (deny-by-default, mesmo
/// cenário do primeiro teste deste arquivo), e confirma via
/// `NetworkAuditRepo::list_for_run` que:
///
/// 1. A entrada foi persistida (o sink real está de fato
///    injetado — não só compila).
/// 2. `run_id` da entrada é `Some(ctx.run_id)` — não `None`. Esse
///    é o ponto que mais importa: `RunId` tem `Display` custom
///    (`"RunId(<uuid>)"`), e `start_network_proxy` (Etapa 6+1)
///    passa `run_id.0.to_string()` (o uuid puro) pro
///    `start_proxy`, não `run_id.to_string()` (o `Display`
///    wrapped). Se alguém trocar de volta pro `Display`, o
///    `Uuid::parse_str` dentro do `DbNetworkAuditSink` falha
///    silenciosamente (vira `None`, sem panic, sem warn) e esta
///    assertion pega.
/// 3. `host`/`decision`/`deny_reason` batem com o que o proxy
///    realmente decidiu.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_python_network_deny_is_persisted_via_db_network_audit_sink() {
    let db = frederico_storage::Database::open_in_memory()
        .await
        .expect("Database::open_in_memory");
    let network_audit: Arc<dyn NetworkAuditSink> =
        Arc::new(DbNetworkAuditSink::new_unbound(db.clone()));

    // `network_audit` (id ?1) INSERTs com FK CASCADE pra `runs.id`.
    // Sem uma linha real em `runs`, a INSERT falha com
    // `FOREIGN KEY constraint failed` — descoberto rodando este
    // teste pela primeira vez (achado da própria investigação: o
    // erro é engolido pelo `tracing::warn!` best-effort do
    // `DbNetworkAuditSink`, sem panic, sem teste pegando, até
    // este). Cria a cadeia mínima real (conversation → message →
    // run) e usa o `run.id` **gerado pelo repo** — `RunRepo::create`
    // não aceita um `RunId` do caller, então o `ctx.run_id` tem
    // que vir do `Run` retornado, não de `RunId::new()` solto.
    let conv = frederico_storage::ConversationRepo::new(&db)
        .create(
            &frederico_core::ProviderId::new("simulated"),
            &frederico_core::ModelId::new("simulated"),
            None,
        )
        .await
        .expect("ConversationRepo::create");
    let msg = frederico_storage::MessageRepo::new(&db)
        .create(&conv.id, "user", "teste", None)
        .await
        .expect("MessageRepo::create");
    let run = frederico_storage::RunRepo::new(&db)
        .create(&conv.id, &msg.id)
        .await
        .expect("RunRepo::create");

    // Allowlist vazia = deny-by-default (mesmo cenário do
    // primeiro teste deste arquivo).
    let allowlist = NetworkAllowlist::new();
    let (tool, ctx, _workdir) = setup(allowlist, network_audit, run.id).await;

    let code = r#"
import urllib.request
try:
    urllib.request.urlopen("http://example.com/", timeout=5)
except Exception:
    pass
"#;
    let args = json!({"code": code, "max_wall_clock_ms": 10000});

    // Não importa o `ToolResult` aqui — o que este teste prova é
    // o que foi parar no banco, não o comportamento do Python
    // (isso já é coberto por `exec_python_blocked_by_network_proxy_deny_by_default`).
    let _ = tool.execute(&ctx, &args).await;

    let repo = frederico_storage::NetworkAuditRepo::new(&db);
    let entries = repo.list_for_run(&ctx.run_id).await.expect("list_for_run");

    assert!(
        !entries.is_empty(),
        "esperava pelo menos 1 entrada em network_audit pro run_id {:?} — \
         o DbNetworkAuditSink não persistiu nada (sink não injetado, ou \
         run_id não bateu na query).",
        ctx.run_id
    );

    let entry = entries
        .iter()
        .find(|e| e.host == "example.com")
        .unwrap_or_else(|| panic!("nenhuma entrada com host=example.com em {entries:?}"));

    assert_eq!(
        entry.run_id,
        Some(ctx.run_id),
        "run_id da entrada persistida não bate com o run_id da invocação — \
         provável regressão do round-trip String->Uuid (RunId::Display \
         em vez do uuid puro). entry: {entry:?}"
    );
    assert_eq!(entry.decision, "deny", "entry: {entry:?}");
    assert_eq!(
        entry.deny_reason.as_deref(),
        Some("allowlist_empty"),
        "entry: {entry:?}"
    );
}
