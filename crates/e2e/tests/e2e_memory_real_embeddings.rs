//! E2E — memória real em produção: classificador LLM + embedding
//! reais, salvos e recuperados por similaridade semântica.
//!
//! **Caminho exercitado (sem subir a casca Tauri):**
//! `MemoryExtractor::start` (background) + `LlmMemoryClassifier`
//! com `OpenRouterCompletionProvider` real (gpt-4o-mini) +
//! `EmbeddingWorker` (background) com
//! `OpenRouterEmbeddingAdapter` real (text-embedding-3-small) +
//! `HybridRetriever` com o mesmo provider real. **O teste prova
//! comportamento end-to-end do pipeline de memória** (não
//! fiação — o ponto do PR #26 é exatamente evitar testes
//! "afirma construção").
//!
//! Ver [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (fronteira dos E2E) e [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2.
//!
//! ## O que este teste prova
//!
//! 1. O `LlmMemoryClassifier` real (chama OpenRouter + gpt-4o-mini)
//!    classifica uma mensagem factual como memória de longo
//!    prazo e persiste via `MemoryRepo::insert_auto_captured`.
//! 2. O `OpenRouterEmbeddingAdapter` real (chama OpenRouter +
//!    text-embedding-3-small) calcula embedding e persiste via
//!    `MemoryRepo::set_embedding`.
//! 3. O `HybridRetriever` com embedding real **encontra a
//!    memória por paráfrase** ("em que ano Maria veio ao
//!    mundo?") que lexical puro **não** acharia — "veio ao
//!    mundo" não tem overlap textual com "nasceu em 1985".
//! 4. `score_breakdown.semantic > 0` prova que a semântica
//!    contribuiu (vs. `HybridRetriever` com `NoopEmbeddingAdapter`
//!    que daria `semantic_used = false`).
//!
//! **Helper `memory_real_providers_or_skip!`:** se
//! `OPENROUTER_API_KEY` não está no env, **panic com mensagem
//! clara** apontando pra env var. Marcado `#[ignore]` —
//! ativado pelo `verify-external.ps1` step 8. CI seta a env
//! via secret do repositório; dev local `export
//! OPENROUTER_API_KEY=<key>` antes de `--include-ignored`.

#![cfg(windows)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use frederico_core::{
    ConversationMessage, EmbeddingStatus, MemoryId, MemoryRecord, MemoryScopeType,
    MemorySourceType, ModelId, ProviderId, RetrievalRequest,
};
use frederico_memory::classifier::{
    CompletionProvider, LlmMemoryClassifier, OpenRouterCompletionProvider,
};
use frederico_memory::embedding::OpenRouterEmbeddingAdapter;
use frederico_memory::memory_repo::MemoryRepo;
use frederico_memory::retriever::{HybridRetriever, Retriever};
use frederico_memory::worker::{EmbeddingWorker, MemoryExtractionJob, MemoryExtractor};
use frederico_storage::{ConversationRepo, Database};
use secrecy::SecretString;

/// Helper: panic com mensagem clara apontando pra env var
/// `OPENROUTER_API_KEY` se ela não estiver setada. Mesma
/// forma que o `doc_worker_config` do
/// `e2e_docs_generate_with_real_worker` aponta pro
/// `bootstrap.ps1`.
fn memory_real_providers_or_skip() -> SecretString {
    match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) if !key.is_empty() => SecretString::new(key.into_boxed_str()),
        _ => panic!(
            "OPENROUTER_API_KEY ausente. O e2e_memory_real_embeddings \
             precisa da key real do OpenRouter pra chamar o LLM \
             classificador (gpt-4o-mini) e o embedding \
             (text-embedding-3-small). \
             \n\nCI: a env e setada no step 8 do verify-external.ps1. \
             Local: `export OPENROUTER_API_KEY=<sua_key>` antes \
             de `cargo test -p frederico-e2e -- \
             --include-ignored e2e_memory_real_embeddings`."
        ),
    }
}

