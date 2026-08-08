//! `PythonRuntime` — implementação concreta do trait `Runtime`
//! para Python 3.12.4 (build oficial CPython, `Windows
//! embeddable package`).
//!
//! Source URL pinned em [`PYTHON_SOURCE_URL`], SHA-256 em
//! [`PYTHON_SHA256`]. Tamanho esperado em [`PYTHON_ARCHIVE_SIZE`].
//! Bump de versão = editar essas 3 consts + o `RuntimeId` + commit.
//!
//! ## Layout do embeddable package
//!
//! O zip do `python-3.12.4-embed-amd64.zip` extrai em uma
//! estrutura flat (sem subpasta). `python.exe` fica na raiz;
//! `python312.dll`, `python312.zip`, `Lib/`, `tcl/`, etc. também.
//!
//! ## `site-packages` (Python)
//!
//! Diferente do Node (que usa `node_modules` por projeto),
//! Python tem um `site-packages` global. No embeddable
//! package v3.12.4, o `site-packages` **não existe por default**
//! — é criado em runtime via `pip install --target <dir>`. A
//! Etapa 4 (exec python) cria o `site-packages` por workspace
//! (em `<workspace>/.frederico/python-site-packages/`) e injeta
//! via `PYTHONPATH`. Por isso `site_packages()` retorna `None`
//! aqui — caller adiciona via `PYTHONPATH` no `env_vars`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bootstrap::{download_archive_blocking, extract_zip, sha256_file, validate_runtime};
use crate::error::{BootstrapError, ValidationError};
use crate::manifest::Manifest;
use crate::registry::RuntimeConfig;
use crate::runtime::{Runtime, RuntimeId};

/// Python 3.12.4 (embed-amd64, Windows). Source URL pinned.
pub const PYTHON_VERSION: &str = "3.12.4";
pub const PYTHON_SOURCE_URL: &str =
    "https://www.python.org/ftp/python/3.12.4/python-3.12.4-embed-amd64.zip";
/// SHA-256 do archive (hex lowercase). Validado pós-download.
/// Pinned conforme spec §"Source URL pinned" — bump de versão
/// requer editar este valor **e** o `runtime.toml` (v2) **e**
/// migration `0039_runtimes_manifest.sql` (v2).
///
/// **Verificado em 2026-08-08** via download de teste (PowerShell
/// `Invoke-WebRequest` + `Get-FileHash`).
pub const PYTHON_SHA256: &str = "15fea3c9367653a85086fe37216b4d1a1c78688fa5e1587e1db0b0f658856564";
/// Tamanho esperado do archive (bytes) — sanity check adicional.
/// Valor real (11_065_736) — ver `Content-Length` do python.org
/// (verificado em 2026-08-08).
pub const PYTHON_ARCHIVE_SIZE: u64 = 11_065_736;

/// `PythonRuntime` — implementação concreta para Python 3.12.4.
pub struct PythonRuntime {
    id: RuntimeId,
    /// `install_root` armazenado pra helpers futuros
    /// (ex.: `cleanup_old_versions` resolve versões via
    /// `install_root/<id>/`). v1 só usa `home_dir`.
    #[allow(dead_code)]
    install_root: PathBuf,
    home_dir: PathBuf,
    executable: PathBuf,
    /// `allow_download` da `RuntimeConfig`. Lido pelo
    /// `bootstrap_sync` pra decidir se baixa da rede ou
    /// retorna `Err(OfflineRequired)`.
    allow_download: bool,
    /// Env vars que entram no `EnvAllowlist::REQUIRED` (ADR-0031 D5).
    /// A Etapa 4 consome via `runtime.env_vars().to_vec()`.
    /// `site-packages` é responsabilidade da Etapa 4 (per-workspace),
    /// então não vai aqui.
    env_vars: Vec<(String, String)>,
}

impl std::fmt::Debug for PythonRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonRuntime")
            .field("id", &self.id)
            .field("home_dir", &self.home_dir)
            .field("executable", &self.executable)
            .finish()
    }
}

impl PythonRuntime {
    /// Cria o runtime. **Não** faz bootstrap — `bootstrap_if_needed`
    /// é lazy (chamado pelo `RuntimeRegistry::bootstrap_all` ou
    /// pelo exec tool antes do primeiro `python --version`).
    pub fn new(config: &RuntimeConfig) -> Self {
        let id = RuntimeId::new(format!("python-{}", PYTHON_VERSION));
        let home_dir = config.install_root.join(id.as_str()).join(PYTHON_VERSION);
        let executable = home_dir.join("python.exe");

        // Env vars REQUIRED (ADR-0031 D5):
        // - PATH: runtime portátil (sem path do user)
        // - PYTHONHOME: onde o interpretador procura stdlib
        // - PYTHONIOENCODING: utf-8 (evita cp1252 default em Windows)
        // - PYTHONDONTWRITEBYTECODE: não poluir o workspace com .pyc
        // - PYTHONUNBUFFERED: stdout/stderr sem buffer (output em tempo real)
        //
        // NOTA: `site-packages` por workspace é responsabilidade
        // da Etapa 4 (não vai aqui). O caller adiciona via
        // `PYTHONPATH` se quiser.
        let env_vars = vec![
            (
                "PATH".to_string(),
                format!("{};{}", home_dir.display(), r"%SystemRoot%\System32"),
            ),
            ("PYTHONHOME".to_string(), home_dir.display().to_string()),
            ("PYTHONIOENCODING".to_string(), "utf-8".to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
        ];

        Self {
            id,
            install_root: config.install_root.clone(),
            home_dir,
            executable,
            allow_download: config.allow_download,
            env_vars,
        }
    }
}

impl Runtime for PythonRuntime {
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
        // Python embeddable v3.12.4 não tem `site-packages` por
        // default. A Etapa 4 cria per-workspace e injeta via
        // `PYTHONPATH`. Aqui, retornar `None` é o contrato.
        None
    }

