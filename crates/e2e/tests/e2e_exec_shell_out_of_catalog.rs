//! E2E — `exec.shell` **não** está no catálogo (ADR-0037).
//!
//! Substitui `e2e_exec_shell_allowlist.rs` e
//! `e2e_exec_shell_denylist.rs`, apagados pelo ADR-0037 (REGRA §3.4
//! exige ADR pra apagar teste nomeado no `status.md` — este é o
//! ADR). Aqueles dois testavam o comportamento de uma ferramenta
//! que saiu do produto; o que precisa de cobertura agora é a
//! **ausência** dela.
//!
//! ## Por que a ferramenta saiu
//!
//! `exec.shell` executava `cmd.exe /c "<command>"` e se defendia
//! com uma allowlist de comandos. Mas
//! `frederico_security::exec_patterns::is_allowed` valida **só o
//! primeiro token**, e o `build_args` entregava o command string
//! **inteiro** pro `cmd.exe` — que interpreta `&`, `&&`, `||` e
//! `|` como separadores. Resultado: `echo x & <qualquer coisa>`
//! passava pelos dois gates.
//!
//! Isso foi medido, não deduzido: antes da remoção, um teste
//! rodou `echo marcador & ver` pelo caminho real e recebeu a
//! saída dos **dois** comandos, com `ver` sozinho sendo recusado
//! pela allowlist. A prova permanece viva como teste de unidade em
//! `frederico_security::exec_patterns::tests::allowlist_is_defeated_by_cmd_exe_command_separators`.
//!
//! Regra aplicada: **capacidade incompleta é capacidade
//! indisponível** — a mesma que tirou `exec.python`/`exec.node` do
//! catálogo na Etapa 5+ e deletou o `dns_intercept` na Etapa 6.

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use frederico_runtimes::{RuntimeConfig, RuntimeRegistry};
use frederico_security::exec_patterns::{denylist_hit, is_allowed, SHELL_ALLOWLIST_DEFAULT};
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::{AuditSink, NoopAuditSink, Tool};
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

/// **Teste de negação:** `build_default_exec_tools` — o construtor
/// do catálogo `exec.*` usado pela casca em produção — não devolve
/// `exec.shell`. Se alguém religar a ferramenta sem passar por um
/// ADR novo, este teste quebra.
#[test]
fn exec_shell_is_not_in_default_catalog() {
    let tools = build_exec_tools();
    let ids: Vec<String> = tools.iter().map(|t| t.manifest().id.to_string()).collect();

    assert!(
        !ids.iter().any(|id| id == "exec.shell"),
        "exec.shell voltou ao catalogo sem ADR (ver ADR-0037): {ids:?}"
    );
    // Controle positivo: as duas que continuam no catálogo estão
    // lá. Sem isto, o teste passaria com o catálogo vazio.
    assert!(
        ids.iter().any(|id| id == "exec.python"),
        "exec.python sumiu do catalogo: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id == "exec.node"),
        "exec.node sumiu do catalogo: {ids:?}"
    );
}

/// **A razão da remoção, fixada em teste.** Enquanto esta
/// asserção passar, a allowlist de comandos não é uma barreira —
/// e `exec.shell` não pode voltar ao catálogo confiando nela.
///
/// O dia em que o `exec_patterns` ganhar recusa de separadores de
/// shell, este teste passa a falhar. Isso é o sinal de que a
/// pendência do ADR-0037 pode ser reaberta — não um defeito.
#[test]
fn allowlist_that_justified_the_tool_is_still_defeated_by_cmd_separators() {
    let smuggled = "echo marcador & ver";

    assert!(
        is_allowed(smuggled, SHELL_ALLOWLIST_DEFAULT),
        "allowlist passou a recusar separadores — reabra o ADR-0037"
    );
    assert_eq!(
        denylist_hit(smuggled),
        None,
        "denylist passou a pegar separadores — reabra o ADR-0037"
    );
    // E o comando carona, sozinho, seria recusado — é isso que
    // torna o caso um bypass e não um uso legítimo.
    assert!(!is_allowed("ver", SHELL_ALLOWLIST_DEFAULT));
}
