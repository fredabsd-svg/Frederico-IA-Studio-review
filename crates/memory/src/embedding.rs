//! Trait `EmbeddingProvider` (ADR-0010), o `NoopEmbeddingAdapter`
//! da Etapa 1 e o `OpenRouterEmbeddingAdapter` da Etapa 2.
//!
//! O `NoopEmbeddingAdapter` devolve `Err(EmbeddingError::Unavailable)`
//! sempre. É o que a Etapa 1 usa no baseline lexical — força o
//! caminho "FTS5 funciona sem embeddings" do `PROMPT MESTRE`
//! §10.13.
//!
//! O `OpenRouterEmbeddingAdapter` é a implementação de produção:
//! POST OpenAI-compat pra `/embeddings` (default gateway OpenRouter
//! com `text-embedding-3-small` da OpenAI, conforme ADR-0010 §3).
//! Auth via `SecretString` (DPAPI no Windows via `CredentialStore`
//! da Fase 2, jamais em texto puro no banco — ADR-0007).

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::MemoryError;

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

/// Adapter de produção: OpenRouter (ADR-0010) com `POST
/// /embeddings` no formato OpenAI-compat. Suporta qualquer
/// modelo de embedding que o gateway expor (default
/// `openai/text-embedding-3-small`, 1536 dim).
///
/// Auth via `SecretString` — o caller (casca Tauri) busca a
/// key do `CredentialStore` (DPAPI no Windows, ADR-0007) e
/// constrói o adapter. A `SecretString` evita que a key vaze
/// em logs/debug (`Debug` não a exibe; `Display` retorna
/// `[REDACTED]`).
///
/// **Timeout:** 1.5s por request. O `Retriever` tem orçamento
/// total de 2s (`PROMPT MESTRE` §10.13); 500ms de margem
/// pra cosine + ordenação + I/O de banco. Sem retry —
/// `Retriever` trata timeout como "sem semântica" e cai
/// pra lexical-only.
pub struct OpenRouterEmbeddingAdapter {
    api_key: SecretString,
    base_url: String,
    model: String,
    dimensions: usize,
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenRouterEmbeddingAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterEmbeddingAdapter")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .finish()
    }
}

impl OpenRouterEmbeddingAdapter {
    /// Cria um adapter OpenRouter com defaults sensatos
    /// (modelo `text-embedding-3-small` da OpenAI, 1536 dim,
    /// base URL OpenRouter, timeout 1.5s). O caller pode
    /// customizar via [`Self::with_model`] se precisar.
    ///
    /// `api_key` vem do `CredentialStore` da Fase 2
    /// (DPAPI no Windows, ADR-0007). **Nunca** é persistido
    /// em banco, log ou config.
    #[must_use]
    pub fn new(api_key: SecretString) -> Self {
        Self {
            api_key,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "openai/text-embedding-3-small".into(),
            dimensions: 1536,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(1500))
                .build()
                .expect("reqwest::Client::builder não deveria falhar em runtime"),
        }
    }

    /// Customiza o modelo. `dimensions` deve bater com o
    /// modelo (e.g. 1536 pro `text-embedding-3-small`,
    /// 3072 pro `text-embedding-3-large`).
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>, dimensions: usize) -> Self {
        self.model = model.into();
        self.dimensions = dimensions;
        self
    }

    /// Customiza a base URL (default OpenRouter; pode apontar
    /// pra OpenAI direto, Azure, etc).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[async_trait]
impl EmbeddingProvider for OpenRouterEmbeddingAdapter {
    fn provider_id(&self) -> &str {
        "openrouter"
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // Request body (formato OpenAI-compat).
        #[derive(Serialize)]
        struct EmbeddingRequest<'a> {
            model: &'a str,
            input: &'a [&'a str],
        }

        // Response (subset do OpenAI — só o que usamos).
        #[derive(Deserialize)]
        struct EmbeddingResponse {
            data: Vec<EmbeddingData>,
        }
        #[derive(Deserialize)]
        struct EmbeddingData {
            embedding: Vec<f32>,
        }

        let url = format!("{}/embeddings", self.base_url);
        let body = EmbeddingRequest {
            model: &self.model,
            input: inputs,
        };

        let response = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EmbeddingError::Timeout(1500)
                } else {
                    EmbeddingError::Transport(format!("send: {e}"))
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|e| format!("<erro ao ler corpo: {e}>"));
            return Err(EmbeddingError::Transport(format!("HTTP {status}: {body}")));
        }

        let parsed: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| EmbeddingError::Transport(format!("parse da resposta: {e}")))?;

        if parsed.data.len() != inputs.len() {
            return Err(EmbeddingError::Transport(format!(
                "resposta tem {} embeddings, esperado {}",
                parsed.data.len(),
                inputs.len()
            )));
        }

        // Valida dimensionalidade (se o servidor mandou dim
        // diferente da prometida, é bug do adapter — mas
        // capturamos pra evitar panics downstream).
        for (i, emb) in parsed.data.iter().enumerate() {
            if emb.embedding.len() != self.dimensions {
                return Err(EmbeddingError::DimensionMismatch {
                    expected: self.dimensions,
                    actual: emb.embedding.len(),
                });
            }
            // Silencia o warning de `i` não usado (poderia
            // usar pra mensagem de erro mais detalhada).
            let _ = i;
        }

        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

/// Helper de teste: monta um adapter apontando pra um
/// `TcpListener` local. O caller sobe o listener e passa
/// o `addr`. Usado pelos integration tests da Etapa 2
/// (mesma técnica do `cancel_http.rs` da Fase 3).
impl OpenRouterEmbeddingAdapter {
    /// Cria um adapter com `base_url` customizado (e.g.
    /// `http://127.0.0.1:8080` pra testes locais).
    #[must_use]
    pub fn for_test(api_key: SecretString, base_url: impl Into<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.into(),
            model: "test-model".into(),
            dimensions: 4, // pequeno pra testes
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(500))
                .build()
                .expect("reqwest::Client::builder não deveria falhar em runtime"),
        }
    }
}

/// Converte `EmbeddingError` em `MemoryError` para callers
/// que preferem o tipo unificado.
pub fn embedding_error_to_memory(err: EmbeddingError) -> MemoryError {
    match err {
        EmbeddingError::Unavailable(s) => MemoryError::EmbeddingRequired(s),
        EmbeddingError::Transport(s) => MemoryError::EmbeddingHttp(s),
        EmbeddingError::Timeout(_) => {
            MemoryError::EmbeddingHttp("timeout do embedding adapter".into())
        }
        EmbeddingError::DimensionMismatch { expected, actual } => {
            MemoryError::EmbeddingDimensionMismatch { expected, actual }
        }
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
