//! E2E — memória REAL (OpenRouter + gpt-4o-mini +
//! text-embedding-3-small). **Só roda no CI noturno** (1x/dia,
//! secret `OPENROUTER_API_KEY` requerida). Marcado
//! `#[ignore]` — ativado pelo `ci-nightly.yml` step novo
//! (não pelo `verify-external.ps1`).
//!
//! ## Por que `#[ignore]` (decisão da Etapa 3 da Fase de Ligação
//! — revisão pós PR #27)
//!
//! O PR #27 colocou este teste no `verify-external.ps1` step 8
//! (roda em toda PR). CI vermelho expôs o problema: **toda
//! PR passaria a depender de OpenRouter** (latência, cota,
//! custo em dinheiro a cada execução, 429/5xx). Contraria
//! o ADR-0008 (provedor simulado em CI de PR; real fora).
//!
//! Mesma forma do ADR-0019 (Tesseract): provider com
//! dependência externa pesada vai pro noturno. O
//! `e2e_memory_pipeline_end_to_end_deterministic` (sempre
//! roda, sem rede) cobre o pipeline. **Este teste cobre o
//! que o determinístico não cobre:** que o adapter real
//! (`OpenRouterCompletionProvider` + `OpenRouterEmbeddingAdapter`)
//! funciona ponta a ponta contra a API real.
//!
//! ## O que este teste prova
//!
//! 1. O `OpenRouterCompletionProvider` (gpt-4o-mini) autentica
//!    no OpenRouter e classifica a mensagem como memória.
//! 2. O `OpenRouterEmbeddingAdapter` (text-embedding-3-small)
//!    autentica e calcula embedding válido.
//! 3. O `HybridRetriever` com o provider real recupera por
//!    **paráfrase** com `score_breakdown.semantic > 0`.
//!
//! Ver [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md).

#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use frederico_core::{ConversationMessage, MemoryScopeType, MemorySourceType, RetrievalRequest};
use frederico_memory::classifier::{
    CompletionProvider, LlmMemoryClassifier, OpenRouterCompletionProvider,
};
use frederico_memory::embedding::OpenRouterEmbeddingAdapter;
use frederico_memory::retriever::{HybridRetriever, Retriever};
use frederico_memory::worker::{MemoryExtractionJob, MemoryExtractor};
use frederico_storage::Database;
use secrecy::SecretString;

mod common;

/// Liga o `tracing` para que a **razão** de uma falha apareça no
/// log do CI.
///
/// Sem isto, este teste só sabe dizer que nada foi persistido em 30
/// segundos. O `MemoryExtractor` captura o erro do classificador e o
/// registra via `tracing::warn!` **sem propagar** (regra do ADR-0012
/// §2, deliberada: um erro de provedor não pode derrubar o worker), e
/// a decisão de "não criar memória" sai como `tracing::info!`. Com
/// nenhum subscriber instalado, os dois desaparecem — e as três
/// causas que a mensagem de timeout lista (provider indisponível,
/// classificador descartou, bug de pipeline) ficam indistinguíveis.
///
/// Foi exatamente o que aconteceu no run `32031207520` (2026-08-17),
/// o primeiro depois de o secret `OPENROUTER_API_KEY` passar a
/// existir: o passo finalmente executou, falhou por timeout, e o log
/// não permitiu dizer se a chave não tinha crédito, se o modelo
/// estava indisponível, ou se o classificador simplesmente decidiu
/// que a frase não era memorável.
///
/// `with_test_writer` manda a saída pelo mecanismo de captura do
/// harness de teste, então ela aparece junto do panic quando o teste
/// falha — que é justamente quando ela importa.
///
/// `try_init` em vez de `init`: se outro teste do mesmo binário já
/// instalou um subscriber global, isto vira no-op em vez de panic.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filtro = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,frederico_memory=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filtro)
        .with_test_writer()
        .try_init();
}

