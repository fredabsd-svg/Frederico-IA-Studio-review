//! `frederico-agent-engine` — máquina de estados do `Run` (Fase 3, Etapa 1).
//!
//! O que está aqui é **a máquina pura, sem dependência de plataforma**:
//!
//! - A enum [`RunState`] com 23 variantes do `PROMPT MESTRE` §6.1
//!   (especificada em [`docs/architecture/agent-state-machine.md`][spec]).
//! - Os eventos [`RunEventKind`] (25 variantes: 20 estruturais + 5
//!   globais) e a struct [`RunEvent`].
//! - A tabela [`TRANSITIONS`] (21 arestas estruturais) e a tabela
//!   [`GLOBAL_TRANSITIONS`] (5 arestas globais).
//! - A função [`apply_transition`] — pura, determinística, testada
//!   par a par.
//! - O [`Budget`] (tipo; enforcement fica pro executor da Etapa 4).
//! - A struct [`Run`] em memória.
//!
//! O que **não** está aqui (vem nas próximas etapas da Fase 3):
//!
//! - **Tool Registry** (Etapa 2) — `files.read` in-process com jail.
//! - **Permissões** (Etapa 3) — `PermissionSet` hierárquico.
//! - **Executor** (Etapa 4) — a transação SQL `BEGIN IMMEDIATE; ...
//!   COMMIT;` que grava o journal + checkpoint antes de retornar.
//! - **Watchdog, recovery, `CancellationToken` hierárquico** (Etapa 5).
//! - **UI** (Etapa 6).
//!
//! O crate é **puro**: `unsafe_code = "forbid"`, sem `tauri`/`windows`/
//! `winapi`, verificado por `scripts/check-core-purity.ps1`. Depende
//! apenas de `frederico-core` e crates utilitários (`serde`,
//! `serde_json`, `chrono`, `thiserror`).
//!
//! [spec]: ../../docs/architecture/agent-state-machine.md

#![deny(missing_docs)]

pub mod budget;
pub mod budget_allocation;
pub mod error;
pub mod event;
pub mod run;
pub mod state;
pub mod subagent_budget;
pub mod subagent_error;
pub mod transition;

pub use budget::Budget;
pub use budget_allocation::{AllocationError, BudgetAllocation, SpentBudget};
pub use error::{RunEventKindParseError, RunStateParseError, TransitionError};
pub use event::{RunEvent, RunEventKind};
pub use run::Run;
pub use state::RunState;
pub use subagent_budget::{BudgetAllocationSum, LedgerError, SubagentBudgetLedger};
pub use subagent_error::{SubagentError, UnknownSpecialistDetail};
pub use transition::{apply_transition, Transition, GLOBAL_TRANSITIONS, TRANSITIONS};
