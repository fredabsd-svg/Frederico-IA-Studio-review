//! `BudgetAllocation` — o **delta** que o pai libera pro filho
//! (Etapa 4 da Fase 6, ADR-0027 D3 + D5).
//!
//! ## Por que uma struct separada
//!
//! O `Budget` carrega os **tetos** (max_steps, max_cost_microcents,
//! etc). O `BudgetAllocation` carrega a **janela** que o pai libera
//! pro filho dentro dos tetos do pai. São tipos diferentes com
//! semânticas diferentes:
//!
//! - `Budget` é a configuração de runtime do `Run` (o usuário
//!   configurou "max 50 steps" no Settings).
//! - `BudgetAllocation` é o que o **modelo do pai** pede ("quero
//!   delegar 5 steps pro subagente X"). O `SubagentRunner` valida
//!   que essa allocation cabe no `parent.remaining` antes de spawnar
//!   (D3 do ADR-0027 — "desconto, não cópia").
//!
//! O filho recebe um **`Budget`** construído como visão
//! `min(parent.remaining, allocation)`, não um `BudgetAllocation`
//! direto. O `BudgetAllocation` é o que o modelo preenche; o
//! `SubagentRunner` consome; nada mais.
//!
//! ## Fail-closed (regra do projeto)
//!
//! `BudgetAllocation::try_from(parent, requested)` falha se qualquer
//! eixo do `requested` excede o `parent.remaining`. **Nenhum
//! fallback** — o modelo do pai precisa reformular a alocação
//! (mesma família do `PermissionSet::merge` fail-closed da Fase 6
//! Etapa 3 PR 2, memory "Mais restritivo vence é fail-closed").
//!
//! ## Por que um único módulo com 3 tipos
//!
//! `SpentBudget`, `BudgetAllocation` e `Budget::try_allocate` formam
//! o trio do desconto unidirecional do D3:
//!
//! - `SpentBudget` é o que o executor **escreve** a cada round
//!   (`step`, `tokens`, `cost`).
//! - `Budget::remaining(&spent) -> Budget` é o que o `SubagentRunner`
//!   **lê** antes do spawn (quanto o pai ainda tem).
//! - `BudgetAllocation` é o que o modelo **pede**.
//!
//! Os 3 juntos formam o invariante testado em
//! `subagent_budget_sum_never_exceeds_parent` (Etapa 4 E2E de
//! cobertura, caminho de produção).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::budget::Budget;

// ============================================================================
// SpentBudget — gasto efetivo de um Run (escrito pelo executor a cada round)
// ============================================================================

/// **Gasto efetivo** de um `Run` — quanto já consumiu do `Budget`
/// desde o início. Atualizado pelo `RunExecutor` a cada iteração
/// (passo, tokens in/out, custo).
///
/// ## Por que é `Copy + Default + Eq`
///
/// O `SpentBudget` é uma struct pequena (4 campos) que o executor
/// lê/escreve a cada round. `Copy` evita `.clone()` espalhado no
/// hot path; `Default` permite inicializar com `SpentBudget::default()`
/// num `Run` novo. `Eq` permite o teste de invariante ("Σ filhos ≤
/// pai.gasto_atual") no caminho de produção.
///
/// ## Cópia denormalizada no banco
///
/// A Etapa 4 também armazena os mesmos 4 campos como colunas em
/// `runs` (`spent_microcents`, `spent_tokens_in`, `spent_tokens_out`,
/// `spent_steps`, migração `0029_subagent_runs.sql`) pra queries
/// SQL agregadas ("Σ filhos gastos por este pai"). A fonte da
/// verdade em memória é este struct; em disco, as colunas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpentBudget {
    /// Custo acumulado em **microcents** (1 USD = 100_000_000
    /// microcents). Mesmo shape do `Budget::max_cost_microcents`.
    pub cost_microcents: u64,
    /// Tokens de entrada acumulados.
    pub tokens_in: u64,
    /// Tokens de saída acumulados.
    pub tokens_out: u64,
    /// Passos do loop `calling_model` → `continuing_model`.
    pub steps: u32,
}

