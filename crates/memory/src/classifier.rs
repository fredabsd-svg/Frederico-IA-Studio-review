//! Trait `MemoryClassifier` (ADR-0012) e implementações.
//!
//! **Etapa 1**: trait + `NoopMemoryClassifier` (sempre devolve
//! `None`).
//!
//! **Etapa 3**: `CompletionProvider` trait (porta fina pra
//! o `provider-engine` da Fase 2 — sem dependência cíclica)
//! + `NoopCompletionProvider` + `LlmMemoryClassifier` real
//!   (LLM com prompt restrito, output JSON validado, cota).
//!
//! Integração com `provider-engine`: a casca Tauri provê um
//! adapter que delega pro `OpenAiCompatAdapter::complete`.
//! O `frederico-memory` continua puro (sem dep do
//! `provider-engine` — ver ADR-0010 §1 sobre o grafo de
//! crates).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use frederico_core::{
    ClassificationContext, ConversationMessage, MemoryClassifierOutput, MemoryScopeType, NewMemory,
};

use crate::error::MemoryError;

/// Erro do classificador. O `MemoryExtractor` (worker da
/// Etapa 3) captura e registra via `tracing::warn!` — **nunca**
/// aborta o run (regra do `ADR-0012 §2`).
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
/// o worker pós-resposta da Etapa 3 (`MemoryExtractor`).
#[async_trait]
pub trait MemoryClassifier: Send + Sync {
    /// Nome do classificador (pra log e painel de debug da Etapa 5).
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

// =============================================================================
// Etapa 3: CompletionProvider + LlmMemoryClassifier
// =============================================================================

/// Request de completion. O `LlmMemoryClassifier` monta
/// `system` (instruções + schema) e `user` (mensagens
/// recentes + escopo candidato) e chama `CompletionProvider::complete`.
///
/// **Por que um trait separado?** Pra não acoplar o
/// `frederico-memory` ao `provider-engine` (ciclo: o
/// `provider-engine` depende do `execution-engine` que
/// depende do `memory`). O trait `CompletionProvider` é
/// a porta — a casca Tauri provê um adapter que delega pro
/// `OpenAiCompatAdapter::complete` do `provider-engine`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// System prompt (instruções + schema esperado).
    pub system: String,
    /// User prompt (mensagens recentes + escopo).
    pub user: String,
    /// Modelo a usar (ex: "openai/gpt-4o-mini"). O
    /// `CompletionProvider` decide se respeita ou usa
    /// default.
    #[serde(default)]
    pub model: String,
}

/// Trait de completion. A casca Tauri provê um adapter
/// que delega pro `provider-engine::OpenAiCompatAdapter::complete`
/// (mesma config do `provider-engine` da Fase 2 — OpenAI-compat,
/// timeout 5s, retry 2x).
#[async_trait]
pub trait CompletionProvider: Send + Sync {
    /// Nome do provider (pra log).
    fn name(&self) -> &str;

    /// Faz uma completion. Devolve o texto cru da resposta
    /// (o `LlmMemoryClassifier` parseia o JSON).
    async fn complete(&self, request: CompletionRequest) -> Result<String, ClassifierError>;
}

/// Adapter "sem provider" — Etapa 1 path. Sempre devolve
/// `Err(ClassifierError::Unavailable)`. O `LlmMemoryClassifier`
/// com esse provider se comporta como o `NoopMemoryClassifier`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCompletionProvider;

#[async_trait]
impl CompletionProvider for NoopCompletionProvider {
    fn name(&self) -> &str {
        "noop"
    }

    async fn complete(&self, _request: CompletionRequest) -> Result<String, ClassifierError> {
        Err(ClassifierError::Unavailable(
            "NoopCompletionProvider: sem provider configurado".into(),
        ))
    }
}

/// Config do `LlmMemoryClassifier`.
#[derive(Debug, Clone)]
pub struct LlmClassifierConfig {
    /// Modelo default (ex: "openai/gpt-4o-mini" via OpenRouter).
    pub model: String,
    /// Quantas mensagens recentes considerar. Default 6.
    pub max_messages: usize,
    /// Confiança mínima pra criar memória. Saída do LLM com
    /// `confidence < threshold` vira `record = None`. Default
    /// 0.6 (conservador — só salva o que o LLM tem certeza).
    pub confidence_threshold: f32,
    /// Cota de chamadas por minuto. Default 5 (regra do
    /// `ADR-0012 §2`).
    pub quota_per_minute: u32,
}

