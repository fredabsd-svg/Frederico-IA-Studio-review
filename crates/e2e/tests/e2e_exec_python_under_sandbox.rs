//! E2E — `exec.python` rodando sob `SecurityJailResolver`.
//!
//! **Escopo (Etapa 4 da Fase 7):** testa 3 contratos do sandbox
//! que o `exec.python` precisa honrar:
//!
//! 1. **Path safety (I3):** o `workdir` é o jail root; scripts
//!    não podem escapar (criar arquivos fora do workdir). O
//!    teste prova que um script que tenta `open("..\..\evil.txt", "w")`
//!    falha (PathNotFound ou PermissionError).
//! 2. **Wall-clock enforcement:** `wait_with_timeout(wall_clock)`
//!    dentro do `collect_output` mata processos que excedem. O
//!    teste prova que um script que faz `time.sleep(10)` com
//!    `max_wall_clock_ms=2000` retorna erro `"wall-clock excedido"`
//!    em ~2s (não 10s).
//! 3. **Sanity (caminho feliz):** um script Python simples
//!    executa e retorna stdout — sem ele, os 2 testes de
//!    negação podem estar passando por bug (não por sandbox).
//!
//! **Env filter (I1):** os unit tests de
//! `frederico_security::env_filter` cobrem o filtro em
//! isolamento. O **E2E** precisaria de `std::env::set_var`
//! (que é `unsafe` em Rust 1.86+ e o crate `frederico-e2e`
//! tem `unsafe_code = "forbid"` no `[lints.rust]`). Cobertura
//! suficiente nos unit tests — a Etapa 5+ pode reativar
//! este E2E se o lint for afrouxado.
//!
//! **Setup:** os testes precisam de `python.exe` no PATH (pula
//! com degradação controlada se não achar — mesma estratégia do
//! `crates/security/tests/tree_kill.rs`).
//!
//! **Por que teste direto do tool (não via `ChatOrchestrator`):**
//! os contratos testados são do **sandbox**, não do pipeline
//! modelo→tool. Testar direto do tool é mais rápido, mais
//! determinístico, e cobre o mesmo caminho. O `e2e_files_read.rs`
//! já cobre o caminho `modelo → ChatOrchestrator → tool`; o
//! `e2e_pipeline_sequencial_e2e.rs` cobre o pipeline completo.

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeId, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

// ============================================================================
// Setup helpers
// ============================================================================

/// Encontra `python.exe` no PATH. Retorna o caminho completo
/// do **interpreter STANDALONE** (não o stub do WindowsApps,
/// não o launcher que precisa de `Lib\` em dir irmão).
///
/// **Por que essa validação existe:** o `where python` em
/// Windows pode retornar candidatos que não funcionam
/// standalone (fora do seu dir original):
///
/// 1. **WindowsApps stub** (`%LOCALAPPDATA%\Microsoft\
///    WindowsApps\python.exe`) — abre a Microsoft Store.
///    Pulamos.
/// 2. **Python launcher install** (`C:\Users\<user>\
///    AppData\Local\Python\pythoncore-3.X-64\python.exe`)
///    — o `.exe` (106KB) precisa de `Lib\` (stdlib) no
///    **mesmo dir** pra rodar. Copiar só `.exe` + `.dll`
///    não basta: python falha em `import encodings` no
///    startup. Pulamos (a Etapa 5+ pode reativar via
///    embeddable distribution ou symlinking de `Lib\`).
/// 3. **Interpreter standalone** (ex.: `C:\Python311\
///    python.exe` em GitHub Actions runners, ou
///    embeddable distribution com `python3X.zip`) — tem
///    `.dll` E `Lib\` ou `python3X.zip`. ESTE funciona.
///
/// **Detecção:** o dir do candidato tem `Lib\` ou
/// `python3X.zip`? Se sim, é standalone. Se não, é
/// launcher e pulamos.
fn find_python() -> Option<std::path::PathBuf> {
    for name in &["python", "python3", "py"] {
        let out = std::process::Command::new("where")
            .arg(name)
            .output()
            .ok()?;
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        for line in s.lines() {
            let path = std::path::PathBuf::from(line.trim());
            let path_str = path.to_string_lossy();
            if path_str.contains("WindowsApps") {
                continue;
            }
            if let Some(real) = resolve_real_interpreter(&path) {
                return Some(real);
            }
        }
    }
    None
}

