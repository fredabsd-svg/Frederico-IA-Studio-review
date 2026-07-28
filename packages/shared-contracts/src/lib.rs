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
    /// Etapa 6 — lista entries da fila de aprovação
    /// (`approval_queue` com `status = 'pending'`).
    ApprovalList,
    /// Etapa 6 — responde a uma approval (approve/reject).
    /// O `decision` carrega `{approved: bool, scope: "Once" |
    /// "Run" | "Project", reason?: string}`.
    ApprovalRespond {
        approval_id: String,
        decision: serde_json::Value,
    },

    // --- Etapa 5 (Fase 4): painel de memória ---
    /// Lista memórias visíveis em um escopo (pré-filtro de
    /// escopo + `active` + `!superseded` + `!pending_review` +
    /// `!expirada`, mesma regra do `MemoryRepo::list_by_scope`).
    /// `scope_type` é `"project" | "preference" | "profile" |
    /// "assistant" | "client" | "conversation" | "document" |
    /// "task" | "session"`. `include_pending` inclui as
    /// `pending_review` (queue de ExternalContent) — a Etapa 5
    /// usa isso pra mostrar a fila de revisão na UI.
    MemoryList {
        scope_type: String,
        scope_id: String,
        include_pending: bool,
    },
    /// Recupera memórias relevantes pra uma query no escopo
    /// (retrieval híbrido do `Retriever`). Devolve
    /// `Vec<MemoryHitView>` com `score_breakdown` visível
    /// (explicabilidade `PROMPT MESTRE` §10.11).
    MemoryRetrieve {
        scope_type: String,
        scope_id: String,
        query: String,
        k: u32,
    },
    /// Aplica correção ("corrija para X" da Etapa 4):
    /// insere a nova memória + marca a antiga como superseded,
    /// atomicamente. Devolve `CorrectionResultView` com
    /// `old_id`, `new_record` e `superseded_at` — a Etapa 5
    /// usa o `superseded_at` pra "essa memória foi substituída
    /// há 3 dias".
    MemoryApplyCorrection {
        old_id: String,
        replacement: NewMemoryInputView,
    },
    /// Confirma uma memória com `pending_review = true`
    /// (fila de ExternalContent). Vira `user_confirmed = true`
    /// e `pending_review = false`.
    MemoryConfirmPending {
        id: String,
    },
    /// Rejeita (deleta) uma memória com `pending_review = true`.
    MemoryRejectPending {
        id: String,
    },
    /// Limpa memórias expiradas + superseded + soft-deleted
    /// (best-effort, análogo ao `purge_expired` no startup).
    /// Devolve o número de linhas deletadas. Atalho pra UI.
    MemoryPurgeExpired,
}

/// View de uma entry da fila de aprovação (Etapa 6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntryView {
    pub id: String,
    pub run_id: String,
    pub tool_id: String,
    pub request_json: String,
    pub created_at: String,
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

// --- Etapa 5 (Fase 4): views do painel de memória -------------------

/// View de uma memória (espelha `frederico_core::MemoryRecord`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryView {
    pub id: String,
    pub scope_type: String,
    pub scope_id: String,
    /// Tipo canônico: `"preference" | "fact" | "decision" | "correction"
    /// | "project_instruction" | "client_context" | "procedure"
    /// | "delivery_pattern" | "temporary" | "conversation_summary"
    /// | "document_reference" | "user_pinned"`.
    pub type_: String,
    pub content: String,
    /// `"user" | "assistant" | "external_content"`.
    pub origin: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub confidence: f32,
    pub importance: f32,
    pub embedding_status: String,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    pub superseded_by: Option<String>,
    pub superseded_at: Option<String>,
    pub user_confirmed: bool,
    pub user_pinned: bool,
    pub pending_review: bool,
}

/// Decomposição do score final (`PROMPT MESTRE` §10.11 — explicabilidade).
/// O painel da Etapa 5 mostra essa decomposição ao usuário
/// ("essa memória foi recuperada por match lexical + recente
/// + confirmada").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdownView {
    pub lexical: f32,
    pub recency: f32,
    pub semantic: f32,
    pub importance: f32,
    pub confirmation: f32,
    pub scope_match: bool,
}

/// Hit do retrieval com score e explicabilidade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHitView {
    pub record: MemoryView,
    pub score: f32,
    pub score_breakdown: ScoreBreakdownView,
    /// Frase curta explicando por que essa memória foi
    /// recuperada. Mostrada na UI.
    pub explanation: String,
}

/// Resultado do `MemoryApplyCorrection` (espelha
/// `frederico_memory::CorrectionResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionResultView {
    pub old_id: String,
    pub new_record: MemoryView,
    pub superseded_at: String,
}

