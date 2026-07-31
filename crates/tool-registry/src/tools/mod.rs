//! Trait `Tool` e o tipo de retorno [`ToolResult`].
//!
//! Cada ferramenta concreta (Etapa 2: `files.read`; Etapa 3 da
//! Fase 3: `docs.generate`; Etapas seguintes: `files.write`,
//! `files.list`, `docs.inspect`, ...) implementa `Tool`. O
//! executor da Etapa 4 consome o resultado e traduz em
//! `message_events` / transições da máquina de estados.
//!
//! ## `Tool::execute` é `async`
//!
//! A partir da Etapa 3 da Fase 5, `Tool::execute` é `async fn`.
//! A decisão foi tomada para eliminar a ponte sync→async que a
//! Etapa 3 da Fase 5 teria que construir entre o `Tool::execute`
//! síncrono e o `WorkerHandle::invoke` assíncrono. O `RunExecutor`
//! (Etapa 4 da Fase 3) já é async — a ponte só serviria para a
//! ferramenta do kit de documentos, e ferramentas in-process
//! como `files.read` não ganham nem perdem com a mudança (file
//! I/O continua síncrono dentro do `async fn`). O `async_trait`
//! mantém `Arc<dyn Tool>` dyn-compatible.
//!
//! Testes que chamam `tool.execute(...)` direto precisam ser
//! `#[tokio::test]` (default `current_thread` runtime é
//! suficiente — só o worker-backed tools precisam de
//! `flavor = "multi_thread"`, e isso é documentado em
//! `worker_dispatch.rs`).
use std::path::PathBuf;

use async_trait::async_trait;
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

/// Trait comum a todas as ferramentas.
///
/// **`async fn execute`** (Etapa 3 da Fase 5): o `RunExecutor`
/// (Etapa 4 da Fase 3) é async; tornar `Tool::execute` async
/// elimina a ponte sync→async para ferramentas worker-backed
/// (como `docs.generate` da Etapa 3 da Fase 5) sem custo para
/// ferramentas in-process (file I/O do `files.read` continua
/// síncrono dentro do `async fn`).
///
/// Etapas seguintes podem estender o trait com `validate_paths`,
/// `prepare_approval_request`, etc. A Etapa 2 da Fase 3
/// definiu o mínimo necessário para `files.read` funcionar.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Manifesto da ferramenta. O `ToolRegistry` usa isso pra
    /// descobrir o que a ferramenta é, sem precisar instanciá-la.
    fn manifest(&self) -> &ToolManifest;

    /// Executa a ferramenta. Os argumentos **já foram validados**
    /// pelo `validate_tool_call` (schema, jail, permissões). O
    /// executor só chama isso se a validação passou.
    ///
    /// `async` para que ferramentas worker-backed (Etapa 3 da
    /// Fase 5+) possam chamar `WorkerHandle::invoke` direto,
    /// sem ponte sync→async. `Arc<dyn Tool>` continua
    /// funcionando (`async_trait` mantém a trait dyn-compatible).
    async fn execute(&self, arguments: &serde_json::Value) -> ToolResult;
}

pub mod files_read;
pub use files_read::FilesReadTool;