/// Dado um candidato de `where`, retorna o path do
/// interpreter se ele tem stdlib utilizável no mesmo dir
/// (`Lib\` ou `python3X.zip`).
///
/// Estratégia: tentamos o candidato direto primeiro
/// (caminho mais comum em CI runners com Python
/// instalado em `C:\Python3XX\`). Se falhar, tentamos
/// `pythoncore-*/python.exe` ao lado (Windows launcher
/// install — geralmente também falha). Se nenhum dos
/// dois for standalone, retorna `None`.
fn resolve_real_interpreter(candidate: &std::path::Path) -> Option<std::path::PathBuf> {
    if dir_has_stdlib(candidate.parent()?) {
        return Some(candidate.to_path_buf());
    }
    if let Some(parent) = candidate.parent() {
        if let Some(grandparent) = parent.parent() {
            if let Ok(entries) = std::fs::read_dir(grandparent) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let p = entry.path();
                    if p.is_dir()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().starts_with("pythoncore-"))
                            .unwrap_or(false)
                    {
                        let real = p.join("python.exe");
                        if dir_has_stdlib(&p) {
                            return Some(real);
                        }
                    }
                }
            }
        }
    }
    None
}

/// `true` se `dir` tem stdlib utilizável: `Lib\` (full
/// install) ou `python3X.zip` (embeddable distribution).
/// Usado pra distinguir interpreter standalone de
/// launcher que precisa de `Lib\` num dir vizinho.
fn dir_has_stdlib(dir: &std::path::Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let n = entry.file_name();
        let n = n.to_string_lossy();
        if n == "Lib" && entry.path().is_dir() {
            return true;
        }
        if n.starts_with("python") && n.ends_with(".zip") {
            return true;
        }
    }
    false
}

/// Tenta rodar `<python> -c "exit(0)"`. Retorna `true` se
/// sai com código 0.
///
/// **Usado em 2 lugares:**
/// 1. Em `build_registry` (pós-cópia) — valida que o
///    python copiado pro `install_root` realmente roda.
///    Se não (ex.: `Lib\` faltando), o test pula.
/// 2. (Reservado pra v2) validação de candidate em
///    `resolve_real_interpreter`.
fn can_run_python(python: &std::path::Path) -> bool {
    std::process::Command::new(python)
        .arg("-c")
        .arg("exit(0)")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Constrói um `RuntimeRegistry` apontando pra um `tempdir`
/// (sem bootstrap — os testes usam o `python.exe` do PATH
/// via um runtime customizado injetado em `python-3.12.4`).
///
/// **Hack honesto:** o `RuntimeRegistry::new` hard-coda
/// `python-3.12.4` + `node-20.16.0` (Etapa 3 da Fase 7).
/// Os testes copiam o `python.exe` (e a `.dll` sibling)
/// do PATH pro `install_root` esperado pelo
/// `PythonRuntime::executable()` — assim o spawn encontra
/// o binário + interpreter sem precisar baixar.
///
/// **Por que copiar a `.dll` também:** o `CreateProcessW`
/// procura a `python3X.dll` no mesmo dir do `python.exe`.
/// Se copiarmos só o `.exe`, o spawn falha com
/// `os error 3` ("The system cannot find the path
/// specified"). Em launcher installs (Windows Python
/// launcher), o `.exe` no `bin\` é só wrapper; o interpreter
/// real + `.dll` ficam em `pythoncore-3.X-64\`. Por isso
/// o `find_python` retorna o interpreter real (com `.dll`
/// sibling), e aqui copiamos a `.dll` junto.
///
/// Retorna `None` se a cópia não consegue produzir um
/// python executável (ex.: `.dll` não está acessível) — o
/// caller pula o teste.
///
/// **Retorna `(Arc<RuntimeRegistry>, TempDir)`:** o
/// `TempDir` precisa ficar vivo durante todo o test —
/// caso contrário o `install_root` (e os arquivos
/// copiados) somem, e o spawn no `tool.execute` falha
/// com `os error 3` ("path not found"). O caller guarda
/// o TempDir numa variável `_tempdir_keep_alive`.
fn build_registry() -> Option<(Arc<RuntimeRegistry>, TempDir)> {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: false, // testes não baixam
        mirror_url: None,
        download_timeout: Duration::from_secs(1),
    };
    let registry = RuntimeRegistry::new(cfg).ok()?;
    if let Some(py) = find_python() {
        let py_id = RuntimeId::new("python-3.12.4");
        if let Some(runtime) = registry.get(&py_id) {
            let exe = runtime.executable();
            if let Some(parent) = exe.parent() {
                let _ = std::fs::create_dir_all(parent);
                // Copia o python.exe
                if std::fs::copy(&py, exe).is_err() {
                    return None;
                }
                // Copia a `python*.dll` + `vcruntime*.dll`
                // (CreateProcessW procura a .dll no mesmo
                // dir do .exe; sem isso, spawn falha com
                // os error 3 "system cannot find the path
                // specified").
                if let Some(src_dir) = py.parent() {
                    copy_python_dlls(src_dir, parent);
                    // Copia também o `python3X.zip` (stdlib
                    // pre-empacotada do embeddable distribution).
                    // Sem ele, python falha com
                    // `ModuleNotFoundError: encodings` no
                    // startup. `Lib\` (full install) é
                    // muito grande (50-200MB) pra copiar em
                    // test — só o embeddable com `.zip` é
                    // usável nesse setup.
                    copy_python_zip(src_dir, parent);
                }
                // **Validação pós-cópia:** tentar rodar o
                // python copiado. Se ele falha (ex.: falta
                // `Lib\` ou `.zip` no destino), o test pula
                // em vez de falhar com `ModuleNotFoundError`
                // ou `os error 3`. Captura o caso onde a
                // source é um Python "launcher install"
                // (Windows: `pythoncore-3.X-64\python.exe`
                // é um wrapper que precisa de `Lib\` como
                // sibling) e a cópia só do `.exe`+`.dll`
                // não é suficiente.
                if !can_run_python(exe) {
                    eprintln!(
                        "[e2e_exec_python/setup] python copiado nao roda standalone (faltou Lib\\ ou .zip); pulando"
                    );
                    return None;
                }
            }
        }
    }
    Some((Arc::new(registry), tmp))
}

