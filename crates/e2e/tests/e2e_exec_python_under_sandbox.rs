//! E2E — `exec.python` rodando sob `SecurityJailResolver`.
//!
//! **Escopo (Etapa 4 da Fase 7):** testa os contratos do sandbox
//! que o `exec.python` precisa honrar:
//!
//! 1. **Path safety (I3):** o `workdir` é o jail root; scripts
//!    não podem escapar (criar arquivos fora do workdir). O
//!    teste prova que um script que tenta `open("..\..\evil.txt", "w")`
//!    falha (PathNotFound ou PermissionError).
//! 2. **Wall-clock enforcement:** `wait_with_timeout(wall_clock)`
//!    dentro do `collect_output` recusa invocações que excedem o
//!    orçamento. O teste prova que um script que faz
//!    `time.sleep(60)` com `max_wall_clock_ms=2000` volta como
//!    erro `"wall-clock excedido"`.
//! 3. **O veredito do wall clock não depende de escalonamento**
//!    ([ADR-0046](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/decisions/0046-wall-clock-do-sandbox-decidido-pelo-kernel.md)):
//!    o teste **constrói** a starvation (runtime de 1 worker e 1
//!    thread de bloqueio, as duas ocupadas) e prova que um filho
//!    que estourou o orçamento continua sendo recusado mesmo
//!    quando só conseguimos olhar depois de ele já ter morrido
//!    sozinho. É a regressão do fail-open medido em 2026-08-17.
//! 4. **Sanity (caminho feliz):** um script Python simples
//!    executa e retorna stdout — sem ele, os testes de
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
//! **Setup:** os testes precisam de `python-3.12.4` (embeddable)
//! bootstrapped via `frederico-runtimes` (rede necessária; **falha**
//! com mensagem clara se indisponível — degradação declarada).
//! Mesma decisão do `memory_real_providers_or_fail` da Fase de
//! Ligação: teste de segurança que **pula** por ausência do runtime
//! que ele deveria testar é **fail-open com outra roupa** (o test
//! passa sem ter provado nada). Por isso o setup hard-fails se o
//! bootstrap não consegue entregar um python-3.12.4 rodando.
//!
//! **Por que bootstrap (não `find_python` no PATH):** o test deve
//! usar o **mesmo** resolvedor de runtime que a produção usa
//! (`frederico-runtimes`). O caminho anterior (`find_python()` +
//! cópia de `.exe`/`.dll` do PATH) testava o setup do test, não o
//! que o app realmente executa — sem isso, o test podia passar
//! com python do sistema (full install) e falhar no CI com
//! embeddable, ou vice-versa.
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
use frederico_security::jail::{SandboxConfig, SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

// ============================================================================
// Setup helpers
// ============================================================================

/// Constrói um `RuntimeRegistry` com bootstrap do `python-3.12.4`
/// (embeddable distribution baixada do python.org pelo
/// `frederico-runtimes`). **Hard-fail** se o bootstrap não
/// consegue entregar um python rodando — mesma decisão do
/// `memory_real_providers_or_fail` da Fase de Ligação:
/// teste de segurança que **pula** por ausência do runtime
/// que ele deveria testar é fail-open com outra roupa
/// (o test passa sem ter provado nada).
///
/// **Por que bootstrap (não `find_python` no PATH):** o test
/// precisa usar o **mesmo** resolvedor de runtime que a
/// produção usa (`frederico-runtimes`). O caminho anterior
/// (`find_python()` + cópia de `.exe`/`.dll` do PATH) testava
/// o setup do test, não o que o app realmente executa — sem
/// isso, o test podia passar com python do sistema (full
/// install) e falhar no CI com embeddable, ou vice-versa.
///
/// **Rede necessária:** o bootstrap baixa ~11 MB do
/// python.org. Em air-gapped, falha com
/// `BootstrapError::OfflineRequired` — o test hard-fail com
/// a mesma mensagem, **não** pula.
///
/// **Lifetime do `install_root`:** o `TempDir` é
/// `Box::leak`-ado (`'static`) pra que o `install_root`
/// (e os arquivos extraídos pro `.../python-3.12.4/3.12.4/`
/// subdir) sobrevivam ao fim de `build_registry` e ao test
/// inteiro. Vaza um TempDir por test (OS limpa `%TEMP%`
/// periodicamente) — aceitável em test.
async fn build_registry() -> Arc<RuntimeRegistry> {
    let tmp: &'static TempDir = Box::leak(Box::new(TempDir::new().expect("tempdir")));
    let cfg = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: true, // bootstrap baixa o embeddable do python.org
        mirror_url: None,
        download_timeout: Duration::from_secs(300),
    };
    let registry = RuntimeRegistry::new(cfg).expect("RuntimeRegistry::new");

    // **Bootstrap só do python (2026-08-18).** Antes daqui isto
    // chamava `bootstrap_all()`, que baixa **também** o
    // `node-20.16.0` (~30 MB de nodejs.org) — runtime que este
    // arquivo não usa — e o hard-fail olhava o `report.failed`
    // inteiro. Resultado: uma falha de download do node reprovava
    // um teste de Python. Com 4 provas rodando em paralelo, cada
    // uma baixando os dois runtimes por conta própria, o arquivo
    // ficou instável por rede: a vítima variava de rodada para
    // rodada. Medido em 2026-08-18:
    // `DownloadFailed { id: RuntimeId("node-20.16.0"), attempts: 3,
    // message: "read body: request or response body error" }`.
    // Baixar só o que o teste usa tira o node do caminho e corta
    // o volume por prova de ~41 MB para ~11 MB.
    let py_id = RuntimeId::new("python-3.12.4");
    let runtime = registry
        .get(&py_id)
        .expect("python-3.12.4 no registry (hard-coded na Etapa 3)");

    // Hard-fail se python não bootstrappou. Teste de segurança
    // que pula por ausência do runtime é fail-open.
    if let Err(e) = runtime.bootstrap_if_needed() {
        panic!(
            "runtime python-3.12.4 indisponível: {e:?}.              Teste de segurança não pode pular — pular por              ausência do runtime é fail-open (mesma decisão do              memory_real_providers_or_fail da Fase de Ligação).              Verifique a rede ou pre-popule o cache em {}",
            tmp.path().display()
        );
    }

    // Validação extra: o python.exe existe onde o runtime diz.
    let exe = runtime.executable();
    if !exe.exists() {
        panic!(
            "python-3.12.4 bootstrapped (sem erro) mas exe não existe em {}.              Bug do bootstrap — abrir issue.",
            exe.display()
        );
    }

    Arc::new(registry)
}

