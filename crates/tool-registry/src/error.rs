//! Erros estruturados do Tool Registry.
//!
//! Cada variante carrega os dados mínimos para o caller (executor
//! da Etapa 4, UI da Etapa 6) decidir o que fazer. A regra do spec
//! `tool-registry-specification.md` §"Validação antes de execução" é
//! "Falha em qualquer etapa produz erro estruturado, sem fallback
//! silencioso" — `ToolError` é onde essa regra vira tipo.

use std::fmt;

use frederico_core::ToolId;

/// Códigos de erro canônicos. Mantidos como `&'static str` para
/// casar com os literais do spec §7.7 e poderem ser usados como chave
/// em `serde_json::Value` (audit log, eventos de UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorCode {
    /// Spec §7.7 item 1 — ID não existe no registro.
    ToolNotFound,
    /// Spec §7.7 item 2 — versão do `tool_call` não bate com a do
    /// manifesto desta execução.
    VersionMismatch,
    /// Spec §7.7 item 3 — ferramenta indisponível ou unhealthy no
    /// momento da chamada.
    Unavailable,
    /// Spec §7.7 item 4 — `tool_call` que o modelo emitiu não está
    /// no inventário desta execução (defesa contra manifest injection).
    NotInExecutionInventory,
    /// Spec §7.7 item 5 — permissões hierárquicas negaram a chamada.
    PermissionDenied,
    /// Spec §7.7 item 6 — argumentos não validam contra o
    /// `input_schema`.
    SchemaInvalid,
    /// Spec §7.7 item 7 — caminho normalizado foge do jail.
    JailViolation,
    /// Spec §7.7 item 8 — limites aplicados (`timeoutMs`, tamanho
    /// máximo de output) não aguentam a chamada.
    LimitsExceeded,
    /// Spec §7.7 item 9 — ferramenta exige aprovação do usuário e
    /// nenhuma foi obtida (ou foi negada).
    ApprovalRequired,
    /// Spec §7.7 item 10 — entrada de auditoria não pôde ser
    /// registrada.
    AuditFailed,
    /// Erro durante a execução da ferramenta em si (não na validação).
    /// Diferente dos códigos acima, que são "rejeitado antes de
    /// executar".
    ExecutionFailed,
}

impl ToolErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolNotFound => "TOOL_NOT_FOUND",
            Self::VersionMismatch => "TOOL_VERSION_MISMATCH",
            Self::Unavailable => "TOOL_UNAVAILABLE",
            Self::NotInExecutionInventory => "TOOL_NOT_IN_INVENTORY",
            Self::PermissionDenied => "TOOL_PERMISSION_DENIED",
            Self::SchemaInvalid => "TOOL_SCHEMA_INVALID",
            Self::JailViolation => "TOOL_JAIL_VIOLATION",
            Self::LimitsExceeded => "TOOL_LIMITS_EXCEEDED",
            Self::ApprovalRequired => "TOOL_APPROVAL_REQUIRED",
            Self::AuditFailed => "TOOL_AUDIT_FAILED",
            Self::ExecutionFailed => "TOOL_EXECUTION_FAILED",
        }
    }
}

impl fmt::Display for ToolErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Erro estruturado de uma `tool_call` (validação ou execução).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub tool_id: Option<ToolId>,
    pub message: String,
    /// Detalhes opcionais (ex.: caminho tentado, razão do schema
    /// inválido). Mantido como `serde_json::Value` para casar com
    /// o que vai pro journal (`message_events.data`).
    pub details: Option<serde_json::Value>,
}

impl ToolError {
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            tool_id: None,
            message: message.into(),
            details: None,
        }
    }

    #[must_use]
    pub fn with_tool_id(mut self, id: ToolId) -> Self {
        self.tool_id = Some(id);
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.tool_id {
            Some(id) => write!(f, "[{}] {}: {}", self.code, id, self.message),
            None => write!(f, "[{}] {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ToolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_match_spec_codes() {
        // Sanidade contra typos nos literais. Se algum desses mudar,
        // é decisão consciente (revisar o spec também).
        assert_eq!(ToolErrorCode::ToolNotFound.as_str(), "TOOL_NOT_FOUND");
        assert_eq!(
            ToolErrorCode::VersionMismatch.as_str(),
            "TOOL_VERSION_MISMATCH"
        );
        assert_eq!(ToolErrorCode::Unavailable.as_str(), "TOOL_UNAVAILABLE");
        assert_eq!(
            ToolErrorCode::NotInExecutionInventory.as_str(),
            "TOOL_NOT_IN_INVENTORY"
        );
        assert_eq!(
            ToolErrorCode::PermissionDenied.as_str(),
            "TOOL_PERMISSION_DENIED"
        );
        assert_eq!(ToolErrorCode::SchemaInvalid.as_str(), "TOOL_SCHEMA_INVALID");
        assert_eq!(ToolErrorCode::JailViolation.as_str(), "TOOL_JAIL_VIOLATION");
        assert_eq!(
            ToolErrorCode::LimitsExceeded.as_str(),
            "TOOL_LIMITS_EXCEEDED"
        );
        assert_eq!(
            ToolErrorCode::ApprovalRequired.as_str(),
            "TOOL_APPROVAL_REQUIRED"
        );
        assert_eq!(ToolErrorCode::AuditFailed.as_str(), "TOOL_AUDIT_FAILED");
        assert_eq!(
            ToolErrorCode::ExecutionFailed.as_str(),
            "TOOL_EXECUTION_FAILED"
        );
    }

    #[test]
    fn builder_methods_chain() {
        let err = ToolError::new(ToolErrorCode::JailViolation, "path traversal")
            .with_tool_id(ToolId::new("files.read"))
            .with_details(serde_json::json!({"path": "..\\..\\etc\\passwd"}));
        assert_eq!(err.code, ToolErrorCode::JailViolation);
        assert_eq!(err.tool_id, Some(ToolId::new("files.read")));
        assert!(err.details.is_some());
    }
}
