//! `Manifest` — registro do estado do bootstrap. Vive em
//! `<install_root>/<id>/<version>/manifest.json`.
//!
//! Schema v1 (campos são versionados; novos campos podem
//! ser adicionados sem bump de schema, mas renomeação
//! requer `v2`):
//!
//! ```json
//! {
//!   "runtime_id": "python-3.12.4",
//!   "version": "3.12.4",
//!   "source_url": "...",
//!   "source_sha256": "1d2b89c2...",
//!   "archive_size_bytes": 12345678,
//!   "bootstrap_at": "2026-08-08T12:34:56Z",
//!   "validated": true,
//!   "validation_output": "Python 3.12.4"
//! }
//! ```
//!
//! O `validated: true` só é setado depois que `validate()` passa.
//! Re-validar é caro (`<runtime> --version` spawna processo), então
//! o `bootstrap_if_needed` confia no manifest para cache hit
//! (segundo o spec §"Comportamento de bootstrap" passo 2b).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::RegistryError;
use crate::runtime::RuntimeId;

/// Schema v1 do manifest. Campos novos podem ser adicionados sem
/// bump (`#[serde(default)]`), mas renomeação requer `v2`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// ID canônico (ex.: `python-3.12.4`).
    pub runtime_id: RuntimeId,
    /// Versão (ex.: `3.12.4`).
    pub version: String,
    /// URL de onde o archive foi baixado. Pinned no source
    /// (v1 hard-coda; v2 lê do runtime.toml/SQL).
    pub source_url: String,
    /// SHA-256 hex lowercase do archive.
    pub source_sha256: String,
    /// Tamanho do archive em bytes (sanity check adicional).
    pub archive_size_bytes: u64,
    /// ISO 8601 (UTC) do momento do bootstrap.
    pub bootstrap_at: String,
    /// `true` se `validate()` (--version + sanity check) passou
    /// pelo menos uma vez após o bootstrap. Usado pelo
    /// `bootstrap_if_needed` para cache hit sem spawnar processo.
    pub validated: bool,
    /// Output da validação (ex.: `Python 3.12.4`). Informativo;
    /// logado em `tracing::info!` no startup.
    #[serde(default)]
    pub validation_output: String,
}

impl Manifest {
    /// Lê o manifest de `<target_dir>/manifest.json`. Retorna
    /// `None` se o arquivo não existe (cache miss). Falha de
    /// parse é propagada como `RegistryError` — caller deleta
    /// o `target_dir` e re-tenta do zero.
    pub fn read(target_dir: &Path) -> Result<Option<Self>, RegistryError> {
        let path = target_dir.join("manifest.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|e| RegistryError::InstallRootInaccessible {
            path: path.clone(),
            source: e,
        })?;
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|e| RegistryError::InstallRootInaccessible {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;
        Ok(Some(manifest))
    }

    /// Escreve o manifest em `<target_dir>/manifest.json`. Caller
    /// deve ter criado `target_dir` antes. Atomic via `tempfile
    /// + rename` (defesa contra crash mid-write).
    pub fn write(&self, target_dir: &Path) -> Result<(), RegistryError> {
        fs::create_dir_all(target_dir).map_err(|e| RegistryError::InstallRootInaccessible {
            path: target_dir.to_path_buf(),
            source: e,
        })?;
        let final_path = target_dir.join("manifest.json");
        let tmp_path = target_dir.join(format!("manifest.json.tmp.{}", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| {
            RegistryError::InstallRootInaccessible {
                path: final_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            }
        })?;
        fs::write(&tmp_path, &bytes).map_err(|e| RegistryError::InstallRootInaccessible {
            path: tmp_path.clone(),
            source: e,
        })?;
        fs::rename(&tmp_path, &final_path).map_err(|e| {
            // Cleanup best-effort do tmp.
            let _ = fs::remove_file(&tmp_path);
            RegistryError::InstallRootInaccessible {
                path: final_path.clone(),
                source: e,
            }
        })?;
        Ok(())
    }
}
