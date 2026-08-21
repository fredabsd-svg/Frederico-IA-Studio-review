//! Adapter para a API Anthropic Messages.
//!
//! Formato genuinamente diferente do OpenAI-compat:
//! - `POST /v1/messages` (não `/chat/completions`).
//! - Conteúdo em `messages[].content` é **lista de blocos**:
//!   `{type: "text", text: "..."}` ou `{type: "tool_use", ...}`.
//! - System prompt vai em campo separado `system`, não em `messages`.
//! - SSE emite `event: content_block_delta` (não `data:` simples).
//! - Tool calls: `input_schema` (não `parameters`); resultado vai em
//!   bloco `{type: "tool_result"}` (Fase 3, fora do escopo da v1).
//!
//! Ver [ADR-0005](../decisions/0005-provider-engine-crate.md) §Decisão.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use frederico_core::{ModelId, ProviderId};
use frederico_security::{CredentialStore, SecurityError};
use futures::stream::{BoxStream, StreamExt};
use secrecy::ExposeSecret;

use crate::parser::sse_stream;
use crate::provider::{AdapterCapabilities, CostModel, ProviderAdapter, RunHandle};
use crate::types::{
    ChatMessage, ChatRequest, ChatResponse, ProviderError, ProviderErrorKind, Role, StopReason,
    StreamEvent, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicAdapter {
    base_url: String,
    credentials: Arc<dyn CredentialStore>,
    http: reqwest::Client,
}

impl AnthropicAdapter {
    pub fn new(credentials: Arc<dyn CredentialStore>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Frederico-IA-Studio/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest::Client::builder");
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            credentials,
            http,
        }
    }

    async fn fetch_credential(&self) -> Result<String, ProviderError> {
        let creds = self
            .credentials
            .get(&ProviderId::new("anthropic"))
            .await
            .map_err(|e: SecurityError| ProviderError {
                kind: ProviderErrorKind::Unknown,
                upstream_status: None,
                upstream_message: Some(format!("cofre: {e}")),
                retry_after: None,
            })?;
        creds
            .map(|s| s.expose_secret().to_string())
            .ok_or_else(|| ProviderError {
                kind: ProviderErrorKind::Auth,
                upstream_status: Some(401),
                upstream_message: Some("credencial Anthropic ausente".to_string()),
                retry_after: None,
            })
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::new("anthropic")
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: true,
            supports_usage_in_stream: true,
        }
    }

    fn cost_model(&self) -> CostModel {
        CostModel::default()
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let key = self.fetch_credential().await?;
        let url = format!("{}/messages", self.base_url);
        let body = build_request_body(&request, /* stream = */ false);

        let response = self
            .http
            .post(&url)
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::network(format!("request: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let kind = match status.as_u16() {
                401 => ProviderErrorKind::Auth,
                403 => ProviderErrorKind::Forbidden,
                404 => ProviderErrorKind::NotFound,
                429 => ProviderErrorKind::RateLimited,
                500..=599 => ProviderErrorKind::Server,
                _ => ProviderErrorKind::Unknown,
            };
            return Err(ProviderError {
                kind,
                upstream_status: Some(status.as_u16()),
                upstream_message: Some(body),
                retry_after: None,
            });
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::network(format!("parse: {e}")))?;
        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let stop_reason = json
            .get("stop_reason")
            .and_then(|r| r.as_str())
            .map(|r| match r {
                "end_turn" => StopReason::Stop,
                "max_tokens" => StopReason::Length,
                "tool_use" => StopReason::ToolCalls,
                _ => StopReason::Error,
            })
            .unwrap_or(StopReason::Stop);
        let usage = json
            .get("usage")
            .map(|u| Usage {
                prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                    as u32,
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
        let key = futures::executor::block_on(self.fetch_credential())?;
        let body = build_request_body(&request, /* stream = */ true);
        let url = format!("{}/messages", self.base_url);
        let http = self.http.clone();
        let cancel = request.cancel.clone();

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
        tokio::spawn(async move {
            let response = http
                .post(&url)
                .header("x-api-key", &key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await;
            let response = match response {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(Err(std::io::Error::other(format!("request: {e}"))))
                        .await;
                    return;
                }
            };
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(std::io::Error::other(format!(
                        "HTTP {}: {}",
                        status.as_u16(),
                        body
                    ))))
                    .await;
                return;
            }
            let mut byte_stream = response.bytes_stream();
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    chunk = byte_stream.next() => match chunk {
                        Some(Ok(b)) => {
                            if tx.send(Ok(b)).await.is_err() { break; }
                        }
                        Some(Err(e)) => {
                            let _ = tx.send(Err(std::io::Error::other(format!("stream: {e}")))).await;
                            break;
                        }
                        None => break,
                    }
                }
            }
        });

        let byte_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let event_stream = sse_stream(byte_stream).filter_map(|r| async move {
            match r {
                Ok(raw) => {
                    // Anthropic usa `event: content_block_delta` para deltas.
                    // Mapeamos por event_type quando presente.
                    let event_kind = raw.event_type.as_deref().unwrap_or("message").to_string();
                    let data = raw.data.clone();
                    translate_anthropic_event(&event_kind, &data)
                }
                Err(e) => Some(StreamEvent::Error(ProviderError::network(format!(
                    "SSE: {e}"
                )))),
            }
        });
        Ok(Box::pin(event_stream))
    }

    fn cancel(&self, _handle: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }

    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![
            (
                ModelId::new("claude-3-5-sonnet-latest"),
                "Claude 3.5 Sonnet",
            ),
            (ModelId::new("claude-3-5-haiku-latest"), "Claude 3.5 Haiku"),
        ]
    }
}

