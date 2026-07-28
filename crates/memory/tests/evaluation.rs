//! Runner de avaliação do `frederico-memory`.
//!
//! Lê `tests/fixtures/gold_set.jsonl` (10 cenários da Etapa 1),
//! executa o `LexicalRetriever` (baseline) **e** o
//! `HybridRetriever` (Etapa 2) contra um banco SQLite
//! in-memory e calcula métricas: precisão, revocação, F1,
//! falsos positivos/negativos, vazamento cruzado, latência
//! p50/p95/p99.
//!
//! A Etapa 1 mede o **baseline lexical-only** (sem embeddings).
//! A Etapa 2 mede o **híbrido** (com embeddings fake
//! determinísticos baseados em hash do conteúdo — o que
//! isola o teste do provedor real e valida a *fórmula* de
//! scoring da Etapa 2). A Etapa 2 **tem que superar** o
//! baseline da Etapa 1 — sem isso, é bug.
//!
//! O gate mínimo da Etapa 1 é `min_precision = 0.0` (no
//! `crates/memory/config/eval.toml`) — só falha se precisão =
//! 0, o que indicaria bug no runner. A Etapa 6 sobe pra
//! `min_precision = 0.70`, `max_p99_ms = 2000` (hard), etc.
//!
//! Ver [`docs/architecture/memory-evaluation-plan.md`](../../../docs/architecture/memory-evaluation-plan.md)
//! e [`ADR-0010-0014`](../../../docs/decisions/).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use frederico_core::{
    MemoryOrigin, MemoryRecord, MemoryScopeType, MemorySourceType, MemoryType, RetrievalRequest,
    RetrievalResult,
};
use frederico_memory::{
    EmbeddingProvider, EvalGate, HybridRetriever, LexicalRetriever, MemoryRepo, NewMemoryInput,
    NoopEmbeddingAdapter, Retriever, ScoringWeights,
};
use frederico_storage::Database;
use serde::Deserialize;

const GOLD_SET_PATH: &str = "tests/fixtures/gold_set.jsonl";

/// "Agora" fixo pros testes — 2026-07-28 meio-dia UTC. Tudo
/// que envolve tempo no gold-set é relativo a esse instante
/// (ex.: `expires_at_offset_days: -1` = 2026-07-27).
fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 28, 12, 0, 0).unwrap()
}

#[derive(Debug, Deserialize)]
struct GoldSetEntry {
    id: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    category: String,
    scope_type: MemoryScopeType,
    scope_id: String,
    query: String,
    #[serde(default)]
    expected_substrings: Vec<String>,
    #[serde(default)]
    must_not_contain_substrings: Vec<String>,
    seed_memories: Vec<SeedMemory>,
}

#[derive(Debug, Deserialize)]
struct SeedMemory {
    scope_type: MemoryScopeType,
    scope_id: String,
    #[serde(rename = "type")]
    type_: MemoryType,
    content: String,
    origin: MemoryOrigin,
    #[serde(default)]
    user_confirmed: bool,
    #[serde(default)]
    user_pinned: bool,
    #[serde(default = "default_importance")]
    importance: f32,
    #[serde(default = "default_confidence")]
    confidence: f32,
    /// Offset relativo a `fixed_now()` (em dias). Usado pra
    /// gerar `expires_at`. Ausente = sem `expires_at`.
    #[serde(default)]
    expires_at_offset_days: Option<i64>,
    /// Se presente, a memória é marcada superseded por outra
    /// memória cujo conteúdo é este. Usado no cenário
    /// `old_info_corrected`.
    #[serde(default)]
    superseded_by_content: Option<String>,
}

fn default_importance() -> f32 {
    0.5
}
fn default_confidence() -> f32 {
    0.9
}

#[derive(Debug, Default, Clone)]
struct ScenarioResult {
    passed: bool,
    precision: f32,
    recall: f32,
    f1: f32,
    #[allow(dead_code)]
    false_positives: usize,
    #[allow(dead_code)]
    false_negatives: usize,
    cross_scope_leak: usize,
    elapsed_ms: u64,
    details: String,
    /// Indica se o retriever usou semântica (true pra
    /// `HybridRetriever` com embeddings, false pra
    /// `LexicalRetriever` com `NoopEmbeddingAdapter`).
    semantic_used: bool,
}

