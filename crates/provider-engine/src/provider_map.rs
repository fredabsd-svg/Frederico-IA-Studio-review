//! Mapa de `ProviderId` → `Arc<dyn ProviderAdapter>` para resolver o
//! adapter certo por conversa. O casca Tauri monta o mapa na
//! inicialização com base nas credenciais disponíveis (ou em todos
//! os adapters pré-registrados para o provedor simulado).

use std::collections::HashMap;
use std::sync::Arc;

use frederico_core::ProviderId;

use crate::provider::ProviderAdapter;

#[derive(Default)]
pub struct ProviderMap {
    inner: HashMap<ProviderId, Arc<dyn ProviderAdapter>>,
}

impl ProviderMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adiciona um adapter. Se o `ProviderId` já existe, substitui
    /// (útil quando o usuário cadastra uma chave e o adapter real
    /// passa a estar disponível).
    pub fn insert(&mut self, adapter: Arc<dyn ProviderAdapter>) {
        self.inner.insert(adapter.id(), adapter);
    }

    #[must_use]
    pub fn get(&self, provider: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
        self.inner.get(provider).cloned()
    }

    /// Lista os `ProviderId` registrados.
    pub fn providers(&self) -> Vec<ProviderId> {
        self.inner.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::trait_level::FakeProviderAdapter;

    #[test]
    fn insert_and_get() {
        let mut map = ProviderMap::new();
        let a = Arc::new(FakeProviderAdapter::new("openai"));
        map.insert(a.clone());
        let got = map.get(&ProviderId::new("openai")).unwrap();
        assert_eq!(got.id().as_str(), "openai");
        assert!(map.get(&ProviderId::new("anthropic")).is_none());
    }

    #[test]
    fn providers_lists_registered() {
        let mut map = ProviderMap::new();
        map.insert(Arc::new(FakeProviderAdapter::new("openai")));
        map.insert(Arc::new(FakeProviderAdapter::new("anthropic")));
        let mut ps = map.providers();
        ps.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        // Ordem alfabética ("a" < "o").
        assert_eq!(
            ps,
            vec![ProviderId::new("anthropic"), ProviderId::new("openai"),]
        );
    }
}
