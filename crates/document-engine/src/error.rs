//! Erros do `frederico-document-engine`.
//!
//! A Etapa 1 só produz dois tipos de erro: falha de deserialização
//! (JSON → `DocumentSpec`) e falha de validação (JSON Schema + regras
//! semânticas). Os códigos são `snake_case` estáveis — o
//! `execution-engine` da Etapa 3 mapeia estes códigos para o envelope
//! `TOOL_ERROR` que volta pro modelo.

use thiserror::Error;

/// Erro do Document Engine. Vem do `validate` ou do `parse`.
#[derive(Debug, Error)]
pub enum DocumentError {
    /// JSON malformado ou campo obrigatório faltando.
    #[error("JSON inválido: {message} (path: {path})")]
    Parse {
        /// Caminho JSON pointer do ponto de falha (ex: `/blocks/3/text`).
        path: String,
        /// Mensagem curta do `serde_json` (ou do JSON Schema, se a
        /// desserialização tipada falhou).
        message: String,
    },

    /// JSON válido em formato, mas viola o JSON Schema.
    #[error("Spec viola o JSON Schema: {message} (path: {path})")]
    Schema {
        /// Caminho JSON pointer do ponto de falha.
        path: String,
        /// Mensagem do validador (`jsonschema`).
        message: String,
    },

    /// JSON válido em formato e no schema, mas viola uma regra
    /// semântica que o JSON Schema não expressa (ex: `Kpis` com 5
    /// cartões — schema aceita, regra rejeita).
    #[error("Spec viola regra semântica: {message} (path: {path})")]
    Semantic {
        /// Caminho JSON pointer do ponto de falha.
        path: String,
        /// Mensagem da regra violada.
        message: String,
    },
}

impl DocumentError {
    /// Código estável do erro (em `snake_case`). O `execution-engine`
    /// consome isto pra mapear pro envelope `TOOL_ERROR` que volta
    /// pro modelo — em snake_case pra casar com o resto do registry.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Parse { .. } => "document_parse_error",
            Self::Schema { .. } => "document_schema_invalid",
            Self::Semantic { .. } => "document_semantic_invalid",
        }
    }

    /// Path JSON pointer onde o erro ocorreu.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Parse { path, .. } | Self::Schema { path, .. } | Self::Semantic { path, .. } => {
                path
            }
        }
    }
}
