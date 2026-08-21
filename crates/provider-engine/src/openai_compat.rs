//! Adapter para provedores no formato OpenAI-compat.
//!
//! Cobre: OpenAI, OpenRouter, DeepSeek, Mistral, NVIDIA NIM, Ollama e
//! LM Studio. A diferença entre eles é parametrizada em construção:
//!
//! - `base_url`: endpoint raiz (e.g. `https://api.openai.com/v1`).
//! - `auth_header`: função que monta o cabeçalho de autorização a
//!   partir do `SecretString` da credencial. OpenRouter usa
//!   `Authorization: Bearer` + header `HTTP-Referer` para a
//!   atribuição; Ollama/LM Studio não precisam de auth.
//!
//! Ver [ADR-0005](../decisions/0005-provider-engine-crate.md) §Decisão.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use frederico_core::{ModelId, ProviderId};
use frederico_security::{CredentialStore, SecurityError};
use futures::stream::{BoxStream, StreamExt};
use secrecy::{ExposeSecret, SecretString};

use crate::accumulator::ToolCallDeltaAccumulator;
use crate::parser::{openai_compat_translate, SseParser};
use crate::provider::{AdapterCapabilities, CostModel, ProviderAdapter, RunHandle};
use crate::types::{
    ChatRequest, ChatResponse, ProviderError, ProviderErrorKind, StopReason, StreamEvent, Usage,
};

/// Forma do cabeçalho de autorização. A closure recebe a credencial
/// e devolve pares (nome, valor).
pub type AuthHeaderFn = Arc<dyn Fn(&SecretString) -> Vec<(String, String)> + Send + Sync>;

/// Adapter OpenAI-compat. Parametrizado por `base_url` e `auth_header`.
pub struct OpenAiCompatAdapter {
    id: ProviderId,
    base_url: String,
    auth_header: AuthHeaderFn,
    credentials: Arc<dyn CredentialStore>,
    http: reqwest::Client,
}

enum TransportItem {
    Bytes(Bytes),
    Error(ProviderError),
    Cancelled,
}

/// Traduz ids internos (`files.read`) para nomes aceitos pelas APIs.
/// DeepSeek e OpenAI exigem `^[a-zA-Z0-9_-]+$`; o registry interno usa
/// pontos. O mapa desfaz a tradução antes de executar a ferramenta.
#[derive(Clone, Default)]
struct ToolNameMap {
    to_wire: HashMap<String, String>,
    from_wire: HashMap<String, String>,
}

impl ToolNameMap {
    fn new(tools: &[crate::types::ToolDescriptor]) -> Self {
        let mut map = Self::default();
        let mut usados = HashSet::new();
        for tool in tools {
            let mut base: String = tool
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            if base.is_empty() {
                base = "tool".to_string();
            }
            base.truncate(64);
            let mut wire = base.clone();
            let mut n = 2usize;
            while usados.contains(&wire) {
                let suffix = format!("_{n}");
                let keep = 64usize.saturating_sub(suffix.len());
                let mut candidate = base[..base.len().min(keep)].to_string();
                candidate.push_str(&suffix);
                wire = candidate;
                n += 1;
            }
            usados.insert(wire.clone());
            map.to_wire.insert(tool.name.clone(), wire.clone());
            map.from_wire.insert(wire, tool.name.clone());
        }
        map
    }

    fn encode(&self, canonical: &str) -> String {
        self.to_wire
            .get(canonical)
            .cloned()
            .unwrap_or_else(|| canonical.to_string())
    }

    fn decode_event(&self, event: &mut StreamEvent) {
        if let StreamEvent::ToolCall { name, .. } = event {
            if let Some(canonical) = self.from_wire.get(name) {
                *name = canonical.clone();
            }
        }
    }
}

impl OpenAiCompatAdapter {
    pub fn new(
        id: impl Into<ProviderId>,
        base_url: impl Into<String>,
        auth_header: AuthHeaderFn,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Frederico-IA-Studio/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest::Client::builder");
        Self {
            id: id.into(),
            base_url: base_url.into(),
            auth_header,
            credentials,
            http,
        }
    }

    /// Constrói um adapter "sem auth" (para Ollama/LM Studio locais).
    /// A credencial é opcional; se não existir, o adapter funciona
    /// sem header de autorização.
    pub fn without_auth(
        id: impl Into<ProviderId>,
        base_url: impl Into<String>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let auth: AuthHeaderFn = Arc::new(|_secret| Vec::new());
        Self::new(id, base_url, auth, credentials)
    }

