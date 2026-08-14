//! `EnvFilter` + `EnvAllowlist` — fechamento da ameaça `I1` do
//! [`security-threat-model.md`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/architecture/security-threat-model.md)
//! (Sandbox herda env do processo pai).
//!
//! O filtro é **fail-closed**: vars não-listadas são removidas; vars
//! sensíveis são sobrescritas com string vazia **antes** de remover
//! (defesa contra cache de libc que pode ter visto o valor original
//! via `getenv` antes do fork).
//!
//! Ver [ADR-0031 §D5](../../../decisions/0031-fase-7-isolation-model-windows.md)
//! e [ADR-0036 §D5](../../../decisions/0036-security-jail-resolver-windows-job-objects.md)
//! para a decisão completa.
//!
//! ## Por que 3 categorias (REQUIRED, ALLOWED, DENIED)
//!
//! - **`REQUIRED`** — vars que **sempre** passam, sem chance do
//!   usuário desligar. Incluem `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`
//!   (necessárias pro proxy do sandbox funcionar, ADR-0033), `PATH`
//!   (apontando pro runtime portátil), `TEMP`/`TMP`, `LANG`/`LC_ALL`,
//!   `PYTHONHOME`/`PYTHONPATH`/`NODE_PATH` (runtimes portáteis, ADR-0037),
//!   `HOME`/`USERPROFILE`. **Não-editável pelo usuário** — fazer uma
//!   dessas cair é defeito.
//!
//! - **`ALLOWED`** — vars adicionais que passam se presentes no env
//!   pai. Editável pelo usuário no painel de configurações (Etapa 7
//!   UI/Polish da Fase 7). Default: vazio.
//!
//! - **`DENIED`** — vars que são sobrescritas com `""` antes de
//!   remover. Match exato + match por sufixo (`*_TOKEN` casa
//!   `GITHUB_TOKEN`, `READ_TOKEN`, etc.). Hardcoded com os segredos
//!   comuns (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`,
//!   `GITHUB_TOKEN`, `GH_TOKEN`, `*_TOKEN`, `*_SECRET`, `*_KEY`,
//!   `*_PRIVATE_KEY`, `DATABASE_URL`).
//!
//! ## Defesa contra cache de libc
//!
//! Algumas libc (notavelmente glibc em Linux) cacheiam o resultado de
//! `getenv()` em um buffer interno por thread. O `setenv()`/`unsetenv()`
//! invalida o cache **na thread que chamou** — outras threads podem
//! ter lido o valor antigo e mantido em uma cópia local. Pra cobrir
//! isso, o `EnvFilter::apply` faz **duas passadas**:
//!
//! 1. Sobrescreve as vars em `DENIED` com string vazia **in-place**
//!    no `parent_env` recebido (que é uma `Vec<(String, String)>`
//!    controlada pelo caller). O caller é responsável por passar uma
//!    cópia fresca, não o resultado de `std::env::vars()` direto.
//! 2. Constrói o vetor final só com as vars permitidas (`REQUIRED` +
//!    `ALLOWED` que estão presentes no pai), **excluindo** as
//!    `DENIED`.
//!
//! O resultado é o que vai pra `CreateProcess` via `lpEnvironment` no
//! Windows. O processo filho, ao chamar `getenv`, recebe `NULL` ou
//! string vazia para as vars filtradas. **Sem** cópia em cache
//! sobrevive (o cache é thread-local e o filho é processo novo).

use thiserror::Error;