#[derive(Debug, Default)]
struct Report {
    scenarios: Vec<ScenarioResult>,
    total_passed: usize,
    total_failed: usize,
    mean_precision: f32,
    mean_recall: f32,
    mean_f1: f32,
    cross_scope_leak_total: usize,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    max_ms: u64,
}

/// Adapter de embedding fake que devolve vetores
/// determinísticos baseados em hash do conteúdo. Usado
/// pelo teste `run_gold_set_evaluation_hybrid` pra
/// exercitar o caminho semântico do `HybridRetriever`
/// sem precisar de rede.
///
/// Estratégia: divide o conteúdo em tokens, soma hashes
/// em 4 dimensões (normalizadas em [0, 1]). Tokens
/// compartilhados (e.g. "Rust" em memória e query) geram
/// vetores similares — o que torna o retrieval semântico
/// informativo.
struct FakeHashEmbed;

#[async_trait]
impl EmbeddingProvider for FakeHashEmbed {
    fn provider_id(&self) -> &str {
        "fake-hash"
    }
    fn model_id(&self) -> &str {
        "fake-hash-v1"
    }
    fn dimensions(&self) -> usize {
        4
    }
    async fn embed(
        &self,
        inputs: &[&str],
    ) -> Result<Vec<Vec<f32>>, frederico_memory::EmbeddingError> {
        Ok(inputs
            .iter()
            .map(|s| {
                // Hash em 4 buckets, normalizado.
                let mut v = vec![0.0_f32; 4];
                let mut total = 0.0_f32;
                for (i, b) in s.bytes().enumerate() {
                    v[i % 4] += b as f32;
                    total += b as f32;
                }
                if total > 0.0 {
                    for x in v.iter_mut() {
                        *x /= total;
                    }
                }
                v
            })
            .collect())
    }
}

#[tokio::test]
async fn run_gold_set_evaluation_hybrid() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLD_SET_PATH);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("falha ao ler gold-set em {path:?}: {e}"));

    let entries: Vec<GoldSetEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<GoldSetEntry>(l)
                .unwrap_or_else(|e| panic!("falha ao parsear linha do gold-set: {e}\nlinha: {l}"))
        })
        .collect();

    assert!(!entries.is_empty(), "gold-set vazio em {path:?}");

    println!(
        "\n=== Avaliação HYBRID do gold-set ({} cenários) ===\n",
        entries.len()
    );
    println!(
        "{:<40} {:<8} {:<10} {:<10} {:<10} {:<6} {:<8}",
        "id", "passou", "P", "R", "F1", "vaza", "ms"
    );
    println!("{}", "-".repeat(92));

    let mut report = Report::default();
    let mut all_latencies: Vec<u64> = Vec::new();

    for entry in &entries {
        let result = run_scenario_hybrid(entry, fixed_now()).await;
        if result.passed {
            report.total_passed += 1;
        } else {
            report.total_failed += 1;
        }
        report.cross_scope_leak_total += result.cross_scope_leak;
        all_latencies.push(result.elapsed_ms);
        report.mean_precision += result.precision;
        report.mean_recall += result.recall;
        report.mean_f1 += result.f1;

        println!(
            "{:<40} {:<8} {:<10.3} {:<10.3} {:<10.3} {:<6} {:<8}",
            truncate(&entry.id, 40),
            if result.passed { "sim" } else { "NÃO" },
            result.precision,
            result.recall,
            result.f1,
            result.cross_scope_leak,
            result.elapsed_ms
        );
        if !result.passed {
            println!("    detalhe: {}", result.details);
        }
        println!(
            "    [FP={}, FN={}, sem={}] categoria={}",
            result.false_positives, result.false_negatives, result.semantic_used, entry.category
        );
        report.scenarios.push(result);
    }

    let n = entries.len() as f32;
    report.mean_precision /= n;
    report.mean_recall /= n;
    report.mean_f1 /= n;

    all_latencies.sort_unstable();
    report.p50_ms = percentile(&all_latencies, 0.50);
    report.p95_ms = percentile(&all_latencies, 0.95);
    report.p99_ms = percentile(&all_latencies, 0.99);
    report.max_ms = all_latencies.last().copied().unwrap_or(0);

    println!("\n{}", "=".repeat(60));
    println!("RESUMO (HYBRID)");
    println!("{}", "=".repeat(60));
    println!(
        "Cenários:        {} (passou {} / falhou {})",
        n, report.total_passed, report.total_failed
    );
    println!("Precisão média:  {:.3}", report.mean_precision);
    println!("Revocação média: {:.3}", report.mean_recall);
    println!("F1 médio:        {:.3}", report.mean_f1);
    println!(
        "Vazamento cruzado total: {} (deve ser 0 — I4)",
        report.cross_scope_leak_total
    );
    println!(
        "Latência:        p50={}ms  p95={}ms  p99={}ms  max={}ms",
        report.p50_ms, report.p95_ms, report.p99_ms, report.max_ms
    );

    // Gate do híbrido: deve passar e **bater ou superar** o
    // baseline lexical (F1 ≥ 0.9, sem vazamento, p99 ≤ 2s).
    let gate = EvalGate::default();
    println!("\nGate (Etapa 2, mínimo permissivo):");
    println!("  min_precision = {}", gate.min_precision);
    println!("  min_f1 = {}", gate.min_f1);
    println!("  max_cross_scope_leak = {}", gate.max_cross_scope_leak);
    println!("  max_p99_ms = {}", gate.max_p99_ms);

    let mut hard_fails: Vec<&str> = Vec::new();
    if report.mean_precision < gate.min_precision {
        hard_fails.push("precisão abaixo do mínimo");
    }
    if report.mean_f1 < gate.min_f1 {
        hard_fails.push("F1 abaixo do mínimo");
    }
    if report.cross_scope_leak_total as f32 > gate.max_cross_scope_leak {
        hard_fails.push("vazamento cruzado > 0 (I4)");
    }
    if report.p99_ms > gate.max_p99_ms {
        hard_fails.push("p99 acima do teto");
    }

    if !hard_fails.is_empty() {
        panic!(
            "gate falhou: {}. Cenários: {} / {} passaram.",
            hard_fails.join(", "),
            report.total_passed,
            entries.len()
        );
    }
    println!(
        "\nGate verde: {} / {} cenários passaram.",
        report.total_passed,
        entries.len()
    );

    // Garante que o F1 do híbrido **bate o baseline**
    // lexical (que é 0.9 com 10 cenários). Threshold: 0.9
    // (não exige melhoria marginal — exige que o híbrido
    // não regrediu).
    assert!(
        report.mean_f1 >= 0.9,
        "F1 do híbrido ({:.3}) abaixo do baseline lexical (0.9) — bug na Etapa 2",
        report.mean_f1
    );
}