/// Helper: panic com mensagem clara se `OPENROUTER_API_KEY`
/// não está no env. **Sempre panic** — não tem skip silencioso
/// (mesma regra do `e2e_docs_generate_with_real_worker` apontando
/// pro `bootstrap.ps1`).
fn memory_real_providers_or_fail() -> SecretString {
    match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) if !key.is_empty() => SecretString::new(key.into_boxed_str()),
        _ => panic!(
            "OPENROUTER_API_KEY ausente. O e2e_memory_real_embeddings \
             precisa da key real do OpenRouter pra chamar o LLM \
             classificador (gpt-4o-mini) e o embedding \
             (text-embedding-3-small). \n\nCI noturno: a env e setada \
             como secret do repositorio em ci-nightly.yml. \
             Local: `export OPENROUTER_API_KEY=<sua_key>` antes \
             de `cargo test -p frederico-e2e -- \
             --include-ignored e2e_memory_real_embeddings`."
        ),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requer OPENROUTER_API_KEY em runtime; roda no CI noturno via ci-nightly.yml (nao no CI de PR)"]
async fn e2e_memory_real_embeddings_recall_by_paraphrase() {
    init_tracing();
    let key = memory_real_providers_or_fail();
    let db = Arc::new(Database::open_in_memory().await.expect("open in-memory db"));

    // Providers reais.
    let completion: Arc<dyn CompletionProvider> =
        Arc::new(OpenRouterCompletionProvider::new(key.clone()));
    let embedding: Arc<dyn frederico_memory::embedding::EmbeddingProvider> =
        Arc::new(OpenRouterEmbeddingAdapter::new(key.clone()));

    // Pipeline.
    let classifier: Arc<dyn frederico_memory::classifier::MemoryClassifier> =
        Arc::new(LlmMemoryClassifier::new(completion));
    let extractor = MemoryExtractor::start(db.pool(), classifier);
    let extractor_handle = extractor.handle();
    let embedding_worker =
        frederico_memory::worker::EmbeddingWorker::start(db.pool(), embedding.clone(), 50);
    let embedding_handle = embedding_worker.handle();

    // Conversa + job.
    let (_conv, conv_id_str) = common::create_memory_test_conversation(&db).await;
    let messages = vec![ConversationMessage {
        role: "user".into(),
        content: "Maria nasceu em 1985 no Rio de Janeiro.".into(),
        source: MemorySourceType::new("user_message"),
    }];
    extractor_handle.enqueue(MemoryExtractionJob::new(
        "e2e-run-memory-real-1",
        conv_id_str,
        messages,
        Utc::now(),
    ));

    // Espera o classificador real (HTTP roundtrip, ~2-5s).
    let memory_id = common::wait_for_classified_memory(&db, Duration::from_secs(30)).await;

    // Espera o embedding real.
    embedding_handle.enqueue(memory_id);
    common::wait_for_embedding(&db, memory_id, Duration::from_secs(30)).await;

    // Retriever real + query por paráfrase.
    let retriever = HybridRetriever::new(
        &db,
        embedding.clone(),
        frederico_memory::config::ScoringWeights::default(),
    );
    let req = RetrievalRequest {
        scope_type: MemoryScopeType::Profile,
        scope_id: String::new(),
        query: "em que ano Maria veio ao mundo?".to_string(),
        k: 8,
        token_budget: 1500,
        recency_epsilon: 0.01,
    };
    let result = retriever.retrieve(req).await.expect("retrieve");

    assert!(
        result.semantic_used,
        "embedding provider real deveria ter rodado"
    );
    assert!(
        !result.hits.is_empty(),
        "retriever deveria ter encontrado Maria por paráfrase"
    );
    let hit = &result.hits[0];
    assert!(
        hit.record.content.contains("1985"),
        "hit deveria conter 1985, got: {}",
        hit.record.content
    );
    assert!(
        hit.score_breakdown.semantic > 0.0,
        "score_breakdown.semantic deveria ser > 0, got: {}",
        hit.score_breakdown.semantic
    );
}