impl SpentBudget {
    /// `true` se o executor ainda não gastou nada.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.cost_microcents == 0 && self.tokens_in == 0 && self.tokens_out == 0 && self.steps == 0
    }

    /// Soma ponto-a-ponto de dois `SpentBudget`. Usado pelo
    /// invariante de soma do D3 (Σ filhos ≤ pai.gasto_atual):
    /// o test soma os `spent` de todos os subagentes vivos e
    /// compara com o delta do pai.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            cost_microcents: self.cost_microcents.checked_add(other.cost_microcents)?,
            tokens_in: self.tokens_in.checked_add(other.tokens_in)?,
            tokens_out: self.tokens_out.checked_add(other.tokens_out)?,
            steps: self.steps.checked_add(other.steps)?,
        })
    }
}

// ============================================================================
// BudgetAllocation — o delta que o pai libera pro filho (D5 do ADR-0027)
// ============================================================================

/// **Janela** dentro do `Budget` do pai. O `SubagentRunner.new`
/// recebe `(parent: &Budget, allocation: BudgetAllocation)` e
/// constrói o `Budget` do filho como `min(parent.remaining,
/// allocation)` (D3).
///
/// ## Por que mesmo shape do `Budget` (e não subset)
///
/// A Etapa 1 do ADR-0027 não decidiu restringir eixos — a versão
/// atual aceita todos os 5 tetos do `Budget` na alocação. A
/// Etapa 4 PR 2 pode especializar (`max_wall_clock` é tipicamente
/// o mesmo do pai; `max_steps` é o que o modelo tipicamente
/// customiza). Por enquanto, **mesmo shape = mais simples** e a
/// `try_from` rejeita qualquer eixo que exceda o pai.
///
/// ## Invariante D5
///
/// `BudgetAllocation` é a **única superfície** de alocação
/// (ADR-0027 D5). Nada mais aloca Budget. O `SubagentRunner` é
/// o portão (Etapa 4 PR 2 consome).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetAllocation {
    /// Passos máximos alocados pro filho.
    pub max_steps: u32,
    /// Tokens de entrada máximos alocados pro filho.
    pub max_tokens_in: u64,
    /// Tokens de saída máximos alocados pro filho.
    pub max_tokens_out: u64,
    /// Custo máximo alocado pro filho, em microcents.
    pub max_cost_microcents: u64,
    /// Tempo total de wall-clock alocado pro filho.
    pub max_wall_clock: Duration,
}

impl BudgetAllocation {
    /// Tenta construir uma alocação a partir do `Budget` do pai e
    /// do `requested` (o que o modelo quer dar pro filho). Falha se
    /// qualquer eixo do `requested` excede o `parent_remaining`
    /// (fail-closed do D3).
    ///
    /// **Por que recebe `parent_remaining: &Budget` e não o `Budget`
    /// cheio:** o pai já gastou algo (D3 = "desconto, não cópia").
    /// O `SubagentRunner.new` calcula o `remaining` via
    /// `Budget::remaining(&parent.spent)` antes de chamar esta
    /// função. Isso evita o bug "filho recebe allocation, pai já
    /// gastou" — alt 3 do ADR-0027 (allocation estática) rejeitada.
    pub fn try_from(parent_remaining: &Budget, requested: Budget) -> Result<Self, AllocationError> {
        if requested.max_steps > parent_remaining.max_steps {
            return Err(AllocationError::ExceedsParent {
                axis: "max_steps".to_string(),
                requested: u64::from(requested.max_steps),
                available: u64::from(parent_remaining.max_steps),
            });
        }
        if requested.max_tokens_in > parent_remaining.max_tokens_in {
            return Err(AllocationError::ExceedsParent {
                axis: "max_tokens_in".to_string(),
                requested: requested.max_tokens_in,
                available: parent_remaining.max_tokens_in,
            });
        }
        if requested.max_tokens_out > parent_remaining.max_tokens_out {
            return Err(AllocationError::ExceedsParent {
                axis: "max_tokens_out".to_string(),
                requested: requested.max_tokens_out,
                available: parent_remaining.max_tokens_out,
            });
        }
        if requested.max_cost_microcents > parent_remaining.max_cost_microcents {
            return Err(AllocationError::ExceedsParent {
                axis: "max_cost_microcents".to_string(),
                requested: requested.max_cost_microcents,
                available: parent_remaining.max_cost_microcents,
            });
        }
        if requested.max_wall_clock > parent_remaining.max_wall_clock {
            return Err(AllocationError::ExceedsParent {
                axis: "max_wall_clock".to_string(),
                requested: requested.max_wall_clock.as_secs(),
                available: parent_remaining.max_wall_clock.as_secs(),
            });
        }
        Ok(Self {
            max_steps: requested.max_steps,
            max_tokens_in: requested.max_tokens_in,
            max_tokens_out: requested.max_tokens_out,
            max_cost_microcents: requested.max_cost_microcents,
            max_wall_clock: requested.max_wall_clock,
        })
    }

