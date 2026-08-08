//! `NodeRuntime` — implementação concreta do trait `Runtime`
//! para Node 20.16.0 (build oficial Node.js, distribuição zip
//! portable).
//!
//! Source URL pinned em [`NODE_SOURCE_URL`], SHA-256 em
//! [`NODE_SHA256`]. Tamanho esperado em [`NODE_ARCHIVE_SIZE`].
//!
//! ## Layout do zip portable
//!
//! O zip do `node-v20.16.0-win-x64.zip` extrai com uma
//! subpasta `node-v20.16.0-win-x64/` no root. Após extração,
//! o `node.exe` está em `<home>/node-v20.16.0-win-x64/node.exe`.
//! O bootstrap extrai em `target_dir` e depois move
//! (ou cria link) para `target_dir` flat, ou usa
//! `target_dir/node-v20.16.0-win-x64/` como home.
//! **v1**: usa o layout nativo (com subpasta) — `home_dir` é
//! `target_dir/node-v20.16.0-win-x64/`. Caller ajusta `PATH`
//! para incluir essa subpasta.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bootstrap::{download_archive_blocking, extract_zip, sha256_file, validate_runtime};
use crate::error::{BootstrapError, ValidationError};
use crate::manifest::Manifest;
use crate::registry::RuntimeConfig;
use crate::runtime::{Runtime, RuntimeId};

/// Node 20.16.0 (win-x64, portable). Source URL pinned.
pub const NODE_VERSION: &str = "20.16.0";
pub const NODE_SOURCE_URL: &str = "https://nodejs.org/dist/v20.16.0/node-v20.16.0-win-x64.zip";
/// SHA-256 do archive (hex lowercase). Validado pós-download.
/// **Verificado em 2026-08-08** via download de teste (PowerShell
/// `Invoke-WebRequest` + `Get-FileHash`).
pub const NODE_SHA256: &str = "4e88373ac5ae859ad4d50cc3c5fa86eb3178d089b72e64c4dbe6eeac5d7b5979";
/// Tamanho esperado do archive (bytes).
/// Valor real (29_553_046) — ver `Content-Length` do nodejs.org
/// (verificado em 2026-08-08).
pub const NODE_ARCHIVE_SIZE: u64 = 29_553_046;

/// `NodeRuntime` — implementação concreta para Node 20.16.0.
///
/// **Layout diferente do Python**: o zip do Node extrai com
/// uma subpasta `node-v20.16.0-win-x64/` dentro do zip. O
/// `home_dir` do runtime é **essa subpasta**, não o
/// `target_dir` direto. Isso é o que o PATH injetado aponta.
pub struct NodeRuntime {
    id: RuntimeId,
    /// `install_root` armazenado pra helpers futuros.
    /// v1 só usa `target_dir` + `home_dir`.
    #[allow(dead_code)]
    install_root: PathBuf,
    /// `<install_root>/node-20.16.0/`. Aqui fica o archive
    /// extraído (e o `manifest.json`).
    target_dir: PathBuf,
    /// `<target_dir>/node-v20.16.0-win-x64/`. Aqui fica o
    /// `node.exe`, `node_modules/`, etc.
    home_dir: PathBuf,
    executable: PathBuf,
    /// `<home_dir>/node_modules/`. PathBuf owned (não
    /// referência a `home_dir.join()` temporário) — o trait
    /// `Runtime::site_packages` retorna `Option<&Path>` com
    /// lifetime de `self`.
    node_modules: PathBuf,
    /// `allow_download` da `RuntimeConfig`. Lido pelo
    /// `bootstrap_sync` pra decidir se baixa da rede ou
    /// retorna `Err(OfflineRequired)`.
    allow_download: bool,
    env_vars: Vec<(String, String)>,
}

impl std::fmt::Debug for NodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRuntime")
            .field("id", &self.id)
            .field("home_dir", &self.home_dir)
            .field("executable", &self.executable)
            .finish()
    }
}

impl NodeRuntime {
    /// Cria o runtime. **Não** faz bootstrap.
    pub fn new(config: &RuntimeConfig) -> Self {
        let id = RuntimeId::new(format!("node-{}", NODE_VERSION));
        let target_dir = config.install_root.join(id.as_str()).join(NODE_VERSION);
        // Subpasta que o zip do Node cria.
        let home_dir = target_dir.join(format!("node-v{}-win-x64", NODE_VERSION));
        let executable = home_dir.join("node.exe");

        // Env vars REQUIRED (ADR-0031 D5):
        // - PATH: home do node + System32
        // - NODE_PATH: home/node_modules (resolve `require` global)
        // - NODE_NO_WARNINGS: silencia deprecation noise
        let env_vars = vec![
            (
                "PATH".to_string(),
                format!("{};{}", home_dir.display(), r"%SystemRoot%\System32"),
            ),
            (
                "NODE_PATH".to_string(),
                home_dir.join("node_modules").display().to_string(),
            ),
            ("NODE_NO_WARNINGS".to_string(), "1".to_string()),
        ];

        Self {
            id,
            install_root: config.install_root.clone(),
            target_dir,
            home_dir: home_dir.clone(),
            executable,
            node_modules: home_dir.join("node_modules"),
            allow_download: config.allow_download,
            env_vars,
        }
    }
}

