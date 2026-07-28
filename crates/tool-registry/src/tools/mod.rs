//! Trait `Tool` e o tipo de retorno [`ToolResult`].
//!
//! Cada ferramenta concreta (Etapa 2: `files.read`; Etapas
//! seguintes: `files.write`, `files.list`, `docs.inspect`, ...)
/// implementa `Tool`. O executor da Etapa 4 consome o resultado e
/// traduz em `message_events` / transições da máquina de estados.
use std::path::PathBuf;

use frederico_core::ToolId;
use serde::{Deserialize, Serialize};

use crate::manifest::ToolManifest;

/// Resultado de uma `tool_call`. O conteúdo é opaco (JSON
/// arbitrário) — cada ferramenta define seu próprio `output_schema`
/// no manifesto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_id: ToolId,
    /// `true` se a ferramenta executou com sucesso. `false` é um
    /// erro reportado pela ferramenta (não pelo validador — esses
    /// seriam `ValidationOutcome::Rejected`).
    pub ok: bool,
    /// Output conforme `output_schema` (quando `ok`).
    pub output: serde_json::Value,
    /// Mensagem de erro (quando `!ok`). PT-BR pela regra do
    /// spec `chat-and-providers.md` §"Mensagens PT-BR".
    pub error_message: Option<String>,
    /// Caminhos que foram efetivamente acessados. Vai pro
    /// `message_events.data` pra audit log.
    pub accessed_paths: Vec<PathBuf>,
}

impl ToolResult {
    #[must_use]
    pub fn ok(tool_id: ToolId, output: serde_json::Value, accessed: Vec<PathBuf>) -> Self {
        Self {
            tool_id,
            ok: true,
            output,
            error_message: None,
            accessed_paths: accessed,
        }
    }

    #[must_use]
    pub fn err(tool_id: ToolId, message: impl Into<String>) -> Self {
        Self {
            tool_id,
            ok: false,
            output: serde_json::Value::Null,
            error_message: Some(message.into()),
            accessed_paths: Vec::new(),
        }
    }
}

/// Trait comum a todas as ferramentas. Etapa 4 vai
/// estender isso com `validate_paths`, `prepare_approval_request`,
/// etc. A Etapa 2 define o mínimo necessário para `files.read`
/// funcionar.
pub trait Tool: Send + Sync {
    /// Manifesto da ferramenta. O `ToolRegistry` usa isso pra
    /// descobrir o que a ferramenta é, sem precisar instanciá-la.
    fn manifest(&self) -> &ToolManifest;

    /// Executa a ferramenta. Os argumentos **já foram validados**
    /// pelo `validate_tool_call` (schema, jail, permissões). O
    /// executor só chama isso se a validação passou.
    fn execute(&self, arguments: &serde_json::Value) -> ToolResult;
}

pub mod files_read;
pub use files_read::FilesReadTool;