impl Default for LlmClassifierConfig {
    fn default() -> Self {
        Self {
            model: "openai/gpt-4o-mini".into(),
            max_messages: 6,
            confidence_threshold: 0.6,
            quota_per_minute: 5,
        }
    }
}

/// Classificador LLM-based (ADR-0012 §2).
///
/// Algoritmo:
/// 1. Constrói o prompt (system + user) com as últimas N
///    mensagens.
/// 2. Verifica a cota (5/min default).
/// 3. Chama o `CompletionProvider::complete`.
/// 4. Faz parse do JSON retornado.
/// 5. Valida: `record` é `NewMemory` válido, `confidence >=
///    threshold`, `scope` consistente, `origin` permitido.
/// 6. Devolve `MemoryClassifierOutput`.
///
/// **Saída do LLM** (formato esperado, validado em parse):
/// ```json
/// {
///   "record": {
///     "scope_type": "project",
///     "scope_id": "proj-1",
///     "type": "preference",
///     "content": "...",
///     "origin": "user",
///     "source_type": "user_message",
///     "source_id": "msg-123",
///     "importance": 0.7,
///     "confidence": 0.8
///   } | null,
///   "scope": "project" | null,
///   "importance": 0.7,
///   "reason": "..."
/// }
/// ```
pub struct LlmMemoryClassifier {
    completion: Arc<dyn CompletionProvider>,
    config: LlmClassifierConfig,
    /// Cota: lista de instantes das últimas chamadas. Mutex
    /// simples (5/min = 1 chamada a cada 12s, contenção
    /// desprezível).
    quota: Mutex<Vec<Instant>>,
}

impl LlmMemoryClassifier {
    /// Cria um classificador com config default.
    #[must_use]
    pub fn new(completion: Arc<dyn CompletionProvider>) -> Self {
        Self::with_config(completion, LlmClassifierConfig::default())
    }

    /// Cria com config customizada.
    #[must_use]
    pub fn with_config(
        completion: Arc<dyn CompletionProvider>,
        config: LlmClassifierConfig,
    ) -> Self {
        Self {
            completion,
            config,
            quota: Mutex::new(Vec::new()),
        }
    }

    /// Provider que está sendo usado.
    #[must_use]
    pub fn completion(&self) -> &Arc<dyn CompletionProvider> {
        &self.completion
    }

    /// Config atual.
    #[must_use]
    pub fn config(&self) -> &LlmClassifierConfig {
        &self.config
    }

    /// Verifica e renova a cota. Retorna `Err(QuotaExceeded)`
    /// se a cota estourar. Janela: últimos 60 segundos.
    async fn check_quota(&self) -> Result<(), ClassifierError> {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut calls = self.quota.lock().await;
        // Descarta chamadas fora da janela.
        calls.retain(|t| now.duration_since(*t) < window);
        if calls.len() >= self.config.quota_per_minute as usize {
            return Err(ClassifierError::QuotaExceeded);
        }
        calls.push(now);
        Ok(())
    }
}

#[async_trait]
impl MemoryClassifier for LlmMemoryClassifier {
    fn name(&self) -> &str {
        "llm"
    }

    async fn classify(
        &self,
        context: ClassificationContext,
    ) -> Result<MemoryClassifierOutput, ClassifierError> {
        // 1. Cota primeiro.
        self.check_quota().await?;

        // 2. Monta o prompt.
        let system = build_system_prompt();
        let user = build_user_prompt(&context, self.config.max_messages);

        // 3. Chama o provider.
        let request = CompletionRequest {
            system,
            user,
            model: self.config.model.clone(),
        };
        let raw = self.completion.complete(request).await?;

        // 4. Parse do JSON.
        let parsed: LlmOutput = serde_json::from_str(&raw)
            .map_err(|e| ClassifierError::InvalidOutput(format!("parse JSON: {e}")))?;

        // 5. Validação.
        let record = if let Some(rec) = parsed.record {
            // `confidence` é obrigatório pra `record`. Threshold
            // aplicado: confiança baixa → record = None.
            if rec.confidence < self.config.confidence_threshold {
                tracing::info!(
                    run_id = %context.run_id,
                    confidence = rec.confidence,
                    threshold = self.config.confidence_threshold,
                    "LLM propôs memória mas confiança abaixo do threshold — descartando"
                );
                None
            } else {
                // Validação adicional: scope_id não-vazio se
                // escopo é específico. (Escopos globais aceitam
                // vazio.)
                if !rec.scope_type.is_global() && rec.scope_id.trim().is_empty() {
                    return Err(ClassifierError::InvalidOutput(format!(
                        "scope_id vazio pra escopo específico {:?}",
                        rec.scope_type
                    )));
                }
                if !(0.0..=1.0).contains(&rec.confidence) {
                    return Err(ClassifierError::InvalidOutput(format!(
                        "confidence fora de [0,1]: {}",
                        rec.confidence
                    )));
                }
                if !(0.0..=1.0).contains(&rec.importance) {
                    return Err(ClassifierError::InvalidOutput(format!(
                        "importance fora de [0,1]: {}",
                        rec.importance
                    )));
                }
                Some(rec)
            }
        } else {
            None
        };

        Ok(MemoryClassifierOutput {
            record,
            scope: parsed.scope,
            importance: parsed.importance,
            reason: parsed.reason,
        })
    }
}