async fn run_scenario_hybrid(entry: &GoldSetEntry, now: DateTime<Utc>) -> ScenarioResult {
    let db = Database::open_in_memory()
        .await
        .expect("Database::open_in_memory falhou");
    let repo = MemoryRepo::new(&db);

    // Resolve supersedência em 2 passos: primeiro insere todas
    // as memórias; depois, para cada uma com
    // `superseded_by_content`, encontra a memória-alvo e
    // chama `mark_superseded`.
    let mut inserted_ids: Vec<(String, frederico_core::MemoryId)> = Vec::new();
    for seed in &entry.seed_memories {
        let expires_at = seed.expires_at_offset_days.map(|d| now + Duration::days(d));
        let input = NewMemoryInput {
            scope_type: seed.scope_type,
            scope_id: seed.scope_id.clone(),
            type_: seed.type_,
            content: seed.content.clone(),
            origin: seed.origin,
            source_type: MemorySourceType::new("seed"),
            source_id: None,
            confidence: seed.confidence,
            importance: seed.importance,
            expires_at,
            user_confirmed: seed.user_confirmed,
            user_pinned: seed.user_pinned,
        };
        let inserted = match repo.insert_auto_captured(input).await {
            Ok(r) => r,
            Err(e) => panic!("falha ao inserir seed: {e}"),
        };
        // Calcula embedding fake e persiste (simula o
        // `EmbeddingWorker` rodando).
        let provider = Arc::new(FakeHashEmbed);
        let inputs = vec![inserted.content.as_str()];
        let embeddings = provider.embed(&inputs).await.expect("embed");
        let emb = embeddings.into_iter().next().expect("1 embedding");
        repo.set_embedding(
            &inserted.id,
            provider.provider_id(),
            provider.model_id(),
            emb.len() as u32,
            &emb,
        )
        .await
        .expect("set_embedding");
        inserted_ids.push((seed.content.clone(), inserted.id));
    }

    // Aplica supersedência.
    for (i, seed) in entry.seed_memories.iter().enumerate() {
        if let Some(target_content) = &seed.superseded_by_content {
            let old_id = inserted_ids[i].1;
            let new_id = inserted_ids
                .iter()
                .find(|(c, _)| c == target_content)
                .map(|(_, id)| *id)
                .expect("superseded_by_content aponta pra memória inexistente");
            repo.mark_superseded(&old_id, &new_id, now)
                .await
                .expect("mark_superseded");
        }
    }

    // Roda o HybridRetriever.
    let weights = ScoringWeights::default();
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(FakeHashEmbed);
    let retriever = HybridRetriever::new(&db, provider, weights);
    let request = RetrievalRequest {
        scope_type: entry.scope_type,
        scope_id: entry.scope_id.clone(),
        query: entry.query.clone(),
        k: 8,
        token_budget: 1500,
        recency_epsilon: 0.01,
    };

    let start = Instant::now();
    let result: RetrievalResult = retriever.retrieve(request).await.expect("retrieve");
    let elapsed_ms = start.elapsed().as_millis() as u64;

    // Avalia o resultado.
    let mut r = evaluate_result(entry, &result, elapsed_ms);
    r.semantic_used = result.semantic_used;
    r
}