/// Categoria da var. `Required` e `Denied` são hardcoded e não
/// editáveis; `Allowed` é configurável pelo usuário.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvEntry {
    /// Passa sempre que estiver presente no parent env. Hardcoded.
    Required(&'static str),
    /// Passa se presente no parent env. Editável pelo usuário.
    Allowed(String),
    /// Bloqueada: sobrescrita com `""` no parent, depois removida do
    /// env final. Hardcoded (deny-by-default para segredos comuns).
    Denied(&'static str),
    /// Bloqueada por padrão de sufixo (ex.: `*_TOKEN` casa
    /// `GITHUB_TOKEN`). Hardcoded.
    DeniedSuffix(&'static str),
}

/// Config do filtro. Construtor padrão já tem o conjunto mínimo
/// seguro; o caller pode adicionar mais entries em `ALLOWED` via
/// [`EnvAllowlist::with_allowed`].
#[derive(Debug, Clone)]
pub struct EnvAllowlist {
    pub entries: Vec<EnvEntry>,
}

impl EnvAllowlist {
    /// Conjunto mínimo seguro. `REQUIRED` cobre o que o sandbox
    /// precisa pra rodar; `DENIED` cobre os segredos comuns.
    #[must_use]
    pub fn secure_default() -> Self {
        Self {
            entries: vec![
                // Proxy (ADR-0033) — sem isso, o proxy local não
                // funciona. NÃO-editável.
                EnvEntry::Required("HTTP_PROXY"),
                EnvEntry::Required("HTTPS_PROXY"),
                EnvEntry::Required("NO_PROXY"),
                // PATH do runtime portátil (Etapa 3 da Fase 7,
                // ADR-0037) — sem isso, `python`/`node` não são
                // encontrados. NÃO-editável.
                EnvEntry::Required("PATH"),
                EnvEntry::Required("PATHEXT"), // Windows: sufixos de executáveis
                // **`SystemRoot`/`windir` — achado da Etapa 6+1.**
                // Sem `SystemRoot`, `WSAStartup` não consegue
                // expandir `%SystemRoot%\system32\mswsock.dll`
                // (o path que o catálogo Winsock guarda pros
                // providers base). Resultado: **qualquer**
                // `socket.socket(...)` no filho falha com
                // `WSAEPROVIDERFAILEDINIT` (WinError 10106) —
                // não é específico de HTTP, proxy, ou allowlist;
                // é qualquer uso de rede, e o efeito era
                // indistinguível de "proxy bloqueando" até o
                // teste `e2e_network_proxy_wired_into_exec_python.rs`
                // isolar `socket.socket()` puro. Confirmado
                // empiricamente: sem essas duas vars, todo
                // `exec.python`/`exec.node` que toca rede quebra
                // silenciosamente (o Python reporta um erro de
                // rede genérico, fácil de confundir com o proxy
                // funcionando). `windir` é o mesmo path por um
                // nome alternativo que ferramentas mais antigas
                // ainda consultam.
                EnvEntry::Required("SystemRoot"),
                EnvEntry::Required("windir"),
                // Scratch dir. NÃO-editável.
                EnvEntry::Required("TEMP"),
                EnvEntry::Required("TMP"),
                EnvEntry::Required("TMPDIR"),
                // Locale.
                EnvEntry::Required("LANG"),
                EnvEntry::Required("LC_ALL"),
                EnvEntry::Required("LC_CTYPE"),
                // Python portátil (ADR-0037).
                EnvEntry::Required("PYTHONHOME"),
                EnvEntry::Required("PYTHONPATH"),
                EnvEntry::Required("PYTHONIOENCODING"),
                EnvEntry::Required("PYTHONDONTWRITEBYTECODE"),
                // Node portátil (ADR-0037).
                EnvEntry::Required("NODE_PATH"),
                EnvEntry::Required("NODE_OPTIONS"),
                // Home dirs (alguns runtimes precisam).
                EnvEntry::Required("HOME"),
                EnvEntry::Required("USERPROFILE"),
                // DENIED — segredos comuns (sobrescritos com "" antes
                // de remover).
                EnvEntry::Denied("OPENAI_API_KEY"),
                EnvEntry::Denied("OPENAI_ORG_ID"),
                EnvEntry::Denied("ANTHROPIC_API_KEY"),
                EnvEntry::Denied("ANTHROPIC_ORG_ID"),
                EnvEntry::Denied("OPENROUTER_API_KEY"),
                EnvEntry::Denied("GITHUB_TOKEN"),
                EnvEntry::Denied("GH_TOKEN"),
                EnvEntry::Denied("GITLAB_TOKEN"),
                EnvEntry::Denied("HUGGINGFACE_TOKEN"),
                EnvEntry::Denied("HUGGINGFACE_HUB_TOKEN"),
                EnvEntry::Denied("DATABASE_URL"),
                EnvEntry::Denied("REDIS_URL"),
                EnvEntry::Denied("AWS_ACCESS_KEY_ID"),
                EnvEntry::Denied("AWS_SECRET_ACCESS_KEY"),
                EnvEntry::Denied("AWS_SESSION_TOKEN"),
                // Sufixos — cobrem vars customizadas (CI pipelines,
                // proxies internos, etc.) que seguem o padrão de nome
                // *_TOKEN, *_SECRET, *_KEY, *_PRIVATE_KEY.
                EnvEntry::DeniedSuffix("_TOKEN"),
                EnvEntry::DeniedSuffix("_SECRET"),
                EnvEntry::DeniedSuffix("_KEY"),
                EnvEntry::DeniedSuffix("_PRIVATE_KEY"),
            ],
        }
    }

    /// Adiciona entries `Allowed` (configurável pelo usuário). O
    /// caller é responsável por validar que a var é segura pra
    /// passar pro sandbox.
    #[must_use]
    pub fn with_allowed<I, S>(mut self, allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for s in allowed {
            self.entries.push(EnvEntry::Allowed(s.into()));
        }
        self
    }

    /// Testa se a var é `Required`.
    #[must_use]
    pub fn is_required(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, EnvEntry::Required(s) if *s == name))
    }

    /// Testa se a var é `Denied` (match exato ou por sufixo).
    #[must_use]
    pub fn is_denied(&self, name: &str) -> bool {
        self.entries.iter().any(|e| match e {
            EnvEntry::Denied(s) => *s == name,
            EnvEntry::DeniedSuffix(suffix) => name.ends_with(suffix),
            _ => false,
        })
    }
}