/// Input do `MemoryApplyCorrection` — substituto da memória
/// antiga. Espelha `frederico_memory::NewMemoryInput` mas
/// com `Deserialize` (o original é só `Clone` pra evitar
/// acoplar a UI a tipos internos do `memory`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemoryInputView {
    pub scope_type: String,
    pub scope_id: String,
    /// `serde(rename = "type_")` — o Rust não pode usar `type`
    /// como nome de campo.
    #[serde(rename = "type_")]
    pub type_: String,
    pub content: String,
    pub origin: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub confidence: f32,
    pub importance: f32,
    /// ISO 8601 (RFC 3339). Opcional — só obrigatório se
    /// `type_ == "temporary"`.
    pub expires_at: Option<String>,
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

    // --- Etapa 5 (Fase 4): roundtrip das ops de memória ---

    #[test]
    fn app_op_memory_list_roundtrip() {
        let op = AppOp::MemoryList {
            scope_type: "project".to_string(),
            scope_id: "proj-1".to_string(),
            include_pending: true,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_list\""));
        assert!(json.contains("\"include_pending\":true"));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::MemoryList {
                scope_type,
                scope_id,
                include_pending,
            } => {
                assert_eq!(scope_type, "project");
                assert_eq!(scope_id, "proj-1");
                assert!(include_pending);
            }
            _ => panic!("esperava MemoryList"),
        }
    }

    #[test]
    fn app_op_memory_retrieve_roundtrip() {
        let op = AppOp::MemoryRetrieve {
            scope_type: "project".to_string(),
            scope_id: "proj-1".to_string(),
            query: "como faço X?".to_string(),
            k: 8,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_retrieve\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::MemoryRetrieve { k, .. } => assert_eq!(k, 8),
            _ => panic!("esperava MemoryRetrieve"),
        }
    }

    #[test]
    fn app_op_memory_apply_correction_roundtrip() {
        // O `replacement` é um `NewMemoryInputView` (com
        // `Deserialize`) — espelha `frederico_memory::NewMemoryInput`.
        let replacement = NewMemoryInputView {
            scope_type: "project".to_string(),
            scope_id: "proj-1".to_string(),
            type_: "correction".to_string(),
            content: "na verdade uso Postgres".to_string(),
            origin: "user".to_string(),
            source_type: "user_message".to_string(),
            source_id: None,
            confidence: 0.9,
            importance: 0.7,
            expires_at: None,
        };
        let op = AppOp::MemoryApplyCorrection {
            old_id: "old-uuid-123".to_string(),
            replacement: replacement.clone(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_apply_correction\""));
        assert!(json.contains("\"old_id\":\"old-uuid-123\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::MemoryApplyCorrection {
                old_id,
                replacement: r,
            } => {
                assert_eq!(old_id, "old-uuid-123");
                assert_eq!(r.content, "na verdade uso Postgres");
                assert_eq!(r.type_, "correction");
                assert_eq!(r.scope_type, "project");
            }
            _ => panic!("esperava MemoryApplyCorrection"),
        }
    }

    #[test]
    fn app_op_memory_confirm_pending_roundtrip() {
        let op = AppOp::MemoryConfirmPending {
            id: "pending-uuid".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_confirm_pending\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        match back {
            AppOp::MemoryConfirmPending { id } => assert_eq!(id, "pending-uuid"),
            _ => panic!("esperava MemoryConfirmPending"),
        }
    }

    #[test]
    fn app_op_memory_reject_pending_roundtrip() {
        let op = AppOp::MemoryRejectPending {
            id: "pending-uuid".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_reject_pending\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppOp::MemoryRejectPending { .. }));
    }

    #[test]
    fn app_op_memory_purge_expired_roundtrip() {
        let op = AppOp::MemoryPurgeExpired;
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("\"kind\":\"memory_purge_expired\""));
        let back: AppOp = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppOp::MemoryPurgeExpired));
    }

    #[test]
    fn score_breakdown_view_roundtrip() {
        let b = ScoreBreakdownView {
            lexical: 0.8,
            recency: 0.6,
            semantic: 0.4,
            importance: 0.5,
            confirmation: 0.3,
            scope_match: true,
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: ScoreBreakdownView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lexical, 0.8);
        assert!(back.scope_match);
    }

    #[test]
    fn memory_hit_view_roundtrip() {
        let hit = MemoryHitView {
            record: MemoryView {
                id: "mem-1".to_string(),
                scope_type: "project".to_string(),
                scope_id: "proj-1".to_string(),
                type_: "fact".to_string(),
                content: "conteúdo da memória".to_string(),
                origin: "user".to_string(),
                source_type: "user_message".to_string(),
                source_id: None,
                confidence: 0.9,
                importance: 0.5,
                embedding_status: "ready".to_string(),
                created_at: "2026-07-28T00:00:00Z".to_string(),
                updated_at: "2026-07-28T00:00:00Z".to_string(),
                expires_at: None,
                superseded_by: None,
                superseded_at: None,
                user_confirmed: true,
                user_pinned: false,
                pending_review: false,
            },
            score: 1.5,
            score_breakdown: ScoreBreakdownView {
                lexical: 0.9,
                recency: 1.0,
                semantic: 0.0,
                importance: 0.5,
                confirmation: 0.0,
                scope_match: true,
            },
            explanation: "match lexical + recente".to_string(),
        };
        let json = serde_json::to_string(&hit).unwrap();
        let back: MemoryHitView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.score, 1.5);
        assert_eq!(back.record.content, "conteúdo da memória");
        assert_eq!(back.explanation, "match lexical + recente");
    }

    #[test]
    fn correction_result_view_roundtrip() {
        let result = CorrectionResultView {
            old_id: "old-uuid".to_string(),
            new_record: MemoryView {
                id: "new-uuid".to_string(),
                scope_type: "project".to_string(),
                scope_id: "proj-1".to_string(),
                type_: "correction".to_string(),
                content: "corrigido".to_string(),
                origin: "user".to_string(),
                source_type: "user_message".to_string(),
                source_id: None,
                confidence: 0.9,
                importance: 0.7,
                embedding_status: "pending".to_string(),
                created_at: "2026-07-28T00:00:00Z".to_string(),
                updated_at: "2026-07-28T00:00:00Z".to_string(),
                expires_at: None,
                superseded_by: None,
                superseded_at: None,
                user_confirmed: true,
                user_pinned: false,
                pending_review: false,
            },
            superseded_at: "2026-07-28T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: CorrectionResultView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.old_id, "old-uuid");
        assert_eq!(back.new_record.id, "new-uuid");
        assert_eq!(back.new_record.type_, "correction");
    }
}