    /// Constrói um adapter com `Authorization: Bearer <key>` simples.
    /// Cobre OpenAI, DeepSeek, Mistral, NVIDIA NIM e OpenRouter
    /// (que aceita o mesmo esquema).
    pub fn with_bearer_auth(
        id: impl Into<ProviderId>,
        base_url: impl Into<String>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let auth: AuthHeaderFn = Arc::new(|secret| {
            vec![(
                "Authorization".to_string(),
                format!("Bearer {}", secret.expose_secret()),
            )]
        });
        Self::new(id, base_url, auth, credentials)
    }

    /// Constrói um adapter para OpenRouter. Adiciona o header
    /// `HTTP-Referer` (recomendado pela OpenRouter para atribuição).
    pub fn with_openrouter_auth(
        id: impl Into<ProviderId>,
        base_url: impl Into<String>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Self {
        let auth: AuthHeaderFn = Arc::new(|secret| {
            vec![
                (
                    "Authorization".to_string(),
                    format!("Bearer {}", secret.expose_secret()),
                ),
                (
                    "HTTP-Referer".to_string(),
                    "https://frederico-ia.studio".to_string(),
                ),
                ("X-Title".to_string(), "Frederico IA Studio".to_string()),
            ]
        });
        Self::new(id, base_url, auth, credentials)
    }

    async fn fetch_credential(&self) -> Result<SecretString, ProviderError> {
        let creds = self
            .credentials
            .get(&self.id)
            .await
            .map_err(provider_security_error)?;
        creds.ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::Auth,
            upstream_status: Some(401),
            upstream_message: Some(format!(
                "credencial ausente para o provedor '{}'",
                self.id.as_str()
            )),
            retry_after: None,
        })
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

/// Um modelo como o provedor o descreve no `/models`.
///
/// **Campos opcionais porque os provedores discordam.** Medido em
/// 2026-08-19: o `/models` do OpenRouter devolve preço e janela de
/// contexto por modelo; o da OpenAI devolve só a lista de ids. É por
/// isso que o [ADR-0052] §D3 mantém o preço vindo do catálogo
/// embutido — se o remoto mandasse em tudo, um refresh da OpenAI
/// apagaria todos os preços e nenhum modelo dela rodaria.
///
/// [ADR-0052]: ../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeloRemoto {
    pub id: String,
    pub nome: Option<String>,
    pub janela_de_contexto: Option<u32>,
    /// Preço por 1M de tokens, na unidade do catálogo (dólar × 10⁵).
    /// `None` quando o provedor não informa — o caso da OpenAI.
    pub entrada: Option<u64>,
    pub saida: Option<u64>,
}

impl OpenAiCompatAdapter {
    pub(crate) fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
}

/// Converte a resposta do `/models` na lista tipada.
///
/// Separada da chamada de rede para poder ser testada contra as
/// formas reais das duas famílias de resposta, sem socket.
///
/// **Entrada malformada não derruba a lista inteira**: item sem `id`
/// é pulado, e campo extra ilegível vira `None`. Um provedor que
/// muda o formato de um campo não pode custar ao usuário o acesso a
/// todos os modelos dele.
pub fn parse_lista_de_modelos(json: &serde_json::Value) -> Vec<ModeloRemoto> {
    let Some(itens) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    itens
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|v| v.as_str())?.to_string();
            if id.trim().is_empty() {
                return None;
            }
            let nome = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let janela = item
                .get("context_length")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            // O OpenRouter dá preço **por token**, em string decimal.
            // A unidade do catálogo é dólar × 10⁵ por 1M de tokens,
            // então o fator é 10¹¹ — conferido contra o Claude Opus 5,
            // que sai por `0.000005`/token e vale `500000` na tabela.
            let preco = |campo: &str| -> Option<u64> {
                let bruto = item.get("pricing")?.get(campo)?;
                let por_token: f64 = match bruto {
                    serde_json::Value::String(s) => s.parse().ok()?,
                    serde_json::Value::Number(n) => n.as_f64()?,
                    _ => return None,
                };
                if !por_token.is_finite() || por_token < 0.0 {
                    return None;
                }
                Some((por_token * 1e11).round() as u64)
            };
            Some(ModeloRemoto {
                id,
                nome,
                janela_de_contexto: janela,
                entrada: preco("prompt"),
                saida: preco("completion"),
            })
        })
        .collect()
}

