//! Trait `EmbeddingProvider` (ADR-0010) e o `NoopEmbeddingAdapter`
//! da Etapa 1.
//!
//! O `NoopEmbeddingAdapter` devolve `Err(EmbeddingError::Unavailable)`
//! sempre. É o que a Etapa 1 usa no baseline lexical — força o
//! caminho "FTS5 funciona sem embeddings" do `PROMPT MESTRE`
//! §10.13. A Etapa 2 introduz o `OpenRouterEmbeddingAdapter`
//! real (OpenAI-compat, mesmo gateway default do `provider-engine`).

use async_trait::async_trait;
use thiserror::Error;

/// Erro de embedding. O `Retriever` traduz em "retrieval sem
/// semântica" (cai pra lexical-only) e registra via `tracing::warn!`.
#[derive(Debug, Error)]
pub enum EmbeddingError {
    /// Credencial ausente / provedor não configurado. Sem
    /// retry. O `Retriever` cai pra lexical-only.
    #[error("embedding provider indisponível: {0}")]
    Unavailable(String),

    /// Erro de transporte (HTTP 4xx/5xx, parsing, conexão).
    /// O `Retriever` tenta até 2x com backoff exponencial curto
    /// (200ms, 800ms); depois `Unavailable`.
    #[error("embedding transport error: {0}")]
    Transport(String),

    /// Timeout. O `Retriever` tem orçamento total de 2s
    /// (`PROMPT MESTRE` §10.13) — o `embed` recebe 1.5s no
    /// máximo. Sem retry após timeout — cai pra lexical-only.
    #[error("embedding timeout após {0}ms")]
    Timeout(u64),

    /// Provider devolveu dimensionalidade inconsistente com
    /// o que prometeu. Erro de programação do adapter.
    #[error("dimensionalidade inconsistente: esperado {expected}, recebi {actual}")]
    DimensionMismatch {
        /// Dimensões prometidas pelo `EmbeddingProvider::dimensions()`.
        expected: usize,
        /// Dimensões realmente devolvidas.
        actual: usize,
    },
}

/// Provider de embeddings (ADR-0010).
///
/// A trait é **a única porta** entre o `Retriever` e qualquer
/// backend de embeddings. `NoopEmbeddingAdapter` é a
/// implementação de `Etapa 1` (sempre devolve `Unavailable` —
/// força o caminho lexical-only); a Etapa 2 introduz
/// `OpenRouterEmbeddingAdapter` e `OpenAiDirectEmbeddingAdapter`.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Identificador do provedor (`"openrouter"`, `"openai"`,
    /// `"noop"`). Usado como partição na tabela
    /// `memory_embeddings` — embeddings de providers
    /// diferentes não são comparáveis (ADR-0010 §1).
    fn provider_id(&self) -> &str;

    /// Modelo de embedding (e.g. `"openai/text-embedding-3-small"`).
    /// Segunda chave de partição.
    fn model_id(&self) -> &str;

    /// Dimensões do vetor (e.g. 1536, 3072). O `Retriever`
    /// usa pra validar a tabela `memory_embeddings` antes
    /// do primeiro `embed`.
    fn dimensions(&self) -> usize;

    /// Embute um lote de textos. Devolve `EmbeddingError` em
    /// falha — o `Retriever` registra via `tracing::warn!` e
    /// cai pra lexical-only (regra do `PROMPT MESTRE` §10.13).
    async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
}

/// Adapter "sem provider". Usado pela Etapa 1 (baseline
/// lexical) e por testes que querem provar §10.13
/// "FTS5 funciona sem embeddings".
///
/// Sempre devolve `Err(EmbeddingError::Unavailable)`. O
/// `Retriever` trata isso como "sem semântica" e usa só
/// lexical+recência+importância+confirmação.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEmbeddingAdapter;

#[async_trait]
impl EmbeddingProvider for NoopEmbeddingAdapter {
    fn provider_id(&self) -> &str {
        "noop"
    }

    fn model_id(&self) -> &str {
        "noop"
    }

    fn dimensions(&self) -> usize {
        0
    }

    async fn embed(&self, _inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Err(EmbeddingError::Unavailable(
            "NoopEmbeddingAdapter: sem provedor configurado".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_adapter_returns_unavailable() {
        let adapter = NoopEmbeddingAdapter;
        let result = adapter.embed(&["texto qualquer"]).await;
        assert!(matches!(result, Err(EmbeddingError::Unavailable(_))));
    }

    #[test]
    fn noop_adapter_id_strings() {
        let adapter = NoopEmbeddingAdapter;
        assert_eq!(adapter.provider_id(), "noop");
        assert_eq!(adapter.model_id(), "noop");
        assert_eq!(adapter.dimensions(), 0);
    }

    #[test]
    fn embedding_error_displays() {
        let e = EmbeddingError::Unavailable("teste".into());
        assert!(e.to_string().contains("indisponível"));
        let e = EmbeddingError::Timeout(1500);
        assert!(e.to_string().contains("1500"));
        let e = EmbeddingError::DimensionMismatch {
            expected: 1536,
            actual: 768,
        };
        assert!(e.to_string().contains("1536"));
        assert!(e.to_string().contains("768"));
    }
}
