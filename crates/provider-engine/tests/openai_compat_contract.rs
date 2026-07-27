//! Contract tests off-PR contra provedores OpenAI-compat.
//!
//! **Quando rodam**: CI noturno (com `OPENAI_API_KEY` e
//! `OPENROUTER_API_KEY` configurados) e manualmente por devs.
//!
//! **O que validam**: que o `OpenAiCompatAdapter` se comporta
//! igual ao provedor real — os eventos parseados batem com o que
//! a API entrega, o header de auth chega, o streaming funciona
//! fim-a-fim, e o `Done` chega.
//!
//! **Como rodar local**:
//! ```text
//! OPENAI_API_KEY=sk-... \
//!   cargo test -p frederico-provider-engine --test openai_compat_contract -- --nocapture
//! ```
//!
//! Se a env var não está setada, o test pula com mensagem clara em
//! vez de falhar — o `cargo test` no PR continua verde.

#![cfg(not(doctest))]

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::provider::ProviderAdapter;
use frederico_provider_engine::types::{
    ChatMessage, ChatRequest, Role, StopReason, StreamEvent,
};
use tokio::time::timeout;

const OPENAI_BASE: &str = "https://api.openai.com/v1";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

/// Helper local: monta um `ChatRequest` trivial. `max_tokens: 1`
/// mantém o custo do contract test baixo.
fn trivial_request(provider: &str, model: &str) -> ChatRequest {
    ChatRequest {
        provider: ProviderId::new(provider),
        model: ModelId::new(model),
        messages: vec![ChatMessage {
            role: Role::User,
            content: "Reply with the single word: ok".to_string(),
            name: None,
        }],
        tools: vec![],
        temperature: None,
        max_tokens: Some(1),
        cancel: tokio_util::sync::CancellationToken::new(),
        event_timeout: Duration::from_secs(common::CONTRACT_TIMEOUT_SECS),
    }
}

#[tokio::test]
async fn openai_stream_completes_with_done() -> Result<(), Box<dyn std::error::Error>> {
    let adapter = match common::openai_adapter_or_skip("OPENAI_API_KEY", OPENAI_BASE) {
        Some(a) => a,
        None => return Ok(()), // skip limpo
    };

    let req = trivial_request("openai", "gpt-4o-mini");
    let stream = adapter.stream(req)?;
    let events = timeout(
        Duration::from_secs(common::CONTRACT_TIMEOUT_SECS),
        common::drain_events(stream),
    )
    .await
    .map_err(|_| "timeout esperando o stream")?;

    // Sanity: pelo menos um Delta e um Done.
    let deltas = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Delta { .. }))
        .count();
    let dones = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Done { .. }))
        .count();
    assert!(deltas >= 1, "esperado ≥1 Delta, recebi {events:?}");
    assert_eq!(dones, 1, "esperado exatamente 1 Done, recebi {events:?}");

    // O Done tem StopReason populated.
    if let StreamEvent::Done { stop_reason } = &events[events.len() - 1] {
        assert!(
            matches!(stop_reason, StopReason::Stop | StopReason::Length),
            "StopReason inesperado: {stop_reason:?}"
        );
    }

    eprintln!(
        "[contract] openai OK — {} eventos, {deltas} deltas",
        events.len()
    );
    Ok(())
}

#[tokio::test]
async fn openrouter_stream_completes_with_done() -> Result<(), Box<dyn std::error::Error>> {
    // OpenRouter é o gateway preferencial (uma chave, muitos
    // modelos) — ADR de user memory + `model-catalog` cobre.
    let adapter = match common::openai_adapter_or_skip("OPENROUTER_API_KEY", OPENROUTER_BASE) {
        Some(a) => a,
        None => return Ok(()),
    };

    // Modelo gratuito no OpenRouter quando disponível.
    let req = trivial_request("openrouter", "meta-llama/llama-3.1-8b-instruct:free");
    let stream = adapter.stream(req)?;
    let events = timeout(
        Duration::from_secs(common::CONTRACT_TIMEOUT_SECS),
        common::drain_events(stream),
    )
    .await
    .map_err(|_| "timeout esperando o stream")?;

    let deltas = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Delta { .. }))
        .count();
    let dones = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Done { .. }))
        .count();
    assert!(deltas >= 1, "esperado ≥1 Delta, recebi {events:?}");
    assert_eq!(dones, 1, "esperado exatamente 1 Done, recebi {events:?}");

    eprintln!(
        "[contract] openrouter OK — {} eventos, {deltas} deltas",
        events.len()
    );
    Ok(())
}
