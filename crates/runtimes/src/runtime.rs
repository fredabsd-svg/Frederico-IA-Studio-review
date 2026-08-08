//! `Runtime` trait + `RuntimeId` + tipos auxiliares.
//!
//! Cada runtime concreto (Python, Node) implementa o trait. O
//! trait é o ponto único de interação com a Etapa 4 (exec tools).
//!
//! ## Por que trait e não enum fechado
//!
//! A Etapa 3 v1 implementa Python + Node. A Etapa 4 (exec tools)
//! consome o trait via `RuntimeRegistry::get(id)` — caller recebe
//! `Arc<dyn Runtime>`. Adicionar Ruby / Go / etc. no futuro é só
//! implementar o trait; nenhum caller muda.
//!
//! ## v1: SHA-256 pinned como `const` no código
//!
//! O spec (`runtimes-architecture.md` §"Decisões a aprofundar")
//! reserva `runtime.toml` em disco + migration SQL para v2. A v1
//! hard-coda o URL + SHA-256 como `const` no `python.rs`/`node.rs`
//! e expõe via `Runtime::source_url` / `Runtime::expected_sha256`.
//! Bump de versão = commit + release (mesma regra do `no interruptor`
//! do §19.6 do PROMPT MESTRE).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

/// Identificador do runtime. Formato: `<name>-<version>` (ex.:
/// `python-3.12.4`, `node-20.16.0`). Usado como chave do
/// `RuntimeRegistry` e como nome do diretório `<install_root>/<id>/`.
///
/// **Não-opaco** (string nova-typed): permite interop com `serde`,
/// `clap`, e logs. A newtype garante que não confunde com
/// `&str`/`String` em APIs internas.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeId(String);

impl RuntimeId {
    /// Cria um `RuntimeId` a partir de qualquer string. Caller
    /// é responsável por validar o formato (helper `parse_strict`
    /// se necessário). O `From<&str>` é fornecido por conveniência.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Parse estrito: aceita só `^[a-z0-9]+-[0-9]+\.[0-9]+\.[0-9]+$`
    /// (lowercase, sem path injection). Usado em boundaries
    /// externas (config do usuário).
    pub fn parse_strict(s: &str) -> Result<Self, RuntimeError> {
        let valid = !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let has_dash_version = s
            .rsplit_once('-')
            .map(|(_, v)| {
                v.split('.').count() == 3
                    && v.split('.')
                        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            })
            .unwrap_or(false);
        if valid && has_dash_version {
            Ok(Self(s.to_string()))
        } else {
            Err(RuntimeError::ValidationFailed {
                id: Self(s.to_string()),
                message: format!(
                    "formato invalido: esperado '<name>-<major>.<minor>.<patch>' (lowercase, sem path injection), recebeu {s:?}"
                ),
            })
        }
    }

    /// `&str` da string subjacente. Usado para formatar logs
    /// e para construir paths de diretório.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Versão: a parte depois do último `-`. Para `python-3.12.4`
    /// retorna `3.12.4`.
    pub fn version(&self) -> &str {
        self.0.rsplit_once('-').map(|(_, v)| v).unwrap_or(&self.0)
    }

    /// Nome: a parte antes do último `-`. Para `python-3.12.4`
    /// retorna `python`.
    pub fn name(&self) -> &str {
        self.0.rsplit_once('-').map(|(n, _)| n).unwrap_or(&self.0)
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for RuntimeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for RuntimeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for RuntimeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Trait `Runtime` — contrato único para Python/Node (e futuros
/// runtimes). Cada implementação concreta mora em `python.rs` /
/// `node.rs` e declara suas `const SOURCE_URL` + `EXPECTED_SHA256`.
///
/// **Send + Sync** para que `RuntimeRegistry` possa compartilhar
/// `Arc<dyn Runtime>` entre tasks (a Etapa 4 vai spawnar `python`
/// em runtime async, e o registry pode ser compartilhado entre
/// múltiplos exec tools).
pub trait Runtime: Send + Sync {
    /// ID canônico (ex.: `python-3.12.4`).
    fn id(&self) -> &RuntimeId;

    /// Versão string (ex.: `3.12.4`). Helper de `id()`.
    fn version(&self) -> &str {
        self.id().version()
    }

    /// Path absoluto pro executável (python.exe, node.exe). Já
    /// bootstrapado (`bootstrap_if_needed` rodou).
    fn executable(&self) -> &Path;

    /// Diretório raiz do runtime (onde fica `python.exe`,
    /// `python312.dll`, `Lib/`, etc.). Setado em `new()` a
    /// partir de `install_root/<id>/<version>/`.
    fn home_dir(&self) -> &Path;

    /// `site-packages` (Python) — `Some(python_dir/Lib/site-packages)`.
    /// `None` para Node (Node usa `node_modules`, sem path global).
    /// Usado pela Etapa 4 pra popular `PYTHONPATH`.
    fn site_packages(&self) -> Option<&Path>;

    /// `env_vars` que entram no `EnvAllowlist::REQUIRED` (ADR-0031 D5):
    /// `PATH` apontando pro runtime portátil + `PYTHONHOME`/
    /// `PYTHONPATH`/`NODE_PATH` específicos do runtime.
    /// A Etapa 4 consome via `runtime.env_vars().to_vec()`.
    fn env_vars(&self) -> &[(String, String)];

    /// URL do archive para download. Pinned em `const` na
    /// implementação concreta.
    fn source_url(&self) -> &str;

    /// SHA-256 esperado do archive (hex lowercase, 64 chars).
    /// `bootstrap_if_needed` valida o hash do arquivo baixado
    /// contra este valor — mismatch deleta + re-download.
    fn expected_sha256(&self) -> &str;

    /// Tamanho esperado do archive em bytes (sanity check
    /// adicional antes de extrair; zip corrompido geralmente
    /// tem tamanho diferente).
    fn expected_archive_size(&self) -> u64;

    /// Timeout de download (default 5 min).
    fn download_timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    /// Bootstrap idempotente. Se o cache em
    /// `<install_root>/<id>/<version>/` está válido (manifest
    /// presente, sha256 OK), é no-op. Senão: download + extract
    /// + validate. Sempre idempotente.
    fn bootstrap_if_needed(&self) -> Result<(), crate::error::BootstrapError>;

    /// Validação: roda `<runtime> --version` + sanity check
    /// (Python: `import sys; assert sys.version_info >= (3, 12)`;
    /// Node: `console.log(process.versions.node)`). Falha aborta
    /// o bootstrap (`target_dir` é deletado).
    fn validate(&self) -> Result<(), crate::error::ValidationError>;

    /// Path completo do binário (`<home_dir>/<exe_name>`).
    /// Helper usado por `executable()` e pelo validator.
    fn executable_path(&self, install_root: &Path) -> PathBuf;
}