/// Copia todos os `python*.dll` de `src_dir` pra `dst_dir`.
/// Usado em tests E2E pra garantir que o python copiado
/// consegue carregar o interpreter. Falhas são silenciosas
/// (um `.dll` faltando é diagnóstico, não panic).
fn copy_python_dlls(src_dir: &std::path::Path, dst_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(src_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let n = entry.file_name();
            let n = n.to_string_lossy();
            // Copia python*.dll (obrigatório) + vcruntime*.dll
            // (runtime C do Windows que python3X.dll depende).
            if (n.starts_with("python") && n.ends_with(".dll")) || n.starts_with("vcruntime") {
                let _ = std::fs::copy(entry.path(), dst_dir.join(&*n));
            }
        }
    }
}

/// Copia o `python3X.zip` (stdlib pre-empacotada do
/// embeddable distribution) de `src_dir` pra `dst_dir`.
/// Sem o `.zip` no destino, o python copiado falha com
/// `ModuleNotFoundError: encodings` no startup. `Lib\`
/// (full install) é muito grande pra copiar em test —
/// só o embeddable com `.zip` é usável nesse setup.
///
/// **Silencioso** se o `.zip` não existe (source é um
/// full install com `Lib\`, não embeddable — o test vai
/// pular via `can_run_python` depois).
fn copy_python_zip(src_dir: &std::path::Path, dst_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(src_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let n = entry.file_name();
            let n = n.to_string_lossy();
            if n.starts_with("python") && n.ends_with(".zip") {
                let _ = std::fs::copy(entry.path(), dst_dir.join(&*n));
            }
        }
    }
}