/// Schema de output esperado do LLM (formato interno).
#[derive(Debug, Deserialize)]
struct LlmOutput {
    record: Option<NewMemory>,
    scope: Option<MemoryScopeType>,
    importance: f32,
    reason: String,
}

/// System prompt do classificador. Instruções + schema
/// esperado. O LLM deve devolver JSON com a estrutura
/// `LlmOutput`.
fn build_system_prompt() -> String {
    // Nota: o LLM pode alucinar. Por isso a validação em
    // `LlmMemoryClassifier::classify` é estrita — não
    // confiamos no output sem validar.
    "Você é um classificador de memórias de longo prazo para o Frederico IA Studio. \
     Dada uma conversa, identifique APENAS informações que devem virar memórias de longo prazo \
     (preferências do usuário, factos, decisões, instruções de projeto, contexto de cliente, \
     procedimentos, padrões de entrega). Ignore conversa casual, saudações curtas, e \
     informações temporárias ou já sabidas.\n\n\
     Retorne JSON com a estrutura:\n\
     {\n\
       \"record\": <objeto | null>,\n\
       \"scope\": <\"project\" | \"preference\" | \"profile\" | ... | null>,\n\
       \"importance\": <número entre 0.0 e 1.0>,\n\
       \"reason\": <string explicando a decisão>\n\
     }\n\n\
     O objeto `record` tem os campos:\n\
     - scope_type: um dos 9 escopos (project, preference, profile, assistant, client, \
     conversation, document, task, session)\n\
     - scope_id: id do escopo (vazio para escopos globais)\n\
     - type: um dos 12 tipos (preference, fact, decision, correction, project_instruction, \
     client_context, procedure, delivery_pattern, temporary, conversation_summary, \
     document_reference, user_pinned)\n\
     - content: o texto da memória (1-2 frases)\n\
     - origin: quem originou (user, assistant, ou external_content)\n\
     - source_type: tipo da fonte (user_message, assistant_message, tool_output:..., etc)\n\
     - source_id: id da fonte (opcional)\n\
     - importance: 0.0-1.0\n\
     - confidence: 0.0-1.0 (sua confiança na classificação)\n\
     - expires_at: ISO 8601 (opcional, só pra 'temporary')\n\n\
     IMPORTANTE: se não houver memória de longo prazo clara, devolva `record: null`. \
     É melhor errar por omitir do que por inventar."
        .into()
}

/// User prompt: serializa as últimas N mensagens + o escopo
/// candidato.
fn build_user_prompt(context: &ClassificationContext, max_messages: usize) -> String {
    let messages: Vec<&ConversationMessage> = context
        .messages
        .iter()
        .rev()
        .take(max_messages)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "Conversa: {}\nRun: {}\nÚltimas {} mensagens:\n\n",
        context.conversation_id,
        context.run_id,
        messages.len()
    ));
    for (i, m) in messages.iter().enumerate() {
        out.push_str(&format!(
            "[{}] role={} source={}\n{}\n\n",
            i + 1,
            m.role,
            m.source.as_str(),
            m.content
        ));
    }
    out.push_str("Classifique se há memória de longo prazo. Retorne JSON.");
    out
}

