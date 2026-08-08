//! Lógica de bootstrap: download, validação SHA-256, extração
//! de zip, validação do binário via `--version`.
//!
//! Funções são `pub(crate)` porque são testadas em
//! `tests/*_bootstrap.rs` via `__test_only` re-export em `lib.rs`.
//! API pública é apenas o trait `Runtime::bootstrap_if_needed`.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::{BootstrapError, ValidationError};
use crate::runtime::RuntimeId;

/// Calcula SHA-256 de um arquivo. Helper usado pela validação
/// pós-download e pelo teste `manifest_corruption`.
pub(crate) fn sha256_file(path: &Path) -> Result<String, BootstrapError> {
    let mut file = File::open(path).map_err(|e| BootstrapError::Io {
        id: RuntimeId::from("?"),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| BootstrapError::Io {
            id: RuntimeId::from("?"),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(format!("{:x}", digest))
}

/// Extrai um zip para `dest_dir`. **Não** checa path traversal
/// (os archives pinned são de fontes confiáveis — python.org
/// e nodejs.org — mas se isso virar problema, o teste
/// `manifest_corruption` pode crescer). Retorna o número de
/// entries extraídas.
pub(crate) fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<usize, BootstrapError> {
    let file = File::open(archive_path).map_err(|e| BootstrapError::ExtractFailed {
        id: RuntimeId::from("?"),
        message: format!("open archive: {e}"),
    })?;
    let mut archive = ZipArchive::new(file).map_err(|e| BootstrapError::ExtractFailed {
        id: RuntimeId::from("?"),
        message: format!("zip open: {e}"),
    })?;

    fs::create_dir_all(dest_dir).map_err(|e| BootstrapError::ExtractFailed {
        id: RuntimeId::from("?"),
        message: format!("mkdir dest: {e}"),
    })?;

    let count = archive.len();
    for i in 0..count {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| BootstrapError::ExtractFailed {
                id: RuntimeId::from("?"),
                message: format!("entry {i}: {e}"),
            })?;
        let entry_path = match entry.enclosed_name() {
            Some(p) => dest_dir.join(p),
            None => {
                // Path inseguro (path traversal). Skip silenciosamente
                // — archives pinned não têm isso, mas defesa em
                // profundidade.
                tracing::warn!("zip entry com path inseguro, pulando");
                continue;
            }
        };

        if entry.is_dir() {
            fs::create_dir_all(&entry_path).map_err(|e| BootstrapError::ExtractFailed {
                id: RuntimeId::from("?"),
                message: format!("mkdir entry: {e}"),
            })?;
        } else {
            if let Some(parent) = entry_path.parent() {
                fs::create_dir_all(parent).map_err(|e| BootstrapError::ExtractFailed {
                    id: RuntimeId::from("?"),
                    message: format!("mkdir parent: {e}"),
                })?;
            }
            let mut out = File::create(&entry_path).map_err(|e| BootstrapError::ExtractFailed {
                id: RuntimeId::from("?"),
                message: format!("create file: {e}"),
            })?;
            io::copy(&mut entry, &mut out).map_err(|e| BootstrapError::ExtractFailed {
                id: RuntimeId::from("?"),
                message: format!("copy entry: {e}"),
            })?;
        }
    }
    Ok(count)
}

/// Helper para testes: valida que um `Runtime::validate()`
/// customizado consegue rodar. Implementações concretas
/// (Python/Node) fornecem o seu próprio validate via spawn
/// do `<runtime> --version` + sanity check.
pub(crate) fn validate_runtime(
    binary: &Path,
    version_check: &str,
) -> Result<String, ValidationError> {
    use std::process::Command;

    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|e| ValidationError::SpawnFailed {
            binary: binary.to_path_buf(),
            source: e,
        })?;

    if !output.status.success() {
        return Err(ValidationError::NonZeroExit {
            binary: binary.to_path_buf(),
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");

    if !combined.contains(version_check) {
        return Err(ValidationError::ParseFailed {
            binary: binary.to_path_buf(),
            message: format!("output nao contem {version_check:?}: {combined:?}"),
        });
    }

    Ok(combined.trim().to_string())
}

/// Helper para testes: cria o diretório `dest_dir` (e parents).
/// Retorna o path canônico.
#[allow(dead_code)]
pub(crate) fn ensure_dir(dest_dir: &Path) -> Result<PathBuf, io::Error> {
    fs::create_dir_all(dest_dir)?;
    Ok(dest_dir.to_path_buf())
}

/// Versão **blocking** do `download_archive` (chamada via
/// `tokio::task::block_in_place` do `bootstrap_if_needed`).
/// Compartilhada entre `python.rs` e `node.rs` — evita
/// duplicar a lógica de retry.
///
/// **Por que blocking e não async**: o `bootstrap_if_needed`
/// é sync (a trait `Runtime` não é async). Usar
/// `reqwest::blocking` aqui é seguro porque o caller já está
/// dentro de `block_in_place` (multi-thread runtime) ou em
/// um contexto onde o I/O blocking é aceitável (testes,
/// scripts, app startup).
pub(crate) fn download_archive_blocking(
    client: &reqwest::blocking::Client,
    id: &RuntimeId,
    source_url: &str,
    dest_path: &Path,
    download_timeout: Duration,
) -> Result<(), BootstrapError> {
    let backoff = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ];
    let mut last_err: Option<String> = None;

    for (attempt, delay) in backoff.iter().enumerate() {
        if attempt > 0 {
            tracing::info!(
                "[{id}] retry {}/{} apos {delay:?}",
                attempt + 1,
                backoff.len()
            );
            std::thread::sleep(*delay);
        }

        let result = (|| -> Result<(), String> {
            let mut response = client
                .get(source_url)
                .header("User-Agent", "FredericoIAStudio/0.1")
                .timeout(download_timeout)
                .send()
                .map_err(|e| format!("send: {e}"))?;
            if !response.status().is_success() {
                return Err(format!("HTTP {}", response.status()));
            }
            let mut bytes = Vec::new();
            response
                .copy_to(&mut bytes)
                .map_err(|e| format!("read body: {e}"))?;
            // Atomic write: tmp + rename.
            let tmp = dest_path.with_extension(format!(
                "{}.tmp.{}",
                dest_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("download"),
                uuid::Uuid::new_v4()
            ));
            fs::write(&tmp, &bytes).map_err(|e| format!("write tmp: {e}"))?;
            fs::rename(&tmp, dest_path).map_err(|e| format!("rename: {e}"))?;
            Ok(())
        })();

        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!("[{id}] download attempt {} failed: {e}", attempt + 1);
                last_err = Some(e);
            }
        }
    }

    Err(BootstrapError::DownloadFailed {
        id: id.clone(),
        url: source_url.to_string(),
        attempts: backoff.len() as u32,
        message: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}
