//! Trait `MemoryClassifier` (ADR-0012) e o `NoopMemoryClassifier`
//! da Etapa 1.
//!
//! A Etapa 1 **não** classifica memórias automaticamente —
//! o `NoopMemoryClassifier` sempre devolve `record = None`. A
//! Etapa 3 introduz o `LlmMemoryClassifier` (LLM com prompt
//! restrito, output estruturado, pós-resposta, falsificável
//! pelo fake provider da Fase 2 — `ADR-0008`).
//!
//! A trait já existe na Etapa 1 pra fixar o contrato que a
//! Etapa 3 implementa. O `NoopMemoryClassifier` é o que
//! `MemoryRepo::insert_auto_captured` chama por default no
//! worker de classificação (Etapa 3) — durante a Etapa 1
//! não há worker, mas o tipo já está pronto.

use async_trait::async_trait;
use thiserror::Error;

use frederico_core::{ClassificationContext, MemoryClassifierOutput};

use crate::error::MemoryError;

/// Erro do classificador. O worker da Etapa 3 captura e
/// registra via `tracing::warn!` — **nunca** aborta o run
/// (regra do `ADR-0012 §2`).
#[derive(Debug, Error)]
pub enum ClassifierError {
    /// Provider LLM indisponível. Sem retry — o worker
    /// descarta e segue.
    #[error("classificador indisponível: {0}")]
    Unavailable(String),

    /// Output do LLM falhou validação de JSON schema. O
    /// worker descarta e segue.
    #[error("output inválido do classificador: {0}")]
    InvalidOutput(String),

    /// Cota de 5 chamadas/min estourada. O worker descarta
    /// e segue. (Regra do `ADR-0012 §2`.)
    #[error("cota de classificação estourada")]
    QuotaExceeded,
}

/// Trait do classificador (ADR-0012 §2).
///
/// Recebe o contexto (últimas N mensagens + escopo candidato)
/// e devolve a decisão estruturada. Se `record = None`, nada
/// vira memória — pode acontecer na maioria das conversas.
///
/// O `Retriever` **não** chama o classificador. Quem chama é
/// o worker pós-resposta da Etapa 3. A Etapa 1 só define a
/// trait porque o `MemoryRepo` consome o output
/// (`MemoryClassifierOutput`) e o teste do runner pode
/// precisar dela.
#[async_trait]
pub trait MemoryClassifier: Send + Sync {
    /// Nome do classificador (pra log e painel de debug).
    fn name(&self) -> &str;

    /// Classifica o contexto. Devolve `Ok(output)` mesmo se
    /// `output.record = None` — o caso normal é o classificador
    /// ter rodado e decidido "nada relevante aqui". Erro é
    /// só pra falha estrutural (provider indisponível, output
    /// malformado, cota estourada).
    async fn classify(
        &self,
        context: ClassificationContext,
    ) -> Result<MemoryClassifierOutput, ClassifierError>;
}

/// Adapter "sem classificador" — Etapa 1. Sempre devolve
/// `record = None`. Usado em testes e no caminho default
/// antes da Etapa 3.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMemoryClassifier;

#[async_trait]
impl MemoryClassifier for NoopMemoryClassifier {
    fn name(&self) -> &str {
        "noop"
    }

    async fn classify(
        &self,
        _context: ClassificationContext,
    ) -> Result<MemoryClassifierOutput, ClassifierError> {
        // Etapa 1: nada vira memória automaticamente. A Etapa 3
        // substitui por LlmMemoryClassifier.
        Ok(MemoryClassifierOutput {
            record: None,
            scope: None,
            importance: 0.0,
            reason: "noop: classificador não habilitado (Etapa 1)".into(),
        })
    }
}

/// Converte `ClassifierError` em `MemoryError` para o caller
/// que prefere o tipo unificado. Não há From automático
/// porque o `MemoryError` é parte da API pública.
pub fn classifier_error_to_memory(err: ClassifierError) -> MemoryError {
    MemoryError::GoldSetParse(format!("classifier: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_core::MemorySourceType;

    #[tokio::test]
    async fn noop_classifier_returns_none() {
        let c = NoopMemoryClassifier;
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![frederico_core::ConversationMessage {
                role: "user".into(),
                content: "olá".into(),
                source: MemorySourceType::new("user_message"),
            }],
        };
        let out = c.classify(ctx).await.unwrap();
        assert!(out.record.is_none());
        assert_eq!(c.name(), "noop");
    }

    #[test]
    fn classifier_error_displays() {
        let e = ClassifierError::Unavailable("teste".into());
        assert!(e.to_string().contains("indisponível"));
        let e = ClassifierError::InvalidOutput("schema falhou".into());
        assert!(e.to_string().contains("inválido"));
        let e = ClassifierError::QuotaExceeded;
        assert!(e.to_string().contains("cota"));
    }
}
