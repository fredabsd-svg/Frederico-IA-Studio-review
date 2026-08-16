//! E2E — `exec.shell` de volta ao catálogo, endurecido (ADR-0044,
//! Etapa 2b da Fase 8).
//!
//! Substitui `e2e_exec_shell_out_of_catalog.rs`, que cobria a
//! **ausência** da ferramenta enquanto o ADR-0037 a mantinha fora do
//! produto. Aquele arquivo estava nomeado na coluna `E2E de
//! cobertura` do `status.md`, e a REGRA §3.4 proíbe apagar teste
//! nomeado sem ADR — a autorização é o ADR-0044 §D6.
//!
//! ## O que este arquivo cobre
//!
//! Um teste de negação **por caminho de fuga conhecido**, que é o
//! item 3 do ADR-0037 §D5, mais um controle positivo. O controle
//! não é decoração: sem ele, todas as negações passariam com a
//! ferramenta simplesmente quebrada, e "não executou" seria
//! confundido com "recusou".
//!
//! | Caminho de fuga | Teste |
//! |---|---|
//! | Contrabando atrás de separador (`echo x & ver`) | `refuses_command_smuggled_behind_a_separator` |
//! | Sequestro de binário pelo diretório corrente | `refuses_binary_planted_in_the_workspace` |
//! | Programa fora da lista fechada | `refuses_program_outside_the_closed_list` |
//! | Comando destrutivo (denylist) | `refuses_destructive_command_by_denylist` |
//! | Aspas não balanceadas | `refuses_unbalanced_quotes_instead_of_guessing` |
//!
//! ## Por que teste direto do tool
//!
//! Os contratos aqui são do `exec.shell` e do sandbox, não do
//! pipeline modelo→tool (esse já tem cobertura em
//! `e2e_files_read.rs` e `e2e_pipeline_sequencial_e2e.rs`). Mesma
//! decisão do `e2e_exec_python_under_sandbox.rs`.
//!
//! **Sem `RuntimeRegistry` real:** `exec.shell` não usa runtime
//! portátil — os programas são do próprio Windows. O registry vazio
//! basta e evita baixar 11 MB do python.org por um teste que não
//! toca Python.

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_runtimes::{RuntimeConfig, RuntimeRegistry};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool, ToolContext, ToolResult};
use serde_json::json;
use tempfile::TempDir;

// ============================================================================
// Setup
// ============================================================================

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

fn shell_tool() -> Arc<dyn Tool> {
    build_exec_tools()
        .into_iter()
        .find(|t| t.manifest().id.to_string() == "exec.shell")
        .expect("exec.shell no catalogo (ADR-0044)")
}

fn make_ctx(workspace: &std::path::Path) -> ToolContext {
    let jail = Jail::new(workspace).expect("Jail::new em workspace tempdir");
    ToolContext::new(ConversationId::new(), RunId::new(), MessageId::new(), jail)
}

/// Roda um comando pelo caminho real da ferramenta.
async fn run_shell(workspace: &std::path::Path, command: &str) -> ToolResult {
    let tool = shell_tool();
    let ctx = make_ctx(workspace);
    tool.execute(&ctx, &json!({ "command": command })).await
}

fn stdout_of(result: &ToolResult) -> String {
    result
        .output
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Workspace com dois arquivos de amostra.
fn workspace() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("amostra.txt"), "alfa\nbeta\ngama\n").expect("amostra");
    std::fs::write(tmp.path().join("outra.txt"), "alfa\nbeta\ndelta\n").expect("outra");
    tmp
}

// ============================================================================
// Catálogo
// ============================================================================

/// `exec.shell` está no catálogo default outra vez (ADR-0044),
/// junto com as duas ferramentas que nunca saíram.
///
/// O controle positivo (`exec.python`/`exec.node`) existe porque
/// sem ele um catálogo vazio faria a asserção principal falhar por
/// motivo errado — e um catálogo quebrado é exatamente o tipo de
/// regressão que este teste deveria pegar.
#[test]
fn exec_shell_is_in_default_catalog() {
    let ids: Vec<String> = build_exec_tools()
        .iter()
        .map(|t| t.manifest().id.to_string())
        .collect();

    assert!(
        ids.iter().any(|id| id == "exec.shell"),
        "exec.shell fora do catalogo (ver ADR-0044): {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == "exec.python"),
        "exec.python sumiu do catalogo: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == "exec.node"),
        "exec.node sumiu do catalogo: {ids:?}"
    );
}

// ============================================================================
// Controle positivo
// ============================================================================

/// **Controle positivo.** Sem ele, todos os testes de negação
/// abaixo passariam com a ferramenta quebrada — "não executou"
/// leria como "recusou".
#[tokio::test]
async fn runs_an_allowlisted_command_for_real() {
    let ws = workspace();
    let result = run_shell(ws.path(), "type amostra.txt").await;

    assert!(
        result.ok,
        "comando allowlisted falhou: {:?}",
        result.error_message
    );
    let stdout = stdout_of(&result);
    assert!(
        stdout.contains("alfa") && stdout.contains("gama"),
        "stdout nao tem o conteudo do arquivo: {stdout:?}"
    );
}

