//! `provider-engine` — adaptadores de provedor de LLM e a fita de eventos.
//!
//! Este crate entrega a fronteira entre o motor de chat do Frederico IA
//! Studio e os provedores reais (OpenAI-compat, Anthropic) ou simulados.
//! Toda a lógica de streaming, cancelamento, parse de SSE e contrato
//! fechado de eventos vive aqui.
//!
//! ## Princípios
//!
//! 1. **Núcleo puro** ([ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md)):
//!    este crate não importa `tauri`, `windows`, nem nada de plataforma.
//!    Credenciais vêm via trait `Platform::credentials()` do
//!    `frederico-security` — nunca de env, dotenv, ou config em arquivo
//!    (ver [ADR-0007](../decisions/0007-credential-store-trait.md)).
//! 2. **`StreamEvent` é enum fechado** ([ADR-0005](../decisions/0005-provider-engine-crate.md)):
//!    adicionar provedor novo que emita algo novo exige variante nova do
//!    enum (mudança explícita, revisável).
//! 3. **Fake no nível do transporte** ([ADR-0008](../decisions/0008-fake-provider-strategy.md)):
//!    o provedor simulado produz bytes que o mesmo parser de SSE consome,
//!    não implementa o trait `ProviderAdapter`.
//! 4. **Relógio injetado**: o watchdog de 60s consulta o `Clock` do
//!    `frederico-security`. Nenhum teste dorme de verdade.
//!
//! ## Mapa do crate
//!
//! - [`types`] — tipos públicos (`StreamEvent`, `ProviderError`, `ChatRequest`).
//! - [`provider`] — trait `ProviderAdapter`.
//! - [`parser`] — parser de SSE usado por todos os adapters e pelos fakes.
//! - [`fake`] — provedores simulados: golden file (fidelidade), scripted
//!   (patologia) e trait-level (testes rápidos).

#![allow(clippy::module_inception)]

pub mod accumulator;
pub mod anthropic;
pub mod event_sink;
pub mod fake;
pub mod openai_compat;
pub mod orchestrator;
pub mod parser;
pub mod provider;
pub mod provider_map;
pub mod run_registry;
pub mod sanitize;
pub mod types;

pub use event_sink::{EventSink, NoopEventSink, RecordingEventSink};
// NOTA (Etapa 4.x.y): `ChatOrchestrator` foi movido pro
// `frederico-execution-engine::orchestrator`. A casca Tauri e os
// tests importam de lá direto. `OrchestratorError` e
// `OrchestratorResult` ficam definidos em `orchestrator` (localmente,
// pra evitar ciclo) e continuam sendo re-exportados daqui pra
// compat.
pub use orchestrator::{error_to_view, OrchestratorError, OrchestratorResult, ProviderErrorView};
pub use provider::{AdapterCapabilities, CostModel, ProviderAdapter, RunHandle};
pub use provider_map::ProviderMap;
pub use run_registry::RunRegistry;
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, ProviderError, ProviderErrorKind, Role, StopReason,
    StreamEvent, ToolDescriptor,
};
