//! `frederico-runtimes` — gerencia runtimes portáteis (Python + Node)
//! para os `exec.python` / `exec.node` da Fase 7 Etapa 4.
//!
//! Ver [`docs/architecture/runtimes-architecture.md`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/architecture/runtimes-architecture.md)
//! para o spec completo e [`ADR-0031 §D5`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/decisions/0031-fase-7-isolation-model-windows.md)
//! para o papel do `Runtime::env_vars` no `EnvAllowlist::REQUIRED`.
//!
//! ## Componentes
//!
//! - [`Runtime`] — trait abstrato de um runtime (Python/Node). Cada
//!   runtime concreto implementa os paths de binário, `env_vars` que
//!   entram no `EnvAllowlist::REQUIRED`, e o bootstrap idempotente.
//! - [`RuntimeRegistry`] — ponto único de acesso aos runtimes. Cria
//!   a partir de [`RuntimeConfig`], expõe `get`/`all`/`bootstrap_all`/
//!   `cleanup_old_versions`.
//! - [`PythonRuntime`] / [`NodeRuntime`] — implementações concretas
//!   para Python 3.12.4 e Node 20.16.0. Source URLs e SHA-256 pinned
//!   como `const` no código (v1; v2 vai para migration SQL).
//!
//! ## Localização
//!
//! Os runtimes ficam em `<install_root>/<id>/<version>/` (default
//! `%LOCALAPPDATA%\FredericoAIStudio\runtimes\` em Windows), separados
//! do workspace do sandbox (`<install_root>/../workspaces/<id>/`).
//!
//! ## v1 simplificações
//!
//! - Source URLs e SHA-256 pinned **no código** (const em `python.rs`/
//!   `node.rs`). Bump de versão requer commit + release. O spec
//!   (`runtimes-architecture.md` §"Decisões (a aprofundar antes da
//!   Etapa 3)") reserva migration SQL para v2.
//! - Sem `mirror_url` na `RuntimeConfig` (válvula de escape para
//!   ambiente corporativo). Adicionar quando UI de settings precisar.
//! - Sem virtualenv / pip-offline / npm-offline. `pip install` na
//!   Etapa 4 usa a rede do sandbox via proxy local (ADR-0033).
//! - Testes de bootstrap pulam se sem rede (mesma estratégia do
//!   `tree_kill.rs` da Etapa 2 — degradação controlada).

#![deny(unsafe_code)]
#![warn(missing_debug_implementations)]

mod bootstrap;
mod error;
mod manifest;
mod node;
mod python;
mod registry;
mod runtime;

pub use error::{BootstrapError, CleanupError, RegistryError, RuntimeError, ValidationError};
pub use manifest::Manifest;
pub use node::NodeRuntime;
pub use python::PythonRuntime;
pub use registry::{BootstrapReport, RuntimeConfig, RuntimeRegistry};
pub use runtime::{Runtime, RuntimeId};

// Re-exports internos (privados ao crate) usados pelos tests
// de integracao em `tests/`. `pub` aqui e' necessario pro
// integration test acessar; `#[doc(hidden)]` esconde da doc
// publica. Os items sao wrappers `pub` (com a logica
// `pub(crate)` por tras) — Rust nao deixa `pub use` reexport
// um item `pub(crate)` de outro modulo.
#[doc(hidden)]
pub mod __test_only {
    pub use crate::manifest::Manifest;

    // Wrappers publicos que delegam aos helpers `pub(crate)`.
    // Por que wrappers e nao `pub use`: Rust nao permite
    // `pub use foo::bar` se `bar` e' `pub(crate)` — o re-export
    // precisa ser `pub(crate)` tambem. Wrappers preservam o
    // encapsulamento.
    pub fn sha256_file(path: &std::path::Path) -> Result<String, crate::error::BootstrapError> {
        crate::bootstrap::sha256_file(path)
    }
    pub fn extract_zip(
        archive: &std::path::Path,
        dest: &std::path::Path,
    ) -> Result<usize, crate::error::BootstrapError> {
        crate::bootstrap::extract_zip(archive, dest)
    }
    pub fn download_archive_blocking(
        client: &reqwest::blocking::Client,
        id: &crate::runtime::RuntimeId,
        url: &str,
        dest: &std::path::Path,
        timeout: std::time::Duration,
    ) -> Result<(), crate::error::BootstrapError> {
        crate::bootstrap::download_archive_blocking(client, id, url, dest, timeout)
    }
}
