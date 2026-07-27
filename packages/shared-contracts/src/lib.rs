//! Contratos compartilhados entre o núcleo e a casca (e, no futuro, o servidor).
//!
//! A Fase 1 entrega apenas o envelope genérico [`IpcRequest`] / [`IpcResponse`]
//! e a operação [`AppOp::GetAppInfo`]. Operações de domínio (chat, tools,
//! memória, documentos) chegam nas fases 2-5.

use serde::{Deserialize, Serialize};

use frederico_core::CoreError;

/// Envelope de requisição vinda da casca (Tauri) para o núcleo.
/// Por enquanto, JSON síncrono via `tauri::command`. Workers (Fase 5+)
/// usarão named pipes com o mesmo envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub op: AppOp,
}

/// Resposta genérica do núcleo para a casca. `Ok` carrega o payload
/// específico da operação; `Err` carrega uma mensagem segura para a UI.
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

/// Operações da aplicação expostas pela Fase 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppOp {
    /// Devolve o `AppInfo` persistido (versão, started_at, last_seen_at).
    GetAppInfo,
    /// Pinga o núcleo. Útil pra smoke test do IPC.
    Ping,
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
        matches!(back, AppOp::GetAppInfo);
    }
}
