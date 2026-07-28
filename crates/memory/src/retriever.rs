//! `Retriever` — busca memórias visíveis dado um contexto.
//!
//! **Etapa 1**: `LexicalRetriever` (BM25 do FTS5 + recência +
//! importance + confirmation; sem semântica). Caminho
//! baseline que **funciona sem embeddings** (regra do `PROMPT
//! MESTRE` §10.13).
//!
//! **Etapa 2**: `HybridRetriever` (lexical + cosine semântica
//! + recência + importance + confirmation). **Tem que
//!   superar o baseline da Etapa 1** — sem isso, é bug.
//!
//! Ambos implementam o trait `Retriever`. O `HybridRetriever`
//! aceita qualquer `Arc<dyn EmbeddingProvider>`. Se o
//! provider for `NoopEmbeddingAdapter` (Etapa 1), age como
//! o `LexicalRetriever` (sem semântica, mas a `LexicalRetriever`
//! continua existindo pra explicitar o caminho "sem
//! embeddings" no gold-set).
//!
//! **Orçamento de latência (regra §10.13):** 2s. Breakdown:
//! 1.5s pro adapter de embedding (timeout do `reqwest`),
//! ~300ms pro FTS5 + cosine + ordenação + I/O de banco.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use frederico_core::{
    EmbeddingStatus, MemoryHit, MemoryRecord, RetrievalRequest, RetrievalResult, ScoreBreakdown,
};

use crate::config::ScoringWeights;
use crate::embedding::EmbeddingProvider;
use crate::embedding_codec::cosine_similarity;
use crate::error::MemoryResult;
use crate::memory_repo::MemoryRepo;

/// Orçamento total do `Retriever::retrieve` (regra do `PROMPT
/// MESTRE` §10.13). Estourou, a execução segue sem semântica
/// e o fato é registrado.
pub const RETRIEVER_BUDGET: Duration = Duration::from_millis(2000);

/// Trait do retriever. A Etapa 1 expõe a forma lexical
/// (via [`LexicalRetriever`]). A Etapa 2 adiciona a forma
/// híbrida (via [`HybridRetriever`]).
pub trait Retriever: Send + Sync {
    /// Nome do retriever (pra log e painel de debug da Etapa 5).
    fn name(&self) -> &str;

    /// Busca memórias visíveis dado o request.
    fn retrieve(
        &self,
        request: RetrievalRequest,
    ) -> impl std::future::Future<Output = MemoryResult<RetrievalResult>> + Send;
}

/// Retriever lexical (Etapa 1). Usa só BM25 do FTS5 mais os
/// sinais não-semânticos do `ADR-0011 §2`: recência,
/// importância, confirmação. Sem semântica (provider é
/// `NoopEmbeddingAdapter`).
///
/// Funciona sem embeddings (`PROMPT MESTRE` §10.13) — é o
/// baseline que a Etapa 2 (híbrido) tem que **superar**.
pub struct LexicalRetriever<'a> {
    repo: MemoryRepo<'a>,
    embedding: Arc<dyn EmbeddingProvider>,
    weights: ScoringWeights,
}

impl<'a> LexicalRetriever<'a> {
    /// Cria um `LexicalRetriever` com o `Database`, o
    /// `EmbeddingProvider` (use `NoopEmbeddingAdapter` pro
    /// caminho lexical-only da Etapa 1) e os pesos do scoring.
    #[must_use]
    pub fn new(
        db: &'a frederico_storage::Database,
        embedding: Arc<dyn EmbeddingProvider>,
        weights: ScoringWeights,
    ) -> Self {
        Self {
            repo: MemoryRepo::new(db),
            embedding,
            weights,
        }
    }

    /// Acesso ao `MemoryRepo` (pra testes que querem verificar
    /// o estado do banco após um retrieve).
    #[must_use]
    pub fn repo(&self) -> &MemoryRepo<'a> {
        &self.repo
    }
}

