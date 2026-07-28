//! `ChatOrchestrator` (módulo de compat) — a maioria foi movida
//! pro `frederico-execution-engine` (Etapa 4.x.y).
//!
//! O que ainda vive aqui:
//!
//! 1. `error_to_view` e `ProviderErrorView` — função pura de
//!    tradução de `ProviderError` PT-BR.
//! 2. `OrchestratorError` / `OrchestratorResult` — definidos
//!    localmente (sem `Executor` variant) pra compat com código
//!    da Fase 2 que importava daqui. O `ChatOrchestratorError`
//!    oficial vive no `execution-engine::orchestrator`.
//! 3. Tests de `error_to_view` (puros, não precisam de
//!    `ChatOrchestrator`).
//!
//! **Onde estão os tests do `ChatOrchestrator`?**
//! `tests/integration_orchestrator.rs` no `execution-engine` (Etapa
//! 4.x.y). Os tests estavam aqui na Fase 2; moveram junto com o
//! `ChatOrchestrator`.

use crate::ProviderError;
use thiserror::Error;

// `OrchestratorError` definido localmente (não como alias do
// `execution-engine::ChatOrchestratorError`) pra evitar ciclo de
// dependência: o `provider-engine` não depende de `execution-engine`
// em runtime. As variantes são equivalentes, sem `Executor` (que
// o `send_message` não vê — o `RunExecutor` roda em background).
#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("storage: {0}")]
    Storage(#[from] frederico_storage::StorageError),
    #[error("provedor: {0}")]
    Provider(#[from] ProviderError),
    #[error("provedor '{0}' não tem adapter registrado")]
    ProviderNotFound(frederico_core::ProviderId),
    #[error("modelo '{provider}/{model}' não está no catálogo")]
    ModelNotFound {
        provider: frederico_core::ProviderId,
        model: frederico_core::ModelId,
    },
    #[error("modelo '{provider}/{model}' sem preço cadastrado")]
    ModelWithoutPrice {
        provider: frederico_core::ProviderId,
        model: frederico_core::ModelId,
    },
}

/// `Result` padrão do orquestrador.
pub type OrchestratorResult<T> = Result<T, OrchestratorError>;

/// Traduz `ProviderError` em `ProviderErrorView` (PT-BR com ação).
pub fn error_to_view(err: &ProviderError) -> ProviderErrorView {
    use crate::ProviderErrorKind;
    use ProviderErrorKind::*;
    let (code, title, action) = match err.kind {
        Auth => (
            "auth_invalid",
            "Chave de API inválida",
            "Abra Configurações → Provedores, confira a chave e salve de novo.",
        ),
        Payment => (
            "no_credit",
            "Sem crédito no provedor",
            "Veja o painel de billing do provedor para adicionar saldo.",
        ),
        Forbidden => (
            "forbidden",
            "Sem acesso a este modelo",
            "Sua chave não tem acesso ao modelo. Escolha outro ou peça acesso ao provedor.",
        ),
        NotFound => (
            "model_not_found",
            "Modelo não encontrado",
            "O modelo solicitado não está disponível. Escolha outro na lista.",
        ),
        RateLimited => (
            "rate_limited",
            "Limite de requisições atingido",
            "Aguarde alguns instantes e tente de novo.",
        ),
        Server => (
            "provider_error",
            "Provedor instável",
            "Tente de novo em alguns instantes.",
        ),
        Network => (
            "network_error",
            "Falha de rede",
            "Confira sua conexão e tente de novo.",
        ),
        Cancelled => ("cancelled", "Interrompido", ""),
        Timeout => (
            "timeout",
            "Provedor sem resposta",
            "O provedor não respondeu em 60s. Tente de novo ou troque de modelo.",
        ),
        Unknown => (
            "unknown",
            "Erro do provedor",
            "Veja os logs para mais detalhes.",
        ),
    };
    ProviderErrorView {
        code: code.to_string(),
        title: title.to_string(),
        detail: err.upstream_message.clone().unwrap_or_default(),
        action: if action.is_empty() {
            None
        } else {
            Some(action.to_string())
        },
        retry_after_secs: err.retry_after.map(|d| d.as_secs()),
    }
}

/// View PT-BR de um `ProviderError`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderErrorView {
    pub code: String,
    pub title: String,
    pub detail: String,
    pub action: Option<String>,
    pub retry_after_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderErrorKind;

    #[test]
    fn error_to_view_pt_br_format() {
        let err = ProviderError {
            kind: ProviderErrorKind::Auth,
            upstream_status: Some(401),
            upstream_message: None,
            retry_after: None,
        };
        let v = error_to_view(&err);
        assert_eq!(v.code, "auth_invalid");
        assert!(v.title.contains("Chave"));
        assert!(v.action.is_some());
        assert!(v.action.unwrap().contains("Configurações"));
    }

    #[test]
    fn error_to_view_timeout_has_action_with_60s() {
        let err = ProviderError::timeout();
        let v = error_to_view(&err);
        assert_eq!(v.code, "timeout");
        assert_eq!(v.title, "Provedor sem resposta");
        let action = v.action.expect("timeout tem action");
        assert!(action.contains("60s"));
    }

    #[test]
    fn error_to_view_cancelled_has_no_action() {
        let v = error_to_view(&ProviderError::cancelled());
        assert_eq!(v.code, "cancelled");
        assert!(v.action.is_none());
    }
}