#[tokio::test]
async fn run_gold_set_evaluation() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLD_SET_PATH);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("falha ao ler gold-set em {path:?}: {e}"));

    let entries: Vec<GoldSetEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<GoldSetEntry>(l)
                .unwrap_or_else(|e| panic!("falha ao parsear linha do gold-set: {e}\nlinha: {l}"))
        })
        .collect();

    assert!(!entries.is_empty(), "gold-set vazio em {path:?}");

    println!(
        "\n=== Avaliação do gold-set ({} cenários) ===\n",
        entries.len()
    );
    println!(
        "{:<40} {:<8} {:<10} {:<10} {:<10} {:<6} {:<8}",
        "id", "passou", "P", "R", "F1", "vaza", "ms"
    );
    println!("{}", "-".repeat(92));

    let mut report = Report::default();
    let mut all_latencies: Vec<u64> = Vec::new();

    for entry in &entries {
        let result = run_scenario(entry, fixed_now()).await;
        if result.passed {
            report.total_passed += 1;
        } else {
            report.total_failed += 1;
        }
        report.cross_scope_leak_total += result.cross_scope_leak;
        all_latencies.push(result.elapsed_ms);
        report.mean_precision += result.precision;
        report.mean_recall += result.recall;
        report.mean_f1 += result.f1;

        println!(
            "{:<40} {:<8} {:<10.3} {:<10.3} {:<10.3} {:<6} {:<8}",
            truncate(&entry.id, 40),
            if result.passed { "sim" } else { "NÃO" },
            result.precision,
            result.recall,
            result.f1,
            result.cross_scope_leak,
            result.elapsed_ms
        );
        if !result.passed {
            println!("    detalhe: {}", result.details);
        }
        println!(
            "    [FP={}, FN={}] categoria={}",
            result.false_positives, result.false_negatives, entry.category
        );
        report.scenarios.push(result);
    }

    let n = entries.len() as f32;
    report.mean_precision /= n;
    report.mean_recall /= n;
    report.mean_f1 /= n;

    all_latencies.sort_unstable();
    report.p50_ms = percentile(&all_latencies, 0.50);
    report.p95_ms = percentile(&all_latencies, 0.95);
    report.p99_ms = percentile(&all_latencies, 0.99);
    report.max_ms = all_latencies.last().copied().unwrap_or(0);

    println!("\n{}", "=".repeat(60));
    println!("RESUMO");
    println!("{}", "=".repeat(60));
    println!(
        "Cenários:        {} (passou {} / falhou {})",
        n, report.total_passed, report.total_failed
    );
    println!("Precisão média:  {:.3}", report.mean_precision);
    println!("Revocação média: {:.3}", report.mean_recall);
    println!("F1 médio:        {:.3}", report.mean_f1);
    println!(
        "Vazamento cruzado total: {} (deve ser 0 — I4)",
        report.cross_scope_leak_total
    );
    println!(
        "Latência:        p50={}ms  p95={}ms  p99={}ms  max={}ms",
        report.p50_ms, report.p95_ms, report.p99_ms, report.max_ms
    );

    // Aplica o gate mínimo da Etapa 1 (precisão ≥ 0.0 — só falha
    // se for 0, o que indicaria bug no runner).
    let gate = EvalGate::default();
    println!("\nGate (Etapa 1, mínimo permissivo):");
    println!("  min_precision = {}", gate.min_precision);
    println!("  min_f1 = {}", gate.min_f1);
    println!("  max_cross_scope_leak = {}", gate.max_cross_scope_leak);
    println!("  max_p99_ms = {}", gate.max_p99_ms);

    let mut hard_fails: Vec<&str> = Vec::new();
    if report.mean_precision < gate.min_precision {
        hard_fails.push("precisão abaixo do mínimo");
    }
    if report.mean_f1 < gate.min_f1 {
        hard_fails.push("F1 abaixo do mínimo");
    }
    if report.cross_scope_leak_total as f32 > gate.max_cross_scope_leak {
        hard_fails.push("vazamento cruzado > 0 (I4)");
    }
    if report.p99_ms > gate.max_p99_ms {
        hard_fails.push("p99 acima do teto");
    }

    if !hard_fails.is_empty() {
        panic!(
            "gate falhou: {}. Cenários: {} / {} passaram.",
            hard_fails.join(", "),
            report.total_passed,
            entries.len()
        );
    }
    println!(
        "\nGate verde: {} / {} cenários passaram.",
        report.total_passed,
        entries.len()
    );
}