    fn env_vars(&self) -> &[(String, String)] {
        &self.env_vars
    }

    fn source_url(&self) -> &str {
        PYTHON_SOURCE_URL
    }

    fn expected_sha256(&self) -> &str {
        PYTHON_SHA256
    }

    fn expected_archive_size(&self) -> u64 {
        PYTHON_ARCHIVE_SIZE
    }

    fn download_timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    fn executable_path(&self, _install_root: &Path) -> PathBuf {
        self.executable.clone()
    }

    fn bootstrap_if_needed(&self) -> Result<(), BootstrapError> {
        // Síncrono no v1 (download bloqueante). Etapa 4 pode
        // exigir async; este método é chamado do `bootstrap_all`
        // que é async via `tokio::task::spawn_blocking`.
        // Aqui dentro é OK ser sync — `reqwest::blocking`?
        // Não — v1 do crate tem `reqwest` async. Usar
        // `tokio::task::block_in_place` ou refatorar.
        //
        // Solução simples: implementar a lógica sync via
        // `reqwest::blocking` apenas aqui. v2 pode refatorar.
        // **NA VERDADE**: v1 não bloqueia — usa `tokio::fs` e
        // chama o `download_archive` (async) via
        // `tokio::runtime::Handle::current()` se há runtime
        // ativo, ou cria um local.
        //
        // Implementação real: delegamos a um helper async e
        // usamos `tokio::task::block_in_place` se estamos
        // dentro de um runtime. Para o bootstrap_all
        // (que é o caller típico), ele já é async — o
        // `block_in_place` é no-op fora de multi-thread runtime.
        tokio::task::block_in_place(|| self.bootstrap_sync())
    }

    fn validate(&self) -> Result<(), ValidationError> {
        // `python --version` retorna "Python 3.12.4" em stdout
        // (no v3.12 embeddable, o output vai pro stdout).
        self.validate_internal().map(|_| ())
    }
}

impl PythonRuntime {
    /// Validação que retorna o output (String) — usado pelo
    /// `bootstrap_sync` pra popular `manifest.validation_output`.
    fn validate_internal(&self) -> Result<String, ValidationError> {
        // `python --version` retorna "Python 3.12.4" em stdout
        // (no v3.12 embeddable, o output vai pro stdout).
        let output = validate_runtime(&self.executable, "Python 3.12")?;
        tracing::info!("[{}] validate OK: {output}", self.id);
        Ok(output)
    }
}

impl PythonRuntime {
    /// Bootstrap síncrono (chamado via `block_in_place`).
    ///
    /// **Importante**: este método usa `reqwest::blocking`
    /// apenas aqui, encapsulado, pra evitar expor blocking API
    /// no resto do crate.
    fn bootstrap_sync(&self) -> Result<(), BootstrapError> {
        // 1. Compute target_dir.
        let target_dir = &self.home_dir;

        // 2. Se target_dir existe, valida manifest + sha256.
        if target_dir.exists() {
            if let Some(manifest) = Manifest::read(target_dir).map_err(|e| BootstrapError::Io {
                id: self.id.clone(),
                source: std::io::Error::other(e.to_string()),
            })? {
                // Cache hit? Checar sha256 do archive armazenado
                // (se ainda existe) ou apenas o validated flag.
                if manifest.validated
                    && manifest
                        .source_sha256
                        .eq_ignore_ascii_case(self.expected_sha256())
                {
                    tracing::info!("[{}] cache hit (validated=true, sha256 OK)", self.id);
                    return Ok(());
                }
                tracing::warn!(
                    "[{}] cache existe mas hash mismatch ou validated=false; refazendo",
                    self.id
                );
                // Delete e fall through.
                std::fs::remove_dir_all(target_dir).map_err(|e| BootstrapError::Io {
                    id: self.id.clone(),
                    source: e,
                })?;
            }
        }

        // 3. Sem cache e sem rede: falha com OfflineRequired
        //    (regra D2 do air-gapped do spec).
        if !self.allow_download {
            return Err(BootstrapError::OfflineRequired {
                id: self.id.clone(),
            });
        }

        // 4. Cria target_dir.
        std::fs::create_dir_all(target_dir).map_err(|e| BootstrapError::Io {
            id: self.id.clone(),
            source: e,
        })?;

        // 4. Download (via reqwest::blocking dentro de
        //    block_in_place).
        let archive_path = target_dir.join("archive.zip");
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

        // 5. Sanity check: tamanho do archive.
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

        // 6. SHA-256 do archive.
        let actual_sha = sha256_file(&archive_path)?;
        if !actual_sha.eq_ignore_ascii_case(self.expected_sha256()) {
            return Err(BootstrapError::Sha256Mismatch {
                id: self.id.clone(),
                expected: self.expected_sha256().to_string(),
                actual: actual_sha,
            });
        }

        // 7. Extrai zip para target_dir.
        let extracted = extract_zip(&archive_path, target_dir)?;
        tracing::info!("[{}] extraido {extracted} entries", self.id);

        // 8. Valida (`<runtime> --version`).
        let validation_output =
            self.validate_internal()
                .map_err(|e| BootstrapError::ValidationFailed {
                    id: self.id.clone(),
                    message: format!("{e}"),
                })?;

        // 9. Escreve manifest.
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
        manifest.write(target_dir).map_err(|e| BootstrapError::Io {
            id: self.id.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;

        // 10. Cleanup: deleta o archive (não precisa mais).
        let _ = std::fs::remove_file(&archive_path);

        Ok(())
    }
}