impl Retriever for LexicalRetriever<'_> {
    fn name(&self) -> &str {
        "lexical"
    }

    async fn retrieve(&self, request: RetrievalRequest) -> MemoryResult<RetrievalResult> {
        let start = Instant::now();
        let now = Utc::now();

        if request.query.trim().is_empty() {
            return Ok(RetrievalResult {
                hits: Vec::new(),
                semantic_used: false,
                elapsed_ms: start.elapsed().as_millis() as u64,
            });
        }

        let raw = self
            .repo
            .search_lexical(
                request.scope_type,
                &request.scope_id,
                &request.query,
                request.k.max(self.weights.max_k),
                now,
            )
            .await?;

        let semantic_used = self.embedding.provider_id() != "noop";

        let mut hits: Vec<MemoryHit> = raw
            .into_iter()
            .map(|(record, lexical_score)| {
                let recency = recency_factor(&record, now, self.weights.recency_half_life_days);
                let importance = record.importance;
                let confirmation = confirmation_factor(&record, &self.weights);

                let score = self.weights.lexical_weight * lexical_score
                    + self.weights.recency_weight * recency
                    + self.weights.semantic_weight * 0.0
                    + self.weights.importance_weight * importance
                    + self.weights.confirmation_weight * confirmation;

                let breakdown = ScoreBreakdown {
                    lexical: lexical_score,
                    recency,
                    semantic: 0.0,
                    importance,
                    confirmation,
                    scope_match: true,
                };

                let explanation = build_explanation(&breakdown, &record);

                MemoryHit {
                    record,
                    score,
                    score_breakdown: breakdown,
                    explanation,
                }
            })
            .collect();

        // Regra de desempate por recência (ADR-0011 §5).
        hits.sort_by(|a, b| {
            let a_ts = a.record.last_used_at.unwrap_or(a.record.updated_at);
            let b_ts = b.record.last_used_at.unwrap_or(b.record.updated_at);
            match b
                .score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
            {
                std::cmp::Ordering::Equal => b_ts.cmp(&a_ts),
                other => other,
            }
        });

        hits.truncate(request.k);

        Ok(RetrievalResult {
            hits,
            semantic_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// Retriever híbrido (Etapa 2). Combina BM25 do FTS5 com
/// cosine similarity sobre embeddings. **Tem que superar
/// o baseline lexical** (Etapa 1) — sem isso, é bug.
///
/// **Algoritmo:**
/// 1. Calcula embedding da query (timeout 1.5s no adapter).
/// 2. Pega top-N (`4 * k`) por BM25 do FTS5 (lexical pré-filtro).
/// 3. Carrega embeddings dos top-N (mesmo `provider`/`model`
///    da config atual).
/// 4. Calcula score composto pra cada candidato (lexical +
///    recency + semantic + importance + confirmation).
/// 5. Ordena por score, desempate por recência, trunca pra `k`.
///
/// Se o embedding da query falhar (timeout, indisponível),
/// cai pra lexical puro (mesma lógica do `LexicalRetriever`).
pub struct HybridRetriever<'a> {
    repo: MemoryRepo<'a>,
    embedding: Arc<dyn EmbeddingProvider>,
    weights: ScoringWeights,
}

impl<'a> HybridRetriever<'a> {
    /// Cria um `HybridRetriever` com o `Database`, o
    /// `EmbeddingProvider` (default `OpenRouterEmbeddingAdapter`
    /// em produção, `NoopEmbeddingAdapter` se quiser forçar
    /// lexical puro) e os pesos do scoring.
    #[must_use]
    pub fn new(
        db: &'a frederico_storage::Database,
        embedding: Arc<dyn EmbeddingProvider>,
        weights: ScoringWeights,
    ) -> Self {
        Self {
            repo: MemoryRepo::new(db),
            embedding,
            weights,
        }
    }

    /// Acesso ao `MemoryRepo` (pra testes).
    #[must_use]
    pub fn repo(&self) -> &MemoryRepo<'a> {
        &self.repo
    }

    /// Provider que está sendo usado.
    #[must_use]
    pub fn embedding(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedding
    }
}

impl Retriever for HybridRetriever<'_> {
    fn name(&self) -> &str {
        "hybrid"
    }

    async fn retrieve(&self, request: RetrievalRequest) -> MemoryResult<RetrievalResult> {
        let start = Instant::now();
        let now = Utc::now();

        if request.query.trim().is_empty() {
            return Ok(RetrievalResult {
                hits: Vec::new(),
                semantic_used: false,
                elapsed_ms: start.elapsed().as_millis() as u64,
            });
        }

        // 1. Calcula embedding da query (se provider != noop).
        let query_embedding: Option<Vec<f32>> = if self.embedding.provider_id() == "noop" {
            None
        } else {
            // Verifica orçamento: se já gastou mais que o
            // teto do adapter, nem tenta.
            let elapsed = start.elapsed();
            if elapsed > RETRIEVER_BUDGET {
                tracing::warn!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    "orçamento do retriever esgotado antes do embedding da query — caindo pra lexical"
                );
                None
            } else {
                match self.embedding.embed(&[request.query.as_str()]).await {
                    Ok(mut v) if v.len() == 1 => Some(v.remove(0)),
                    Ok(_) => {
                        // Provider devolveu tamanho errado.
                        None
                    }
                    Err(e) => {
                        // Provider falhou — cai pra lexical.
                        tracing::warn!(
                            error = ?e,
                            "provider de embedding falhou — caindo pra lexical"
                        );
                        None
                    }
                }
            }
        };

        // 2. Pega top-N por BM25 (4*k pra dar margem pro
        // cosine reordenar).
        let top_n = (request.k * 4).max(self.weights.max_k * 4);
        let raw = self
            .repo
            .search_lexical(
                request.scope_type,
                &request.scope_id,
                &request.query,
                top_n,
                now,
            )
            .await?;

        if raw.is_empty() {
            return Ok(RetrievalResult {
                hits: Vec::new(),
                semantic_used: query_embedding.is_some(),
                elapsed_ms: start.elapsed().as_millis() as u64,
            });
        }

        // 3. Carrega embeddings dos top-N (mesmo provider/model).
        let provider = self.embedding.provider_id();
        let model = self.embedding.model_id();
        let mut candidates: Vec<MemoryHit> = Vec::with_capacity(raw.len());
        for (record, lexical_score) in raw {
            // Calcula cosine se a query tem embedding e a
            // memória também.
            let semantic = if let Some(ref qe) = query_embedding {
                if record.embedding_status == EmbeddingStatus::Ready {
                    match self.repo.get_embedding(&record.id, provider, model).await {
                        Ok(Some(emb)) => cosine_similarity(qe, &emb),
                        Ok(None) => 0.0, // Memória sem embedding pronto.
                        Err(_) => 0.0,   // Erro silencioso — não derruba o retrieve.
                    }
                } else {
                    0.0
                }
            } else {
                0.0
            };

            let recency = recency_factor(&record, now, self.weights.recency_half_life_days);
            let importance = record.importance;
            let confirmation = confirmation_factor(&record, &self.weights);

            // Score composto (ADR-0011 §4). Se não tem
            // semântica, o peso da semântica vira 0 — o
            // lexical pode compensar (seus tokens dominam).
            let score = self.weights.lexical_weight * lexical_score
                + self.weights.recency_weight * recency
                + self.weights.semantic_weight * semantic.max(0.0) // cosine pode ser negativo; clipamos
                + self.weights.importance_weight * importance
                + self.weights.confirmation_weight * confirmation;

            let breakdown = ScoreBreakdown {
                lexical: lexical_score,
                recency,
                semantic,
                importance,
                confirmation,
                scope_match: true,
            };

            let explanation = build_explanation(&breakdown, &record);

            candidates.push(MemoryHit {
                record,
                score,
                score_breakdown: breakdown,
                explanation,
            });
        }

        // 4. Ordena por score + desempate por recência
        // (ADR-0011 §5).
        candidates.sort_by(|a, b| {
            let a_ts = a.record.last_used_at.unwrap_or(a.record.updated_at);
            let b_ts = b.record.last_used_at.unwrap_or(b.record.updated_at);
            match b
                .score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
            {
                std::cmp::Ordering::Equal => b_ts.cmp(&a_ts),
                other => other,
            }
        });

        // 5. Trunca pra `k`.
        candidates.truncate(request.k);

        Ok(RetrievalResult {
            hits: candidates,
            semantic_used: query_embedding.is_some(),
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// Fator de recência: decay exponencial com half-life
/// configurável. Devolve 1.0 para memórias recém-criadas,
/// caindo até 0.0 conforme envelhecem.
fn recency_factor(record: &MemoryRecord, now: chrono::DateTime<Utc>, half_life_days: f32) -> f32 {
    let ts = record.last_used_at.unwrap_or(record.updated_at);
    let age_days = (now - ts).num_seconds() as f32 / 86_400.0;
    if age_days <= 0.0 {
        return 1.0;
    }
    let half_life = half_life_days.max(0.001);
    (-std::f32::consts::LN_2 * age_days / half_life).exp()
}

/// Boost por confirmação. `user_pinned = true` é o boost
/// máximo; `user_confirmed = true` é um boost médio. Nenhum
/// dos dois significa boost 0.
fn confirmation_factor(record: &MemoryRecord, weights: &ScoringWeights) -> f32 {
    if record.user_pinned {
        weights.user_pinned_boost
    } else if record.user_confirmed {
        weights.user_confirmed_boost
    } else {
        0.0
    }
}

/// Constrói a explicação textual de um hit (painel da Etapa
/// 5 consome — `PROMPT MESTRE` §10.11). Uma frase curta
/// listando os sinais que contribuíram.
fn build_explanation(breakdown: &ScoreBreakdown, record: &MemoryRecord) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if breakdown.lexical > 0.3 {
        parts.push("match lexical");
    }
    if breakdown.recency > 0.7 {
        parts.push("recente");
    }
    if breakdown.semantic > 0.0 {
        parts.push("similaridade semântica");
    }
    if breakdown.importance > 0.7 {
        parts.push("alta importância");
    }
    if record.user_pinned {
        parts.push("fixada por você");
    } else if record.user_confirmed {
        parts.push("confirmada");
    }
    if parts.is_empty() {
        "match genérico".to_string()
    } else {
        parts.join(" + ")
    }
}

/// Helper para testes: cria um `LexicalRetriever` com
/// defaults razoáveis (incluindo `NoopEmbeddingAdapter`).
#[cfg(test)]
pub fn lexical_retriever_for_tests<'a>(
    db: &'a frederico_storage::Database,
) -> LexicalRetriever<'a> {
    use crate::embedding::NoopEmbeddingAdapter;
    LexicalRetriever::new(
        db,
        Arc::new(NoopEmbeddingAdapter),
        ScoringWeights::default(),
    )
}