/// Converte `ClassifierError` em `MemoryError` para o caller
/// que prefere o tipo unificado. Não há `From` automático
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
            messages: vec![ConversationMessage {
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

    #[tokio::test]
    async fn llm_classifier_with_noop_completion_returns_unavailable() {
        let classifier = LlmMemoryClassifier::new(Arc::new(NoopCompletionProvider));
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![],
        };
        let result = classifier.classify(ctx).await;
        assert!(matches!(result, Err(ClassifierError::Unavailable(_))));
    }

    #[tokio::test]
    async fn llm_classifier_parses_valid_output() {
        // Mock provider que devolve um JSON válido.
        struct MockProvider(String);
        #[async_trait]
        impl CompletionProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<String, ClassifierError> {
                Ok(self.0.clone())
            }
        }

        let json = r#"{
            "record": {
                "scope_type": "project",
                "scope_id": "proj-1",
                "type_": "preference",
                "content": "o usuário prefere Rust",
                "origin": "user",
                "source_type": "user_message",
                "source_id": null,
                "confidence": 0.9,
                "importance": 0.7,
                "expires_at": null
            },
            "scope": "project",
            "importance": 0.7,
            "reason": "preferência explícita"
        }"#;
        let classifier = LlmMemoryClassifier::new(Arc::new(MockProvider(json.into())));
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![ConversationMessage {
                role: "user".into(),
                content: "eu gosto de Rust".into(),
                source: MemorySourceType::new("user_message"),
            }],
        };
        let out = classifier.classify(ctx).await.unwrap();
        let record = out.record.expect("record");
        assert_eq!(record.content, "o usuário prefere Rust");
        assert_eq!(record.confidence, 0.9);
        assert_eq!(out.scope, Some(MemoryScopeType::Project));
    }

    #[tokio::test]
    async fn llm_classifier_below_threshold_returns_none() {
        struct MockProvider(String);
        #[async_trait]
        impl CompletionProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<String, ClassifierError> {
                Ok(self.0.clone())
            }
        }

        // confidence = 0.5 (default threshold = 0.6) → deve descartar.
        let json = r#"{
            "record": {
                "scope_type": "project",
                "scope_id": "proj-1",
                "type_": "fact",
                "content": "algo incerto",
                "origin": "user",
                "source_type": "user_message",
                "source_id": null,
                "confidence": 0.5,
                "importance": 0.5,
                "expires_at": null
            },
            "scope": "project",
            "importance": 0.5,
            "reason": "incerto"
        }"#;
        let classifier = LlmMemoryClassifier::new(Arc::new(MockProvider(json.into())));
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![],
        };
        let out = classifier.classify(ctx).await.unwrap();
        assert!(out.record.is_none(), "confiança baixa deve descartar");
    }

    #[tokio::test]
    async fn llm_classifier_rejects_invalid_json() {
        struct MockProvider(String);
        #[async_trait]
        impl CompletionProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<String, ClassifierError> {
                Ok(self.0.clone())
            }
        }

        let classifier = LlmMemoryClassifier::new(Arc::new(MockProvider("não é json".into())));
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![],
        };
        let result = classifier.classify(ctx).await;
        assert!(matches!(result, Err(ClassifierError::InvalidOutput(_))));
    }

    #[tokio::test]
    async fn llm_classifier_quota_enforced() {
        struct MockProvider;
        #[async_trait]
        impl CompletionProvider for MockProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<String, ClassifierError> {
                // Devolve um record válido.
                Ok(r#"{
                    "record": {
                        "scope_type": "project",
                        "scope_id": "proj-1",
                        "type_": "fact",
                        "content": "x",
                        "origin": "user",
                        "source_type": "user_message",
                        "source_id": null,
                        "confidence": 0.9,
                        "importance": 0.5,
                        "expires_at": null
                    },
                    "scope": "project",
                    "importance": 0.5,
                    "reason": "x"
                }"#
                .into())
            }
        }

        let config = LlmClassifierConfig {
            quota_per_minute: 2, // baixo pra teste
            ..Default::default()
        };
        let classifier = LlmMemoryClassifier::with_config(Arc::new(MockProvider), config);
        let ctx = ClassificationContext {
            run_id: "run-1".into(),
            conversation_id: "conv-1".into(),
            messages: vec![],
        };

        // Primeiras 2 chamadas passam.
        let _ = classifier.classify(ctx.clone()).await.unwrap();
        let _ = classifier.classify(ctx.clone()).await.unwrap();
        // 3ª estoura a cota.
        let result = classifier.classify(ctx).await;
        assert!(matches!(result, Err(ClassifierError::QuotaExceeded)));
    }
}
