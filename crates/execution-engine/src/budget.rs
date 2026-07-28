//! Aplicação do `Budget` (Etapa 1 do agent-engine).
//!
//! O `Budget` define tetos (`max_steps`, `max_tokens_in`,
//! `max_tokens_out`, `max_cost_microcents`, `max_wall_clock`).
//! O `BudgetEnforcer` carrega o budget + contadores acumulados e
//! decide se uma próxima iteração pode acontecer.
//!
//! Por enquanto, só `max_steps` é checado (Etapa 4). Os outros
//! tetos (`max_tokens_in/out`, `max_cost_microcents`,
//! `max_wall_clock`) são da Etapa 5 (watchdog + accounting
//! de uso).

use frederico_agent_engine::Budget;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BudgetError {
    /// `current_step >= max_steps` — o loop não pode mais iterar.
    /// A Etapa 4 traduz isso em `RunState::Failed` (não é
    /// interrompendo; é o motor declarando que o budget estourou).
    #[error("budget estourado: steps={current_steps} >= max_steps={max_steps}")]
    MaxStepsExceeded { current_steps: u32, max_steps: u32 },
}

/// O aplicador do budget. Carry-on entre iterações do loop.
pub struct BudgetEnforcer {
    budget: Budget,
    current_step: u32,
}

impl BudgetEnforcer {
    #[must_use]
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            current_step: 0,
        }
    }

    /// Incrementa o step counter e checa o teto. Retorna `Err`
    /// se a próxima iteração estouraria.
    pub fn tick(&mut self) -> Result<u32, BudgetError> {
        self.current_step += 1;
        if self.current_step > self.budget.max_steps {
            return Err(BudgetError::MaxStepsExceeded {
                current_steps: self.current_step,
                max_steps: self.budget.max_steps,
            });
        }
        Ok(self.current_step)
    }

    #[must_use]
    pub fn current_step(&self) -> u32 {
        self.current_step
    }

    #[must_use]
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_increments_and_checks() {
        let mut b = BudgetEnforcer::new(Budget {
            max_steps: 3,
            ..Budget::default()
        });
        assert_eq!(b.tick().unwrap(), 1);
        assert_eq!(b.tick().unwrap(), 2);
        assert_eq!(b.tick().unwrap(), 3);
        let err = b.tick().unwrap_err();
        match err {
            BudgetError::MaxStepsExceeded {
                current_steps,
                max_steps,
            } => {
                assert_eq!(current_steps, 4);
                assert_eq!(max_steps, 3);
            }
        }
    }
}