/// Helper para testes: cria um `HybridRetriever` com
/// defaults razoáveis. O caller injeta o `embedding`
/// (use um `FakeEmbed` que devolve vetores determinísticos).
#[cfg(test)]
pub fn hybrid_retriever_for_tests<'a>(
    db: &'a frederico_storage::Database,
    embedding: Arc<dyn EmbeddingProvider>,
) -> HybridRetriever<'a> {
    HybridRetriever::new(db, embedding, ScoringWeights::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn recency_factor_at_zero_age_is_one() {
        let now = Utc::now();
        let r = recency_factor_dummy(now, now, 30.0);
        assert!(approx_eq(r, 1.0, 0.01));
    }

    #[test]
    fn recency_factor_at_one_half_life_is_half() {
        let now = Utc::now();
        let past = now - chrono::Duration::days(30);
        let r = recency_factor_dummy(past, now, 30.0);
        assert!(approx_eq(r, 0.5, 0.05));
    }

    #[test]
    fn confirmation_pinned_is_max() {
        let w = ScoringWeights::default();
        let r = dummy_record(true, false);
        assert!(approx_eq(confirmation_factor(&r, &w), 1.5, 0.01));
    }

    #[test]
    fn confirmation_confirmed_is_mid() {
        let w = ScoringWeights::default();
        let r = dummy_record(false, true);
        assert!(approx_eq(confirmation_factor(&r, &w), 1.2, 0.01));
    }

    #[test]
    fn confirmation_neither_is_zero() {
        let w = ScoringWeights::default();
        let r = dummy_record(false, false);
        assert_eq!(confirmation_factor(&r, &w), 0.0);
    }

    fn recency_factor_dummy(
        ts: chrono::DateTime<Utc>,
        now: chrono::DateTime<Utc>,
        half_life: f32,
    ) -> f32 {
        let r = dummy_record(false, false);
        recency_factor_with_ts(&r, ts, now, half_life)
    }

    fn recency_factor_with_ts(
        record: &MemoryRecord,
        ts: chrono::DateTime<Utc>,
        now: chrono::DateTime<Utc>,
        half_life_days: f32,
    ) -> f32 {
        let mut r = record.clone();
        r.updated_at = ts;
        r.last_used_at = Some(ts);
        recency_factor(&r, now, half_life_days)
    }

    fn dummy_record(pinned: bool, confirmed: bool) -> MemoryRecord {
        MemoryRecord {
            id: frederico_core::MemoryId::new(),
            scope_type: frederico_core::MemoryScopeType::Project,
            scope_id: "p".into(),
            type_: frederico_core::MemoryType::Fact,
            content: "x".into(),
            origin: frederico_core::MemoryOrigin::User,
            source_type: frederico_core::MemorySourceType::new("user_message"),
            source_id: None,
            confidence: 0.5,
            importance: 0.5,
            embedding_status: frederico_core::EmbeddingStatus::Pending,
            embedding_provider: None,
            embedding_model: None,
            embedding_dimensions: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_used_at: None,
            expires_at: None,
            superseded_by: None,
            superseded_at: None,
            user_confirmed: confirmed,
            user_pinned: pinned,
            active: true,
            pending_review: false,
        }
    }
}