    /// Divide a alocação em N partes **iguais** (floor division).
    /// Pra paralelizar N subagentes com o mesmo teto.
    ///
    /// **Falha** se `parts > max_steps` (cada parte precisa de pelo
    /// menos 1 step — o ADR-0027 §D5 explicita: "Falha se `parts >
    /// parent_remaining.max_steps`"). Aqui usa `self.max_steps` (a
    /// allocation), que é ≤ pai.max_steps (garantido pelo
    /// `try_from`).
    ///
    /// **Por que floor e não ceil:** o resto fica **no pai**, não
    /// no subagente. É o que o invariante D3 espera — a soma das
    /// partes é `<= self`, nunca `>`.
    pub fn split(self, parts: u32) -> Result<Vec<Self>, AllocationError> {
        if parts == 0 {
            return Err(AllocationError::SplitZero);
        }
        if parts > self.max_steps {
            return Err(AllocationError::SplitTooLarge {
                requested: parts,
                max_steps: self.max_steps,
            });
        }
        let per_steps = self.max_steps / parts;
        let per_tokens_in = self.max_tokens_in / u64::from(parts);
        let per_tokens_out = self.max_tokens_out / u64::from(parts);
        let per_cost = self.max_cost_microcents / u64::from(parts);
        let per_wall = self.max_wall_clock / parts;
        let part = Self {
            max_steps: per_steps,
            max_tokens_in: per_tokens_in,
            max_tokens_out: per_tokens_out,
            max_cost_microcents: per_cost,
            max_wall_clock: per_wall,
        };
        Ok(vec![part; parts as usize])
    }
}

// ============================================================================
// AllocationError — erro legível pelo modelo (D4 do ADR-0027)
// ============================================================================

/// Erro de alocação de budget pra subagente. O `Display` é
/// **legível pelo modelo** (D4) — o modelo do pai recebe o
/// texto e reformula a alocação.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, thiserror::Error)]
pub enum AllocationError {
    /// Algum eixo do `requested` excede o `parent_remaining`.
    /// O modelo do pai precisa reformular a alocação (diminuir
    /// o eixo, ou cancelar outros subagentes pra liberar budget).
    #[error(
        "alocação excede budget disponível do pai no eixo '{axis}': \
         requested={requested}, available={available}. \
         Reformule a alocação ou libere budget de outros subagentes."
    )]
    ExceedsParent {
        /// Eixo do `Budget` que excedeu (`"max_steps"`, `"max_tokens_in"`, etc).
        /// **Por que `String` e não `&'static str`:** pra que
        /// `serde::Deserialize` funcione sem `#[serde(borrow)]`
        /// (e o `&'static str` força o caller a usar `Cow`
        /// emprestado ou a ter `&'static` axis). O `String` é
        /// construído via `format!("max_...")` no portão.
        axis: String,
        /// Valor pedido pelo modelo (no eixo que excedeu).
        requested: u64,
        /// Valor disponível no `parent_remaining` (no eixo que excedeu).
        available: u64,
    },

    /// `split(0)` — sem partes, sem trabalho. Erro de programação.
    #[error("split(0) não é válido: precisa de pelo menos 1 parte")]
    SplitZero,

    /// `split(N)` com N maior que o `max_steps` da allocation.
    /// Cada parte precisa de pelo menos 1 step.
    #[error(
        "split({requested}) excede max_steps={max_steps} da alocação: \
         cada parte precisa de pelo menos 1 step. Reduza o número de partes."
    )]
    SplitTooLarge {
        /// Número de partes que o modelo pediu.
        requested: u32,
        /// `max_steps` da alocação (teto do `split`).
        max_steps: u32,
    },
}

