//! Traits de plataforma do Frederico IA Studio.
//!
//! Conforme [ADR-0003](../../decisions/0003-nucleo-desacoplado-da-casca-tauri.md),
//! o núcleo não importa `tauri` nem `windows`. As dependências de
//! plataforma passam por traits implementadas pela casca Tauri e por
//! fakes em testes.
//!
//! A Fase 1 entrega o esboço mínimo: o trait [`Platform`] com dois
//! componentes básicos ([`AppPaths`] e [`Clock`]). As demais
//! (credenciais, sandbox, notifier) entram nas fases 2-7.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use thiserror::Error;

use frederico_storage::AppPaths;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("operação de plataforma não suportada no ambiente atual: {0}")]
    Unsupported(&'static str),
}

/// Fonte de tempo injetável. A casca usa `SystemClock` (relógio do SO);
/// os testes usam `FakeClock` (avanço manual).
#[async_trait]
pub trait Clock: Send + Sync {
    async fn now_unix(&self) -> u64;
}

/// Implementação padrão do `Clock` usando o relógio do sistema.
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    async fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Trait de plataforma injetado pela casca. A Fase 1 só exige `AppPaths`
/// e `Clock`; credenciais, sandbox, paths adicionais e notifier virão
/// nas fases 2-7 (ver [`process-architecture.md`](https://github.com)).
#[async_trait]
pub trait Platform: Send + Sync {
    fn paths(&self) -> &dyn AppPaths;
    fn clock(&self) -> &dyn Clock;
}

pub mod fake {
    //! Implementações de plataforma para testes. Vivem no mesmo crate
    //! para garantir que o gate de pureza do núcleo (sem `tauri`, sem
    //! `windows`) não precise importar nada externo.

    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// `AppPaths` fake, sempre retorna um diretório em `temp`.
    pub struct FakePaths(pub PathBuf);

    impl AppPaths for FakePaths {
        fn database_path(&self) -> PathBuf {
            self.0.join("test.db")
        }
    }

    /// `Clock` fake, com avanço manual. Inicia em 0 e cresce em
    /// `advance()` ou a cada leitura se `auto` for true.
    pub struct FakeClock {
        now: AtomicU64,
        auto: bool,
    }

    impl FakeClock {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                now: AtomicU64::new(0),
                auto: false,
            })
        }

        pub fn with_auto() -> Arc<Self> {
            Arc::new(Self {
                now: AtomicU64::new(0),
                auto: true,
            })
        }

        pub fn advance(&self, seconds: u64) {
            self.now.fetch_add(seconds, Ordering::SeqCst);
        }

        pub fn set(&self, seconds: u64) {
            self.now.store(seconds, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Clock for FakeClock {
        async fn now_unix(&self) -> u64 {
            if self.auto {
                self.now.fetch_add(1, Ordering::SeqCst)
            } else {
                self.now.load(Ordering::SeqCst)
            }
        }
    }

    /// Plataforma fake que junta `FakePaths` e `FakeClock`.
    pub struct FakePlatform {
        paths: FakePaths,
        clock: Arc<FakeClock>,
    }

    impl FakePlatform {
        pub fn new(dir: PathBuf, clock: Arc<FakeClock>) -> Self {
            Self {
                paths: FakePaths(dir),
                clock,
            }
        }
    }

    #[async_trait]
    impl Platform for FakePlatform {
        fn paths(&self) -> &dyn AppPaths {
            &self.paths
        }
        fn clock(&self) -> &dyn Clock {
            self.clock.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[tokio::test]
    async fn fake_clock_starts_at_zero() {
        let c = FakeClock::new();
        assert_eq!(c.now_unix().await, 0);
    }

    #[tokio::test]
    async fn fake_clock_advance() {
        let c = FakeClock::new();
        c.advance(42);
        assert_eq!(c.now_unix().await, 42);
        c.advance(8);
        assert_eq!(c.now_unix().await, 50);
    }

    #[tokio::test]
    async fn fake_platform_returns_paths_and_clock() {
        let clock = FakeClock::new();
        let platform = FakePlatform::new(std::env::temp_dir(), clock.clone());
        assert!(platform
            .paths()
            .database_path()
            .to_string_lossy()
            .ends_with("test.db"));
        assert_eq!(platform.clock().now_unix().await, 0);
    }
}