/// E2E principal: salvar um facto, recuperar por paráfrase.
///
/// O ponto crítico é que a **paráfrase** ("em que ano Maria
/// veio ao mundo?") não tem overlap lexical com o texto
/// original ("Maria nasceu em 1985"). Lexical puro
/// (FTS5/BM25) não acharia. O embedding real (cosine
/// semântica) tem que achar — e `score_breakdown.semantic
/// > 0` prova que contribuiu.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requer OPENROUTER_API_KEY em runtime (rode export antes; CI ativa via verify-external.ps1 step 8)"]
async fn e2e_memory_real_embeddings_recall_by_paraphrase() {
    let key = memory_real_providers_or_skip();
    let db = Arc::new(Database::open_in_memory().await.expect("open in-memory db"));

    // 1. Completion provider real (gpt-4o-mini via OpenRouter).
    let completion: Arc<dyn CompletionProvider> =
        Arc::new(OpenRouterCompletionProvider::new(key.clone()));

    // 2. Embedding provider real (text-embedding-3-small via
    //    OpenRouter, 1536 dim).
    let embedding: Arc<dyn frederico_memory::embedding::EmbeddingProvider> =
        Arc::new(OpenRouterEmbeddingAdapter::new(key.clone()));

    // 3. Classificador LLM + MemoryExtractor em background.
    let classifier: Arc<dyn frederico_memory::classifier::MemoryClassifier> =
        Arc::new(LlmMemoryClassifier::new(completion));
    let extractor = MemoryExtractor::start(db.pool(), classifier);
    let extractor_handle = extractor.handle();

    // 4. EmbeddingWorker em background — processa memórias
    //    com `embedding_status = Pending`.
    let embedding_worker = EmbeddingWorker::start(db.pool(), embedding.clone(), 50);
    let embedding_handle = embedding_worker.handle();

    // 5. Cria uma conversa de teste (precisa existir pra
    //    satisfazer a FK; o LLM pode emitir o `conversation_id`
    //    que o classificador vai usar como scope).
    let conv_repo = ConversationRepo::new(&*db);
    let conv = conv_repo
        .create(
            &ProviderId::new("openai"),
            &ModelId::new("gpt-4o-mini"),
            Some("e2e memory real"),
        )
        .await
        .expect("cria conversa de teste");
    let conv_id_str = conv.id.as_uuid().to_string();

    // 6. Enfileira o job de extração com a mensagem factual.
    let messages = vec![
        ConversationMessage {
            role: "user".into(),
            content: "Maria nasceu em 1985 no Rio de Janeiro.".into(),
            source: MemorySourceType::new("user_message"),
        },
        ConversationMessage {
            role: "assistant".into(),
            content: "Vou lembrar dessa informacao sobre a Maria.".into(),
            source: MemorySourceType::new("assistant_message"),
        },
    ];
    extractor_handle.enqueue(MemoryExtractionJob::new(
        "e2e-run-memory-real-1",
        conv_id_str.clone(),
        messages,
        Utc::now(),
    ));

    // 7. Espera o classificador processar e persistir a
    //    memória (polling `list_pending_embeddings` ate
    //    ter 1+). O classificador real gasta ~2-5s (HTTP
    //    roundtrip + JSON parse).
    let memory_id = wait_for_classified_memory(&db, Duration::from_secs(30)).await;
    eprintln!(
        "classificador persistiu memoria id={memory_id} (content='{}')",
        peek_memory_content(&db, memory_id).await
    );

    // 8. Dispara o embedding worker pra essa memória e
    //    espera o embedding ser calculado.
    embedding_handle.enqueue(memory_id);
    wait_for_embedding(&db, memory_id, Duration::from_secs(30)).await;
    eprintln!("embedding calculado para memoria id={memory_id}");

    // 9. Constrói o retriever com o embedding provider real
    //    e faz retrieval por paráfrase.
    let retriever = HybridRetriever::new(
        &*db,
        embedding.clone(),
        frederico_memory::config::ScoringWeights::default(),
    );

    // A query é uma paráfrase que **nao** tem overlap lexical
    // com o texto original. "veio ao mundo" != "nasceu em";
    // "ano" so sobrepoe se o sistema achar sinonimos
    // (FTS5/BM25 nao faz isso). Embedding semantico acha.
    //
    // Escopo: a memória pode ter sido classificada como
    // Profile (global) ou Conversation (específico). Pra
    // cobrir os 2 caminhos, busca em Profile (escopo global
    // com `scope_id` vazio) que atravesa todas as conversas.
    let req = RetrievalRequest {
        scope_type: MemoryScopeType::Profile,
        scope_id: String::new(),
        query: "em que ano Maria veio ao mundo?".into(),
        k: 8,
        token_budget: 1500,
        recency_epsilon: 0.01,
    };
    let result = retriever.retrieve(req).await.expect("retrieve");

    // 10. Asserções.
    assert!(
        result.semantic_used,
        "embedding provider real deveria ter rodado \
         (semantic_used=false indica fallback lexical-only, \
         que e exatamente o bug que este teste detecta)"
    );
    assert!(
        !result.hits.is_empty(),
        "retriever deveria ter encontrado a memoria de Maria \
         por parafrase. hits.len()={} elapsed_ms={} \
         (talvez o classificador tenha decidido escopo \
         Conversation em vez de Profile, ou a parafrase \
         nao casou o suficiente)",
        result.hits.len(),
        result.elapsed_ms
    );
    let hit = &result.hits[0];
    assert!(
        hit.record.content.contains("1985"),
        "hit deveria conter o ano (1985), got content='{}'",
        hit.record.content
    );
    assert!(
        hit.score_breakdown.semantic > 0.0,
        "score_breakdown.semantic deveria ser > 0 \
         (embedding real contribuiu), got: {} \
         (0.0 indica que o retriever caiu pra lexical-only \
         mesmo com provider real)",
        hit.score_breakdown.semantic
    );
    eprintln!(
        "retrieve OK: hit.content='{}' score={:.3} \
         breakdown={:?} semantic_used={} elapsed_ms={}",
        hit.record.content, hit.score, hit.score_breakdown, result.semantic_used, result.elapsed_ms
    );
}

