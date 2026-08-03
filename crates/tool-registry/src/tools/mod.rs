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
//!
//! ## `Tool::execute` recebe `ToolContext` (Etapa 1 da Fase de Ligação)
//!
//! A partir do commit `fase-ligacao/conectar-motor-a-casca` Etapa 1
//! commit 4a, a trait `Tool` carrega um `ToolContext` além dos
//! `arguments`. O contexto entrega pra ferramenta tudo que é
//! estável durante a execução **mas que muda entre runs** (a
//! fronteira de isolamento, em particular): o `Jail` resolvido
//! por `ConversationId` (ADR-0022 §D3), além dos IDs imutáveis
//! do run. **Breaking change** — registrado no `CHANGELOG.md`
//! da Etapa 1. `#[non_exhaustive]` no `ToolContext` permite
//! acrescentar campos depois sem nova quebra.
use std::path::PathBuf;

use async_trait::async_trait;
use frederico_core::{ConversationId, MessageId, RunId, ToolId};
use serde::{Deserialize, Serialize};

use crate::manifest::ToolManifest;
use crate::workspace::Jail;

/// Contexto de execução entregue a cada `Tool::execute`.
///
/// **Carregado uma vez por run pelo `RunExecutor`** (não por
/// `tool_call`): o `RunExecutor` resolve o `conversation_id`
/// (query única no `RunRepo::get(run_id)` no início do `run()`),
/// resolve o `Jail` correspondente (via `JailResolver`), e
/// constrói o `ToolContext` por tool_call com custo O(1) (sem
/// I/O). O `conversation_id` é imutável durante o run, então
/// carregá-lo uma vez elimina query por chamada e ponto de
/// falha em caminho quente.
///
/// Ver ADR-0022 §D3.
///
/// ## `#[non_exhaustive]`
///
/// Acrescentar campo no contexto não é breaking change para
/// quem constrói o valor (não pode usar struct literal fora do
/// crate), mas a Etapa 7 (modo desenvolvedor) pode adicionar
/// `workspace: Option<WorkspaceSnapshot>` etc. sem nova quebra.
/// O mesmo padrão é usado pelo `MemoryHit` no
/// `frederico-memory` (§3 do `memory-architecture.md`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolContext {
    /// `ConversationId` do run. Usado pela `FilesReadTool` para
    /// resolver o `Jail` per-conversa (se a tool usar
    /// `JailResolver` em vez do `Jail` injetado).
    pub conversation_id: ConversationId,
    /// `RunId` em andamento. Disponível para tools que precisem
    /// correlacionar com a tabela de auditoria (`tool_audit`).
    pub run_id: RunId,
    /// `MessageId` da resposta assistant em construção.
    pub message_id: MessageId,
    /// `Jail` resolvido para esta conversa. **Defesa em
    /// profundidade**: o `validate_tool_call` (Passo 7) já
    /// rodou contra este mesmo jail; o `Tool::execute` revalida
    /// o path antes de tocar o disco.
    pub jail: Jail,
}

impl ToolContext {
    /// Construtor usado pelo `RunExecutor` (única chamada
    /// externa; testes podem usar livremente).
    #[must_use]
    pub fn new(
        conversation_id: ConversationId,
        run_id: RunId,
        message_id: MessageId,
        jail: Jail,
    ) -> Self {
        Self {
            conversation_id,
            run_id,
            message_id,
            jail,
        }
    }
}

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
    ///
    /// **`ctx: &ToolContext`** (Etapa 1 da Fase de Ligação): o
    /// contexto entrega o `Jail` resolvido para a conversa
    /// corrente, além dos IDs imutáveis do run. O `RunExecutor`
    /// resolve o `Jail` uma vez por run (não por tool_call) e
    /// constrói o `ToolContext` por chamada. Breaking change
    /// em relação à Etapa 3 da Fase 5 — registrado no
    /// `CHANGELOG.md` da Etapa 1.
    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult;
}

pub mod files_read;
pub use files_read::FilesReadTool;
