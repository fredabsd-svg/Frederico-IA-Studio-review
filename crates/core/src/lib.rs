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
    CheckpointId,
    "Identificador opaco de um checkpoint de execução."
);
opaque_id!(
    ArtifactId,
    "Identificador opaco de um artefato gerado (PDF, DOCX, planilha, etc.)."
);

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
}