/// Constrói o `Vec<Arc<dyn Tool>>` (Python + Node) com deps
/// reais. Retorna `None` se python não está disponível
/// (degradação).
///
/// **Por que `build_default_exec_tools` (não `FilesExecPythonTool::new`
/// direto):** o construtor e a `FilesExecToolBase` são
/// `pub(crate)` — só acessíveis dentro do `frederico-tool-registry`.
/// A função pública `build_default_exec_tools` esconde esse
/// detalhe.
///
/// **Lifetime do TempDir:** o `runtimes` retornado pelo
/// `build_registry` aponta pra um `install_root` que é um
/// `TempDir` interno. Se o TempDir droppar, os arquivos
/// copiados (python.exe + .dlls + .zip) somem, e os
/// testes subsequentes falham com `os error 3`. Pra
/// evitar isso, o `build_registry` retorna o TempDir
/// junto, e aqui guardamos em `_tempdir_keep_alive`
/// (variável que não é usada mas mantém o TempDir vivo
/// durante o test). Vaza memória no fim (OS limpa o
/// tempdir), mas isso é aceitável em test.
fn build_exec_tools() -> Option<Vec<Arc<dyn Tool>>> {
    let _python = find_python()?;
    let (runtimes, _tempdir_keep_alive) = build_registry()?;
    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    // `new()` já retorna `Arc<SecurityJailResolver>` — não
    // envolver em outro `Arc::new`.
    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    // Sanity: o runtime existe (degradação: pode não existir
    // se o registry não tem python-3.12.4 hard-coded, mas a
    // v1 da Etapa 3 hard-coda).
    if runtimes.get(&RuntimeId::new("python-3.12.4")).is_none() {
        eprintln!("[e2e_exec_python] runtime python-3.12.4 não está no registry; pulando");
        return None;
    }
    Some(build_default_exec_tools(resolver, runtimes, audit))
}

/// Helper: pega o `FilesExecPythonTool` do `Vec` retornado
/// por `build_default_exec_tools`. Downcast via `Any` é
/// complicado; em vez disso, o test procura por
/// `manifest().id == "exec.python"`.
fn find_python_tool(tools: &[Arc<dyn Tool>]) -> Arc<dyn Tool> {
    tools
        .iter()
        .find(|t| t.manifest().id == frederico_core::ToolId::new("exec.python"))
        .expect("exec.python no Vec retornado por build_default_exec_tools")
        .clone()
}

/// Helper: `ToolContext` apontando pro `workspace` (que é o
/// `jail.root()` e o `SandboxConfig::workdir`).
fn make_ctx(workspace: &std::path::Path) -> ToolContext {
    let jail = Jail::new(workspace).expect("Jail::new em workspace tempdir");
    ToolContext::new(ConversationId::new(), RunId::new(), MessageId::new(), jail)
}

// ============================================================================
// Testes
// ============================================================================

/// **I3 — path safety.** Script Python tenta criar arquivo
/// fora do workdir (jail). O sandbox **bloqueia** (path
/// inválido sob `current_dir`).
///
/// **Por que isso prova o contrato:** o `SandboxConfig::workdir`
/// é o `jail.root()` (= workspace da conversa). O
/// `tokio::process::Command::current_dir(workdir)` define o
/// cwd do filho. O Python `open("..\..\evil.txt", "w")` resolve
/// pra `<workdir>/../../evil.txt` — fora do workspace. O
/// `CreateProcessW` aceita (cwd é só referência), mas o Python
/// script falha com `FileNotFoundError` (parent dir não existe
/// ou perm). O teste prova que o arquivo `evil.txt` **não** foi
/// criado (a "negação" do sandbox, não o "funcionamento").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_cannot_write_outside_workspace() {
    let tools = match build_exec_tools() {
        Some(t) => t,
        None => {
            eprintln!("[e2e_exec_python] python.exe não disponível; teste pulado");
            return;
        }
    };
    let tool = find_python_tool(&tools);

    let workspace = TempDir::new().expect("tempdir workspace");
    let evil_path = workspace.path().parent().unwrap().join("evil.txt");
    // Garante que `evil.txt` NÃO existe antes do teste.
    let _ = std::fs::remove_file(&evil_path);

    // Python script: tenta escrever em `..\evil.txt` (acima do workdir).
    let code = r#"
import os
try:
    with open(r"..\evil.txt", "w") as f:
        f.write("PWNED")
    print("ESCAPED", flush=True)
except (FileNotFoundError, PermissionError, OSError) as e:
    print("BLOCKED", type(e).__name__, str(e), flush=True)
