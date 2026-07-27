//! Contract test off-PR contra o Anthropic.
//!
//! **Quando rodam**: CI noturno (com `ANTHROPIC_API_KEY`
//! configurado).
//!
//! **O que valida**: que o `AnthropicAdapter` (formato genuinamente
//! diferente do OpenAI-compat) faz parsing correto dos content
//! blocks e converte em `StreamEvent::Delta`/`Done` igual à API
//! real entrega.

#![cfg(not(doctest))]

#![allow(clippy::let_unit_value)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::provider::ProviderAdapter;
use frederico_provider_engine::types::{
    ChatMessage, ChatRequest, Role, StopReason, StreamEvent,
};
use frederico_security::fake::FakeCredentialStore;
use tokio::time::timeout;

const ANTHROPIC_KEY_ENV: &str = "ANTHROPIC_API_KEY";

fn trivial_request() -> ChatRequest {
    ChatRequest {
        provider: ProviderId::new("anthropic"),
        model: ModelId::new("claude-haiku-4-5"),
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
async fn anthropic_stream_completes_with_done() -> Result<(), Box<dyn std::error::Error>> {
    let key = match common::require_env(ANTHROPIC_KEY_ENV) {
        Some(k) => k,
        None => return Ok(()), // skip limpo
    };

    // `FakeCredentialStore::with_credentials` exige `&'static str`,
    // mas a chave vem de env. Usamos `set` (async) num mini runtime.
    let creds: Arc<FakeCredentialStore> = FakeCredentialStore::new();
    {
        let creds = creds.clone();
        let key = key.clone();
        let _ = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|rt| {
                rt.block_on(async move {
                    use frederico_security::CredentialStore;
                    use secrecy::SecretString;
                    let sec = SecretString::new(key.into());
                    let _ = creds.set(&ProviderId::new("anthropic"), &sec).await;
                })
            });
    }
    let creds_dyn: Arc<dyn frederico_security::CredentialStore> = creds;
    let adapter = frederico_provider_engine::anthropic::AnthropicAdapter::new(creds_dyn);

    let stream = adapter.stream(trivial_request())?;
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

    if let StreamEvent::Done { stop_reason } = &events[events.len() - 1] {
        assert!(
            matches!(stop_reason, StopReason::Stop | StopReason::Length),
            "StopReason inesperado: {stop_reason:?}"
        );
    }

    eprintln!(
        "[contract] anthropic OK — {} eventos, {deltas} deltas",
        events.len()
    );
    Ok(())
}
