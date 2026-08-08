//! Erros do `frederico-runtimes`. Cada variante carrega o
//! contexto necessário pro caller montar mensagem PT-BR ou
//! tomar decisão automática (ex.: `BootstrapError::OfflineRequired`
//! é recoverable: caller continua com runtimes em cache).

use std::path::PathBuf;
use thiserror::Error;

use crate::runtime::RuntimeId;

/// Erro genérico de runtime. Usado por métodos trait que não
/// precisam expor a causa específica (ex.: `Runtime::validate`).
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Binário não encontrado no `install_root/<id>/<version>/`.
    #[error("runtime {id} nao encontrado em {path}; faca bootstrap primeiro")]
    NotFound { id: RuntimeId, path: PathBuf },
    /// Validação `--version` ou sanity check falhou.
    #[error("validacao do runtime {id} falhou: {message}")]
    ValidationFailed { id: RuntimeId, message: String },
}

/// Erro de bootstrap. Carrega a causa (rede, hash, extração, etc.)
/// e o `RuntimeId` afetado. O `BootstrapReport` agrega
/// múltiplos erros (um por runtime).
#[derive(Debug, Error)]
pub enum BootstrapError {
    /// Falha de rede ao baixar o archive. Inclui o `source_url` e
    /// o número de tentativas (retry exponencial 1s/2s/4s).
    #[error("download de {id} falhou apos {attempts} tentativa(s) ({url}): {message}")]
    DownloadFailed {
        id: RuntimeId,
        url: String,
        attempts: u32,
        message: String,
    },
    /// SHA-256 do archive baixado não bate com o pinned. Defesa
    /// contra MITM. Rejeita o archive; `bootstrap_if_needed` deleta
    /// o `target_dir` e tenta de novo no próximo call.
    #[error("SHA-256 mismatch para {id}: esperado {expected}, obtido {actual}")]
    Sha256Mismatch {
        id: RuntimeId,
        expected: String,
        actual: String,
    },
    /// Falha ao extrair o zip (corrompido, formato inesperado).
    #[error("extracao de {id} falhou: {message}")]
    ExtractFailed { id: RuntimeId, message: String },
    /// `allow_download = false` e cache vazio. Recoverable: o
    /// caller continua com os runtimes que estão em cache.
    #[error("offline required para {id} (sem cache, allow_download=false)")]
    OfflineRequired { id: RuntimeId },
    /// Validação pós-extração falhou (`<runtime> --version` ou
    /// sanity check retornaram erro). O `target_dir` é deletado
    /// antes de propagar — próximo call tenta de novo do zero.
    #[error("validacao pos-bootstrap de {id} falhou: {message}")]
    ValidationFailed { id: RuntimeId, message: String },
    /// I/O error genérico (mkdir, write, etc.).
    #[error("I/O error durante bootstrap de {id}: {source}")]
    Io {
        id: RuntimeId,
        #[source]
        source: std::io::Error,
    },
}

/// Erro de `cleanup_old_versions`. Típico: caller pediu
/// `keep_n = 0` (recusado) ou algum diretório está locked.
#[derive(Debug, Error)]
pub enum CleanupError {
    #[error("keep_n deve ser >= 1 (recebido {0})")]
    InvalidKeepN(usize),
    #[error("I/O error durante cleanup: {source}")]
    Io {
        #[source]
        source: std::io::Error,
    },
}

/// Erro de `RuntimeRegistry::new` (config inválida, install_root
/// inacessível, etc.).
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("install_root nao pode ser criado: {path}: {source}")]
    InstallRootInaccessible {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("runtime id duplicado no manifest: {0}")]
    DuplicateRuntimeId(RuntimeId),
}

/// Erro de validação (chamada direta, fora do bootstrap).
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("`{binary}` retornou exit code {code} (stdout: {stdout}, stderr: {stderr})")]
    NonZeroExit {
        binary: PathBuf,
        code: i32,
        stdout: String,
        stderr: String,
    },
    #[error("`{binary}` nao pode ser executado: {source}")]
    SpawnFailed {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("output de `{binary}` nao parseou: {message}")]
    ParseFailed { binary: PathBuf, message: String },
}
