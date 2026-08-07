//! `SubagentBudgetLedger` — livro de alocações de subagentes (Etapa
//! 4 da Fase 6, ADR-0027 §D3).
//!
//! ## Invariante testado
//!
//! **Σ alocações vivas ≤ pai.remaining_inicial − pai.gasto_atual**
//! (D3 do ADR-0027, fórmula literal da ADR §D3).
//!
//! Em palavras: a soma do que os filhos têm alocado **mais** o
//! que o pai gastou nunca excede o budget que o pai tinha no
//! início. O `Ledger` mantém as alocações vivas; o `SubagentRunner`
//! (Etapa 4 PR 2) consulta essa invariante antes de cada spawn
//! (D3) e rejeita se violar (D4: "erro legível, nunca silent
//! fail").
//!
//! ## Função pura, testável em isolamento
//!
//! O ledger é uma struct sem dependência de I/O, clock, ou
//! plataforma — testada em `crates/agent-engine` puro
//! (mantendo a fronteira crítica do ADR-0025 §D1). A integração
//! com `SubagentRunner` é da Etapa 4 PR 2.
//!
//! ## Por que `HashMap` em vez de `BTreeMap`
//!
//! O `BTreeMap` exigiria `Ord` em `RunId`, que o `opaque_id!` do
//! `frederico-core` não deriva (decisão de Fase 0 — IDs são
//! opacos, ordem não tem semântica). `HashMap` ganha a
//! performance O(1) do `RunId: Hash` que já temos, e o ledger
//! tem 8 entradas max (D1 do ADR-0027) — a perda de
//! determinismo de iteração não importa porque o `total_allocated`
//! é comutativo (soma pura, ordem dos filhos não afeta).
//!
//! [`SubagentRunId`]: frederico_core::RunId

use std::collections::HashMap;

use frederico_core::RunId;

use crate::budget::Budget;
use crate::budget_allocation::{BudgetAllocation, SpentBudget};

/// Livro de alocações de subagentes de um `Run` pai. Função pura,
/// testada em isolamento.
///
/// O ledger **não** sabe o `Budget` do pai — isso fica com o
/// `SubagentRunner` (Etapa 4 PR 2) que passa o `parent_remaining`
/// calculado via `Budget::remaining(&parent.spent)`. Aqui o
/// ledger carrega só o **mapa** de alocações + o **gasto efetivo**
/// dos filhos vivos, e responde "cabe uma nova alocação?".
#[derive(Debug, Default, Clone)]
pub struct SubagentBudgetLedger {
    /// Alocações vivas por `RunId` do subagente. Quando o
    /// subagente termina (sucesso/falha/cancelamento), o
    /// `release` remove a entrada — o budget volta pro pai.
    allocations: HashMap<RunId, BudgetAllocation>,
    /// Gasto efetivo dos filhos vivos, acumulado a cada
    /// round. O `release` zera a entrada do filho (filho
    /// terminou, não tem mais gasto ativo).
    spent_by_child: HashMap<RunId, SpentBudget>,
}

impl SubagentBudgetLedger {
    /// Cria um ledger vazio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tenta registrar uma nova alocação pro filho
    /// `child_id`. Retorna `Err` se a soma das alocações
    /// existentes + a nova excede `parent_remaining`. O
    /// `SubagentRunner` calcula o `parent_remaining` via
    /// `Budget::remaining(&parent.spent)` antes de chamar.
    ///
    /// **Fail-closed (D3 + D4):** o `record` **não** faz
    /// fallback, **não** aloca parcialmente, **não** aceita
    /// "se der". Se a soma excede o pai, o `record` falha e o
    /// `SubagentRunner` rejeita o spawn com erro legível.
    pub fn try_record(
        &mut self,
        child_id: RunId,
        allocation: BudgetAllocation,
        parent_remaining: &Budget,
    ) -> Result<(), LedgerError> {
        // 1. Calcula o `Budget` total que a soma desta alocação +
        //    das alocações existentes (já descontadas do pai) ia
        //    consumir. Falha se algum eixo excede o pai.
        let proposed_total = self.total_allocated().checked_add(&allocation);
        let proposed_total = proposed_total.ok_or(LedgerError::Overflow)?;

        if proposed_total.max_steps > parent_remaining.max_steps {
            return Err(LedgerError::ExceedsParent {
                axis: "max_steps".to_string(),
                requested: u64::from(proposed_total.max_steps),
                available: u64::from(parent_remaining.max_steps),
            });
        }
        if proposed_total.max_tokens_in > parent_remaining.max_tokens_in {
            return Err(LedgerError::ExceedsParent {
                axis: "max_tokens_in".to_string(),
                requested: proposed_total.max_tokens_in,
                available: parent_remaining.max_tokens_in,
            });
        }
        if proposed_total.max_tokens_out > parent_remaining.max_tokens_out {
            return Err(LedgerError::ExceedsParent {
                axis: "max_tokens_out".to_string(),
                requested: proposed_total.max_tokens_out,
                available: parent_remaining.max_tokens_out,
            });
        }
        if proposed_total.max_cost_microcents > parent_remaining.max_cost_microcents {
            return Err(LedgerError::ExceedsParent {
                axis: "max_cost_microcents".to_string(),
                requested: proposed_total.max_cost_microcents,
                available: parent_remaining.max_cost_microcents,
            });
        }
        if proposed_total.max_wall_clock > parent_remaining.max_wall_clock {
            return Err(LedgerError::ExceedsParent {
                axis: "max_wall_clock".to_string(),
                requested: proposed_total.max_wall_clock.as_secs(),
                available: parent_remaining.max_wall_clock.as_secs(),
            });
        }

        // 2. Tudo OK — insere.
        self.allocations.insert(child_id, allocation);
        self.spent_by_child.entry(child_id).or_default();
        Ok(())
    }