// ---------------------------------------------------------------------------
// Helpers de polling
// ---------------------------------------------------------------------------

/// Espera o `MemoryExtractor` (em background) classificar
/// o job e persistir uma memória. Retorna o `MemoryId` da
/// primeira memória persistida (assume que soh 1 memoria
/// foi inserida neste teste).
async fn wait_for_classified_memory(db: &Database, timeout: Duration) -> MemoryId {
    let start = Instant::now();
    loop {
        let repo = MemoryRepo::new(db);
        match repo.list_pending_embeddings(100).await {
            Ok(pending) if !pending.is_empty() => return pending[0].id,
            Ok(_) => {} // vazio, ainda processando
            Err(e) => {
                eprintln!("list_pending_embeddings errou (transient?): {e}");
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "timeout ({}s) esperando classificador processar \
                 o job. Possiveis causas: OPENROUTER_API_KEY \
                 invalida, OpenRouter fora do ar, ou classificador \
                 decidiu descartar (confidence < threshold). \
                 Verifique os logs do MemoryExtractor.",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Espera o `EmbeddingWorker` (em background) calcular o
/// embedding da memória e marcar `embedding_status = Embedded`.
async fn wait_for_embedding(db: &Database, id: MemoryId, timeout: Duration) {
    let start = Instant::now();
    loop {
        let repo = MemoryRepo::new(db);
        if let Ok(Some(m)) = repo.get(&id).await {
            if m.embedding_status == EmbeddingStatus::Ready {
                return;
            }
            // Falha permanente: o worker marcou como Failed e
            // nao vai tentar de novo. Panica com mensagem clara.
            if m.embedding_status == EmbeddingStatus::Failed {
                panic!(
                    "EmbeddingWorker marcou memoria {id} como Failed. \
                     Causas provaveis: timeout do OpenRouter (1.5s), \
                     erro HTTP, ou dim mismatch. Verifique os logs."
                );
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "timeout ({}s) esperando embedding da memoria {id}. \
                 Verifique se o EmbeddingWorker foi spawnado.",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Lê o conteúdo da memória (helper de log).
async fn peek_memory_content(db: &Database, id: MemoryId) -> String {
    let repo = MemoryRepo::new(db);
    repo.get(&id)
        .await
        .ok()
        .flatten()
        .map(|m: MemoryRecord| m.content)
        .unwrap_or_else(|| "<memoria nao encontrada>".to_string())
}
