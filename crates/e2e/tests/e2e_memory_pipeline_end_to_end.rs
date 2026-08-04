//! E2E determinístico — pipeline de memória com providers falsos.
//!
//! **Diferente do `e2e_memory_real_embeddings`:** este usa
//! providers falsos (scripted JSON + vetores fixos) e roda
//! **sempre** no CI de PR, sem dependência de OpenRouter.
//!
//! ## Por que existe (Fase de Ligação, Etapa 3 — revisão pós PR #27)
//!
//! O PR #27 colocou o E2E real com `OpenRouterCompletionProvider`
//! no `verify-external.ps1` step 8 (roda em toda PR). CI vermelho
//! expôs o problema: **toda PR passaria a depender de provedor
//! externo** (latência, cota, custo em dinheiro a cada execução,
//! 429/5xx do OpenRouter). Contraria o ADR-0008 (provedor
//! simulado em CI de PR; real fora).
//!
//! O desenho certo é:
//! - **CI de PR (este teste)** — determinístico, grátis, sempre
//!   roda. Prova que o pipeline está **ligado**:
//!   classificador persiste, embedding é calculado, retriever
//!   pontua semântica acima de zero, escopo respeitado. Mesma
//!   forma do provedor simulado do ADR-0008.
//! - **CI noturno (`e2e_memory_real_embeddings`, `#[ignore]`)**
//!   — com `OpenRouter*` real, secret `OPENROUTER_API_KEY`.
//!   Prova que o **adaptador real** devolve embedding utilizável
//!   e o classificador real classifica. Roda 1x/dia, não
//!   bloqueia PR.
//!
//! Mesma forma do ADR-0019 (Tesseract): provider com dependência
//! externa pesada vai pro noturno, não pro gate de PR.
//!
//! ## O que este teste prova
//!
//! 1. O `LlmMemoryClassifier` com `CompletionProvider` scriptado
//!    (devolve JSON válido hardcoded) classifica a mensagem
//!    como memória de longo prazo e persiste via
//!    `MemoryRepo::insert_auto_capped`.
//! 2. O `EmbeddingWorker` com `FixedVectorEmbeddingAdapter`
//!    (HashMap hardcoded de texto → vetor) calcula embedding
//!    e persiste via `MemoryRepo::set_embedding`.
//! 3. O `HybridRetriever` com o mesmo adapter encontra a
//!    memória por **paráfrase** ("em que ano Maria veio ao
//!    mundo?") com `score_breakdown.semantic` alto (cosine
//!    ~0.99 entre os 2 vetores próximos).
//! 4. O escopo é respeitado (perfil, global).
//!
//! **Cobre o que o `e2e_memory_real_embeddings` cobre menos:**
//! o pipeline. **Não cobre:** que a API real do OpenRouter
//! funciona — isso é o noturno.

#![cfg(windows)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use frederico_core::{ConversationMessage, MemoryScopeType, MemorySourceType, RetrievalRequest};
use frederico_memory::classifier::{
    ClassifierError, CompletionProvider, CompletionRequest, LlmMemoryClassifier, MemoryClassifier,
};
use frederico_memory::embedding::{EmbeddingError, EmbeddingProvider};
use frederico_memory::retriever::{HybridRetriever, Retriever};
use frederico_memory::worker::{EmbeddingWorker, MemoryExtractionJob, MemoryExtractor};
use frederico_storage::Database;

mod common;

// ---------------------------------------------------------------------------
// Providers falsos
// ---------------------------------------------------------------------------

/// `CompletionProvider` que devolve um JSON hardcoded válido do
/// `LlmMemoryClassifier`. Simula a resposta do LLM sem chamar
/// rede. O JSON marca a memória como `fact` no escopo `Profile`
/// (global), com `confidence = 0.9` (acima do threshold 0.6) e
/// `importance = 0.7`.
struct ScriptedCompletionProvider(String);

#[async_trait]
impl CompletionProvider for ScriptedCompletionProvider {
    fn name(&self) -> &str {
        "scripted-test"
    }
    async fn complete(&self, _request: CompletionRequest) -> Result<String, ClassifierError> {
        Ok(self.0.clone())
    }
}

/// `EmbeddingProvider` com vetores fixos hardcoded, escolhidos
/// pra que a **paráfrase** ("em que ano Maria veio ao mundo?")
/// tenha cosine ~0.99 com o texto original ("Maria nasceu em
/// 1985"), e o **distrator** ("qual a capital da França?") seja
/// ortogonal (cosine 0.0). Texto desconhecido recebe um vetor
/// neutro (cosine ~0.5 com qualquer um) — o `Retriever` ainda
/// acha via lexical.
///
/// **Por que vetores fixos e não aleatórios:** o teste precisa
/// ser **determinístico** — toda PR deve ter o mesmo resultado.
/// Vetores hardcoded cumprem isso; embeddings aleatórios dariam
/// flakiness. Mesma forma do provedor simulado do ADR-0008.
struct FixedVectorEmbeddingAdapter {
    map: HashMap<String, Vec<f32>>,
    fallback: Vec<f32>,
}