impl Runtime for NodeRuntime {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn executable(&self) -> &Path {
        &self.executable
    }

    fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    fn site_packages(&self) -> Option<&Path> {
        // Node não tem "site-packages" — usa `node_modules` por
        // projeto. `home_dir/node_modules/` é o global (NODE_PATH).
        Some(&self.node_modules)
    }

    fn env_vars(&self) -> &[(String, String)] {
        &self.env_vars
    }

    fn source_url(&self) -> &str {
        NODE_SOURCE_URL
    }

    fn expected_sha256(&self) -> &str {
        NODE_SHA256
    }

    fn expected_archive_size(&self) -> u64 {
        NODE_ARCHIVE_SIZE
    }

    fn download_timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    fn executable_path(&self, _install_root: &Path) -> PathBuf {
        self.executable.clone()
    }

    fn bootstrap_if_needed(&self) -> Result<(), BootstrapError> {
        tokio::task::block_in_place(|| self.bootstrap_sync())
    }

    fn validate(&self) -> Result<(), ValidationError> {
        self.validate_internal().map(|_| ())
    }
}

impl NodeRuntime {
    /// Validação que retorna o output (String) — usado pelo
    /// `bootstrap_sync` pra popular `manifest.validation_output`.
    fn validate_internal(&self) -> Result<String, ValidationError> {
        // `node --version` retorna "v20.16.0" em stdout.
        let output = validate_runtime(&self.executable, "v20.16")?;
        tracing::info!("[{}] validate OK: {output}", self.id);
        Ok(output)
    }
}

impl NodeRuntime {
    fn bootstrap_sync(&self) -> Result<(), BootstrapError> {
        // 1. Se home_dir existe (já extraído) e validated,
        //    cache hit.
        if self.home_dir.exists() && self.executable.exists() {
            if let Some(manifest) =
                Manifest::read(&self.target_dir).map_err(|e| BootstrapError::Io {
                    id: self.id.clone(),
                    source: std::io::Error::other(e.to_string()),
                })?
            {
                if manifest.validated
                    && manifest
                        .source_sha256
                        .eq_ignore_ascii_case(self.expected_sha256())
                {
                    tracing::info!("[{}] cache hit (validated=true, sha256 OK)", self.id);
                    return Ok(());
                }
            }
            // Cache corrompido: deleta e refaz.
            tracing::warn!("[{}] cache existe mas hash mismatch; refazendo", self.id);
            std::fs::remove_dir_all(&self.target_dir).map_err(|e| BootstrapError::Io {
                id: self.id.clone(),
                source: e,
            })?;
        }

        // 2. Sem cache e sem rede: OfflineRequired.
        if !self.allow_download {
            return Err(BootstrapError::OfflineRequired {
                id: self.id.clone(),
            });
        }

        // 3. Cria target_dir.
        std::fs::create_dir_all(&self.target_dir).map_err(|e| BootstrapError::Io {
            id: self.id.clone(),
            source: e,
        })?;

        // 3. Download.
        let archive_path = self.target_dir.join("archive.zip");
        let client = reqwest::blocking::Client::builder()
            .user_agent("FredericoIAStudio/0.1")
            .timeout(self.download_timeout())
            .build()
            .expect("reqwest::blocking::Client::builder default should never fail");
        download_archive_blocking(
            &client,
            &self.id,
            self.source_url(),
            &archive_path,
            self.download_timeout(),
        )?;

        // 4. Sanity check tamanho.
        let metadata = std::fs::metadata(&archive_path).map_err(|e| BootstrapError::Io {
            id: self.id.clone(),
            source: e,
        })?;
        if metadata.len() != self.expected_archive_size() {
            return Err(BootstrapError::ExtractFailed {
                id: self.id.clone(),
                message: format!(
                    "tamanho do archive divergente: esperado {}, obtido {}",
                    self.expected_archive_size(),
                    metadata.len()
                ),
            });
        }

        // 5. SHA-256.
        let actual_sha = sha256_file(&archive_path)?;
        if !actual_sha.eq_ignore_ascii_case(self.expected_sha256()) {
            return Err(BootstrapError::Sha256Mismatch {
                id: self.id.clone(),
                expected: self.expected_sha256().to_string(),
                actual: actual_sha,
            });
        }

        // 6. Extrai zip para target_dir (cria a subpasta
        //    `node-v20.16.0-win-x64/`).
        let extracted = extract_zip(&archive_path, &self.target_dir)?;
        tracing::info!("[{}] extraido {extracted} entries", self.id);

        // 7. Valida.
        let validation_output =
            self.validate_internal()
                .map_err(|e| BootstrapError::ValidationFailed {
                    id: self.id.clone(),
                    message: format!("{e}"),
                })?;

        // 8. Escreve manifest.
        let manifest = Manifest {
            runtime_id: self.id.clone(),
            version: self.version().to_string(),
            source_url: self.source_url().to_string(),
            source_sha256: actual_sha,
            archive_size_bytes: metadata.len(),
            bootstrap_at: chrono::Utc::now().to_rfc3339(),
            validated: true,
            validation_output,
        };
        manifest
            .write(&self.target_dir)
            .map_err(|e| BootstrapError::Io {
                id: self.id.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;

        // 9. Cleanup archive.
        let _ = std::fs::remove_file(&archive_path);

        Ok(())
    }
}