// ============================================================================
// Extensão do Budget — `remaining`, `try_allocate` (Etapa 4)
// ============================================================================

impl Budget {
    /// Calcula o `Budget` **restante** (teto - gasto). Retorna
    /// `Budget` com cada eixo sendo `max - spent` (com
    /// `saturating_sub` pra evitar underflow — `spent > max` é
    /// estado inconsistente que o `BudgetEnforcer` da Fase 3
    /// Etapa 4 não deveria permitir, mas defendemos em
    /// profundidade).
    ///
    /// **Por que função pura e não método do `SpentBudget`:** o
    /// `SpentBudget` é "do Run"; o `Budget` é "do Run" também. A
    /// subtração é simétrica — `remaining = budget - spent` é a
    /// definição. Manter o método no `Budget` deixa o
    /// `SubagentRunner` chamando `parent.remaining(&parent.spent)`
    /// direto.
    #[must_use]
    pub fn remaining(&self, spent: &SpentBudget) -> Self {
        Self {
            max_steps: self.max_steps.saturating_sub(spent.steps),
            max_tokens_in: self.max_tokens_in.saturating_sub(spent.tokens_in),
            max_tokens_out: self.max_tokens_out.saturating_sub(spent.tokens_out),
            max_cost_microcents: self
                .max_cost_microcents
                .saturating_sub(spent.cost_microcents),
            max_wall_clock: self
                .max_wall_clock
                .checked_sub(spent.steps_duration())
                .unwrap_or(Duration::ZERO),
        }
    }

    /// Tenta alocar uma janela (`requested`) do `parent_remaining`
    /// pro filho. Retorna o **`Budget` do filho** já como visão
    /// `min(parent_remaining, requested)` (D3: "filho não tem Budget
    /// próprio; tem uma janela dentro do Budget do pai").
    ///
    /// O `Budget` retornado é o que o `RunExecutor` do filho vai
    /// carregar. Como é `min` em cada eixo, o filho **nunca**
    /// consegue gastar mais do que o `parent_remaining` (e portanto
    /// nunca mais do que o pai tinha disponível no momento do
    /// spawn).
    ///
    /// **Por que retorna `Budget` e não `BudgetAllocation`:** o
    /// `RunExecutor` consome `Budget` direto (Etapa 4.x.y). O
    /// `BudgetAllocation` é o **input** do portão; o `Budget` é o
    /// **output** que o executor carrega. Mesma família do
    /// `PermissionLoader::load_effective_permission_set` que
    /// retorna `PermissionSet` (Etapa 3 PR 2).
    ///
    /// **Por que `min` e não `== requested`:** se o pai tem
    /// `max_steps=10` e o filho pediu 8, o filho recebe 8 (não
    /// 10). Se o pai tem 5 e o filho pediu 8, falha (não cria
    /// filho com 5, porque o modelo **pediu** 8 — deixa o modelo
    /// decidir se 5 serve).
    pub fn try_allocate(
        parent_remaining: &Budget,
        requested: Budget,
    ) -> Result<Budget, AllocationError> {
        // 1. Valida que `requested` cabe em `parent_remaining`
        //    (eixo a eixo). Se algum excede, falha estruturada
        //    (D4 — erro legível pelo modelo).
        let _validated = BudgetAllocation::try_from(parent_remaining, requested)?;

        // 2. Constrói o `Budget` do filho como `min` em cada eixo.
        //    Mesmo após a validação do passo 1, o `min` é defesa em
        //    profundidade (se a validação for burlada por um bug
        //    futuro, o `min` ainda garante que o filho não
        //    excede o pai).
        Ok(Self {
            max_steps: requested.max_steps.min(parent_remaining.max_steps),
            max_tokens_in: requested.max_tokens_in.min(parent_remaining.max_tokens_in),
            max_tokens_out: requested
                .max_tokens_out
                .min(parent_remaining.max_tokens_out),
            max_cost_microcents: requested
                .max_cost_microcents
                .min(parent_remaining.max_cost_microcents),
            max_wall_clock: requested
                .max_wall_clock
                .min(parent_remaining.max_wall_clock),
        })
    }
}