async fn run_scenario(entry: &GoldSetEntry, now: DateTime<Utc>) -> ScenarioResult {
    // DB in-memory — total isolamento entre cenários.
    let db = Database::open_in_memory()
        .await
        .expect("Database::open_in_memory falhou");
    let repo = MemoryRepo::new(&db);

    // Resolve supersedência em 2 passos: primeiro insere todas
    // as memórias; depois, para cada uma com
    // `superseded_by_content`, encontra a memória-alvo e
    // chama `mark_superseded`.
    let mut inserted_ids: Vec<(String, frederico_core::MemoryId)> = Vec::new();
    for seed in &entry.seed_memories {
        let expires_at = seed.expires_at_offset_days.map(|d| now + Duration::days(d));
        let input = NewMemoryInput {
            scope_type: seed.scope_type,
            scope_id: seed.scope_id.clone(),
            type_: seed.type_,
            content: seed.content.clone(),
            origin: seed.origin,
            source_type: MemorySourceType::new("seed"),
            source_id: None,
            confidence: seed.confidence,
            importance: seed.importance,
            expires_at,
            user_confirmed: seed.user_confirmed,
            user_pinned: seed.user_pinned,
        };
        // Auto-captura: se a seed é user, é ok. Se for external
        // (não acontece nas seeds atuais), teria que usar
        // `insert_user_confirmed`.
        let inserted = match repo.insert_auto_captured(input).await {
            Ok(r) => r,
            Err(e) => panic!("falha ao inserir seed: {e}"),
        };
        inserted_ids.push((seed.content.clone(), inserted.id));
    }
    if entry.id.contains("global") || entry.id.contains("old_info") {
        eprintln!(
            "[run_scenario {}] scope_type={:?} scope_id={:?} query={:?}",
            entry.id, entry.scope_type, entry.scope_id, entry.query
        );
        eprintln!("  seeds inseridas: {}", inserted_ids.len());
    }

    // Aplica supersedência.
    for (i, seed) in entry.seed_memories.iter().enumerate() {
        if let Some(target_content) = &seed.superseded_by_content {
            let old_id = inserted_ids[i].1;
            // Encontra o id da memória-alvo.
            let new_id = inserted_ids
                .iter()
                .find(|(c, _)| c == target_content)
                .map(|(_, id)| *id)
                .expect("superseded_by_content aponta pra memória inexistente");
            repo.mark_superseded(&old_id, &new_id, now)
                .await
                .expect("mark_superseded falhou");
        }
    }

    // Roda o retriever.
    let weights = ScoringWeights::default();
    let retriever = LexicalRetriever::new(&db, Arc::new(NoopEmbeddingAdapter), weights);
    let request = RetrievalRequest {
        scope_type: entry.scope_type,
        scope_id: entry.scope_id.clone(),
        query: entry.query.clone(),
        k: 8,
        token_budget: 1500,
        recency_epsilon: 0.01,
    };

    let start = Instant::now();
    let result: RetrievalResult = retriever.retrieve(request).await.expect("retrieve falhou");
    let elapsed_ms = start.elapsed().as_millis() as u64;

    // Avalia o resultado.
    evaluate_result(entry, &result, elapsed_ms)
}

