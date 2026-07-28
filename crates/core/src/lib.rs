//! Núcleo compartilhado do Frederico IA Studio.
//!
//! Tipos fundamentais, erros e identificadores opacos usados por todos os
//! outros crates. **Não importa nada de plataforma** (sem `tauri`, sem
//! `windows`, sem paths do sistema) — a regra de pureza do núcleo é
//! verificada por `scripts/check-core-purity.ps1` (REGRAS §1.10 e ADR-0003).

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Erro genérico do núcleo. Outros crates adicionam variantes por
/// `From` conforme necessário, mas o ponto de partida é este.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("identificador inválido: {0}")]
    InvalidId(String),
    #[error("entrada vazia não permitida em {field}")]
    EmptyInput { field: &'static str },
    #[error("falha interna: {0}")]
    Internal(String),
}

/// Resultado padrão do núcleo.
pub type CoreResult<T> = Result<T, CoreError>;

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

opaque_id!(RunId, "Identificador opaco de uma execução de agente.");
opaque_id!(
    ConversationId,
    "Identificador opaco de uma conversa com o usuário."
);
opaque_id!(
    ProjectId,
    "Identificador opaco de um projeto do modo desenvolvedor."
);
opaque_id!(
    AssistantId,
    "Identificador opaco de um assistente (perfil de sistema + ferramentas \
     permitidas). Nullable na v1 — Etapa 6 da Fase 3 popula; até lá, a \
     casca usa um assistente default em runtime."
);
opaque_id!(
    CheckpointId,
    "Identificador opaco de um checkpoint de execução."
);
opaque_id!(
    ArtifactId,
    "Identificador opaco de um artefato gerado (PDF, DOCX, planilha, etc.)."
);
opaque_id!(
    MessageId,
    "Identificador opaco de uma mensagem em uma conversa."
);

/// Identificador de um provedor de LLM (e.g. `\"openai\"`, `\"anthropic\"`,
/// `\"openrouter\"`, `\"simulated\"`). É uma string bem-conhecida, não um
/// UUID, porque o conjunto de provedores é finito e versionado com o app
/// (ver [ADR-0006](../decisions/0006-model-catalog-crate.md)).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl ProviderId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ProviderId {
    fn default() -> Self {
        Self::new("simulated")
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ProviderId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Identificador de um modelo dentro de um provedor (e.g. `\"gpt-4o\"`,
/// `\"claude-3-5-sonnet-latest\"`). String bem-conhecida, igual ao
/// `ProviderId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl ModelId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ModelId {
    fn default() -> Self {
        Self::new("fake-model-v1")
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Identificador de uma ferramenta do catálogo (e.g. `\"files.read\"`,
/// `\"docs.generate\"`). String bem-conhecida, igual a `ProviderId` e
/// `ModelId` — o conjunto de ferramentas é finito e versionado com o app
/// (ver [`tool-registry-specification.md`][]). Aparece em
/// `Run.allowed_tools` e nos manifestos do
/// [`tool-registry-specification.md`][].
///
/// [`tool-registry-specification.md`]: ../../docs/architecture/tool-registry-specification.md
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolId(pub String);

impl ToolId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ToolId {
    fn default() -> Self {
        // Placeholder neutro até a Etapa 2 da Fase 3 (tool-registry) definir
        // o catálogo inicial. Não usar como ferramenta real.
        Self::new("__unset__")
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ToolId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ToolId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Identificador opaco de um provedor. **Não** serializado como caminho
/// do sistema de arquivos (preparação para multiusuário, [ADR-0003]).
pub type EventId = i64;

/// Versão semântica do produto, refletida no SQLite na primeira migração.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl AppVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for AppVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Versão de runtime do produto (alinhada com a do Cargo workspace na v1).
pub const APP_VERSION: AppVersion = AppVersion::new(0, 1, 0);

/// Valida que uma string não está vazia. Helper usado por vários crates.
pub fn require_non_empty(field: &'static str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() {
        return Err(CoreError::EmptyInput { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_unique() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_roundtrip_json() {
        let id = RunId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: RunId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn app_version_format() {
        let v = AppVersion::new(0, 1, 0);
        assert_eq!(v.to_string(), "0.1.0");
    }

    #[test]
    fn require_non_empty_rejects_blank() {
        assert!(require_non_empty("name", "").is_err());
        assert!(require_non_empty("name", "   ").is_err());
        assert!(require_non_empty("name", "ok").is_ok());
    }

    #[test]
    fn app_version_constant_is_stable() {
        assert_eq!(APP_VERSION.to_string(), "0.1.0");
    }

    #[test]
    fn provider_id_roundtrip() {
        let id = ProviderId::new("openai");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"openai\"");
        let back: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn model_id_default_is_simulated() {
        assert_eq!(ModelId::default().as_str(), "fake-model-v1");
    }

    #[test]
    fn message_id_roundtrip_json() {
        let id = MessageId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: MessageId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn assistant_id_roundtrip_json() {
        let id = AssistantId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: AssistantId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn tool_id_default_is_unset() {
        assert_eq!(ToolId::default().as_str(), "__unset__");
    }

    #[test]
    fn tool_id_roundtrip_json() {
        let id = ToolId::new("files.read");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"files.read\"");
        let back: ToolId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