    /// Libera a alocação de um filho terminado. O budget volta
    /// pro pai. Idempotente: chamar 2x com o mesmo `child_id` é
    /// no-op (defesa contra chamada duplicada no cleanup).
    pub fn release(&mut self, child_id: &RunId) {
        self.allocations.remove(child_id);
        self.spent_by_child.remove(child_id);
    }

    /// Acumula o gasto do filho no `spent_by_child`. O
    /// `SubagentRunner` chama isso a cada round (junto com
    /// atualizar `parent.spent`).
    pub fn record_spent(&mut self, child_id: &RunId, delta: SpentBudget) {
        if let Some(existing) = self.spent_by_child.get_mut(child_id) {
            // Saturate em u64::MAX no pior caso (não é uma
            // explosão — `spent` é lido pelo watchdog da Etapa
            // 5, e a Etapa 4 PR 2 já tem o budget enforcer
            // que barra antes do overflow).
            existing.cost_microcents = existing
                .cost_microcents
                .saturating_add(delta.cost_microcents);
            existing.tokens_in = existing.tokens_in.saturating_add(delta.tokens_in);
            existing.tokens_out = existing.tokens_out.saturating_add(delta.tokens_out);
            existing.steps = existing.steps.saturating_add(delta.steps);
        }
    }

    /// Soma de todas as alocações vivas. `None` se algum eixo
    /// estourar `u64` (impossível na prática — D1 do ADR-0027
    /// limita a 8 filhos, soma cabe em u32/u64 fácil).
    #[must_use]
    pub fn total_allocated(&self) -> BudgetAllocationSum {
        let mut sum = BudgetAllocationSum::zero();
        for alloc in self.allocations.values() {
            sum = sum.saturating_add(alloc);
        }
        sum
    }

    /// Soma do `spent` de todos os filhos vivos. `None` se
    /// algum eixo estourar `u64`.
    #[must_use]
    pub fn total_spent(&self) -> Option<SpentBudget> {
        let mut sum = SpentBudget::default();
        for spent in self.spent_by_child.values() {
            sum = sum.checked_add(*spent)?;
        }
        Some(sum)
    }

    /// Número de alocações vivas (= número de subagentes vivos
    /// registrados neste ledger).
    #[must_use]
    pub fn len(&self) -> usize {
        self.allocations.len()
    }

    /// `true` se o ledger está vazio.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allocations.is_empty()
    }
}

/// Soma de `BudgetAllocation` em formato de `Budget` (pra
/// comparar com `parent_remaining`). Tipo interno porque o
/// invariante "soma das alocações" só faz sentido aqui — o
/// `BudgetAllocation` por si só é o delta, não a soma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetAllocationSum {
    /// Passos totais alocados.
    pub max_steps: u32,
    /// Tokens de entrada totais alocados.
    pub max_tokens_in: u64,
    /// Tokens de saída totais alocados.
    pub max_tokens_out: u64,
    /// Custo total alocado, em microcents.
    pub max_cost_microcents: u64,
    /// Wall-clock total alocado.
    pub max_wall_clock: std::time::Duration,
}