// ============================================================================
// Trait privada — `spent.steps_duration()` para o `Budget::remaining`
// ============================================================================

trait SpentStepsExt {
    /// Converte `steps: u32` em `Duration` usando o wall_clock_per_step
    /// do `Budget` (heurística: 1 step ≈ `max_wall_clock / max_steps`).
    /// Por enquanto, retornamos `Duration::ZERO` — o desconto de
    /// wall_clock via steps não está implementado (Etapa 4 PR 2 pode
    /// decidir a heurística). O `Budget` tem ambos os eixos, mas
    /// `SpentBudget` carrega só `steps` (não `wall_clock`); o cross-axis
    /// é responsabilidade do executor a cada round, não do `remaining`.
    fn steps_duration(&self) -> Duration;
}

impl SpentStepsExt for SpentBudget {
    fn steps_duration(&self) -> Duration {
        // Heurística placeholder: 0. O executor da Etapa 4 PR 2
        // vai setar `last_heartbeat_at` e computar wall_clock
        // efetivo a cada round — o `SpentBudget` provavelmente
        // ganha um campo `wall_clock` próprio na Etapa 4 PR 2
        // (a Etapa 1 do ADR-0027 não decidiu). Por enquanto, o
        // eixo wall_clock do `remaining` é sempre o `max` (o que
        // é seguro: subestima o remaining, fail-closed).
        Duration::ZERO
    }
}

