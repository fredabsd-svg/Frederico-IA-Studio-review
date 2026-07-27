//! Helpers compartilhados pelos contract tests.
//!
//! Cada test gateado lê uma env var (`OPENAI_API_KEY`,
//! `ANTHROPIC_API_KEY`, etc.). Se ausente, o test **pula** com
//! mensagem clara em vez de falhar — assim a suíte roda limpa no
//! PR (sem chave) e roda de verdade no CI noturno (com chave).
//!
//! Convention de import em `tests/*.rs` (top-level):
//! ```ignore
//! #[path = "common/mod.rs"]
//! mod common;
//! ```
//!
//! O arquivo fica em `tests/common/mod.rs` (e não `tests/common.rs`)
//! justamente para o Cargo **não** tratá-lo como test binary — ele
//! é só um módulo compartilhado.

#![allow(dead_code)] // Cada test usa só um subset dos helpers.
#![allow(clippy::let_unit_value)] // `let _ = <expr>` é o padrão idiomático pra descartar.

use std::env;
use std::sync::Arc;

use frederico_core::ProviderId;
use frederico_provider_engine::openai_compat::OpenAiCompatAdapter;
use frederico_provider_engine::types::StreamEvent;
use frederico_security::fake::FakeCredentialStore;
use frederico_security::CredentialStore;

/// Lê a env var. Retorna `Some(value)` se setada e não-vazia, ou
/// `None` se ausente. Loga em stderr qual variável está faltando
/// para o dev ver o que precisa configurar.
pub fn require_env(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) => {
            eprintln!("[contract] SKIP: env var `{name}` está vazia.");
            None
        }
        Err(_) => {
            eprintln!("[contract] SKIP: env var `{name}` não está definida.");
            eprintln!("[contract]   Dica: defina antes de rodar os contract tests.");
            None
        }
    }
}

/// Constroi um `OpenAiCompatAdapter` com a chave em env, ou retorna
/// `None` se a env var não está setada. Usado pelo test pra fazer
/// "skip" limpo em ambiente sem chave.
///
/// O `FakeCredentialStore` é populado via `set` (assíncrono), não
/// `with_credentials` (que exige `&'static str`). Como o adapter
/// só lê a chave na hora de fazer a request, fazer `set` no
/// `block_on` antes de construir o adapter é seguro.
pub fn openai_adapter_or_skip(
    name: &str,
    base_url: &str,
) -> Option<OpenAiCompatAdapter> {
    let key = require_env(name)?;
    let provider_id_str = provider_id_for_base(base_url);
    let provider_id = ProviderId::new(provider_id_str);
    // `FakeCredentialStore::new()` já retorna `Arc<Self>` (o fake
    // foi desenhado para caber direto no `Arc<dyn CredentialStore>`
    // sem wrapper extra).
    let creds: Arc<FakeCredentialStore> = FakeCredentialStore::new();
    // `FakeCredentialStore::set` é `async`. Rodamos via block_on
    // num mini runtime — o test caller já está num runtime tokio,
    // mas aqui estamos numa função síncrona (não `async`).
    let creds_for_set = creds.clone();
    let provider_id_for_set = provider_id.clone();
    let key_for_set = key.clone();
    // O `let _ =` no final é proposital: queremos descartar o
    // resultado `()`. Silenciado em nível de módulo
    // (`#![allow(clippy::let_unit_value)]`) por ser o padrão
    // idiomático de descarte.
    let _ = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?
        .block_on(async move {
            use secrecy::SecretString;
            let _ = creds_for_set
                .set(&provider_id_for_set, &SecretString::new(key_for_set.into()))
                .await;
        });
    let creds_dyn: Arc<dyn CredentialStore> = creds;
    Some(OpenAiCompatAdapter::with_bearer_auth(
        provider_id_str,
        base_url,
        creds_dyn,
    ))
}

/// Mapeia base_url → id de provedor usado no `FakeCredentialStore`.
/// O id é arbitrário no contexto do test (a chave é lida do
/// `FakeCredentialStore` por `ProviderId`), mas precisa bater com o
/// que o adapter usa internamente.
fn provider_id_for_base(base_url: &str) -> &'static str {
    match base_url {
        "https://api.openai.com/v1" => "openai",
        "https://openrouter.ai/api/v1" => "openrouter",
        "https://api.deepseek.com/v1" => "deepseek",
        "https://api.mistral.ai/v1" => "mistral",
        "https://integrate.api.nvidia.com/v1" => "nvidia",
        _ => "custom",
    }
}

/// Drena o `BoxStream` retornado por `ProviderAdapter::stream` até
/// `Done` ou `Error` (que termina o stream). Retorna a lista de
/// eventos recebidos.
///
/// Note que `BoxStream` produz `StreamEvent` direto, não
/// `Result<StreamEvent, _>` — o erro vem como `StreamEvent::Error(_)`
/// dentro do stream.
pub async fn drain_events<S>(stream: S) -> Vec<StreamEvent>
where
    S: futures::Stream<Item = StreamEvent> + Unpin,
{
    use futures::StreamExt;
    let mut s = stream;
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        let is_terminal = matches!(
            ev,
            StreamEvent::Done { .. } | StreamEvent::Error(_) | StreamEvent::Cancelled
        );
        out.push(ev);
        if is_terminal {
            break;
        }
    }
    out
}

/// Timeout em segundos para um request de contract test.
pub const CONTRACT_TIMEOUT_SECS: u64 = 30;
