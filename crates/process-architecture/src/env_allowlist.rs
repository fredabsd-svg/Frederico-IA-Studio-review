//! Construção do environment do worker a partir de uma **allowlist**
//! explícita.
//!
//! O `WorkerManager` chama [`build_worker_env`] na hora de spawn,
//! passando uma lista de `(name, value)` que o worker **pode** ver.
//! Nada mais é passado — o env do processo pai **não é herdado**
//! pelo filho. É a regra "variáveis de ambiente do processo pai
//! não são herdadas pelos workers" do
//! `docs/architecture/process-architecture.md` §Invariantes.
//!
//! Os testes em `mod tests` provam a invariante: injetam
//! `OPENAI_API_KEY` no test runner, constroem o env pela
//! allowlist, e provam que a chave **não** vaza pro resultado.
//! Esse teste é o que **sobreviveu** à retirada do `WorkerManager`
//! quebrado (ADR-0015) — ele não depende do manager, só da função
//! pura.

use std::collections::BTreeMap;

/// Entrada de env explícita — `(name, value)`.
pub type EnvEntry = (String, String);

/// Constrói o env do worker a partir da allowlist.
///
/// O `cwd` (diretório de trabalho) também é configurável — o
/// `WorkerManager` passa o `AppPaths::worker_cwd()` da casca.
///
/// **Política:** o env do processo pai **não** é lido. Só entra no
/// env do worker o que estiver em `allowlist`. Se `allowlist` tiver
/// duplicatas (mesma `name` aparecendo duas vezes), a última vence
/// — sem warning, é chamada do caller.
#[must_use]
pub fn build_worker_env(allowlist: &[EnvEntry]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in allowlist {
        out.insert(k.clone(), v.clone());
    }
    // Sane defaults que o worker pode depender (ex.: `TEMP` no
    // Windows resolve pra `%LOCALAPPDATA%\Temp`, que Tesseract
    // usa pra arquivos de scratch). Esses são valores fixos —
    // não lidos do pai.
    out.entry("PYTHONIOENCODING".to_string())
        .or_insert_with(|| "utf-8".to_string());
    out
}

/// Constrói o env com sane defaults + extras do caller. Usado pela
/// casca Tauri no boot (Etapa 3) — combina a allowlist que veio do
/// manifesto do worker com variáveis que o app **sempre** passa
/// (ex.: `FREDERICO_APP_VERSION`).
#[must_use]
pub fn build_worker_env_with_defaults(
    allowlist: &[EnvEntry],
    always_include: &[EnvEntry],
) -> BTreeMap<String, String> {
    let mut out = build_worker_env(allowlist);
    for (k, v) in always_include {
        out.insert(k.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_allowlist_does_not_inherit_parent() {
        // Injeta `OPENAI_API_KEY` no test runner (a env do test é a
        // env do processo de teste) e prova que ela **não** vaza
        // pro env construído pela allowlist. Esta é a regra do
        // `process-architecture.md` §Invariantes: "Variáveis de
        // ambiente do processo pai não são herdadas pelos workers".
        std::env::set_var("OPENAI_API_KEY", "sk-leak-me-1234");
        std::env::set_var("PATH", "C:\\secret\\path");

        let env = build_worker_env(&[]);
        assert!(!env.contains_key("OPENAI_API_KEY"), "OPENAI_API_KEY vazou");
        assert!(!env.contains_key("PATH"), "PATH vazou");
        assert!(env.contains_key("PYTHONIOENCODING"));

        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("PATH");
    }

    #[test]
    fn env_allowlist_includes_explicit_entries() {
        let env = build_worker_env(&[
            ("MY_VAR".to_string(), "my_value".to_string()),
            ("ANOTHER".to_string(), "1".to_string()),
        ]);
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("my_value"));
        assert_eq!(env.get("ANOTHER").map(String::as_str), Some("1"));
        assert!(env.contains_key("PYTHONIOENCODING"));
    }

    #[test]
    fn env_allowlist_with_defaults_merges_always_include() {
        let env = build_worker_env_with_defaults(
            &[("MY_VAR".to_string(), "1".to_string())],
            &[("FREDERICO_APP_VERSION".to_string(), "0.1.0".to_string())],
        );
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("FREDERICO_APP_VERSION").map(String::as_str),
            Some("0.1.0")
        );
        assert!(env.contains_key("PYTHONIOENCODING"));
    }
}