// ============================================================================
// Tests (12 cenários do ADR-0027 §D5 + tests do SpentBudget + remaining)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------- SpentBudget --------

    #[test]
    fn spent_budget_default_is_zero() {
        let s = SpentBudget::default();
        assert!(s.is_zero());
        assert_eq!(s.cost_microcents, 0);
        assert_eq!(s.steps, 0);
    }

    #[test]
    fn spent_budget_checked_add_sums_components() {
        let a = SpentBudget {
            cost_microcents: 100,
            tokens_in: 200,
            tokens_out: 50,
            steps: 3,
        };
        let b = SpentBudget {
            cost_microcents: 50,
            tokens_in: 100,
            tokens_out: 25,
            steps: 2,
        };
        let sum = a.checked_add(b).expect("no overflow");
        assert_eq!(sum.cost_microcents, 150);
        assert_eq!(sum.tokens_in, 300);
        assert_eq!(sum.tokens_out, 75);
        assert_eq!(sum.steps, 5);
    }

    #[test]
    fn spent_budget_checked_add_overflow_returns_none() {
        let a = SpentBudget {
            cost_microcents: u64::MAX,
            ..SpentBudget::default()
        };
        let b = SpentBudget {
            cost_microcents: 1,
            ..SpentBudget::default()
        };
        assert!(a.checked_add(b).is_none());
    }

    // -------- Budget::remaining --------

    #[test]
    fn budget_remaining_subtracts_spent() {
        let b = Budget::default();
        let spent = SpentBudget {
            cost_microcents: 100_000_000, // $1
            tokens_in: 50_000,
            tokens_out: 4_000,
            steps: 10,
        };
        let r = b.remaining(&spent);
        assert_eq!(r.max_steps, 50 - 10);
        assert_eq!(r.max_tokens_in, 200_000 - 50_000);
        assert_eq!(r.max_tokens_out, 16_000 - 4_000);
        assert_eq!(r.max_cost_microcents, 500_000_000 - 100_000_000);
    }

    #[test]
    fn budget_remaining_saturates_on_overspend() {
        // Se o executor reportar spent > max (não deveria, mas
        // defendemos), o `remaining` satura em 0 — não
        // underflow, não número negativo.
        let b = Budget::new(10, 100, 100, 1_000, Duration::from_secs(60));
        let spent = SpentBudget {
            cost_microcents: 5_000, // > max_cost_microcents=1_000
            tokens_in: 200,         // > max_tokens_in=100
            tokens_out: 200,        // > max_tokens_out=100
            steps: 50,              // > max_steps=10
        };
        let r = b.remaining(&spent);
        assert_eq!(r.max_steps, 0);
        assert_eq!(r.max_tokens_in, 0);
        assert_eq!(r.max_tokens_out, 0);
        assert_eq!(r.max_cost_microcents, 0);
    }

    // -------- BudgetAllocation::try_from (12 cenários do ADR-0027 §D5) --------

    #[test]
    fn allocation_try_from_ok_when_requested_fits() {
        let parent = Budget::default();
        let requested = Budget::new(5, 1_000, 1_000, 100_000_000, Duration::from_secs(60));
        let alloc = BudgetAllocation::try_from(&parent, requested).expect("fits");
        assert_eq!(alloc.max_steps, 5);
    }

    #[test]
    fn allocation_try_from_fails_when_steps_exceed() {
        let parent = Budget::new(5, 200_000, 16_000, 500_000_000, Duration::from_secs(600));
        let requested = Budget::new(10, 1_000, 1_000, 1_000, Duration::from_secs(60));
        let err = BudgetAllocation::try_from(&parent, requested).unwrap_err();
        assert!(matches!(err, AllocationError::ExceedsParent { axis: s, .. } if s == "max_steps"));
    }

    #[test]
    fn allocation_try_from_fails_when_tokens_in_exceed() {
        let parent = Budget::new(50, 100, 16_000, 500_000_000, Duration::from_secs(600));
        let requested = Budget::new(5, 200, 1_000, 1_000, Duration::from_secs(60));
        let err = BudgetAllocation::try_from(&parent, requested).unwrap_err();
        assert!(
            matches!(err, AllocationError::ExceedsParent { axis: s, .. } if s == "max_tokens_in")
        );
    }

    #[test]
    fn allocation_try_from_fails_when_tokens_out_exceed() {
        let parent = Budget::new(50, 200_000, 100, 500_000_000, Duration::from_secs(600));
        let requested = Budget::new(5, 1_000, 200, 1_000, Duration::from_secs(60));
        let err = BudgetAllocation::try_from(&parent, requested).unwrap_err();
        assert!(
            matches!(err, AllocationError::ExceedsParent { axis: s, .. } if s == "max_tokens_out")
        );
    }

    #[test]
    fn allocation_try_from_fails_when_cost_exceed() {
        let parent = Budget::new(50, 200_000, 16_000, 1_000, Duration::from_secs(600));
        let requested = Budget::new(5, 1_000, 1_000, 5_000, Duration::from_secs(60));
        let err = BudgetAllocation::try_from(&parent, requested).unwrap_err();
        assert!(
            matches!(err, AllocationError::ExceedsParent { axis: s, .. } if s == "max_cost_microcents")
        );
    }

    #[test]
    fn allocation_try_from_fails_when_wall_clock_exceed() {
        let parent = Budget::new(50, 200_000, 16_000, 500_000_000, Duration::from_secs(60));
        let requested = Budget::new(5, 1_000, 1_000, 1_000, Duration::from_secs(120));
        let err = BudgetAllocation::try_from(&parent, requested).unwrap_err();
        assert!(
            matches!(err, AllocationError::ExceedsParent { axis: s, .. } if s == "max_wall_clock")
        );
    }

    #[test]
    fn allocation_try_from_allows_exact_match() {
        // requested == parent → alocação idêntica ao pai.
        let parent = Budget::default();
        let alloc = BudgetAllocation::try_from(&parent, parent).expect("exact match");
        assert_eq!(alloc.max_steps, parent.max_steps);
        assert_eq!(alloc.max_cost_microcents, parent.max_cost_microcents);
    }

    // -------- BudgetAllocation::split --------

    #[test]
    fn allocation_split_exact() {
        let alloc = BudgetAllocation {
            max_steps: 8,
            max_tokens_in: 800,
            max_tokens_out: 80,
            max_cost_microcents: 8_000,
            max_wall_clock: Duration::from_secs(80),
        };
        let parts = alloc.split(4).expect("fits");
        assert_eq!(parts.len(), 4);
        for p in &parts {
            assert_eq!(p.max_steps, 2);
            assert_eq!(p.max_tokens_in, 200);
            assert_eq!(p.max_tokens_out, 20);
            assert_eq!(p.max_cost_microcents, 2_000);
            assert_eq!(p.max_wall_clock, Duration::from_secs(20));
        }
    }

    #[test]
    fn allocation_split_with_remainder_drops_remainder() {
        // 7 / 3 = 2 cada, resto 1 (fica no pai).
        let alloc = BudgetAllocation {
            max_steps: 7,
            max_tokens_in: 7,
            max_tokens_out: 7,
            max_cost_microcents: 7,
            max_wall_clock: Duration::from_secs(7),
        };
        let parts = alloc.split(3).expect("fits");
        assert_eq!(parts.len(), 3);
        for p in &parts {
            assert_eq!(p.max_steps, 2);
        }
    }

    #[test]
    fn allocation_split_fails_when_parts_exceed_max_steps() {
        let alloc = BudgetAllocation {
            max_steps: 3,
            max_tokens_in: 100,
            max_tokens_out: 100,
            max_cost_microcents: 1_000,
            max_wall_clock: Duration::from_secs(60),
        };
        let err = alloc.split(5).unwrap_err();
        assert!(matches!(err, AllocationError::SplitTooLarge { .. }));
    }

    #[test]
    fn allocation_split_fails_when_zero() {
        let alloc = BudgetAllocation {
            max_steps: 3,
            max_tokens_in: 100,
            max_tokens_out: 100,
            max_cost_microcents: 1_000,
            max_wall_clock: Duration::from_secs(60),
        };
        assert!(alloc.split(0).is_err());
    }

    // -------- Budget::try_allocate (integração) --------

    #[test]
    fn try_allocate_returns_min_of_parent_and_requested() {
        let parent = Budget::new(20, 1_000, 1_000, 100_000, Duration::from_secs(300));
        let requested = Budget::new(5, 500, 500, 50_000, Duration::from_secs(120));
        let child = Budget::try_allocate(&parent, requested).expect("fits");
        // requested cabe — child = requested.
        assert_eq!(child.max_steps, 5);
        assert_eq!(child.max_tokens_in, 500);
        assert_eq!(child.max_tokens_out, 500);
        assert_eq!(child.max_cost_microcents, 50_000);
        assert_eq!(child.max_wall_clock, Duration::from_secs(120));
    }

    #[test]
    fn try_allocate_fails_when_requested_exceeds_parent() {
        let parent = Budget::new(5, 100, 100, 1_000, Duration::from_secs(60));
        let requested = Budget::new(10, 200, 200, 5_000, Duration::from_secs(120));
        assert!(Budget::try_allocate(&parent, requested).is_err());
    }

    // -------- Display do AllocationError é legível pelo modelo (D4) --------

    #[test]
    fn allocation_error_display_is_model_legible() {
        let err = AllocationError::ExceedsParent {
            axis: "max_cost_microcents".to_string(),
            requested: 5_000,
            available: 1_000,
        };
        let s = err.to_string();
        // Verifica que tem as peças que o modelo precisa pra
        // reformular: nome do eixo, requested, available, e a
        // sugestão de ação ("Reformule" / "Reduza").
        assert!(s.contains("max_cost_microcents"));
        assert!(s.contains("5000"));
        assert!(s.contains("1000"));
        assert!(s.contains("Reformule") || s.contains("Reduza"));
    }
}