fn provider_security_error(e: SecurityError) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Unknown,
        upstream_status: None,
        upstream_message: Some(format!("erro do cofre de credenciais: {e}")),
        retry_after: None,
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiCompatAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: true,
        }
    }

    fn cost_model(&self) -> CostModel {
        // O preço real vem do `frederico-model-catalog`. Aqui só
        // devolvemos zero; o orquestrador consulta o catálogo.
        CostModel::default()
    }

    /// Lista os modelos que o provedor declara ter.
    ///
    /// Formato OpenAI-compat: `{"data": [{"id": "..."}]}`. O
    /// OpenRouter acrescenta `context_length` e `pricing`, e os
    /// campos extras são aproveitados quando existem.
    ///
    /// **Erro é do chamador, não um panic.** Esta chamada roda em
    /// tarefa de fundo no boot ([ADR-0052] §D1): sem rede, sem
    /// credencial ou com erro do provedor, quem chama registra e
    /// segue com o catálogo embutido.
    ///
    /// [ADR-0052]: ../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
    async fn listar_modelos(&self) -> Result<Vec<ModeloRemoto>, ProviderError> {
        let secret = self.fetch_credential().await?;
        let auth_headers = (self.auth_header)(&secret);

        let mut req = self.http.get(self.models_url());
        for (k, v) in &auth_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.send().await.map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(map_http_status(status, response).await);
        }
        let bytes = response.bytes().await.map_err(network_error)?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::network(format!("resposta não-JSON: {e}")))?;

        Ok(parse_lista_de_modelos(&json))
    }
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let secret = self.fetch_credential().await?;
        let auth_headers = (self.auth_header)(&secret);
        let tool_names = ToolNameMap::new(&request.tools);
        let body = build_request_body(&request, /* stream = */ false, &tool_names);
        let url = self.chat_url();

        let mut req = self.http.post(&url).json(&body);
        for (k, v) in &auth_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let response = req.send().await.map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(map_http_status(status, response).await);
        }
        let bytes = response.bytes().await.map_err(network_error)?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::network(format!("resposta não-JSON: {e}")))?;
        let choice = json
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first());
        let content = choice
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let stop_reason = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|r| r.as_str())
            .map(|r| match r {
                "stop" => StopReason::Stop,
                "length" => StopReason::Length,
                "tool_calls" => StopReason::ToolCalls,
                _ => StopReason::Error,
            })
            .unwrap_or(StopReason::Stop);
        let usage = json
            .get("usage")
            .map(|u| Usage {
                prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
            })
            .unwrap_or_default();
        Ok(ChatResponse {
            content,
            stop_reason,
            usage,
        })
    }

    fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        // `stream` é síncrono (não pode ser `async`) por causa da trait.
        // Bloqueamos aqui só para construir a request; o resto (envio,
        // parsing) é via stream.
        let secret = futures::executor::block_on(self.fetch_credential())?;
        let auth_headers = (self.auth_header)(&secret);
        let tool_names = ToolNameMap::new(&request.tools);
        let body = build_request_body(&request, /* stream = */ true, &tool_names);
        let url = self.chat_url();
        let http = self.http.clone();
        let cancel = request.cancel.clone();
        // Spawn o envio para evitar bloquear o caller.
        let (tx, rx) = tokio::sync::mpsc::channel::<TransportItem>(8);
        tokio::spawn(async move {
            let mut req = http.post(&url).json(&body);
            for (k, v) in &auth_headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let send_result = req.send().await;
            let response = match send_result {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(TransportItem::Error(network_error(e))).await;
                    return;
                }
            };
            let status = response.status();
            if !status.is_success() {
                let _ = tx
                    .send(TransportItem::Error(
                        map_http_status(status, response).await,
                    ))
                    .await;
                return;
            }
            let mut byte_stream = response.bytes_stream();
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let _ = tx.send(TransportItem::Cancelled).await;
                        break;
                    }
                    chunk = byte_stream.next() => {
                        match chunk {
                            Some(Ok(b)) => {
                                if tx.send(TransportItem::Bytes(b)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(TransportItem::Error(network_error(e))).await;
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        let transport = tokio_stream::wrappers::ReceiverStream::new(rx);
        // `ToolCallDeltaAccumulator` (Etapa 4.1): estado entre
        // chunks que agrega os deltas de `tool_call` em múltiplos
        // chunks. O `unfold` aceita estado por valor (resolve o
        // problema de lifetime do `scan` + `async move`) e o
        // closure processa em série. O `flat_map` depois explode
        // o `Vec<StreamEvent>` em eventos individuais (o
        // `BoxStream` do trait `ProviderAdapter::stream` precisa
        // `Item = StreamEvent`, não `Vec<StreamEvent>`).
        let event_stream = futures::stream::unfold(
            (
                ToolCallDeltaAccumulator::new(),
                SseParser::new(),
                transport,
                tool_names,
            ),
            |(mut acc, mut parser, mut transport, tool_names)| async move {
                loop {
                    let item = transport.next().await?;
                    let mut events = match item {
                        TransportItem::Error(error) => vec![StreamEvent::Error(error)],
                        TransportItem::Cancelled => vec![StreamEvent::Cancelled],
                        TransportItem::Bytes(bytes) => {
                            let mut parsed = Vec::new();
                            for raw in parser.feed(&bytes) {
                                let raw = match raw {
                                    Ok(raw) => raw,
                                    Err(e) => {
                                        parsed.push(StreamEvent::Error(ProviderError::network(
                                            format!("SSE: {e}"),
                                        )));
                                        continue;
                                    }
                                };
                                let value: serde_json::Value = match serde_json::from_str(&raw.data)
                                {
                                    Ok(value) => value,
                                    Err(e) => {
                                        parsed.push(StreamEvent::Error(ProviderError::network(
                                            format!("SSE JSON: {e}"),
                                        )));
                                        continue;
                                    }
                                };
                                parsed.extend(acc.feed(&value));
                                match openai_compat_translate(raw) {
                                    Ok(Some(event)) => parsed.push(event),
                                    Ok(None) => {}
                                    Err(e) => parsed.push(StreamEvent::Error(
                                        ProviderError::network(format!("parse SSE: {e}")),
                                    )),
                                }
                            }
                            parsed
                        }
                    };
                    for event in &mut events {
                        tool_names.decode_event(event);
                    }
                    if events.is_empty() {
                        continue;
                    }
                    return Some((events, (acc, parser, transport, tool_names)));
                }
            },
        )
        .flat_map(futures::stream::iter);
        Ok(Box::pin(event_stream))
    }

    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        // A Etapa 3 (orquestrador) cuida do mapping RunHandle → CancellationToken.
        // Por enquanto, o `ChatRequest::cancel` é o ponto de sinalização.
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        // Lista "razoável" — o catálogo embutido tem a lista canônica.
        // Cada provedor concreto pode sobrescrever este método.
        Vec::new()
    }
}

fn build_request_body(
    request: &ChatRequest,
    stream: bool,
    tool_names: &ToolNameMap,
) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|m| {
            let mut message = serde_json::json!({
                "role": role_to_str(m.role),
                "content": m.content,
            });
            if m.role == crate::types::Role::Assistant && !m.tool_calls.is_empty() {
                message["tool_calls"] = serde_json::Value::Array(
                    m.tool_calls
                        .iter()
                        .map(|call| {
                            serde_json::json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": tool_names.encode(&call.name),
                                    "arguments": call.arguments_json,
                                }
                            })
                        })
                        .collect(),
                );
            }
            if m.role == crate::types::Role::Tool {
                if let Some(id) = &m.tool_call_id {
                    message["tool_call_id"] = serde_json::json!(id);
                }
                if let Some(name) = &m.name {
                    message["name"] = serde_json::json!(tool_names.encode(name));
                }
            }
            message
        })
        .collect();
    let mut body = serde_json::json!({
        "model": request.model.as_str(),
        "messages": messages,
        "stream": stream,
    });
    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(max) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max);
    }
    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool_names.encode(&t.name),
                        "description": t.description,
                        "parameters": t.parameters_schema,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }
    if request.provider.as_str() == "deepseek" {
        // V4 usa thinking por padrão. O Studio ainda não tem um estado
        // público para raciocínio; desativá-lo evita o watchdog durante
        // `reasoning_content`, que não deve virar resposta visível.
        body["thinking"] = serde_json::json!({ "type": "disabled" });
    }
    body
}