impl BudgetAllocationSum {
    /// Soma zero (todos os campos 0).
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            max_steps: 0,
            max_tokens_in: 0,
            max_tokens_out: 0,
            max_cost_microcents: 0,
            max_wall_clock: std::time::Duration::ZERO,
        }
    }

    /// Soma saturada (em `u32::MAX` / `u64::MAX` no pior caso
    /// — impossível com D1 do ADR-0027 limitando a 8 filhos, mas
    /// defendemos em profundidade).
    #[must_use]
    pub fn saturating_add(self, other: &BudgetAllocation) -> Self {
        Self {
            max_steps: self.max_steps.saturating_add(other.max_steps),
            max_tokens_in: self.max_tokens_in.saturating_add(other.max_tokens_in),
            max_tokens_out: self.max_tokens_out.saturating_add(other.max_tokens_out),
            max_cost_microcents: self
                .max_cost_microcents
                .saturating_add(other.max_cost_microcents),
            max_wall_clock: self
                .max_wall_clock
                .checked_add(other.max_wall_clock)
                .unwrap_or(std::time::Duration::MAX),
        }
    }

    /// Versão que falha em overflow (em vez de saturar). Usada
    /// no `try_record` pra detectar o caso "D6 do ADR-0027 não
    /// tratado" (8 alocações × 50 steps = 400 steps, mas
    /// `parent.remaining` já é 0, então `ExceedsParent` falha
    /// antes do overflow).
    #[must_use]
    pub fn checked_add(&self, other: &BudgetAllocation) -> Option<Self> {
        Some(Self {
            max_steps: self.max_steps.checked_add(other.max_steps)?,
            max_tokens_in: self.max_tokens_in.checked_add(other.max_tokens_in)?,
            max_tokens_out: self.max_tokens_out.checked_add(other.max_tokens_out)?,
            max_cost_microcents: self
                .max_cost_microcents
                .checked_add(other.max_cost_microcents)?,
            max_wall_clock: self.max_wall_clock.checked_add(other.max_wall_clock)?,
        })
    }
}

