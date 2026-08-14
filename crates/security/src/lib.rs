//! Traits de plataforma do Frederico IA Studio.
//!
//! Conforme [ADR-0003](../../decisions/0003-nucleo-desacoplado-da-casca-tauri.md),
//! o núcleo não importa `tauri` nem `windows`. As dependências de
//! plataforma passam por traits implementadas pela casca Tauri e por
//! fakes em testes.
//!
//! A Fase 1 entregou o esboço mínimo: o trait [`Platform`] com
//! [`AppPaths`] e [`Clock`]. A Fase 2 adiciona [`CredentialStore`]
//! (DPAPI / Windows Credential Manager; ver
//! [ADR-0007](../../decisions/0007-credential-store-trait.md)). As
//! demais (`Sandbox`, `Notifier`) entram nas fases 3-7.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use secrecy::SecretString;
use thiserror::Error;

use frederico_core::ProviderId;
use frederico_storage::AppPaths;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("operação de plataforma não suportada no ambiente atual: {0}")]
    Unsupported(&'static str),
    #[error("erro do cofre de credenciais: {0}")]
    CredentialStore(String),
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

/// Cofre de credenciais. O motor lê a chave de um provedor via este
/// trait — **nunca** via env, dotenv, ou config em arquivo. Ver
/// [ADR-0007](../../decisions/0007-credential-store-trait.md) §Decisão.
///
/// A implementação de produção em Windows usa DPAPI / Windows
/// Credential Manager (gateada em `#[cfg(windows)]` no submódulo
/// [`windows`]). Os testes usam [`fake::FakeCredentialStore`].
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn get(&self, provider: &ProviderId) -> Result<Option<SecretString>, SecurityError>;
    async fn set(&self, provider: &ProviderId, value: &SecretString) -> Result<(), SecurityError>;
    async fn delete(&self, provider: &ProviderId) -> Result<(), SecurityError>;
    async fn list_providers(&self) -> Result<Vec<ProviderId>, SecurityError>;
}

/// Trait de plataforma injetado pela casca. Carrega os três
/// componentes da Fase 1+2: paths, clock, e credenciais. As demais
/// (sandbox, notifier) virão nas fases 3-7 (ver
/// [`process-architecture.md`](https://github.com)).
#[async_trait]
pub trait Platform: Send + Sync {
    fn paths(&self) -> &dyn AppPaths;
    fn clock(&self) -> &dyn Clock;
    fn credentials(&self) -> &dyn CredentialStore;
}

pub mod env_filter;
pub mod exec_patterns;
pub mod fake;
pub mod jail;
pub mod network;
pub mod network_audit_sink;
pub mod raw_child;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use secrecy::ExposeSecret;

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
    async fn fake_platform_returns_paths_clock_and_credentials() {
        let clock = FakeClock::new();
        let platform = FakePlatform::new(std::env::temp_dir(), clock.clone());
        assert!(platform
            .paths()
            .database_path()
            .to_string_lossy()
            .ends_with("test.db"));
        assert_eq!(platform.clock().now_unix().await, 0);
        // credentials() deve existir e ser o fake recém-criado.
        let creds = platform.credentials();
        let listed = creds.list_providers().await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn fake_credential_store_set_get_delete() {
        let creds = FakeCredentialStore::new();
        let p = ProviderId::new("openai");
        let v = SecretString::new("sk-fake-1234".to_string().into());

        // Inicialmente vazio.
        assert!(creds.get(&p).await.unwrap().is_none());

        // Set + get.
        creds.set(&p, &v).await.unwrap();
        let got = creds.get(&p).await.unwrap().unwrap();
        assert_eq!(got.expose_secret(), "sk-fake-1234");

        // list_providers.
        let mut listed = creds.list_providers().await.unwrap();
        // ProviderId não é Ord; ordena por string representation.
        listed.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let mut expected = vec![p.clone()];
        expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(listed, expected);

        // Delete.
        creds.delete(&p).await.unwrap();
        assert!(creds.get(&p).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fake_credential_store_with_credentials_helper() {
        let creds = FakeCredentialStore::with_credentials([
            (ProviderId::new("openai"), "sk-fake-openai"),
            (ProviderId::new("anthropic"), "sk-ant-fake"),
        ]);
        let openai = creds
            .get(&ProviderId::new("openai"))
            .await
            .unwrap()
            .unwrap();
        let anth = creds
            .get(&ProviderId::new("anthropic"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(openai.expose_secret(), "sk-fake-openai");
        assert_eq!(anth.expose_secret(), "sk-ant-fake");
    }

    #[tokio::test]
    async fn fake_credential_store_secret_does_not_leak_in_debug() {
        // Garante que SecretString não implementa Display/Debug que vaze.
        let creds = FakeCredentialStore::new();
        let p = ProviderId::new("openai");
        let v = SecretString::new("sk-fake-1234".to_string().into());
        creds.set(&p, &v).await.unwrap();
        let listed = creds.list_providers().await.unwrap();
        // Serializar ProviderId para string não deve vazar a chave.
        let serialized = serde_json::to_string(&listed).unwrap();
        assert!(!serialized.contains("sk-fake-1234"));
    }
}