fn role_to_str(role: crate::types::Role) -> &'static str {
    match role {
        crate::types::Role::User => "user",
        crate::types::Role::Assistant => "assistant",
        crate::types::Role::System => "system",
        // OpenAI espera `role: "tool"` para a resposta de uma
        // `tool_call` (junto com `tool_call_id`). O executor da Etapa 4
        // emite `ChatMessage::tool(...)` com `tool_call_id` populado
        // quando o modelo pede uma ferramenta; o `OpenAiCompatAdapter`
        // transforma isso em `{"role": "tool", "tool_call_id": "...",
        // "content": "..."}`.
        crate::types::Role::Tool => "tool",
    }
}

fn network_error(e: reqwest::Error) -> ProviderError {
    ProviderError::network(format!("request falhou: {e}"))
}

async fn map_http_status(
    status: reqwest::StatusCode,
    response: reqwest::Response,
) -> ProviderError {
    let upstream_status = Some(status.as_u16());
    let body = response.text().await.unwrap_or_default();
    let kind = match status.as_u16() {
        401 => ProviderErrorKind::Auth,
        402 => ProviderErrorKind::Payment,
        403 => ProviderErrorKind::Forbidden,
        404 => ProviderErrorKind::NotFound,
        429 => ProviderErrorKind::RateLimited,
        500..=599 => ProviderErrorKind::Server,
        _ => ProviderErrorKind::Unknown,
    };
    let retry_after = None;
    ProviderError {
        kind,
        upstream_status,
        upstream_message: Some(body),
        retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    #[test]
    fn build_request_body_omits_unset_fields() {
        let req = ChatRequest::new(
            ProviderId::new("openai"),
            ModelId::new("gpt-4o"),
            vec![crate::types::ChatMessage::user("oi")],
        );
        let body = build_request_body(&req, false, &ToolNameMap::new(&req.tools));
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], false);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_request_body_includes_tools_when_present() {
        let mut req = ChatRequest::new(
            ProviderId::new("openai"),
            ModelId::new("gpt-4o"),
            vec![crate::types::ChatMessage::user("oi")],
        );
        req.tools.push(crate::types::ToolDescriptor {
            name: "get_weather".to_string(),
            description: "consulta o clima".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
        });
        let body = build_request_body(&req, true, &ToolNameMap::new(&req.tools));
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
    }

    #[test]
    fn deepseek_payload_uses_api_safe_tool_names_and_disables_thinking() {
        let mut req = ChatRequest::new(
            ProviderId::new("deepseek"),
            ModelId::new("deepseek-v4-flash"),
            vec![ChatMessage::user("leia o arquivo")],
        );
        req.tools.push(crate::types::ToolDescriptor {
            name: "files.read".to_string(),
            description: "lê arquivo".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
        });
        let names = ToolNameMap::new(&req.tools);
        let body = build_request_body(&req, true, &names);
        assert_eq!(body["tools"][0]["function"]["name"], "files_read");
        assert_eq!(body["thinking"]["type"], "disabled");

        let mut returned = StreamEvent::ToolCall {
            id: "call_1".to_string(),
            name: "files_read".to_string(),
            arguments_json: "{}".to_string(),
        };
        names.decode_event(&mut returned);
        assert!(matches!(
            returned,
            StreamEvent::ToolCall { ref name, .. } if name == "files.read"
        ));
    }

    #[test]
    fn tool_roundtrip_serializes_assistant_call_and_tool_result() {
        let tools = vec![crate::types::ToolDescriptor {
            name: "docs.generate".to_string(),
            description: "gera relatório".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
        }];
        let mut req = ChatRequest::new(
            ProviderId::new("deepseek"),
            ModelId::new("deepseek-v4-pro"),
            vec![
                ChatMessage::assistant_tool_call(
                    "call_report",
                    "docs.generate",
                    r#"{"format":"docx"}"#,
                ),
                ChatMessage::tool(
                    "docs.generate",
                    r#"{"path":"relatorio.docx"}"#,
                    "call_report",
                ),
            ],
        );
        req.tools = tools;
        let body = build_request_body(&req, true, &ToolNameMap::new(&req.tools));
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            "docs_generate"
        );
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call_report");
        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "call_report");
        assert_eq!(body["messages"][1]["name"], "docs_generate");
    }

    #[tokio::test]
    async fn streaming_preserves_http_payment_error() {
        use frederico_security::fake::FakeCredentialStore;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let body = r#"{"error":{"message":"saldo insuficiente"}}"#;
            let response = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let provider = ProviderId::new("deepseek");
        let creds = FakeCredentialStore::new();
        creds
            .set(&provider, &SecretString::new("test-key".into()))
            .await
            .unwrap();
        let adapter =
            OpenAiCompatAdapter::with_bearer_auth("deepseek", format!("http://{addr}"), creds);
        let request = ChatRequest::new(
            provider,
            ModelId::new("deepseek-v4-flash"),
            vec![ChatMessage::user("oi")],
        );
        let mut stream = adapter.stream(request).unwrap();
        let event = stream.next().await.expect("evento de erro");
        match event {
            StreamEvent::Error(error) => {
                assert_eq!(error.kind, ProviderErrorKind::Payment);
                assert_eq!(error.upstream_status, Some(402));
                assert!(error
                    .upstream_message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("saldo insuficiente"));
            }
            other => panic!("esperava erro estruturado, veio {other:?}"),
        }
    }
}
