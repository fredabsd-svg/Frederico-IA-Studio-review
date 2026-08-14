//! Padrões de comando pra `exec.shell` (Etapa 7 da Fase 7, ADR-0034
//! D3, `docs/architecture/exec-tools-specification.md` §"`FilesExecShellTool`").
//!
//! Duas listas hardcoded (editáveis pelo usuário é roadmap — Fase 8):
//!
//! - [`SHELL_DENYLIST`]: comandos destrutivos que **sempre** são
//!   recusados, independente de allowlist. Checado primeiro.
//! - [`SHELL_ALLOWLIST_DEFAULT`]: primeiro token dos comandos
//!   read-only considerados seguros. `exec.shell` v1 aplica esta
//!   lista **incondicionalmente** (não há hoje wiring do
//!   `PermissionSet::terminal` até o `Tool::execute` — ver
//!   `crates/tool-registry/src/exec/shell.rs` — e o teto do
//!   `PermissionSet::allow_all()` já é `TerminalMode::Allowlist`,
//!   nunca "sem restrição"; aplicar a allowlist sempre é
//!   consistente com esse teto, não uma regressão dele).
//!
//! **Limitação honesta:** o match é substring case-insensitive
//! sobre o command string inteiro, não um parser de shell real.
//! `rm -r -f` (flags separadas) ou `rm --recursive --force` **não**
//! casam `"rm -rf"` — é um desvio conhecido, documentado (não
//! escondido) e fixado em teste
//! (`shell_denylist_hit_documents_split_flag_bypass`). A barreira
//! real contra danos ao *host* continua sendo o Jail (Mandatory
//! Label\Low) + Restricted Token — a denylist é defesa em
//! profundidade contra o caso comum, não uma prova de
//! impossibilidade.

/// Comandos destrutivos recusados antes do spawn, independente de
/// allowlist. Substring case-insensitive contra o command string
/// inteiro (não só o primeiro token — `rm -rf` tem 2 palavras).
pub const SHELL_DENYLIST: &[&str] = &[
    "rm -rf",
    "del /f /s /q",
    "remove-item -recurse -force",
    "format",
    "diskpart",
    "bcdedit",
    "reg delete",
    "net user",
    "net localgroup",
    "cipher /w",
    "sfc /scannow",
];

/// Primeiro token (o binário) que o comando precisa ter pra passar
/// na allowlist default. Todos read-only.
pub const SHELL_ALLOWLIST_DEFAULT: &[&str] = &[
    "ls", "cat", "head", "tail", "grep", "find", "wc", "pwd", "echo",
];

/// Normaliza um command string pra comparação: minúsculo + espaços
/// em sequência colapsados em um único espaço. Não tenta tokenizar
/// aspas/escapes — é um filtro de superfície, não um parser de
/// shell (a defesa real é o Jail; ver doc do módulo).
fn normalize(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Retorna o padrão da [`SHELL_DENYLIST`] que casou, se houver.
/// `None` = comando não bate nenhum padrão destrutivo conhecido.
#[must_use]
pub fn denylist_hit(command: &str) -> Option<&'static str> {
    let normalized = normalize(command);
    SHELL_DENYLIST
        .iter()
        .find(|pat| normalized.contains(*pat))
        .copied()
}

/// Primeiro token (whitespace-delimited) do command string, ou
/// `None` se o comando é vazio/só espaços.
#[must_use]
pub fn first_token(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

/// `true` se o primeiro token do comando está na `allowlist`
/// (comparação case-insensitive). `allowlist` vazia nunca aceita
/// nada (fail-closed — mesma regra do `NetworkAllowlist`).
#[must_use]
pub fn is_allowed(command: &str, allowlist: &[&str]) -> bool {
    let Some(token) = first_token(command) else {
        return false;
    };
    let token_lower = token.to_ascii_lowercase();
    allowlist
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&token_lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_hit_catches_rm_rf() {
        assert_eq!(denylist_hit("rm -rf /"), Some("rm -rf"));
    }

    #[test]
    fn denylist_hit_case_insensitive_and_whitespace_tolerant() {
        assert_eq!(
            denylist_hit("DEL   /F /S /Q  C:\\foo"),
            Some("del /f /s /q")
        );
    }

    #[test]
    fn denylist_hit_none_for_safe_command() {
        assert_eq!(denylist_hit("ls -la"), None);
    }

    #[test]
    fn denylist_hit_documents_split_flag_bypass() {
        // Limitação conhecida e documentada (doc do módulo): flags
        // separadas escapam o match por substring literal. Fixado
        // em teste — não escondido.
        assert_eq!(
            denylist_hit("rm -r -f /"),
            None,
            "bypass conhecido: `rm -r -f` não casa `rm -rf` (substring literal, não parser de shell)"
        );
    }

    #[test]
    fn denylist_hit_matches_reg_delete_mid_command() {
        assert_eq!(
            denylist_hit("reg delete HKCU\\Software\\Foo /f"),
            Some("reg delete")
        );
    }

    #[test]
    fn first_token_extracts_program() {
        assert_eq!(first_token("ls -la /tmp"), Some("ls"));
        assert_eq!(first_token("  echo hi  "), Some("echo"));
        assert_eq!(first_token(""), None);
        assert_eq!(first_token("   "), None);
    }

    #[test]
    fn is_allowed_accepts_listed_program() {
        assert!(is_allowed("ls -la", SHELL_ALLOWLIST_DEFAULT));
        assert!(is_allowed("ECHO hi", SHELL_ALLOWLIST_DEFAULT));
    }

    #[test]
    fn is_allowed_rejects_unlisted_program() {
        assert!(!is_allowed("curl http://evil", SHELL_ALLOWLIST_DEFAULT));
        assert!(!is_allowed("rm -rf /", SHELL_ALLOWLIST_DEFAULT));
    }

    #[test]
    fn is_allowed_empty_allowlist_denies_everything() {
        assert!(!is_allowed("ls", &[]));
    }

    #[test]
    fn is_allowed_empty_command_denies() {
        assert!(!is_allowed("", SHELL_ALLOWLIST_DEFAULT));
    }
}
