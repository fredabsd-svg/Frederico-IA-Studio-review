//! Provedores simulados (fake providers) — ver
//! [ADR-0008](../decisions/0008-fake-provider-strategy.md).
//!
//! Dois níveis coexistem:
//!
//! - **Transport-level** ([`transport`]) — produz **bytes brutos** que o
//!   mesmo parser de SSE do [`crate::parser`] consome. Não implementa
//!   o trait `ProviderAdapter`. Cobre fidelidade (golden file) e
//!   patologia (scripted). É o que o §3.1 do ADR-0008 quer exercitado
//!   em todo PR.
//! - **Trait-level** ([`trait_level`]) — implementa `ProviderAdapter`
//!   diretamente, devolvendo `StreamEvent` já construídos. Reservado
//!   para testes rápidos de camadas **acima** do parser — não
//!   exercita o parser de SSE.
//!
//! Quando usar qual: testes que mexem no motor, orquestrador, custo ou
//! cancelamento usam o trait-level. Testes que tocam em SSE, keepalive,
//! UTF-8 partido, fragmentação de `tool_call` etc. usam o transport-level
//! com golden file ou script.

pub mod trait_level;
pub mod transport;