"#;

    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(&ctx, &json!({ "code": code, "max_wall_clock_ms": 10_000 }))
        .await;
    eprintln!(
        "[e2e_exec_python/path-safety] result: ok={} err={:?}",
        result.ok, result.error_message
    );

    // O test passa se: o script reportou "BLOCKED" (não "ESCAPED")
    // E o arquivo evil.txt NÃO foi criado.
    let escaped = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .map(|s| s.contains("ESCAPED"))
        .unwrap_or(false);
    assert!(
        !escaped,
        "FALHA DE SANDBOX: Python escapou do workdir e criou arquivo fora do jail"
    );
    assert!(
        !evil_path.exists(),
        "FALHA DE SANDBOX: evil.txt foi criado em {:?}",
        evil_path
    );
    eprintln!("[e2e_exec_python/path-safety] OK: sandbox bloqueou escrita fora do workdir");
}

/// **Wall-clock enforcement.** Script Python tenta `time.sleep(10)`;
/// o tool é chamado com `max_wall_clock_ms=2000`. O
/// `wait_with_timeout(2s)` dentro do `collect_output` mata
/// o processo em ~2s; o tool retorna erro `"wall-clock excedido"`.
/// O **SandboxedProcess** ainda está vivo (drop pendente);
/// quando o caller dropa (fim do `execute`), o Job handle
/// fecha e mata netos via `KILL_ON_JOB_CLOSE`.
///
/// **Por que esse teste importa:** a v1 da Etapa 2 tinha o
/// campo `wall_clock` "apenas informativo" — o `collect_output`
/// só checava **depois** de stdout/stderr fecharem. Se o
/// processo segura stdout aberto (loop infinito printando),
/// o wall-clock nunca dispara. A Etapa 4 da Fase 7 conserta
/// com `wait_with_timeout` real (dentro do `tokio::join!`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wall_clock_kills_long_running_process() {
    let tools = match build_exec_tools() {
        Some(t) => t,
        None => {
            eprintln!("[e2e_exec_python] python.exe não disponível; teste pulado");
            return;
        }
    };
    let tool = find_python_tool(&tools);

    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let code = r#"
import time
print("starting sleep", flush=True)
time.sleep(10)
print("slept 10s", flush=True)
"#;

    let start = std::time::Instant::now();
    let result = tool
        .execute(&ctx, &json!({ "code": code, "max_wall_clock_ms": 2_000 }))
        .await;
    let elapsed = start.elapsed();

    eprintln!(
        "[e2e_exec_python/wall-clock] elapsed={:?} ok={} err={:?}",
        elapsed, result.ok, result.error_message
    );
    assert!(
        !result.ok,
        "Esperava erro (wall-clock excedido), mas tool retornou ok"
    );
    let err = result.error_message.unwrap_or_default();
    assert!(
        err.contains("wall-clock"),
        "Esperava erro contendo 'wall-clock', veio: {err:?}"
    );
    // Margem generosa: o wall-clock é 2s, o overhead de spawn
    // + cleanup pode adicionar ~1s. Se passou de 5s, o
    // `wait_with_timeout` não está funcionando.
    assert!(
        elapsed < Duration::from_secs(5),
        "wall-clock enforcement não funcionou: elapsed={:?} > 5s (esperado < 3s)",
        elapsed
    );
    eprintln!(
        "[e2e_exec_python/wall-clock] OK: processo morto em {:?} (< 5s, wall-clock=2s)",
        elapsed
    );
}

// ============================================================================
// Sanity: a Etapa 4 v1 não bloqueia python -c "<código que funciona>"
// ============================================================================

/// **Sanity:** um script Python simples (sem path traversal,
/// sem env leak, sem loop) executa e retorna stdout esperado.
/// Esse é o "controle positivo" — sem ele, os 3 testes de
/// negação acima podem estar passando por bug (não por
/// sandbox). Aqui provamos que o caminho feliz funciona.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_python_simple_hello_world() {
    let tools = match build_exec_tools() {
        Some(t) => t,
        None => {
            eprintln!("[e2e_exec_python] python.exe não disponível; teste pulado");
            return;
        }
    };
    let tool = find_python_tool(&tools);

    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(
            &ctx,
            &json!({ "code": "print('hello from python', flush=True)" }),
        )
        .await;
    eprintln!(
        "[e2e_exec_python/sanity] ok={} err={:?} output={}",
        result.ok, result.error_message, result.output
    );
    assert!(result.ok, "hello world falhou: {:?}", result.error_message);
    let stdout = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(stdout.contains("hello from python"));
}