/// O mesmo, para um programa do `System32` (spawn direto, sem
/// `cmd.exe`) — o outro dos dois caminhos de execução.
#[tokio::test]
async fn runs_a_system32_program_for_real() {
    let ws = workspace();
    let result = run_shell(ws.path(), "findstr alfa amostra.txt").await;

    assert!(result.ok, "findstr falhou: {:?}", result.error_message);
    assert!(
        stdout_of(&result).contains("alfa"),
        "findstr nao achou a linha: {:?}",
        stdout_of(&result)
    );
}

/// Aspas agrupam um argumento com espaços — sem elas, um termo de
/// busca com espaço viraria dois argumentos.
#[tokio::test]
async fn quoted_argument_survives_as_one_argument() {
    let ws = workspace();
    std::fs::write(ws.path().join("frase.txt"), "alfa beta gama\nsozinho\n").expect("frase");

    let result = run_shell(ws.path(), "findstr \"alfa beta\" frase.txt").await;

    assert!(
        result.ok,
        "findstr com aspas falhou: {:?}",
        result.error_message
    );
    assert!(
        stdout_of(&result).contains("alfa beta"),
        "argumento com espaco foi quebrado: {:?}",
        stdout_of(&result)
    );
}

// ============================================================================
// Negação 1 — contrabando atrás de separador (o bypass do ADR-0037)
// ============================================================================

/// **O caminho de fuga que tirou a ferramenta do catálogo.**
///
/// O ADR-0037 mediu: `echo marcador & ver` executava os dois
/// comandos, com `ver` sozinho sendo recusado pela allowlist. Aqui
/// o comando é recusado antes do spawn, e o teste checa as duas
/// coisas — que recusou, e que nada do lado direito do separador
/// rodou.
#[tokio::test]
async fn refuses_command_smuggled_behind_a_separator() {
    let ws = workspace();

    for command in [
        "echo marcador & ver",
        "echo marcador && ver",
        "echo marcador | more amostra.txt",
        "echo marcador > escrito.txt",
        "type amostra.txt & findstr alfa amostra.txt",
    ] {
        let result = run_shell(ws.path(), command).await;

        assert!(!result.ok, "comando de contrabando executou: {command:?}");
        let erro = result.error_message.clone().unwrap_or_default();
        assert!(
            erro.contains("metacaractere de shell"),
            "recusou pelo motivo errado ({command:?}): {erro}"
        );
        assert!(
            stdout_of(&result).is_empty(),
            "houve saida de um comando recusado: {command:?}"
        );
    }

    // E nada foi escrito — a recusa é antes do spawn, então o
    // redirecionamento nunca teve chance de criar o arquivo.
    assert!(
        !ws.path().join("escrito.txt").exists(),
        "redirecionamento recusado ainda assim criou arquivo"
    );
}

// ============================================================================
// Negação 2 — sequestro de binário pelo diretório corrente
// ============================================================================

/// **Caminho de fuga que o ADR-0037 não tinha nomeado**, achado ao
/// medir a Etapa 2b: o `cmd.exe` procura o programa no diretório
/// corrente **antes** do `PATH`, e o diretório corrente do filho é
/// o workspace — onde o `files.write` escreve. Com a v1, plantar
/// `findstr.bat` no workspace e pedir `findstr alfa amostra.txt`
/// executava o arquivo plantado.
///
/// Está fechado porque o programa é resolvido por caminho absoluto
/// em `System32` e não há busca. Os três impostores cobrem as três
/// extensões que o `PATHEXT` resolveria antes: `.com`, `.exe`,
/// `.bat`, nessa ordem.
#[tokio::test]
async fn refuses_binary_planted_in_the_workspace() {
    let ws = workspace();
    std::fs::write(
        ws.path().join("findstr.bat"),
        "@echo SEQUESTRADO-PELO-WORKSPACE\r\n",
    )
    .expect("plant .bat");
    std::fs::write(ws.path().join("findstr.com"), b"nao-e-um-com-valido").expect("plant .com");
    std::fs::write(ws.path().join("findstr.exe"), b"nao-e-um-exe-valido").expect("plant .exe");

    let result = run_shell(ws.path(), "findstr alfa amostra.txt").await;

    assert!(
        result.ok,
        "o findstr de verdade nao rodou: {:?}",
        result.error_message
    );
    let stdout = stdout_of(&result);
    assert!(
        !stdout.contains("SEQUESTRADO"),
        "executou o binario plantado no workspace: {stdout:?}"
    );
    assert!(
        stdout.contains("alfa"),
        "o findstr de System32 nao produziu o resultado esperado: {stdout:?}"
    );
    // O programa efetivamente executado é o de `System32` — não um
    // caminho dentro do workspace.
    let executado = result
        .accessed_paths
        .first()
        .expect("accessed_paths registra o programa")
        .clone();
    assert!(
        !executado.starts_with(ws.path()),
        "programa resolvido dentro do workspace: {}",
        executado.display()
    );
}

