//! Teto de uso para uma execução (`Run`).
//!
//! Definido no spec `agent-state-machine.md` §"Contratos". A Etapa 1
//! desta fase define o **tipo** e o `Default` razoável; a **aplicação**
//! do budget (rejeição quando estourar) mora no executor (Etapa 4) e
//! é o que faz a invariante "modelo em loop emitindo tool_calls inúteis
//! não trava o app" (ameaça D2 do `security-threat-model.md`) virar
//! realidade.
//!
//! Custos em **microcents** (`u64`) para evitar ponto flutuante no
//! banco — mesma convenção do `frederico-storage` desde a Fase 2.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Teto de uso para uma execução (`Run`). Veja o module-level docs
/// para o contexto completo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Número máximo de iterações do loop `calling_model` → ...
    /// `continuing_model`. Mitiga a ameaça D2 (modelo em loop).
    pub max_steps: u32,

    /// Tokens de entrada acumulados. Mitiga custo e tempo.
    pub max_tokens_in: u64,

    /// Tokens de saída acumulados.
    pub max_tokens_out: u64,

    /// Custo máximo em **microcents** (1 USD = 100_000_000
    /// microcents). Default: $5, suficiente para a maioria das
    /// conversas curtas; ajustável em runtime pela UI (Etapa 6).
    pub max_cost_microcents: u64,

    /// Tempo total de wall-clock. Default: 10 minutos — conversa
    /// interativa não tem patience pra mais. Ajustável em runtime.
    pub max_wall_clock: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_tokens_in: 200_000,
            max_tokens_out: 16_000,
            max_cost_microcents: 500_000_000, // $5.00
            max_wall_clock: Duration::from_secs(600),
        }
    }
}

impl Budget {
    /// `true` se o budget tem qualquer teto zero. Útil para o executor
    /// da Etapa 4 falhar rápido: um `Budget::zero()` é claramente um
    /// erro de configuração.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.max_steps == 0
            && self.max_tokens_in == 0
            && self.max_tokens_out == 0
            && self.max_cost_microcents == 0
            && self.max_wall_clock.is_zero()
    }

    /// Construtor explícito com `Duration`, evitando que o default
    /// fique implícito. Usado pelos testes do executor da Etapa 4.
    #[must_use]
    pub const fn new(
        max_steps: u32,
        max_tokens_in: u64,
        max_tokens_out: u64,
        max_cost_microcents: u64,
        max_wall_clock: Duration,
    ) -> Self {
        Self {
            max_steps,
            max_tokens_in,
            max_tokens_out,
            max_cost_microcents,
            max_wall_clock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_sane() {
        let b = Budget::default();
        assert_eq!(b.max_steps, 50);
        assert_eq!(b.max_tokens_in, 200_000);
        assert_eq!(b.max_tokens_out, 16_000);
        assert_eq!(b.max_cost_microcents, 500_000_000);
        assert_eq!(b.max_wall_clock, Duration::from_secs(600));
    }

    #[test]
    fn default_roundtrips_via_serde_json() {
        let b = Budget::default();
        let s = serde_json::to_string(&b).unwrap();
        let back: Budget = serde_json::from_str(&s).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn zero_budget_is_detected() {
        let z = Budget::new(0, 0, 0, 0, Duration::ZERO);
        assert!(z.is_zero());
    }

    #[test]
    fn default_is_not_zero() {
        assert!(!Budget::default().is_zero());
    }
}
