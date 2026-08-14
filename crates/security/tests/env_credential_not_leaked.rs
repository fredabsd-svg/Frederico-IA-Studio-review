//! Teste de vazamento de credencial — fecha o buraco que a
//! investigação da Etapa 6+1 (wiring do proxy de rede, Fase 7)
//! expôs.
//!
//! ## Por que este teste existe
//!
//! Até a Etapa 6+1, `spawn_windows` (`crates/security/src/jail.rs`)
//! tinha um fallback: se `CreateProcessAsUserW` falhasse ao usar o
//! env block **controlado** (REQUIRED + `extra_env`), ele
//! reexecutava silenciosamente com `lpEnvironment = None` — o
//! filho herdava o ambiente **inteiro** do processo pai. O
//! fallback existia (por causa de um bug não relacionado, falta
//! de `CREATE_UNICODE_ENVIRONMENT`) e nunca disparava um erro
//! visível pro caller.
//!
//! O `EnvFilter` (`env_filter.rs`) tem unit tests cobrindo a
//! **função** `apply()` isoladamente — provam que, dado um
//! `Vec<(String, String)>` de entrada, a função produz a saída
//! certa. Mas nenhum teste cobria o **processo de verdade**
//! criado por `SecurityJailResolver::spawn`. Os dois podem
//! divergir — foi exatamente isso que o fallback provou: a
//! função de filtro sempre esteve correta, mas o processo real
//! às vezes ignorava o resultado dela por completo.
//!
//! Este teste fecha essa lacuna: planta uma credencial falsa em
//! `OPENAI_API_KEY` (que está em `EnvAllowlist::DENIED`) no
//! ambiente **real** do processo de teste — não um
//! `Vec<(String, String)>` de mentira — spawna `python.exe` real
//! via `SecurityJailResolver::spawn` (o mesmo caminho que
//! `exec.python`/`exec.node` usam em produção via
//! `FilesExecToolBase`), e afirma que a credencial não chega ao
//! filho. Se o fallback (ou qualquer variante futura dele)
//! voltar, este teste pega.
//!
//! Ver ameaça I1 em `docs/architecture/security-threat-model.md`.

#![cfg(windows)]
#![allow(unsafe_code)]
// `std::env::set_var`/`remove_var` são `unsafe fn` desde Rust
// 1.86+ (race entre threads que leem/escrevem env
// concorrentemente). O crate tem `unsafe_code = "deny"` (não
// `forbid`) exatamente pra permitir esse `#![allow]` cirúrgico
// em integration tests — mesmo padrão de
// `windows_credential_store.rs`. Este binário de teste tem uma
// única test function; nada mais no processo toca env vars
// concorrentemente.

use std::time::Duration;

use frederico_security::jail::{SandboxConfig, SecurityJailConfig, SecurityJailResolver};
use frederico_security::raw_child::wrap_pipe_handle_as_async_file;

const FAKE_SECRET: &str = "sk-FAKE-SHOULD-NEVER-LEAK-c7f3a1b2";

/// RAII: remove a credencial falsa do ambiente do processo de
/// teste mesmo se o teste falhar no meio (panic de assert) —
/// não queremos que uma falha aqui vaze `OPENAI_API_KEY` falso
/// pros testes seguintes no mesmo processo.
struct EnvVarGuard {
    name: &'static str,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: ver comentário do `#![allow(unsafe_code)]` no
        // topo do arquivo.
        unsafe {
            std::env::remove_var(self.name);
        }
    }
}

/// **O teste que importa:** planta `OPENAI_API_KEY=<segredo
/// falso>` no ambiente real do processo de teste (não um mock),
/// spawna `python.exe` sob o `SecurityJailResolver` de verdade,
/// e lê o que o Python filho vê em `os.environ`. Afirma
/// ausência total (não só string vazia — `retain()` em
/// `jail.rs` remove a chave inteira, não sobrescreve).
#[tokio::test]
async fn exec_child_does_not_inherit_denied_credential_from_real_parent_env() {
    let python = match find_python() {
        Some(p) => p,
        None => {
            eprintln!(
                "[env-leak] python.exe não encontrado; teste pulado. \
                 Para rodar, instale Python 3 ou adicione ao PATH."
            );
            return;
        }
    };

    // Planta a credencial falsa no ambiente REAL do processo —
    // é isso que `SecurityJailResolver::spawn` vai ler via
    // `std::env::vars_os()` internamente. Um `Vec` de mentira
    // não provaria nada sobre o caminho de produção.
    // SAFETY: ver comentário do `#![allow(unsafe_code)]` no
    // topo do arquivo.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", FAKE_SECRET);
    }
    let _guard = EnvVarGuard {
        name: "OPENAI_API_KEY",
    };

    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");

    // Python lê `OPENAI_API_KEY` e imprime o que vê (ou
    // `<absent>` se `os.environ.get` não achar a chave).
    let script = r#"
import os
print(os.environ.get("OPENAI_API_KEY", "<absent>"), flush=True)
"#;

    // Mesmo motivo do `tree_kill.rs`: `set_low_integrity_label`
    // exige workdir owned pelo user; `tempfile` cria em `%TEMP%`.
    let workdir = tempfile::tempdir().expect("tempdir workdir");
    let config = SandboxConfig::new(
        python,
        vec!["-c".to_string(), script.to_string()],
        workdir.path().to_path_buf(),
    );
    let mut child = resolver.spawn(config).expect("spawn");

    let stdout_handle = child.take_stdout_handle().expect("stdout piped");
    let stdout = wrap_pipe_handle_as_async_file(stdout_handle).expect("wrap stdout");
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read_line");
    let seen = line.trim();

    child
        .wait_with_timeout(Duration::from_secs(10))
        .await
        .expect("wait_with_timeout");

    eprintln!("[env-leak] filho viu OPENAI_API_KEY={seen:?}");

    assert_ne!(
        seen, FAKE_SECRET,
        "VAZAMENTO: o filho do sandbox viu a credencial do processo pai. \
         Ameaça I1 (security-threat-model.md) reaberta — provavelmente \
         um fallback silencioso reintroduziu herança de env."
    );
    assert_eq!(
        seen, "<absent>",
        "esperava OPENAI_API_KEY totalmente ausente no filho (a chave \
         nem deveria existir em os.environ, não só vir vazia). \
         Valor visto: {seen:?}"
    );
}

/// Mesmo helper de `tree_kill.rs` (não exportado de lá —
/// duplicado deliberadamente; dois integration test binaries
/// não compartilham módulos sem um crate auxiliar, que não
/// vale a complexidade extra pra uma função de 15 linhas).
fn find_python() -> Option<std::path::PathBuf> {
    for name in &["python", "python3", "py"] {
        if let Ok(out) = std::process::Command::new("where").arg(name).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                for line in s.lines() {
                    let path = std::path::PathBuf::from(line.trim());
                    // Pula o stub do WindowsApps (não é Python
                    // real, é um launcher que requer Store Python).
                    let path_str = path.to_string_lossy();
                    if path_str.contains("WindowsApps") {
                        continue;
                    }
                    return Some(path);
                }
            }
        }
    }
    None
}