/// Erro do ledger. Diferente do `AllocationError` (que é sobre
/// o **requested** exceder o pai) — aqui é sobre a **soma**
/// (alocações existentes + nova) exceder o pai.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, thiserror::Error)]
pub enum LedgerError {
    /// Soma (existentes + nova) excede o `parent_remaining`.
    /// Modelo do pai precisa cancelar um subagente vivo ou
    /// reformular.
    #[error(
        "soma de alocações excede budget disponível do pai no eixo '{axis}': \
         requested={requested}, available={available}. \
         Cancele um subagente ativo ou reduza a alocação."
    )]
    ExceedsParent {
        /// Eixo do `Budget` que excedeu. `String` (não `&'static str`)
        /// pelo mesmo motivo do `AllocationError::ExceedsParent` —
        /// compatibilidade com `serde::Deserialize` sem
        /// `#[serde(borrow)]`.
        axis: String,
        /// Soma (existentes + nova) no eixo que excedeu.
        requested: u64,
        /// `parent_remaining` no eixo que excedeu.
        available: u64,
    },

    /// Overflow na soma (impossível na prática com D1 = 8
    /// filhos, mas defendemos em profundidade).
    #[error("overflow na soma de alocações de subagentes (impossível com 8 filhos)")]
    Overflow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn alloc(steps: u32, cost: u64) -> BudgetAllocation {
        // wall_clock curto (1s por step) pra não interferir com
        // os outros eixos nos testes — os invariantes de D3
        // focam em `max_steps` e `max_cost_microcents`.
        BudgetAllocation {
            max_steps: steps,
            max_tokens_in: 0,
            max_tokens_out: 0,
            max_cost_microcents: cost,
            max_wall_clock: Duration::from_secs(steps as u64),
        }
    }

    #[test]
    fn empty_ledger_has_zero_total() {
        let l = SubagentBudgetLedger::new();
        assert!(l.is_empty());
        assert_eq!(l.len(), 0);
        let total = l.total_allocated();
        assert_eq!(total.max_steps, 0);
        assert_eq!(total.max_cost_microcents, 0);
    }

    #[test]
    fn record_adds_to_total() {
        let mut l = SubagentBudgetLedger::new();
        let parent = Budget::default();
        let child = RunId::new();
        l.try_record(child, alloc(5, 1_000), &parent).expect("fits");
        assert_eq!(l.len(), 1);
        assert_eq!(l.total_allocated().max_steps, 5);
    }

    #[test]
    fn record_fails_when_sum_exceeds_parent() {
        let mut l = SubagentBudgetLedger::new();
        let parent = Budget::new(10, 0, 0, 0, Duration::from_secs(600));
        let c1 = RunId::new();
        let c2 = RunId::new();
        l.try_record(c1, alloc(5, 0), &parent).expect("first fits");
        let err = l.try_record(c2, alloc(6, 0), &parent).unwrap_err();
        assert!(matches!(err, LedgerError::ExceedsParent { axis: s, .. } if s == "max_steps"));
    }

    #[test]
    fn release_frees_budget() {
        let mut l = SubagentBudgetLedger::new();
        let parent = Budget::new(10, 0, 0, 0, Duration::from_secs(600));
        let c1 = RunId::new();
        l.try_record(c1, alloc(8, 0), &parent).expect("fits");
        assert_eq!(l.total_allocated().max_steps, 8);
        l.release(&c1);
        assert_eq!(l.total_allocated().max_steps, 0);
        // Agora cabe outra alocação de 8 steps.
        let c2 = RunId::new();
        l.try_record(c2, alloc(8, 0), &parent)
            .expect("fits after release");
    }

    #[test]
    fn release_is_idempotent() {
        let mut l = SubagentBudgetLedger::new();
        let c = RunId::new();
        l.release(&c); // não existe, no-op
        let parent = Budget::default();
        l.try_record(c, alloc(5, 0), &parent).expect("fits");
        l.release(&c);
        l.release(&c); // 2ª chamada, no-op
        assert!(l.is_empty());
    }

    #[test]
    fn record_spent_accumulates_per_child() {
        let mut l = SubagentBudgetLedger::new();
        let parent = Budget::default();
        let c = RunId::new();
        l.try_record(c, alloc(10, 1_000), &parent).expect("fits");
        l.record_spent(
            &c,
            SpentBudget {
                cost_microcents: 100,
                steps: 1,
                ..SpentBudget::default()
            },
        );
        l.record_spent(
            &c,
            SpentBudget {
                cost_microcents: 200,
                steps: 2,
                ..SpentBudget::default()
            },
        );
        let spent = l.total_spent().expect("fits");
        assert_eq!(spent.cost_microcents, 300);
        assert_eq!(spent.steps, 3);
    }

    #[test]
    fn invariant_sum_never_exceeds_parent() {
        // Cenário do E2E `subagent_budget_sum_never_exceeds_parent`
        // da Etapa 4 (caminho de produção, PR 2). Aqui testamos
        // a função pura que o E2E vai consumir.
        let mut l = SubagentBudgetLedger::new();
        let parent = Budget::new(50, 0, 0, 10_000, Duration::from_secs(600));
        // Pai delega 4 subagentes de 10 steps cada (total 40).
        for _ in 0..4 {
            let c = RunId::new();
            l.try_record(c, alloc(10, 2_000), &parent).expect("fits");
        }
        let total = l.total_allocated();
        assert_eq!(total.max_steps, 40);
        // Tentar 5º (10 steps) excede os 50 do pai? Não — 40+10=50,
        // exatamente o limite. Deve passar.
        let c5 = RunId::new();
        l.try_record(c5, alloc(10, 2_000), &parent)
            .expect("exactly fits");
        let total = l.total_allocated();
        assert_eq!(total.max_steps, 50);
        // Tentar 6º (qualquer coisa) excede.
        let c6 = RunId::new();
        let err = l.try_record(c6, alloc(1, 0), &parent).unwrap_err();
        assert!(matches!(err, LedgerError::ExceedsParent { .. }));
    }

    #[test]
    fn invariant_d3_discounts_remaining() {
        // **A invariante real do D3**: Σ alocações vivas ≤
        // pai.remaining_inicial − pai.gasto_atual. O pai tem
        // 50 steps; gastou 30; tem 20 restantes. O ledger
        // só conhece o `parent_remaining` (20) — quem
        // desconta os 30 é o `SubagentRunner` (calcula
        // `Budget::remaining(&parent.spent)` antes de passar
        // pro `try_record`).
        let mut l = SubagentBudgetLedger::new();
        let parent_remaining = Budget::new(20, 0, 0, 4_000, Duration::from_secs(240));
        let c1 = RunId::new();
        l.try_record(c1, alloc(15, 3_000), &parent_remaining)
            .expect("fits in 20");
        let c2 = RunId::new();
        // 15 + 10 = 25 > 20 → falha.
        let err = l
            .try_record(c2, alloc(10, 0), &parent_remaining)
            .unwrap_err();
        assert!(matches!(err, LedgerError::ExceedsParent { axis: s, .. } if s == "max_steps"));
    }
}
