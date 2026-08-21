//! Tipos públicos do `provider-engine`.
//!
//! Esta enumeração é o **contrato** entre o motor de chat e os adapters
//! de provedor. Ela é fechada por design — a fronteira do trait
//! ([`crate::ProviderAdapter`]) não vaza o formato de nenhum provedor
//! ([ADR-0005](../decisions/0005-provider-engine-crate.md) §Decisão).

use frederico_core::{ModelId, ProviderId};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Papel de uma mensagem dentro de uma conversa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    /// Resposta de uma ferramenta chamada pelo modelo (Fase 3, Etapa 4).
    /// O adapter OpenAI-compat traduz em `{"role": "tool", "tool_call_id":
    /// "...", "content": "..."}`. Na Etapa 4 o `ScriptedProviderAdapter`
    /// ignora o role (o E2E produz o próximo round programaticamente).
    Tool,
}

/// Mensagem que vai dentro de um [`ChatRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Nome do autor quando o papel é `Tool` (Fase 3+). Na Fase 2 fica
    /// em `None` para todas as mensagens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `tool_call_id` quando o papel é `Tool` (Fase 3, Etapa 4) — o
    /// `id` do `StreamEvent::ToolCall` que está sendo respondido.
    /// O adapter OpenAI-compat exige isso; adapters que não usam
    /// tool calling ignoram.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Chamadas de ferramenta emitidas por uma mensagem `Assistant`.
    /// Precisam voltar no histórico antes do `Role::Tool`; provedores
    /// estritos (DeepSeek incluído) rejeitam um resultado órfão.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChatToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

impl ChatMessage {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    /// Constrói uma `ChatMessage` de papel `Tool` — a resposta de uma
    /// `tool_call` que volta pro modelo. `name` é o `ToolId` (e.g.
    /// `"files.read"`), `tool_call_id` é o `id` do `StreamEvent::ToolCall`
    /// correspondente. Etapa 4 usa para alimentar o loop.
    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        content: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            name: Some(name.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
        }
    }

    /// Mensagem do assistente que originou uma chamada de ferramenta.
    /// O conteúdo pode estar vazio; a informação operacional vive no
    /// bloco estruturado `tool_calls`.
    #[must_use]
    pub fn assistant_tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments_json: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_call_id: None,
            tool_calls: vec![ChatToolCall {
                id: id.into(),
                name: name.into(),
                arguments_json: arguments_json.into(),
            }],
        }
    }
}

/// Esqueleto de uma ferramenta exposta ao provedor (sem execução —
/// execução é Fase 3). Na Fase 2, `tools` pode ir vazio ou com um
/// descritor estático; a enum `StreamEvent::ToolCall` é emitida mas o
/// motor não age sobre ela.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

/// Requisição de chat enviada ao adapter.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub provider: ProviderId,
    pub model: ModelId,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDescriptor>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Token de cancelamento. O adapter monitora este token; quando
    /// disparado, interrompe a request `reqwest` e emite
    /// [`StreamEvent::Cancelled`].
    pub cancel: CancellationToken,
    /// Prazo (em tempo virtual, via `Clock`) para o próximo evento.
    /// Estourar este prazo emite [`StreamEvent::Error`] com
    /// [`ProviderErrorKind::Timeout`]. Padrão: 60s.
    pub event_timeout: Duration,
}

impl ChatRequest {
    #[must_use]
    pub fn new(provider: ProviderId, model: ModelId, messages: Vec<ChatMessage>) -> Self {
        Self {
            provider,
            model,
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            cancel: CancellationToken::new(),
            event_timeout: Duration::from_secs(60),
        }
    }
}

/// Resposta não-streaming (modo `complete`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Por que o provedor parou de gerar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Token de fim natural (`stop`, `end_turn`).
    Stop,
    /// `max_tokens` atingido.
    Length,
    /// O modelo pediu para chamar uma ferramenta (Fase 3 executa).
    ToolCalls,
    /// O provedor reportou erro no `finish_reason` (ex.: filtro de conteúdo).
    Error,
}

/// Enum **fechado** de eventos emitidos durante streaming. Adicionar
/// provedor novo que emita algo novo exige variante nova — mudança
/// explícita, revisável.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Trecho de texto. O `content` é a string bruta; o motor acumula.
    Delta { content: String },
    /// Contagem de tokens do provedor. Disparado no fim, ou em intervalos
    /// (depende do provedor). É o que alimenta o cálculo de custo.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    /// O modelo pediu para chamar uma ferramenta. Fase 2 persiste este
    /// evento no journal, mas não executa — Fase 3 lê e age.
    ToolCall {
        id: String,
        name: String,
        arguments_json: String,
    },
    /// Resultado de uma ferramenta (Fase 3, Etapa 4). O motor emite
    /// este evento **após** a execução da ferramenta (sucesso ou
    /// erro) e o coloca no contexto da próxima chamada ao modelo
    /// (a "resposta" da função no formato OpenAI). O `id` casa com
    /// o `id` do `ToolCall` correspondente.
    ToolResult {
        id: String,
        ok: bool,
        output: serde_json::Value,
    },
    /// O stream terminou.
    Done { stop_reason: StopReason },
    /// Erro do provedor.
    Error(ProviderError),
    /// O [`CancellationToken`] foi disparado (pelo usuário ou watchdog).
    Cancelled,
}

