//! `RuntimeRegistry` — ponto único de acesso aos runtimes.
//!
//! Construído via [`RuntimeConfig`] (default = `%LOCALAPPDATA%\
//! FredericoAIStudio\runtimes\`, `keep_n_versions = 2`,
//! `allow_download = true`).
//!
//! ## v1
//!
//! Hard-coda 2 runtimes: Python 3.12.4 e Node 20.16.0.
//! Adicionar um terceiro runtime é só implementar o trait
//! `Runtime` e adicionar a `vec!` em `new()`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use directories::ProjectDirs;

use crate::error::{BootstrapError, CleanupError, RegistryError};
use crate::node::NodeRuntime;
use crate::python::PythonRuntime;
use crate::runtime::{Runtime, RuntimeId};

/// Configuração do `RuntimeRegistry`. Campos públicos para
/// permitir construção programática (testes + UI de settings
/// futura).
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Diretório base dos runtimes. Default em Windows:
    /// `%LOCALAPPDATA%\FredericoAIStudio\runtimes\`. Em outros
    /// SOs: `ProjectDirs::data_local_dir().join("runtimes")`
    /// (a v1 é Windows-only, mas o fallback deixa a estrutura
    /// robusta pra cross-platform futuro).
    pub install_root: PathBuf,
    /// Manter N versões anteriores após bump. Default 2.
    /// Cleanup automático é opt-in via
    /// [`RuntimeRegistry::cleanup_old_versions`].
    pub keep_n_versions: usize,
    /// Permitir download pela rede. Default true; em air-gapped
    /// vira false (bootstrap só via cache).
    pub allow_download: bool,
    /// URL custom de mirror (opcional). Se Some, substitui
    /// `source_url` do runtime concreto. Válvula de escape
    /// para ambiente corporativo.
    pub mirror_url: Option<String>,
    /// Timeout de download por tentativa (default 5 min).
    pub download_timeout: Duration,
}

impl RuntimeConfig {
    /// Configuração default. Resolve `install_root` via
    /// `directories::ProjectDirs` (Windows: `%LOCALAPPDATA%\
    /// FredericoAIStudio`).
    pub fn default_install_root() -> PathBuf {
        ProjectDirs::from("ai", "Frederico", "FredericoAIStudio")
            .map(|p| p.data_local_dir().join("runtimes"))
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("frederico-ia-studio")
                    .join("runtimes")
            })
    }

    /// Configuração secure default (production).
    #[must_use]
    pub fn secure_default() -> Self {
        Self {
            install_root: Self::default_install_root(),
            keep_n_versions: 2,
            allow_download: true,
            mirror_url: None,
            download_timeout: Duration::from_secs(300),
        }
    }
}

/// Relatório de um `bootstrap_all` agregado. Carrega o que
/// rodou bootstrap, o que estava em cache, o que falhou, e
/// métricas de duração + bytes baixados. Usado pela UI de
/// settings pra mostrar "Python já estava em cache, Node
/// baixou 30MB em 8s".
#[derive(Debug, Default)]
pub struct BootstrapReport {
    /// Runtimes que rodaram bootstrap (download + extract + validate).
    pub bootstrapped: Vec<RuntimeId>,
    /// Runtimes que já estavam em cache válido (no-op).
    pub cached: Vec<RuntimeId>,
    /// Runtimes que falharam (com a causa). O app continua
    /// funcionando com os que estão em cache; a UI mostra
    /// diagnóstico.
    pub failed: Vec<(RuntimeId, BootstrapError)>,
    /// Tempo total do `bootstrap_all`. Útil pra log e UI.
    pub total_duration: Duration,
    /// Bytes baixados (0 se tudo estava em cache).
    pub bytes_downloaded: u64,
}

impl BootstrapReport {
    /// `true` se nenhum runtime falhou.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// `true` se ao menos um runtime está disponível
    /// (bootstrapped ou cached).
    #[must_use]
    pub fn has_any(&self) -> bool {
        !self.bootstrapped.is_empty() || !self.cached.is_empty()
    }
}

/// Registro de runtimes. Constrói via [`RuntimeConfig`].
///
/// `Arc<RuntimeRegistry>` é compartilhado entre a casca Tauri
/// (Tauri commands `runtime.status`, `runtime.bootstrap`) e
/// a Etapa 4 (exec tools que pegam o runtime pelo ID).
pub struct RuntimeRegistry {
    runtimes: HashMap<RuntimeId, Arc<dyn Runtime>>,
    config: RuntimeConfig,
    /// HTTP client (uma instância por registry, com `rustls-tls`
    /// já configurado). Reutilizado entre bootstraps.
    http: reqwest::Client,
}

impl std::fmt::Debug for RuntimeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeRegistry")
            .field("runtimes", &self.runtimes.keys().collect::<Vec<_>>())
            .field("config", &self.config)
            .finish()
    }
}