impl Default for EnvAllowlist {
    fn default() -> Self {
        Self::secure_default()
    }
}

/// Erro do `EnvFilter`. O `apply` é infallible (não há I/O, não há
/// alocação que possa falhar), mas a enum existe pra extensibilidade
/// futura (ex.: se o filtro precisar validar UTF-8 de vars
/// específicas).
#[derive(Debug, Error)]
pub enum EnvFilterError {
    /// Var do parent env tem bytes inválidos UTF-8. Não acontece em
    /// prática (Windows é UTF-16 e o caller normaliza), mas a
    /// fronteira existe pra explicitude.
    #[error("env var {name} tem bytes inválidos UTF-8")]
    InvalidUtf8 { name: String },
}

/// Filtro de env. Estado-mínimo: a allowlist. Stateless entre
/// `apply`s, então é barato clonar e passar entre threads.
#[derive(Debug, Clone)]
pub struct EnvFilter {
    allowlist: EnvAllowlist,
}

impl EnvFilter {
    #[must_use]
    pub fn new(allowlist: EnvAllowlist) -> Self {
        Self { allowlist }
    }

    #[must_use]
    pub fn allowlist(&self) -> &EnvAllowlist {
        &self.allowlist
    }

    /// Aplica o filtro sobre o env do parent. **Duas passadas**:
    ///
    /// 1. Para cada var em `DENIED` (exato ou sufixo) presente em
    ///    `parent_env`, sobrescreve o valor com string vazia
    ///    **in-place**. Defesa contra cache de libc.
    /// 2. Constrói `out` só com vars em `REQUIRED` (sempre que
    ///    presente) + `ALLOWED` (se presente). `DENIED` é pulada
    ///    mesmo se foi "sobrescrita" (a sobrescrita garante que
    ///    qualquer cache de libc vê `""`; a remoção garante que o
    ///    `getenv` retorna `NULL`).
    ///
    /// O `parent_env` é mutado in-place. O caller deve passar uma
    /// cópia fresca (não `std::env::vars()` direto, que compartilharia
    /// a estrutura interna do `std`).
    pub fn apply(
        &self,
        parent_env: &mut [(String, String)],
    ) -> Result<Vec<(String, String)>, EnvFilterError> {
        // Passada 1: sobrescreve DENIED in-place.
        for entry in &self.allowlist.entries {
            match entry {
                EnvEntry::Denied(name) => {
                    if let Some(slot) = parent_env.iter_mut().find(|(k, _)| k == name) {
                        slot.1.clear();
                    }
                }
                EnvEntry::DeniedSuffix(suffix) => {
                    for (k, v) in parent_env.iter_mut() {
                        if k.ends_with(suffix) {
                            v.clear();
                        }
                    }
                }
                _ => {}
            }
        }

        // Passada 2: constrói a saída. REQUIRED tem prioridade —
        // se uma var está em REQUIRED e em ALLOWED, sai só uma vez
        // (a do REQUIRED).
        let mut out: Vec<(String, String)> = Vec::with_capacity(self.allowlist.entries.len());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in &self.allowlist.entries {
            match entry {
                EnvEntry::Required(name) => {
                    if !seen.insert((*name).to_string()) {
                        continue; // já adicionado por um entry anterior
                    }
                    if let Some((_, v)) = parent_env.iter().find(|(k, _)| k == name) {
                        if std::str::from_utf8(v.as_bytes()).is_err() {
                            return Err(EnvFilterError::InvalidUtf8 {
                                name: (*name).to_string(),
                            });
                        }
                        out.push(((*name).to_string(), v.clone()));
                    }
                }
                EnvEntry::Allowed(name) => {
                    if !seen.insert(name.clone()) {
                        continue; // já adicionado por REQUIRED
                    }
                    if let Some((_, v)) = parent_env.iter().find(|(k, _)| k == name) {
                        if std::str::from_utf8(v.as_bytes()).is_err() {
                            return Err(EnvFilterError::InvalidUtf8 { name: name.clone() });
                        }
                        out.push((name.clone(), v.clone()));
                    }
                }
                EnvEntry::Denied(_) | EnvEntry::DeniedSuffix(_) => {
                    // Pula — valor já foi limpo in-place no parent;
                    // aqui não inclui na saída.
                }
            }
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_env() -> Vec<(String, String)> {
        vec![
            (
                "HTTP_PROXY".to_string(),
                "http://127.0.0.1:9000".to_string(),
            ),
            ("PATH".to_string(), "C:\\Python312;C:\\Node".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-secret-12345".to_string()),
            ("USER_TOKEN".to_string(), "gh-secret-67890".to_string()),
            ("GITHUB_TOKEN".to_string(), "ghp-secret-abcdef".to_string()),
            ("CUSTOM_VAR".to_string(), "user-custom-value".to_string()),
            (
                "TEMP".to_string(),
                "C:\\Users\\foo\\AppData\\Local\\Temp".to_string(),
            ),
        ]
    }

    // ... (tests abaixo — inseridos depois do bloco de testes existente)

    #[test]
    fn default_allowlist_is_secure() {
        let al = EnvAllowlist::secure_default();
        assert!(al.is_required("HTTP_PROXY"));
        assert!(al.is_required("PATH"));
        assert!(al.is_denied("OPENAI_API_KEY"));
        assert!(al.is_denied("GITHUB_TOKEN"));
        assert!(al.is_denied("USER_TOKEN")); // sufixo _TOKEN
        assert!(al.is_denied("MY_CUSTOM_KEY")); // sufixo _KEY
        assert!(!al.is_denied("CUSTOM_VAR"));
        assert!(!al.is_required("CUSTOM_VAR"));
    }

    #[test]
    fn apply_removes_denied_and_keeps_required() {
        let mut env = sample_env();
        let filter = EnvFilter::new(EnvAllowlist::secure_default());
        let out = filter.apply(&mut env).unwrap();

        // Required que estava no parent: presente.
        assert!(out.iter().any(|(k, _)| k == "HTTP_PROXY"));
        assert!(out.iter().any(|(k, _)| k == "PATH"));
        assert!(out.iter().any(|(k, _)| k == "TEMP"));
        // Denied exato: ausente.
        assert!(!out.iter().any(|(k, _)| k == "OPENAI_API_KEY"));
        assert!(!out.iter().any(|(k, _)| k == "GITHUB_TOKEN"));
        // Denied por sufixo: ausente.
        assert!(!out.iter().any(|(k, _)| k == "USER_TOKEN"));
        // Custom var (não em REQUIRED, não em ALLOWED, não em DENIED): ausente.
        assert!(!out.iter().any(|(k, _)| k == "CUSTOM_VAR"));
    }

    #[test]
    fn apply_clears_denied_in_place() {
        // Defesa contra cache de libc: a var DENIED é sobrescrita com
        // "" no parent_env antes de remover. Se o filho fizer getenv
        // via cache de thread, vê "" (não o valor original).
        let mut env = sample_env();
        let filter = EnvFilter::new(EnvAllowlist::secure_default());
        let _ = filter.apply(&mut env).unwrap();

        // No parent_env, as DENIED estão com valor "".
        assert!(env
            .iter()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_empty()));
        assert!(env.iter().any(|(k, v)| k == "GITHUB_TOKEN" && v.is_empty()));
        assert!(env.iter().any(|(k, v)| k == "USER_TOKEN" && v.is_empty()));
        // Required preservado.
        assert!(env.iter().any(|(k, v)| k == "HTTP_PROXY" && !v.is_empty()));
    }

    #[test]
    fn with_allowed_passes_through() {
        let mut env = sample_env();
        let allowlist = EnvAllowlist::secure_default()
            .with_allowed(["CUSTOM_VAR", "MY_PROJECT_TOKEN_OVERRIDE"]);
        let filter = EnvFilter::new(allowlist);
        let out = filter.apply(&mut env).unwrap();

        assert!(out
            .iter()
            .any(|(k, v)| k == "CUSTOM_VAR" && v == "user-custom-value"));
        // MY_PROJECT_TOKEN_OVERRIDE não está no parent, então não
        // aparece (ALLOWED só passa se a var está presente).
        assert!(!out.iter().any(|(k, _)| k == "MY_PROJECT_TOKEN_OVERRIDE"));
    }

    #[test]
    fn allowed_does_not_override_denied() {
        // Se o usuário adicionar `OPENAI_API_KEY` como ALLOWED, o
        // filtro ainda recusa (DENIED vence ALLOWED, por
        // segurança). Verifica que `is_denied` cobre.
        let al = EnvAllowlist::secure_default().with_allowed(["OPENAI_API_KEY"]);
        assert!(al.is_denied("OPENAI_API_KEY"));
    }

    #[test]
    fn missing_required_is_skipped_not_error() {
        // Se o parent não tem `HTTP_PROXY`, o filtro simplesmente
        // não inclui (não é erro — pode ser que o proxy esteja
        // desabilitado por feature flag).
        let mut env: Vec<(String, String)> = vec![
            ("PATH".to_string(), "C:\\Python312".to_string()),
            ("TEMP".to_string(), "C:\\Temp".to_string()),
        ];
        let filter = EnvFilter::new(EnvAllowlist::secure_default());
        let out = filter.apply(&mut env).unwrap();

        assert!(!out.iter().any(|(k, _)| k == "HTTP_PROXY"));
        assert!(out.iter().any(|(k, _)| k == "PATH"));
    }

    #[test]
    fn suffix_match_is_case_sensitive() {
        // Sufixo é case-sensitive: `_token` (minúsculo) NÃO casa
        // `USER_TOKEN`. Isso evita bypass via lowercase.
        let al = EnvAllowlist::secure_default();
        assert!(al.is_denied("USER_TOKEN"));
        assert!(!al.is_denied("user_token")); // sufixo é `_TOKEN`, não `_token`
        assert!(!al.is_denied("GITHUBTOKEN")); // sem underscore
    }

    #[test]
    fn allowed_does_not_collide_with_required() {
        // Se o usuário tentar adicionar `PATH` como ALLOWED, o
        // REQUIRED existente já cobre (mesma var). O `apply` não
        // duplica — a primeira iteração (REQUIRED) já inclui.
        let mut env = sample_env();
        let allowlist = EnvAllowlist::secure_default().with_allowed(["PATH"]);
        let filter = EnvFilter::new(allowlist);
        let out = filter.apply(&mut env).unwrap();
        // PATH aparece exatamente uma vez.
        let path_count = out.iter().filter(|(k, _)| k == "PATH").count();
        assert_eq!(path_count, 1);
    }
}
