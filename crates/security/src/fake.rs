//! Implementações de plataforma para testes. Vivem no mesmo crate
//! para garantir que o gate de pureza do núcleo (sem `tauri`, sem
//! `windows`) não precise importar nada externo.
//!
//! O `FakeCredentialStore` cumpre o contrato "nunca shim de texto
//! puro" do [ADR-0007](../decisions/0007-credential-store-trait.md):
//! em testes, o motor conversa com este fake, não com env, dotenv ou
//! config em arquivo.

use super::*;
use secrecy::SecretString;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// `AppPaths` fake, sempre retorna um diretório em `temp`.
pub struct FakePaths(pub PathBuf);

impl frederico_storage::AppPaths for FakePaths {
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
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicU64::new(0),
            auto: false,
        })
    }

    #[must_use]
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

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            now: AtomicU64::new(0),
            auto: false,
        }
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

/// `CredentialStore` fake. Armazena segredos em memória atrás de
/// `Arc<Mutex<HashMap<ProviderId, SecretString>>>`. É o que os testes
/// usam — a `WindowsCredentialStore` é a implementação de produção.
pub struct FakeCredentialStore {
    inner: Mutex<HashMap<ProviderId, SecretString>>,
}

impl FakeCredentialStore {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Constrói um fake com credenciais pré-configuradas. Útil para
    /// testes que não querem chamar `set()` em cada caso.
    #[must_use]
    pub fn with_credentials<I>(creds: I) -> Arc<Self>
    where
        I: IntoIterator<Item = (ProviderId, &'static str)>,
    {
        let map: HashMap<ProviderId, SecretString> = creds
            .into_iter()
            .map(|(p, s)| (p, SecretString::new(s.to_string().into())))
            .collect();
        Arc::new(Self {
            inner: Mutex::new(map),
        })
    }
}

impl Default for FakeCredentialStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl CredentialStore for FakeCredentialStore {
    async fn get(&self, provider: &ProviderId) -> Result<Option<SecretString>, SecurityError> {
        Ok(self.inner.lock().unwrap().get(provider).cloned())
    }

    async fn set(&self, provider: &ProviderId, value: &SecretString) -> Result<(), SecurityError> {
        self.inner
            .lock()
            .unwrap()
            .insert(provider.clone(), value.clone());
        Ok(())
    }

    async fn delete(&self, provider: &ProviderId) -> Result<(), SecurityError> {
        self.inner.lock().unwrap().remove(provider);
        Ok(())
    }

    async fn list_providers(&self) -> Result<Vec<ProviderId>, SecurityError> {
        Ok(self.inner.lock().unwrap().keys().cloned().collect())
    }
}

/// Plataforma fake que junta `FakePaths`, `FakeClock` e
/// `FakeCredentialStore`.
pub struct FakePlatform {
    paths: FakePaths,
    clock: Arc<FakeClock>,
    credentials: Arc<FakeCredentialStore>,
}

impl FakePlatform {
    #[must_use]
    pub fn new(dir: PathBuf, clock: Arc<FakeClock>) -> Self {
        Self {
            paths: FakePaths(dir),
            clock,
            credentials: FakeCredentialStore::new(),
        }
    }

    #[must_use]
    pub fn with_credentials(mut self, credentials: Arc<FakeCredentialStore>) -> Self {
        self.credentials = credentials;
        self
    }

    #[must_use]
    pub fn credentials(&self) -> Arc<FakeCredentialStore> {
        self.credentials.clone()
    }
}

#[async_trait]
impl Platform for FakePlatform {
    fn paths(&self) -> &dyn frederico_storage::AppPaths {
        &self.paths
    }

    fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    fn credentials(&self) -> &dyn CredentialStore {
        self.credentials.as_ref()
    }
}