// ============================================================================
// Negação 3 — programa fora da lista fechada
// ============================================================================

/// Tudo que não está na lista fechada é recusado, inclusive
/// programas que **existem e rodam** sob o sandbox — medido na
/// Etapa 2b: `curl`, `certutil` e `tar` executam com integridade
/// baixa. Ficaram de fora por decisão (saída de rede, escrita em
/// disco), e é a allowlist que sustenta essa decisão.
#[tokio::test]
async fn refuses_program_outside_the_closed_list() {
    let ws = workspace();

    for command in [
        "curl --version",
        "certutil -hashfile amostra.txt SHA256",
        "tar --version",
        "powershell -c echo oi",
        "whoami",
        "ls -la",
        "cat amostra.txt",
        "grep alfa amostra.txt",
    ] {
        let result = run_shell(ws.path(), command).await;
        assert!(!result.ok, "programa fora da lista executou: {command:?}");
        let erro = result.error_message.unwrap_or_default();
        assert!(
            erro.contains("nao esta na allowlist"),
            "recusou pelo motivo errado ({command:?}): {erro}"
        );
    }
}

/// Caminho absoluto não é porta dos fundos para a lista fechada.
/// Sem esta recusa, `C:\Windows\System32\curl.exe` contornaria a
/// allowlist inteira apontando direto para o binário.
#[tokio::test]
async fn refuses_allowlisted_program_reached_by_absolute_path() {
    let ws = workspace();

    for command in [
        r"C:\Windows\System32\findstr.exe alfa amostra.txt",
        r"C:\Windows\System32\curl.exe --version",
        r".\findstr alfa amostra.txt",
    ] {
        let result = run_shell(ws.path(), command).await;
        assert!(!result.ok, "caminho absoluto executou: {command:?}");
    }
}

// ============================================================================
// Negação 4 — denylist
// ============================================================================

/// A denylist continua recusando antes de tudo. Hoje ela é
/// redundante (nada nela resolve na allowlist — invariante
/// verificada em `frederico_security::exec_patterns::tests::denylist_is_redundant_with_allowlist`),
/// mas a recusa dá a mensagem certa em vez de "não está na lista".
#[tokio::test]
async fn refuses_destructive_command_by_denylist() {
    let ws = workspace();

    let result = run_shell(ws.path(), "rm -rf /").await;
    assert!(!result.ok, "comando destrutivo executou");
    let erro = result.error_message.unwrap_or_default();
    assert!(
        erro.contains("denylist"),
        "recusou pelo motivo errado: {erro}"
    );
}

// ============================================================================
// Negação 5 — aspas não balanceadas
// ============================================================================

/// O tokenizador não adivinha intenção. Aspa aberta e não fechada é
/// erro, não um argumento que vai até o fim da linha.
#[tokio::test]
async fn refuses_unbalanced_quotes_instead_of_guessing() {
    let ws = workspace();

    let result = run_shell(ws.path(), "findstr \"alfa amostra.txt").await;
    assert!(!result.ok, "comando com aspa aberta executou");
    let erro = result.error_message.unwrap_or_default();
    assert!(
        erro.contains("aspas nao balanceadas"),
        "recusou pelo motivo errado: {erro}"
    );
}

// ============================================================================
// Limitação declarada — o que esta ferramenta NÃO protege
// ============================================================================

/// **Fixação de limitação conhecida**, mesmo padrão do
/// `e2e_network_raw_socket_bypasses_proxy_documented`: o teste
/// afirma o comportamento **real**, não o desejado.
///
/// O rótulo de integridade baixa restringe **escrita**, não
/// leitura. Um programa da lista lê arquivo fora do workspace se
/// receber o caminho. Não é regressão desta etapa nem específico do
/// `exec.shell` — `exec.python` faz o mesmo com um `open()`, e o
/// `security-threat-model.md` já nomeia "read-up de paths
/// Medium-labeled" entre o que o sandbox não protege. Fechar exige
/// filtro no nível de processo (WFP/WDAC), que o ADR-0039 §D4
/// manteve fora da Fase 8 por ser de outra natureza.
///
/// Se um dia isso for fechado, este teste falha — e a falha é o
/// sinal de que a limitação saiu do `SECURITY.md`, não um defeito.
#[tokio::test]
async fn documented_limit_child_can_read_outside_workspace() {
    let ws = workspace();
    let fora = TempDir::new().expect("tempdir fora do workspace");
    let segredo = fora.path().join("segredo.txt");
    std::fs::write(&segredo, "conteudo-fora-do-workspace\n").expect("segredo");

    let result = run_shell(ws.path(), &format!("type {}", segredo.display())).await;

    assert!(
        result.ok && stdout_of(&result).contains("conteudo-fora-do-workspace"),
        "a leitura fora do workspace passou a ser bloqueada — atualize o \
         SECURITY.md e o security-threat-model.md, este teste fixa a \
         limitacao declarada (ok={}, err={:?})",
        result.ok,
        result.error_message
    );
}