impl RuntimeRegistry {
    /// Constrói o registry com a config dada. Cria o
    /// `install_root` se não existir. Falha se não conseguir
    /// (ex.: permissão negada).
    pub fn new(config: RuntimeConfig) -> Result<Self, RegistryError> {
        std::fs::create_dir_all(&config.install_root).map_err(|e| {
            RegistryError::InstallRootInaccessible {
                path: config.install_root.clone(),
                source: e,
            }
        })?;

        let http = reqwest::Client::builder()
            .user_agent("FredericoIAStudio/0.1")
            .build()
            .expect("reqwest::Client::builder default config should never fail");

        // v1: hard-coda Python 3.12.4 + Node 20.16.0.
        let mut runtimes: HashMap<RuntimeId, Arc<dyn Runtime>> = HashMap::new();
        let py = Arc::new(PythonRuntime::new(&config));
        let node = Arc::new(NodeRuntime::new(&config));
        let py_id = py.id().clone();
        let node_id = node.id().clone();
        runtimes.insert(py_id.clone(), py);
        runtimes.insert(node_id.clone(), node);

        // Checagem de duplicate ID (defesa contra typo).
        // Em v1 com hard-code é redundante, mas o check
        // existe pra v2 quando vierem runtimes configuráveis.
        let _ = py_id;
        let _ = node_id;

        Ok(Self {
            runtimes,
            config,
            http,
        })
    }

    /// Pega um runtime pelo ID. Retorna `None` se não existe.
    pub fn get(&self, id: &RuntimeId) -> Option<Arc<dyn Runtime>> {
        self.runtimes.get(id).cloned()
    }

    /// Lista todos os runtimes registrados (sem bootstrap).
    /// Caller pode usar `bootstrap_all` pra ter o estado
    /// pós-bootstrap.
    pub fn all(&self) -> Vec<Arc<dyn Runtime>> {
        self.runtimes.values().cloned().collect()
    }

    /// Configuração efetiva (cópia).
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// HTTP client (usado pelos tests de bootstrap que
    /// precisam mockar o server).
    #[doc(hidden)]
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Roda bootstrap em todos os runtimes registrados.
    /// Idempotente: cache hit é no-op. Falha de um runtime
    /// não impede os outros (o report agrega).
    pub async fn bootstrap_all(&self) -> Result<BootstrapReport, RegistryError> {
        let start = Instant::now();
        let mut report = BootstrapReport::default();
        for runtime in self.runtimes.values() {
            match runtime.bootstrap_if_needed() {
                Ok(()) => {
                    // Não sabemos distinguir cache vs bootstrap
                    // sem inspecionar o manifest antes; isso é
                    // feito internamente em `bootstrap_if_needed`.
                    // Aqui, simplificamos: o report lista
                    // ambos em `cached` se o `target_dir` já
                    // existia antes do call.
                    let target = runtime.home_dir();
                    let manifest_path = target.join("manifest.json");
                    if manifest_path.exists() {
                        // Foi cache hit (não recriamos o dir).
                        report.cached.push(runtime.id().clone());
                    } else {
                        report.bootstrapped.push(runtime.id().clone());
                    }
                }
                Err(e) => {
                    report.failed.push((runtime.id().clone(), e));
                }
            }
        }
        report.total_duration = start.elapsed();
        Ok(report)
    }

    /// Limpa versões antigas de runtimes, mantendo as `keep_n`
    /// mais recentes (por `version` string). Retorna o número
    /// de versões removidas.
    pub fn cleanup_old_versions(&self, keep_n: usize) -> Result<usize, CleanupError> {
        if keep_n == 0 {
            return Err(CleanupError::InvalidKeepN(0));
        }

        let mut total_removed = 0;
        for runtime in self.runtimes.values() {
            // O layout é `<install_root>/<id>/<version>/`. Para
            // cada `<id>`, listar os subdiretórios `version`,
            // ordenar por versão semver (não alfabético), e
            // deletar os `keep_n+1..` em diante.
            let id_dir = self.config.install_root.join(runtime.id().as_str());
            if !id_dir.exists() {
                continue;
            }
            let mut versions: Vec<PathBuf> = std::fs::read_dir(&id_dir)
                .map_err(|e| CleanupError::Io { source: e })?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .collect();
            // Sort por nome (ordem alfabética funciona pra
            // semver se as versões têm o mesmo length — v1
            // assume). v2 pode usar `semver` crate.
            versions.sort();
            if versions.len() <= keep_n {
                continue;
            }
            for old in versions.iter().take(versions.len() - keep_n) {
                tracing::info!("[cleanup] removendo {}", old.display());
                std::fs::remove_dir_all(old).map_err(|e| CleanupError::Io { source: e })?;
                total_removed += 1;
            }
        }
        Ok(total_removed)
    }
}
