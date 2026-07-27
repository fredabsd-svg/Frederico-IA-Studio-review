//! `RunRegistry` — mapping de `RunId` para `CancellationToken`.
//!
//! Cada run em andamento tem um `CancellationToken` (de
//! `tokio_util::sync::CancellationToken`) que pode ser disparado
//! quando o usuário pede stop ou quando o watchdog expira. O
//! orquestrador registra o token na criação do run, dispara quando
//! recebe `RunCancel`, e desregistra quando o run termina.

use std::collections::HashMap;
use std::sync::Arc;

use frederico_core::RunId;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct RunRegistry {
    inner: Mutex<HashMap<RunId, CancellationToken>>,
}

impl RunRegistry {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registra um token para o `run_id`. Idempotente: se já existe,
    /// substitui.
    pub async fn register(&self, run_id: RunId, token: CancellationToken) {
        self.inner.lock().await.insert(run_id, token);
    }

    /// Dispara o `CancellationToken` do `run_id`, se existir.
    /// Retorna `true` se o token foi encontrado e disparado.
    pub async fn cancel(&self, run_id: RunId) -> bool {
        if let Some(token) = self.inner.lock().await.get(&run_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Pega o token (clone barata) sem disparar. Útil para o
    /// watchdog monitorar a mesma origem de cancelamento.
    pub async fn token(&self, run_id: RunId) -> Option<CancellationToken> {
        self.inner.lock().await.get(&run_id).cloned()
    }

    /// Remove o token (chamado no fim do run).
    pub async fn unregister(&self, run_id: RunId) -> Option<CancellationToken> {
        self.inner.lock().await.remove(&run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_cancel() {
        let reg = RunRegistry::new();
        let run = RunId::new();
        let token = CancellationToken::new();
        reg.register(run, token.clone()).await;
        assert!(reg.cancel(run).await);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_unknown_returns_false() {
        let reg = RunRegistry::new();
        assert!(!reg.cancel(RunId::new()).await);
    }

    #[tokio::test]
    async fn unregister_removes_token() {
        let reg = RunRegistry::new();
        let run = RunId::new();
        let token = CancellationToken::new();
        reg.register(run, token).await;
        let removed = reg.unregister(run).await;
        assert!(removed.is_some());
        assert!(!reg.cancel(run).await);
    }
}