/// Caminho do `python.exe` bootstrappado, para os testes que
/// falam com o `SecurityJailResolver` direto (sem passar pelo
/// `exec.python`). Mesma garantia de `build_registry`: hard-fail
/// se o runtime não está disponível, nunca skip.
async fn python_executable() -> std::path::PathBuf {
    build_registry()
        .await
        .get(&RuntimeId::new("python-3.12.4"))
        .expect("python-3.12.4 no registry")
        .executable()
        .to_path_buf()
}

/// Constrói o `Vec<Arc<dyn Tool>>` (Python + Node) com deps
/// reais. **Hard-fail** se o setup falha (mesma regra:
/// `build_registry`).
///
/// **Por que `build_default_exec_tools` (não `FilesExecPythonTool::new`
/// direto):** o construtor e a `FilesExecToolBase` são
/// `pub(crate)` — só acessíveis dentro do `frederico-tool-registry`.
/// A função pública `build_default_exec_tools` esconde esse
/// detalhe.
async fn build_exec_tools() -> Vec<Arc<dyn Tool>> {
    let runtimes = build_registry().await;
    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    // `new()` já retorna `Arc<SecurityJailResolver>` — não
    // envolver em outro `Arc::new`.
    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    // `network_allowlist` vazia = deny-by-default (a Etapa 6
    // da Fase 7 fecha o proxy de rede, ADR-0033). O teste
    // existente não exercita rede, então o proxy estar
    // ativo ou desativado é irrelevante — só precisa compilar.
    let network_allowlist = frederico_security::network::NetworkAllowlist::new();
    let network_audit: Arc<dyn frederico_security::network::NetworkAuditSink> =
        Arc::new(frederico_security::network::NoopNetworkAuditSink);
    build_default_exec_tools(resolver, runtimes, audit, network_allowlist, network_audit)
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

/// **I3 — path safety.** O sandbox precisa bloquear a **fuga**
/// do workdir, e só ela. O teste faz as duas metades numa
/// invocação só:
///
/// 1. **Controle positivo** — o script escreve `dentro.txt` no
///    próprio workdir e isso **tem** de funcionar.
/// 2. **Negação** — o mesmo script tenta `..\evil.txt` e isso
///    **tem** de falhar, sem deixar arquivo no parent.
///
/// ## Por que o controle positivo não é opcional (2026-08-18)
///
/// Sem ele o teste não distingue "não escapou" de "não escreve
/// em lugar nenhum" — e foi exatamente essa a situação real até
/// [ADR-0051](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/decisions/0051-o-rotulo-de-integridade-do-workdir-nunca-foi-aplicado.md).
/// O `set_low_integrity_label` montava um descritor de segurança
/// self-relative com `OffsetSacl = 0`, o que o Windows lê como
/// "SACL presente porém NULL": o `SetFileSecurityW` devolvia
/// sucesso e não aplicava rótulo nenhum. O workdir ficava em
/// integridade Medium, o filho rodava em Low, e o `NO_WRITE_UP`
/// barrava **toda** escrita — inclusive a que devia passar.
/// Este teste passava assim mesmo, verde, provando bem menos do
/// que o nome dele diz.
///
/// O par também cobre o erro simétrico: um sandbox que largasse
/// o rótulo de lado passaria no controle positivo e falharia na
/// negação.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_cannot_write_outside_workspace() {
    let tools = build_exec_tools().await;
    let tool = find_python_tool(&tools);

    let workspace = TempDir::new().expect("tempdir workspace");
    let evil_path = workspace.path().parent().unwrap().join("evil.txt");
    let inocente_path = workspace.path().join("dentro.txt");
    // Garante que `evil.txt` NÃO existe antes do teste.
    let _ = std::fs::remove_file(&evil_path);

    let code = r#"
# Controle positivo: escrever DENTRO do workdir tem de funcionar.
try:
    with open("dentro.txt", "w") as f:
        f.write("ok")
    print("DENTRO_OK", flush=True)
except OSError as e:
    print("DENTRO_FALHOU", type(e).__name__, str(e), flush=True)

# Negacao: escrever FORA do workdir tem de falhar.
try:
    with open(r"..\evil.txt", "w") as f:
        f.write("PWNED")
    print("ESCAPED", flush=True)
except OSError as e:
    print("BLOCKED", type(e).__name__, str(e), flush=True)
"#;

    let ctx = make_ctx(workspace.path());

    let result = tool
        .execute(&ctx, &json!({ "code": code, "max_wall_clock_ms": 10_000 }))
        .await;
    let stdout = result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    eprintln!(
        "[e2e_exec_python/path-safety] ok={} err={:?} stdout={stdout:?}",
        result.ok, result.error_message
    );

    // --- Controle positivo: o workdir é utilizável ---
    assert!(
        stdout.contains("DENTRO_OK"),
        "o filho não conseguiu escrever no próprio workdir; o teste de \
         negação abaixo passaria por não escrever em lugar nenhum, não \
         por o sandbox ter bloqueado a fuga (ver ADR-0051). stdout={stdout:?}"
    );
    assert!(
        inocente_path.exists(),
        "o script disse DENTRO_OK mas {inocente_path:?} não existe"
    );

    // --- Negação: a fuga é bloqueada ---
    assert!(
        !stdout.contains("ESCAPED"),
        "FALHA DE SANDBOX: Python escapou do workdir e criou arquivo fora do jail"
    );
    assert!(
        !evil_path.exists(),
        "FALHA DE SANDBOX: evil.txt foi criado em {evil_path:?}"
    );
    eprintln!("[e2e_exec_python/path-safety] OK: escreveu dentro do workdir e foi bloqueado fora");
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
///
/// ## O que cada asserção vale (revisto em 2026-08-17)
///
/// A v1 tinha três asserções e tratava as três como se fossem o
/// mesmo contrato. Não são, e foi essa confusão que produziu o
/// flake:
///
/// - **`!ok` + erro com "wall-clock"** — este é o contrato. Diz
///   que a ferramenta **recusa** um run que estourou o orçamento.
///   Depois da correção do `RawChild::wait_with_timeout`
///   (ADR-0046), o veredito sai do `ExitTime` que o kernel
///   carimbou comparado a um deadline absoluto, então esta parte
///   é determinística: não depende de carga, de quantas worker
///   threads o runtime tem, nem de quando conseguimos olhar.
/// - **`elapsed < 5s`** — esta **não** era o contrato, e era a
///   metade instável. O intervalo cronometrado não é o wall
///   clock: inclui `start_network_proxy` +
///   `SecurityJailResolver::spawn` (`CreateProcessAsUserW`,
///   rotulagem de integridade do workdir, token restrito) +
///   teardown. Medido em 2026-08-17 com carga artificial de CPU,
///   só o `spawn` custou de 2,0 s a 7,0 s — a asserção passou a
///   medir a folga da máquina.
///
/// A v2 mantém o contrato intacto e troca a asserção de tempo por
/// uma **guarda de latência** honesta sobre o que ela pode
/// prometer. Matar o filho exige que uma thread nossa rode; sob
/// starvation isso atrasa, e nenhum desenho evita isso (o Windows
/// não oferece limite de wall clock em Job Object — só de tempo
/// de CPU, que um processo dormindo nunca consome). Então o
/// limite é posto uma ordem de grandeza acima do orçamento, e o
/// trabalho do filho sobe de 10 s para 60 s para que a guarda
/// continue discriminando com folga maior que a da v1: a v1
/// separava 5 s de 10 s (fator 2), a v2 separa 20 s de 60 s
/// (fator 3).
///
/// A prova fina de que o mecanismo não pode ser enganado por
/// escalonamento está em
/// `wall_clock_verdict_survives_starved_runtime`, que constrói a
/// starvation em vez de torcer por ela.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wall_clock_kills_long_running_process() {
    let tools = build_exec_tools().await;
    let tool = find_python_tool(&tools);

    let workspace = TempDir::new().expect("tempdir workspace");
    let ctx = make_ctx(workspace.path());

    // 60 s de trabalho para um orçamento de 2 s. Sob a v1 do bug
    // ("wall_clock apenas informativo") o filho completaria os
    // 60 s inteiros; nenhum atraso de escalonamento plausível
    // confunde 60 s com 20 s.
    let code = r#"
import time
print("starting sleep", flush=True)
time.sleep(60)
print("slept 60s", flush=True)
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

    // --- Contrato: determinístico, independente de carga ---
    assert!(
        !result.ok,
        "Esperava erro (wall-clock excedido), mas tool retornou ok. \
         elapsed={elapsed:?}"
    );
    let err = result.error_message.unwrap_or_default();
    assert!(
        err.contains("wall-clock"),
        "Esperava erro contendo 'wall-clock', veio: {err:?}"
    );

    // --- Guarda de latência: não é o contrato, ver doc acima ---
    assert!(
        elapsed < Duration::from_secs(20),
        "o filho não foi morto: elapsed={elapsed:?} para um orçamento de 2s \
         (o script dorme 60s; a v1 do bug deixava ele terminar)"
    );
    eprintln!("[e2e_exec_python/wall-clock] OK: run de 60s recusado em {elapsed:?} (orçamento 2s)");
}