fn build_request_body(request: &ChatRequest, stream: bool) -> serde_json::Value {
    // System prompt vai separado.
    let system: Option<String> = request
        .messages
        .iter()
        .find(|m| m.role == Role::System)
        .map(|m| m.content.clone());
    // Mensagens user/assistant viram `{"role": ..., "content":
    // [{"type": "text", "text": ...}]}`. Mensagens com `Role::Tool`
    // viram `{"role": "user", "content": [{"type": "tool_result",
    // "tool_use_id": <call_id>, "content": <output>}]}` (Etapa 4.1).
    // O `Role::Tool` nunca aparece com system (filtrado acima).
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            let content = match m.role {
                Role::Tool => {
                    // O `name` da `ChatMessage::tool(...)` é o
                    // `ToolId` (e.g. `"files.read"`); o `tool_call_id`
                    // é o `id` do `StreamEvent::ToolCall`
                    // correspondente. O formato Anthropic exige
                    // `tool_use_id` (casa com o `tool_use` emitido
                    // pelo modelo) e o `content` como string
                    // (output do tool). O Anthropic aceita o
                    // `content` como string ou como lista de
                    // content_blocks — string é o caso comum.
                    let tool_use_id = m
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    let output_text = m.content.clone();
                    vec![serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": output_text,
                    })]
                }
                Role::Assistant if !m.tool_calls.is_empty() => {
                    let mut blocks = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    blocks.extend(m.tool_calls.iter().map(|call| {
                        let input = serde_json::from_str(&call.arguments_json)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        serde_json::json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": input,
                        })
                    }));
                    blocks
                }
                _ => vec![serde_json::json!({
                    "type": "text",
                    "text": m.content,
                })],
            };
            serde_json::json!({
                "role": role_to_str(m.role),
                "content": content,
            })
        })
        .collect();
    let mut body = serde_json::json!({
        "model": request.model.as_str(),
        "messages": messages,
        "max_tokens": request.max_tokens.unwrap_or(1024),
        "stream": stream,
    });
    if let Some(sys) = system {
        body["system"] = serde_json::json!(sys);
    }
    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters_schema,
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }
    body
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        // Anthropic não tem `role: "tool"` direto — a resposta de
        // uma ferramenta é um content_block `{"type": "tool_result",
        // ...}` dentro de uma mensagem com `role: "user"`
        // (Etapa 4.1). O `build_request_body` cuida do content
        // block; este `role_to_str` só devolve o "user".
        Role::Tool => "user",
    }
}

/// Traduz um evento SSE do Anthropic para `StreamEvent`. Retorna
/// `None` para eventos que devem ser pulos (keepalives, `message_start`,
/// eventos desconhecidos).
fn translate_anthropic_event(event_kind: &str, data: &str) -> Option<StreamEvent> {
    match event_kind {
        "content_block_delta" => {
            let v: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(e) => {
                    return Some(StreamEvent::Error(ProviderError::network(format!(
                        "SSE JSON: {e}"
                    ))))
                }
            };
            let text = v
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if text.is_empty() {
                None
            } else {
                Some(StreamEvent::Delta {
                    content: text.to_string(),
                })
            }
        }
        "message_delta" => {
            let v: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(e) => {
                    return Some(StreamEvent::Error(ProviderError::network(format!(
                        "SSE JSON: {e}"
                    ))))
                }
            };
            let stop = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|r| r.as_str())
                .map(|r| match r {
                    "end_turn" => StopReason::Stop,
                    "max_tokens" => StopReason::Length,
                    "tool_use" => StopReason::ToolCalls,
                    _ => StopReason::Stop,
                })
                .unwrap_or(StopReason::Stop);
            // Também pode trazer usage em message_delta.usage.
            if let Some(usage) = v.get("usage") {
                let prompt = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let completion = usage
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                // Emite Usage seguido de Done. O orquestrador da
                // Leva 3 sabe lidar com Usage fora de ordem.
                return Some(StreamEvent::Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                });
            }
            Some(StreamEvent::Done { stop_reason: stop })
        }
        "content_block_stop" | "message_stop" => Some(StreamEvent::Done {
            stop_reason: StopReason::Stop,
        }),
        "message_start" | "content_block_start" | "ping" => None,
        _ => {
            // Evento desconhecido — pula silenciosamente. O
            // `eventsource-stream` já loga no nível de trace.
            None
        }
    }
}