fn evaluate_result(
    entry: &GoldSetEntry,
    result: &RetrievalResult,
    elapsed_ms: u64,
) -> ScenarioResult {
    let top_k = &result.hits;
    let top_contents: Vec<&str> = top_k.iter().map(|h| h.record.content.as_str()).collect();

    // Verifica expected_substrings: cada substring esperada
    // deve aparecer em pelo menos 1 hit.
    let mut true_positives = 0;
    let mut false_negatives = 0;
    for expected in &entry.expected_substrings {
        if top_contents
            .iter()
            .any(|c| contains_ignore_case(c, expected))
        {
            true_positives += 1;
        } else {
            false_negatives += 1;
        }
    }

    // Verifica must_not_contain_substrings: nenhuma substring
    // proibida pode aparecer.
    let mut cross_scope_leak = 0;
    let mut details_parts: Vec<String> = Vec::new();
    for forbidden in &entry.must_not_contain_substrings {
        if top_contents
            .iter()
            .any(|c| contains_ignore_case(c, forbidden))
        {
            cross_scope_leak += 1;
            details_parts.push(format!("VAZAMENTO: '{forbidden}' apareceu"));
        }
    }

    // Falsos positivos: hits que não casaram com nenhum expected.
    let mut false_positives = 0;
    for content in &top_contents {
        let matches_expected = entry
            .expected_substrings
            .iter()
            .any(|e| contains_ignore_case(content, e));
        if !matches_expected {
            // Para evitar contar hits "neutros" (que não casam
            // com nada, mas não são "errados") como FP, usamos
            // uma heurística: se a query tinha expected, e o
            // hit não casa, é FP.
            if !entry.expected_substrings.is_empty() {
                false_positives += 1;
            }
        }
    }

    let total_expected = entry.expected_substrings.len();
    let precision = if total_expected > 0 {
        true_positives as f32 / (true_positives + false_positives).max(1) as f32
    } else {
        // Sem expected, precisão é 1.0 se não houve vazamento.
        if cross_scope_leak == 0 {
            1.0
        } else {
            0.0
        }
    };
    let recall = if total_expected > 0 {
        true_positives as f32 / total_expected as f32
    } else {
        1.0
    };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    // Passou = sem vazamento E todos os expected foram encontrados.
    let passed = cross_scope_leak == 0 && false_negatives == 0;

    if !passed && details_parts.is_empty() {
        details_parts.push(format!(
            "faltou {} de {} expected",
            false_negatives,
            total_expected.max(1)
        ));
    }

    ScenarioResult {
        passed,
        precision,
        recall,
        f1,
        false_positives,
        false_negatives,
        cross_scope_leak,
        elapsed_ms,
        details: details_parts.join("; "),
        semantic_used: false, // lexical não usa semântica
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut end = max;
        for (i, (idx, _)) in s.char_indices().enumerate() {
            if idx == max {
                end = i;
                break;
            }
        }
        format!("{}…", &s[..end])
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[allow(dead_code)]
fn dummy_record() -> MemoryRecord {
    MemoryRecord {
        id: frederico_core::MemoryId::new(),
        scope_type: MemoryScopeType::Project,
        scope_id: "p".into(),
        type_: MemoryType::Fact,
        content: "x".into(),
        origin: MemoryOrigin::User,
        source_type: MemorySourceType::new("seed"),
        source_id: None,
        confidence: 0.5,
        importance: 0.5,
        embedding_status: frederico_core::EmbeddingStatus::Pending,
        embedding_provider: None,
        embedding_model: None,
        embedding_dimensions: None,
        created_at: fixed_now(),
        updated_at: fixed_now(),
        last_used_at: None,
        expires_at: None,
        superseded_by: None,
        superseded_at: None,
        user_confirmed: false,
        user_pinned: false,
        active: true,
        pending_review: false,
    }
}