/// **Regressão do fail-open medido em 2026-08-17.** Prova que o
/// veredito do wall clock não depende de quando as *nossas*
/// threads conseguem rodar.
///
/// ## O defeito que este teste tranca
///
/// A v1 do `RawChild::wait_with_timeout` decidia pelo que
/// observasse primeiro: `WaitForSingleObject(h, wall_clock)`
/// dentro de um `spawn_blocking`, envolto por um
/// `tokio::time::timeout`. Se o runtime ficasse sem CPU tempo
/// bastante, o `spawn_blocking` só rodava depois de o filho já
/// ter terminado sozinho — e aí o `WaitForSingleObject` devolvia
/// `WAIT_OBJECT_0` com exit code 0. A ferramenta reportava
/// **sucesso** para um run que estourou o orçamento em 5×. Foi o
/// que o `wall_clock_kills_long_running_process` capturou sob
/// carga: `elapsed=13.4s ok=true` com `max_wall_clock_ms=2000`.
///
/// ## Como a starvation vira determinística aqui
///
/// Em vez de gerar carga (que não reproduz de forma confiável), o
/// teste **constrói** a starvation com um runtime de 1 worker e 1
/// thread de bloqueio, e ocupa as duas:
///
/// - `spawn_blocking` de 8 s toma a única thread de bloqueio, de
///   modo que o wait do `wait_with_timeout` só é despachado
///   **depois** de o filho (4 s) já ter morrido sozinho;
/// - `std::thread::sleep(9s)` dentro do `join!` toma a única
///   worker thread, de modo que o timer de 2 s existe mas
///   ninguém pode observá-lo.
///
/// Resultado: quando finalmente olhamos, o processo está
/// encerrado com exit code 0. A v1 chamava isso de sucesso; a v2
/// pergunta ao kernel **quando** ele saiu (`GetProcessTimes`),
/// compara com o deadline absoluto, e devolve `TimedOut`.
///
/// Runtime construído à mão (não `#[tokio::test]`) porque
/// `max_blocking_threads` não é configurável pela macro.
#[test]
fn wall_clock_verdict_survives_starved_runtime() {
    // Bootstrap do python num runtime normal: com o pool de
    // bloqueio limitado a 1 thread, o download/extração do
    // embeddable não teria como progredir.
    let exe = tokio::runtime::Runtime::new()
        .expect("runtime de bootstrap")
        .block_on(async { python_executable().await });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime faminto");

    rt.block_on(async move {
        let workspace = TempDir::new().expect("tempdir workspace");
        let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
            .expect("SecurityJailResolver::new");

        // Filho morre sozinho aos ~4 s — bem depois do orçamento
        // de 2 s, e bem antes de conseguirmos olhar.
        let config = SandboxConfig::new(
            exe,
            vec!["-c".to_string(), "import time; time.sleep(4)".to_string()],
            workspace.path().to_path_buf(),
        );
        let mut process = resolver.spawn(config).expect("spawn");

        // Ocupa a única thread de bloqueio por 8 s.
        tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(8)));
        // Dá tempo de o `spawn_blocking` acima de fato tomar a
        // thread antes de o wait tentar enfileirar o dele.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let (resultado, ()) =
            tokio::join!(process.wait_with_timeout(Duration::from_secs(2)), async {
                // Toma a única worker thread por 9 s: o timer de 2 s
                // dispara, mas não há quem o observe.
                std::thread::sleep(Duration::from_secs(9));
            });

        let erro = resultado.expect_err(
            "FAIL-OPEN: o filho viveu ~4s com orçamento de 2s e o \
             wait devolveu sucesso — é exatamente o defeito de \
             2026-08-17 (o veredito voltou a depender de quando \
             nossas threads rodam)",
        );
        assert_eq!(
            erro.kind(),
            std::io::ErrorKind::TimedOut,
            "esperava TimedOut, veio: {erro:?}"
        );
        eprintln!(
            "[e2e_exec_python/veredito] OK: estouro reconhecido mesmo observado tarde ({erro})"
        );
    });
}

// ============================================================================
// Sanity: a Etapa 4 v1 não bloqueia python -c "<código que funciona>"
// ============================================================================

/// **Sanity:** um script Python simples (sem path traversal,
/// sem env leak, sem loop) executa e retorna stdout esperado.
/// Esse é o "controle positivo" — sem ele, os 3 testes de
/// negação acima podem estar passando por bug (não por
/// sandbox). Aqui provamos que o caminho feliz funciona.
///
/// **\`#[ignore]\` (Etapa 4 v1):** ver doc do
/// \`child_cannot_write_outside_workspace\` — reabilitar
/// junto com os outros em Etapa 5+ do Phase 7 (path
/// safety enforcement). Cobertura do caminho feliz
/// (python roda + retorna stdout) ja e exercitada
/// implicitamente pelo \`tree_kill.rs\` quando o
/// SandboxedProcess spawna e a stdout do filho e
/// coletada com sucesso.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_python_simple_hello_world() {
    let tools = build_exec_tools().await;
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