#[allow(dead_code)]
fn _unused_chat_message() -> ChatMessage {
    // Manter o import vivo para IDEs; removido pelo compilador.
    ChatMessage::user("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_promotes_system() {
        let req = ChatRequest::new(
            ProviderId::new("anthropic"),
            ModelId::new("claude-3-5-sonnet-latest"),
            vec![
                ChatMessage::system("você é um assistente"),
                ChatMessage::user("oi"),
            ],
        );
        let body = build_request_body(&req, false);
        assert_eq!(body["system"], "você é um assistente");
        // `messages` só tem o user.
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn build_request_body_includes_tools_as_input_schema() {
        let mut req = ChatRequest::new(
            ProviderId::new("anthropic"),
            ModelId::new("claude-3-5-sonnet-latest"),
            vec![ChatMessage::user("oi")],
        );
        req.tools.push(crate::types::ToolDescriptor {
            name: "get_weather".to_string(),
            description: "consulta o clima".to_string(),
            parameters_schema: serde_json::json!({"type": "object"}),
        });
        let body = build_request_body(&req, true);
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "get_weather");
        assert!(body["tools"][0].get("input_schema").is_some());
    }

    #[test]
    fn build_request_body_translates_tool_role_to_tool_result_block() {
        // Etapa 4.1: a mensagem com `Role::Tool` vira um
        // content_block `{"type": "tool_result", "tool_use_id": ...,
        // "content": ...}` dentro de uma mensagem com `role:
        // "user"`. O `tool_call_id` da `ChatMessage` casa com o
        // `tool_use_id` do Anthropic.
        let req = ChatRequest::new(
            ProviderId::new("anthropic"),
            ModelId::new("claude-3-5-sonnet-latest"),
            vec![
                ChatMessage::user("lê o hello.txt"),
                ChatMessage::tool("files.read", "Hello, world!", "toolu_01ABC"),
            ],
        );
        let body = build_request_body(&req, false);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        // A segunda mensagem é o tool_result.
        let tool_msg = &messages[1];
        assert_eq!(tool_msg["role"], "user");
        let content = tool_msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "toolu_01ABC");
        assert_eq!(content[0]["content"], "Hello, world!");
    }

    #[test]
    fn build_request_body_keeps_assistant_tool_use_before_result() {
        let req = ChatRequest::new(
            ProviderId::new("anthropic"),
            ModelId::new("claude-3-5-sonnet-latest"),
            vec![
                ChatMessage::assistant_tool_call(
                    "toolu_01ABC",
                    "files.read",
                    r#"{"path":"hello.txt"}"#,
                ),
                ChatMessage::tool("files.read", "Hello", "toolu_01ABC"),
            ],
        );
        let body = build_request_body(&req, false);
        assert_eq!(body["messages"][0]["role"], "assistant");
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][0]["content"][0]["id"], "toolu_01ABC");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn translate_content_block_delta_emits_delta() {
        let data = r#"{"index":0,"delta":{"type":"text_delta","text":"olá"}}"#;
        let ev = translate_anthropic_event("content_block_delta", data).unwrap();
        match ev {
            StreamEvent::Delta { content } => assert_eq!(content, "olá"),
            _ => panic!("esperava Delta"),
        }
    }

    #[test]
    fn translate_empty_delta_returns_none() {
        let data = r#"{"index":0,"delta":{"type":"text_delta","text":""}}"#;
        let ev = translate_anthropic_event("content_block_delta", data);
        assert!(ev.is_none());
    }

    #[test]
    fn translate_message_delta_emits_done() {
        let data = r#"{"delta":{"stop_reason":"end_turn"}}"#;
        let ev = translate_anthropic_event("message_delta", data).unwrap();
        assert!(matches!(
            ev,
            StreamEvent::Done {
                stop_reason: StopReason::Stop
            }
        ));
    }

    #[test]
    fn translate_message_stop_emits_done() {
        let ev = translate_anthropic_event("message_stop", "{}").unwrap();
        assert!(matches!(ev, StreamEvent::Done { .. }));
    }

    #[test]
    fn translate_ping_returns_none() {
        assert!(translate_anthropic_event("ping", "{}").is_none());
    }

    #[test]
    fn translate_unknown_event_returns_none() {
        assert!(translate_anthropic_event("made_up_event", "{}").is_none());
    }
}