/// Envelope de um `StreamEvent` carregando o `seq` do journal.
///
/// **Por que existe (PR do bug do stream — Etapa 5.X):** o spec
/// `chat-and-providers.md` §"Journal de eventos" exige que cada
/// `StreamEvent` seja **persistido antes** de ser emitido pra UI,
/// e que o `seq` seja monotônico por `message_id` (vem do
/// `message_events.seq`). O frontend reconecta por `fromSeq` no
/// reload de janela (Etapa 5 + §12.6) — pra reconexão ser exata
/// (sem perder nem duplicar), o `seq` do evento **emitido** tem
/// que ser o mesmo do `seq` que o journal gravou.
///
/// O `EventSink::emit_run_event` recebe `payload: serde_json::Value`
/// opaco — o `RunExecutor` monta o envelope e passa a JSON
/// serializado. O `TauriEventSink` (e qualquer outra impl) não
/// precisa conhecer o envelope; ele só emite o JSON.
///
/// **Schema JSON:**
/// ```json
/// { "seq": 7, "event": { "kind": "delta", "content": "olá" } }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEventEnvelope {
    /// `seq` do `MessageEvent` no journal (`message_events.seq`).
    /// Monotônico por `message_id` — o frontend usa pra reconectar
    /// via `RunGetEvents { message_id, since_seq: <último seq recebido> }`.
    pub seq: u32,
    /// O evento do adapter.
    pub event: StreamEvent,
}

/// Erro estruturado de provedor. Cada adapter traduz o vocabulário
/// HTTP do provedor para esta enumeração. O `apps/desktop` traduz
/// depois para PT-BR com ação sugerida.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("erro do provedor: {kind:?} (status={upstream_status:?}, msg={upstream_message:?})")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub upstream_status: Option<u16>,
    pub upstream_message: Option<String>,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// 401 — chave inválida ou ausente.
    Auth,
    /// 402 — sem crédito na conta.
    Payment,
    /// 403 — chave sem acesso ao modelo.
    Forbidden,
    /// 404 — modelo inexistente.
    NotFound,
    /// 429 — limite de requisições, com `retry_after`.
    RateLimited,
    /// 5xx — instabilidade do provedor.
    Server,
    /// Falha de transporte (sem resposta, conexão caiu).
    Network,
    /// Cancelado pelo usuário ou pelo orquestrador.
    Cancelled,
    /// Watchdog estourou (sem evento por `event_timeout`).
    Timeout,
    /// Não classificado.
    Unknown,
}

impl ProviderError {
    #[must_use]
    pub fn auth() -> Self {
        Self {
            kind: ProviderErrorKind::Auth,
            upstream_status: Some(401),
            upstream_message: None,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn timeout() -> Self {
        Self {
            kind: ProviderErrorKind::Timeout,
            upstream_status: None,
            upstream_message: None,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self {
            kind: ProviderErrorKind::Cancelled,
            upstream_status: None,
            upstream_message: None,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn network(msg: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::Network,
            upstream_status: None,
            upstream_message: Some(msg.into()),
            retry_after: None,
        }
    }

    #[must_use]
    pub fn model_not_found(model: &str) -> Self {
        Self {
            kind: ProviderErrorKind::NotFound,
            upstream_status: Some(404),
            upstream_message: Some(format!("modelo '{model}' não encontrado")),
            retry_after: None,
        }
    }

    /// Verdadeiro se o erro é retentável (rate limit, server, network).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            ProviderErrorKind::RateLimited | ProviderErrorKind::Server | ProviderErrorKind::Network
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_constructors() {
        assert_eq!(ChatMessage::user("oi").role, Role::User);
        assert_eq!(ChatMessage::assistant("olá").role, Role::Assistant);
        assert_eq!(ChatMessage::system("sys").role, Role::System);
    }

    #[test]
    fn chat_request_default_timeout() {
        let req = ChatRequest::new(
            ProviderId::new("openai"),
            ModelId::new("gpt-4o"),
            vec![ChatMessage::user("oi")],
        );
        assert_eq!(req.event_timeout, Duration::from_secs(60));
    }

    #[test]
    fn stream_event_serializes_to_tagged_json() {
        let ev = StreamEvent::Delta {
            content: "olá".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"delta\""));
        assert!(json.contains("\"content\":\"olá\""));
    }

    #[test]
    fn stream_event_usage_roundtrip() {
        let ev = StreamEvent::Usage {
            prompt_tokens: 12,
            completion_tokens: 34,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back {
            StreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                assert_eq!(prompt_tokens, 12);
                assert_eq!(completion_tokens, 34);
            }
            _ => panic!("esperava Usage"),
        }
    }

    #[test]
    fn provider_error_retryable() {
        assert!(ProviderError {
            kind: ProviderErrorKind::RateLimited,
            upstream_status: Some(429),
            upstream_message: None,
            retry_after: None,
        }
        .is_retryable());
        assert!(!ProviderError::auth().is_retryable());
        assert!(!ProviderError::cancelled().is_retryable());
    }

    #[test]
    fn provider_error_does_not_leak_key() {
        // O `upstream_message` pode carregar a resposta do provedor, mas a
        // struct em si não tem campo de chave. Este teste documenta o
        // contrato: serialização de `ProviderError` não vaza credencial.
        let json = serde_json::to_string(&ProviderError::auth()).unwrap();
        assert!(!json.contains("sk-"));
        assert!(!json.contains("api_key"));
    }
}
