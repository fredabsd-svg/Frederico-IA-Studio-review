//! Modelo de aprovação de ferramenta.
//!
//! O spec `tool-registry-specification.md` §"Validação antes de
//! execução" item 9 diz: "Aprovação do usuário necessária e obtida?".
//! O spec `tool-permission-model.md` §"UI de aprovação" lista os
//! 4 botões: "Permitir uma vez" / "Permitir para esta execução" /
//! "Permitir para o projeto" / "Negar". Este módulo modela o
//! `ApprovalRequest` (o que o executor entrega pra UI) e a
//! `ApprovalDecision` (o que a UI devolve).
//!
//! A Etapa 4 (integração) traduz `ApprovalRequired` da
//! `validate_tool_call` em `RunState::WaitingUserApproval` na
//! máquina de estados, e enfileira o `ApprovalRequest` na UI. A
//! Etapa 6 (UI) consome a fila e produz a `ApprovalDecision`. A
//! Etapa 2 entrega o **modelo**; Etapa 3 (permissões) decide se a
//! aprovação é obrigatória com base no `PermissionSet`.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use frederico_core::ToolId;
use serde::{Deserialize, Serialize};

/// Escopo de uma aprovação. Spec §"UI de aprovação".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    /// Aprovação só para esta invocação.
    Once,
    /// Aprovação para todas as invocações do mesmo `Run`.
    Run,
    /// Aprovação para todas as invocações do mesmo projeto.
    Project,
}

/// O que o usuário aprovou (ou negou).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub scope: ApprovalScope,
    pub approved: bool,
    /// Timestamp da decisão (audit log).
    pub decided_at: DateTime<Utc>,
}

impl ApprovalDecision {
    #[must_use]
    pub fn approve_once() -> Self {
        Self {
            scope: ApprovalScope::Once,
            approved: true,
            decided_at: Utc::now(),
        }
    }
}

/// Pedido de aprovação enviado à UI.
///
/// `path` é o **caminho normalizado** que será acessado (não o que o
/// modelo pediu — spec §"UI de aprovação": "caminho normalizado, não
/// o que o modelo pediu"). Para ferramentas que não envolvem
/// filesystem, `path` é `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub tool_id: ToolId,
    pub reason: String,
    /// Caminho normalizado (quando aplicável).
    pub path: Option<PathBuf>,
    /// Argumentos sanitizados que serão passados à ferramenta.
    pub arguments: serde_json::Value,
    /// `true` se a aprovação é **obrigatória** (risco alto) — não
    /// pode ser "lembrada para esta execução" (spec §"Invariantes" do
    /// `tool-permission-model.md`).
    pub mandatory: bool,
    pub requested_at: DateTime<Utc>,
}

impl ApprovalRequest {
    pub fn new(tool_id: ToolId, reason: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            tool_id,
            reason: reason.into(),
            path: None,
            arguments,
            mandatory: false,
            requested_at: Utc::now(),
        }
    }

    #[must_use]
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    #[must_use]
    pub fn mandatory(mut self) -> Self {
        self.mandatory = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_decision_approve_once() {
        let d = ApprovalDecision::approve_once();
        assert!(d.approved);
        assert_eq!(d.scope, ApprovalScope::Once);
    }

    #[test]
    fn approval_request_builder_chain() {
        let req = ApprovalRequest::new(
            ToolId::new("files.read"),
            "leitura fora do workspace",
            serde_json::json!({"path": "D:\\Users\\x\\file.txt"}),
        )
        .with_path(PathBuf::from(r"D:\Users\x\file.txt"))
        .mandatory();
        assert_eq!(req.tool_id, ToolId::new("files.read"));
        assert!(req.mandatory);
        assert!(req.path.is_some());
    }
}