impl FixedVectorEmbeddingAdapter {
    fn new() -> Self {
        let mut map = HashMap::new();
        // Texto 1 e 2: próximos (paráfrase). Cosine ~0.99.
        map.insert(
            "Maria nasceu em 1985 no Rio de Janeiro.".to_string(),
            vec![1.0, 0.9, 0.0, 0.0],
        );
        map.insert(
            "em que ano Maria veio ao mundo?".to_string(),
            vec![0.95, 0.92, 0.0, 0.0],
        );
        // Distrator: ortogonal ao texto 1 e 2.
        map.insert(
            "qual a capital da Franca?".to_string(),
            vec![0.0, 0.0, 1.0, 0.9],
        );
        Self {
            map,
            fallback: vec![0.5, 0.5, 0.5, 0.5],
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FixedVectorEmbeddingAdapter {
    fn provider_id(&self) -> &str {
        "fixed-vector-test"
    }
    fn model_id(&self) -> &str {
        "fixed-vector-test-v1"
    }
    fn dimensions(&self) -> usize {
        4
    }
    async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(inputs
            .iter()
            .map(|t| {
                self.map
                    .get(*t)
                    .cloned()
                    .unwrap_or_else(|| self.fallback.clone())
            })
            .collect())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_memory_pipeline_end_to_end_deterministic() {
    // 1. DB in-memory.
    let db = Arc::new(Database::open_in_memory().await.expect("open in-memory db"));

    // 2. Providers falsos (scripted JSON + vetores fixos).
    let scripted_json = r#"{
        "record": {
            "scope_type": "profile",
            "scope_id": "user-1",
            "type_": "fact",
            "content": "Maria nasceu em 1985 no Rio de Janeiro.",
            "origin": "user",
            "source_type": "user_message",
            "source_id": null,
            "confidence": 0.9,
            "importance": 0.7,
            "expires_at": null
        },
        "scope": "profile",
        "importance": 0.7,
        "reason": "fato biografico"
    }"#;
    let completion: Arc<dyn CompletionProvider> =
        Arc::new(ScriptedCompletionProvider(scripted_json.to_string()));
    let embedding: Arc<dyn EmbeddingProvider> = Arc::new(FixedVectorEmbeddingAdapter::new());

    // 3. Pipeline idêntico ao real, sem rede.
    let classifier: Arc<dyn MemoryClassifier> = Arc::new(LlmMemoryClassifier::new(completion));
    let extractor = MemoryExtractor::start(db.pool(), classifier);
    let extractor_handle = extractor.handle();
    let embedding_worker = EmbeddingWorker::start(db.pool(), embedding.clone(), 50);
    let embedding_handle = embedding_worker.handle();

    // 4. Cria conversa + enfileira o job.
    let (_conv, conv_id_str) = common::create_memory_test_conversation(&db).await;
    let messages = vec![ConversationMessage {
        role: "user".into(),
        content: "Maria nasceu em 1985 no Rio de Janeiro.".into(),
        source: MemorySourceType::new("user_message"),
    }];
    extractor_handle.enqueue(MemoryExtractionJob::new(
        "e2e-run-mem-det-1",
        conv_id_str,
        messages,
        common::memory_job_now(),
    ));

    // 5. Espera o classificador (scripted, ms) persistir.
    let memory_id = common::wait_for_classified_memory(&db, Duration::from_secs(5)).await;

    // 6. Dispara o embedding worker (fake, ms).
    embedding_handle.enqueue(memory_id);
    common::wait_for_embedding(&db, memory_id, Duration::from_secs(5)).await;

    // 7. Retriever com o provider falso.
    let retriever = HybridRetriever::new(
        &db,
        embedding.clone(),
        frederico_memory::config::ScoringWeights::default(),
    );

    // 8. Query por paráfrase (lexical puro acharia; queremos
    //    provar que a semântica também achou, com score alto).
    let req = RetrievalRequest {
        scope_type: MemoryScopeType::Profile,
        scope_id: String::new(),
        query: "em que ano Maria veio ao mundo?".to_string(),
        k: 8,
        token_budget: 1500,
        recency_epsilon: 0.01,
    };
    let result = retriever.retrieve(req).await.expect("retrieve");

    // 9. Asserções.
    assert!(
        result.semantic_used,
        "embedding provider falso deveria ter rodado \
         (semantic_used=false indica fallback lexical-only, \
         que e exatamente o bug que este teste detecta)"
    );
    assert!(
        !result.hits.is_empty(),
        "retriever deveria ter encontrado a memoria de Maria \
         por parafrase. hits.len()={}",
        result.hits.len()
    );
    let hit = &result.hits[0];
    assert!(
        hit.record.content.contains("1985"),
        "hit deveria conter o ano (1985), got content='{}'",
        hit.record.content
    );
    assert!(
        hit.score_breakdown.semantic > 0.5,
        "score_breakdown.semantic deveria ser alto \
         (cosine ~0.99 entre texto original e parafrase), \
         got: {}",
        hit.score_breakdown.semantic
    );
}
