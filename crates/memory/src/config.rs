//! Configuração do `frederico-memory`.
//!
//! Os **pesos numéricos do scoring** e o **gate de avaliação**
//! vivem em arquivos TOML versionados, não em código Rust.
//! Mutáveis por PR, com mudança justificada pelo resultado
//! do gold-set (`ADR-0011 §3`).
//!
//! Os arquivos ficam em `crates/memory/config/`. O runner de
//! avaliação lê `eval.toml` em tempo de teste; o `Retriever`
//! lê `scoring.toml` em produção. Nenhum dos dois é
//! obrigatório em runtime — se o arquivo não existir,
//! defaults razoáveis são usados.
//!
//! Nenhum dos pesos ou alvos vive no ADR (imutável, §1.6 do
//! REGRAS). Eles ficam aqui, versionados em PRs de
//! calibragem.

use serde::{Deserialize, Serialize};

/// Pesos e parâmetros do scoring. Mutável por PR (não por ADR).
///
/// Defaults são razoáveis para a Etapa 1 (baseline lexical). A
/// Etapa 2 ajusta para que o híbrido supere o baseline
/// (medido pelo runner de avaliação).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoringWeights {
    /// Peso do BM25 do FTS5 no score final. Default 0.40.
    pub lexical_weight: f32,
    /// Peso do decay exponencial de recência. Default 0.20.
    pub recency_weight: f32,
    /// Peso da cosine similarity semântica. Default 0.20.
    /// Se embeddings ausentes, o sinal é neutro (0.0) e o
    /// peso é redistribuído.
    pub semantic_weight: f32,
    /// Peso da importância da memória. Default 0.10.
    pub importance_weight: f32,
    /// Peso do boost por confirmação do usuário. Default 0.10.
    pub confirmation_weight: f32,

    /// Half-life do decay exponencial de recência, em dias.
    /// Após esse período, o fator de recência cai a 0.5.
    /// Default 30 dias.
    pub recency_half_life_days: f32,

    /// Boost multiplicativo quando `user_pinned = true`. Default 1.5.
    pub user_pinned_boost: f32,
    /// Boost multiplicativo quando `user_confirmed = true` (mas
    /// não pinned). Default 1.2.
    pub user_confirmed_boost: f32,

    /// Teto de memórias retornadas. Default 8.
    pub max_k: usize,

    /// Orçamento de tokens que as memórias podem consumir no
    /// contexto do modelo. Default 1500.
    pub token_budget: usize,

    /// Regra de desempate por recência (`PROMPT MESTRE` §10.4).
    /// Se dois candidatos têm score dentro de `recency_epsilon`,
    /// o mais recente vence. Default 0.01.
    pub recency_epsilon: f32,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            lexical_weight: 0.40,
            recency_weight: 0.20,
            semantic_weight: 0.20,
            importance_weight: 0.10,
            confirmation_weight: 0.10,
            recency_half_life_days: 30.0,
            user_pinned_boost: 1.5,
            user_confirmed_boost: 1.2,
            max_k: 8,
            token_budget: 1500,
            recency_epsilon: 0.01,
        }
    }
}

impl ScoringWeights {
    /// Validação simples: pesos não-negativos, soma > 0.
    /// Não exige soma = 1.0 (a fórmula multiplica por fatores
    /// de recência/semântica, então a "soma" do score final
    /// pode estourar 1.0).
    pub fn validate(&self) -> Result<(), String> {
        if self.lexical_weight < 0.0
            || self.recency_weight < 0.0
            || self.semantic_weight < 0.0
            || self.importance_weight < 0.0
            || self.confirmation_weight < 0.0
        {
            return Err("pesos não podem ser negativos".into());
        }
        if self.recency_half_life_days <= 0.0 {
            return Err("recency_half_life_days deve ser > 0".into());
        }
        if self.max_k == 0 {
            return Err("max_k deve ser > 0".into());
        }
        Ok(())
    }
}

/// Gate de avaliação. Mutável por PR (não por ADR).
///
/// Defaults permissivos na Etapa 1 (precisão ≥ 0.0 — sempre
/// passa). A Etapa 6 sobe os alvos: precisão ≥ 0.70, F1 ≥ 0.65,
/// vazamento cruzado = 0, latência p99 ≤ 2s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalGate {
    /// Precisão mínima no top-K. Hard fail abaixo.
    pub min_precision: f32,
    /// F1 mínimo. Hard fail abaixo.
    pub min_f1: f32,
    /// Taxa máxima de vazamento cruzado (memória de escopo
    /// errado aparecendo no retrieve). Hard fail acima.
    pub max_cross_scope_leak: f32,
    /// Latência p99 máxima em milissegundos. Hard fail acima
    /// (regra do `PROMPT MESTRE` §10.13 — 2000ms).
    pub max_p99_ms: u64,
    /// Latência p95 máxima em milissegundos. Warning (não hard
    /// fail) acima.
    pub max_p95_ms: u64,
}

impl Default for EvalGate {
    fn default() -> Self {
        // Etapa 6 da Fase 4: gate de CI (alvos de produção).
        // A Etapa 1 tinha `min_precision = 0.0` (só falhava
        // se precisão = 0, indicando bug no runner). A Etapa 6
        // sobe pra alvos de produção — bloco de merge se o
        // retrieval ficar abaixo. Os números vêm do
        // `crates/memory/config/eval.toml` (versionado).
        // Ver `docs/architecture/memory-evaluation-plan.md` §"Alvos".
        Self {
            min_precision: 0.70,
            min_f1: 0.65,
            max_cross_scope_leak: 0.0,
            max_p99_ms: 2000,
            max_p95_ms: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scoring_weights_sum_is_sane() {
        let w = ScoringWeights::default();
        // 0.40 + 0.20 + 0.20 + 0.10 + 0.10 = 1.00
        let sum = w.lexical_weight
            + w.recency_weight
            + w.semantic_weight
            + w.importance_weight
            + w.confirmation_weight;
        assert!(
            (sum - 1.0).abs() < 0.01,
            "pesos default devem somar ~1.0, somam {sum}"
        );
    }

    #[test]
    fn default_weights_validate() {
        ScoringWeights::default().validate().unwrap();
    }

    #[test]
    fn negative_weight_rejected() {
        let w = ScoringWeights {
            lexical_weight: -0.1,
            ..Default::default()
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn zero_half_life_rejected() {
        let w = ScoringWeights {
            recency_half_life_days: 0.0,
            ..Default::default()
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn zero_max_k_rejected() {
        let w = ScoringWeights {
            max_k: 0,
            ..Default::default()
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn default_eval_gate_has_p99_ceiling() {
        // Regra do PROMPT MESTRE §10.13.
        assert_eq!(EvalGate::default().max_p99_ms, 2000);
    }
}
