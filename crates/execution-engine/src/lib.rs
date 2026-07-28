//! `frederico-execution-engine` — `RunExecutor` que fecha o loop
//! `tool_call` do Frederico IA Studio (Fase 3, Etapa 4).
//!
//! ## O que está aqui
//!
//! - [`RunExecutor`] — recebe um `ChatRequest` (com `tools`),
//!   consome o stream do `ProviderAdapter`, e quando aparece um
//!   `ToolCall`: valida (via `tool-registry`), executa (via
//!   `Tool::execute`), persiste o `ToolResult` no `Database` e
//!   volta a chamar o modelo com o resultado no contexto. O
//!   loop continua até `Done` com `StopReason::Stop` ou o budget
//!   esgota.
//! - Persistência com a regra **"Journal-then-emit"** (spec
//!   `chat-and-providers.md`): cada evento do modelo é gravado
//!   em `message_events` **antes** de ser emitido para a UI. A
//!   transação SQLite é `BEGIN IMMEDIATE; ...; COMMIT;` —
//!   durability antes da notificação.
//! - Aplicação de [`Budget`](frederico_agent_engine::Budget) —
//!   o executor incrementa `current_step` a cada iteração e
//!   rejeita com `RunState::Failed` se o budget estoura.
//!
//! ## O que **não** está aqui (próximas etapas)
//!
//! - **Watchdog e `CancellationToken` hierárquico** (Etapa 5).
//! - **Recovery de crash** (Etapa 5) — quando o app reinicia,
//!   runs em `interrupted` são detectados.
//! - **UI de aprovação** (Etapa 6) — quando o validador
//!   retorna `ApprovalRequired`, o executor enfileira pro
//!   `ApprovalRequest`. A Etapa 4 consome o `ApprovalRequest`
//!   mas a fila visual é Etapa 6.
//! - **E2E com casca Tauri** (Etapa 7) — o teste da Etapa 4 é
//!   com `ScriptedProviderAdapter` (sem Tauri).
//! - **State granular a partir do journal** (Etapa 5.x) — o
//!   [`state_mapping::run_state_for_event`] mapeia cada `StreamEvent`
//!   pro `RunState` correspondente; o executor persiste via
//!   `RunRepo::set_state_and_heartbeat_tx` numa única transação
//!   `BEGIN IMMEDIATE; ...; COMMIT;`.
//! - **Recovery de crash no startup** (Etapa 5.x) — o
//!   [`recovery::recover_stale_runs`] lista runs não-terminais com
//!   `last_heartbeat_at` velho e marca como `interrupted`. A casca
//!   Tauri chama no `setup`.

#![allow(missing_docs)]

pub mod audit_sink;
pub mod budget;
pub mod executor;
pub mod orchestrator;
pub mod persist;
pub mod recovery;
pub mod state_mapping;
pub mod tool_definitions;

pub use audit_sink::DbAuditSink;
pub use budget::BudgetEnforcer;
pub use executor::{RunExecutor, RunOutcome};
pub use orchestrator::{ChatOrchestrator, ChatOrchestratorError, ChatOrchestratorResult};
pub use persist::persist_journal;
pub use recovery::{recover_stale_runs, spawn_recover_stale_runs, DEFAULT_STALE_THRESHOLD_SECS};
pub use state_mapping::run_state_for_event;
pub use tool_definitions::tools_to_descriptors;
