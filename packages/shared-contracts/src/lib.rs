//! Contratos compartilhados entre o núcleo e a casca (e, no futuro, o servidor).
//!
//! A Fase 1 entregou o envelope genérico [`IpcRequest`] / [`IpcResponse`]
//! e as operações `GetAppInfo` / `Ping`. A Fase 2 adiciona:
//! - Etapa 1: `ProviderList` / `ProviderSetCredential` /
//!   `ProviderDeleteCredential`.
//! - Leva 2: `ModelCatalogList` / `ModelCatalogForProvider`.
//! - Leva 3: `Conversation*` (CRUD) + `MessageSend` + `Run*`
//!   (`Cancel` / `GetEvents`).
//!
//! Os tipos do domínio (`Conversation`, `Message`, `MessageEvent`)
//! vivem no `frederico-storage`. Aqui temos *views* serializáveis
//! para a UI.

use serde::{Deserialize, Serialize};

use frederico_core::CoreError;
use frederico_core::{ModelId, ProviderId};

/// Envelope de requisição vinda da casca (Tauri) para o núcleo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub op: AppOp,
}

/// Resposta genérica do núcleo para a casca.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    pub payload: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl IpcResponse {
    pub fn ok<T: Serialize>(value: T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            ok: true,
            payload: Some(serde_json::to_value(value)?),
            error: None,
        })
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            payload: None,
            error: Some(msg.into()),
        }
    }
}

/// Operações da aplicação expostas pela Fase 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppOp {
    // --- Fase 1 ---
    GetAppInfo,
    Ping,

    // --- Etapa 1: Provedores ---
    ProviderList,
    ProviderSetCredential {
        provider: ProviderId,
        value: String,
    },
    ProviderDeleteCredential {
        provider: ProviderId,
    },

    // --- Leva 2: Catálogo ---
    ModelCatalogList,
    ModelCatalogForProvider {
        provider: ProviderId,
    },

    // --- Leva 3: Conversas ---
    ConversationCreate {
        provider: ProviderId,
        model: ModelId,
        title: Option<String>,
    },
    ConversationList,
    ConversationGet {
        id: String,
    },
    ConversationRename {
        id: String,
        title: Option<String>,
    },
    ConversationSetModel {
        id: String,
        provider: ProviderId,
        model: ModelId,
    },
    ConversationDelete {
        id: String,
    },

    // --- Leva 3: Mensagem + Run ---
    MessageSend {
        conversation_id: String,
        content: String,
    },
    RunGetEvents {
        message_id: String,
        since_seq: u32,
    },
    RunCancel {
        run_id: String,
    },
}

/// Status público de um provedor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigView {
    pub provider: ProviderId,
    pub display_name: String,
    pub configured: bool,
    pub last_ok_at: Option<String>,
    pub last_error_at: Option<String>,
    pub last_error: Option<String>,
}

/// View de um modelo do catálogo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptorView {
    pub provider: ProviderId,
    pub model: ModelId,
    pub display_name: String,
    pub context_window: u32,
    pub modalities: serde_json::Value,
    pub capabilities: serde_json::Value,
    pub pricing_input_microcents_per_million: u64,
    pub pricing_output_microcents_per_million: u64,
}

/// View de uma conversa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationView {
    pub id: String,
    pub title: Option<String>,
    pub provider_id: String,
    pub model_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub total_cost_microcents: u64,
}

/// View de uma mensagem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    pub run_id: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cost_microcents: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// View de um evento do journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEventView {
    pub id: i64,
    pub message_id: String,
    pub seq: u32,
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: String,
}

/// Retorno do `MessageSend`. A UI recebe a mensagem do usuário
/// (criada imediatamente) e o `run_id` em background. O stream
/// vem via Tauri events ou `RunGetEvents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSendResult {
    pub user_message: MessageView,
    pub run_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("erro do núcleo: {0}")]
    Core(#[from] CoreError),
    #[error("serialização falhou: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type ContractResult<T> = Result<T, ContractError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_response_ok_serializes() {
        let r: IpcResponse = IpcResponse::ok(serde_json::json!({"version": "0.1.0"})).unwrap();
        assert!(r.ok);
        assert!(r.error.is_none());
    }

    #[test]
    fn ipc_response_err_carries_message() {
        let r = IpcResponse::err("boom");
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("boom"));
    }

    #[test]
    fn app_op_roundtrip() {
        let op = AppOp::GetAppInfo;
        let json = serde_json::to_string(&op).unwrap();
        let back: AppOp = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppOp::GetAppInfo));
    }

    #[test]
    fn app_op_provider_list_roundtrip() {
        let op = AppOp::ProviderList;
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"provider_list\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppOp::ProviderList));
    }

    #[test]
    fn app_op_set_credential_carries_secret() {
        let op = AppOp::ProviderSetCredential {
            provider: ProviderId::new("openai"),
            value: "sk-fake-1234".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"provider_set_credential\""));
        assert!(json.contains("\"provider\":\"openai\""));
    }

    #[test]
    fn app_op_conversation_create_roundtrip() {
        let op = AppOp::ConversationCreate {
            provider: ProviderId::new("openai"),
            model: ModelId::new("gpt-4o"),
            title: Some("teste".to_string()),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"conversation_create\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppOp::ConversationCreate { .. }));
    }

    #[test]
    fn app_op_message_send_roundtrip() {
        let op = AppOp::MessageSend {
            conversation_id: "abc-123".to_string(),
            content: "olá".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"message_send\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::MessageSend {
                conversation_id,
                content,
            } => {
                assert_eq!(conversation_id, "abc-123");
                assert_eq!(content, "olá");
            }
            _ => panic!("esperava MessageSend"),
        }
    }

    #[test]
    fn app_op_run_get_events_roundtrip() {
        let op = AppOp::RunGetEvents {
            message_id: "msg-1".to_string(),
            since_seq: 5,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"since_seq\":5"));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::RunGetEvents { since_seq, .. } => assert_eq!(since_seq, 5),
            _ => panic!("esperava RunGetEvents"),
        }
    }
}
