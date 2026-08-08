//! Ferramentas `exec.*` — `exec.python` e `exec.node` (Etapa 4 da
//! Fase 7), `exec.shell` (Etapa 6, ainda não implementada).
//!
//! Cada ferramenta spawna um binário (Python, Node, ou shell)
//! sob o `SecurityJailResolver` (Etapa 2 da Fase 7), consumindo
//! o `RuntimeRegistry` (Etapa 3) para o `executable` e o
//! `env_vars` que entram no `EnvAllowlist::REQUIRED`.
//!
//! Ver [`docs/architecture/exec-tools-specification.md`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/architecture/exec-tools-specification.md)
//! para o spec completo e [ADR-0034](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/decisions/0034-fase-7-write-exec-approval-policy.md)
//! para a política de aprovação.
//!
//! ## v1 simplificações
//!
//! - **Sem rede** (Etapa 7 da Fase 7 implementa o proxy local).
//!   `exec.python`/`exec.node` rodam sob sandbox, mas `pip install`
//!   falha (degradação declarada). Banner persistente na UI
//!   durante Etapa 4-6.
//! - **Sem cancelamento cascateado per-invocation** (Etapa 4 da
//!   Fase 7 menciona per-spawn Job Object; a v1 só tem o root
//!   Job do resolver — `KILL_ON_JOB_CLOSE` dispara só no shutdown
//!   do app). Cancelamento do `Run` (botão "Parar" do user) faz
//!   `TerminateProcess` no PID, mas netos sobrevivem. Mesmo
//!   problema da Fase 5 Etapa 2.A.
//! - **Sem UI de aprovação** (Etapa 5+). Backend retorna
//!   `ToolError::PermissionDenied` se `PermissionSet::python`
//!   é `None`; caso contrário, executa sem modal. A Etapa 5
//!   da Fase 7 adiciona o `ApprovalModal` (frontend React).
//! - **Sem `exec_patterns.rs` regex** (auto-approval por
//!   "code não casa padrão perigoso"). A Etapa 5+ adiciona
//!   `os.system`, `subprocess.run`, etc. e a aprovação
//!   automática.
//! - **Audit sink mínimo**: o trait `AuditSink` do
//!   `frederico-tool-registry` é usado. Implementação
//!   completa (`DbAuditSink`) vem com a Etapa 5.

#![allow(missing_docs)]

mod node;
mod output;
mod python;

pub use node::FilesExecNodeTool;
pub use output::{MAX_OUTPUT_BYTES, OUTPUT_CHUNK_SIZE};
pub use python::FilesExecPythonTool;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::ToolId;
use frederico_runtimes::RuntimeRegistry;
use frederico_security::jail::SecurityJailResolver;
use serde_json::Value;
use thiserror::Error;

use crate::audit::AuditSink;
use crate::manifest::ToolManifest;
use crate::tools::ToolContext;

/// Re-export do `FilesExecTool` trait para os módulos
/// `python.rs` e `node.rs` (que são privados ao crate).
pub(crate) trait FilesExecTool: Send + Sync {
    /// ID canônico da ferramenta (`exec.python`, `exec.node`).
    fn tool_id(&self) -> ToolId;

    /// Manifesto da ferramenta (input/output schemas, risk_level, etc.).
    fn manifest(&self) -> &ToolManifest;

    /// Resolve o binário (Python, Node) via `RuntimeRegistry`.
    /// Retorna o ID do runtime (ex.: `python-3.12.4`).
    fn resolve_runtime_id<'a>(
        &self,
        args: &'a Value,
    ) -> Result<&'a str, ExecError>;

    /// Monta os args do `CreateProcess` a partir dos args da
    /// tool_call. Implementação diferente por tool:
    /// - Python: `-c "<code>"` / `<path>` / `-m <module>`
    /// - Node:   `<path>` / `-e "<code>"` / `-m <module>`
    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError>;

    /// Default approval scope (ADR-0034 D2). v1: Etapa 4 não tem
    /// UI, retorna `OneExecution` por default (conservador; UI
    /// da Etapa 5 refina).
    #[allow(dead_code)]
    fn default_approval_scope(&self) -> ApprovalScope;
}

/// Escopo de aprovação (re-export da Etapa 1 da Fase de Ligação).
/// v1 da Etapa 4 sempre pede `OneExecution` (conservador).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalScope {
    /// Aprovação por invocação única.
    OneExecution,
    /// Aprovação por turno (1 turno = várias tool_calls).
    OneTurn,
    /// Aprovação por sessão.
    OneSession,
}

/// Camada comum entre `FilesExecPythonTool` e `FilesExecNodeTool`.
/// Carrega as dependências compartilhadas (resolver + registry +
/// audit + wall-clock + output cap).
#[derive(Clone)]
pub(crate) struct FilesExecToolBase {
    /// Resolver de sandbox (Etapa 2). Compartilhado entre tools
    /// do mesmo processo.
    pub resolver: Arc<SecurityJailResolver>,
    /// Registry de runtimes portáteis (Etapa 3).
    pub runtimes: Arc<RuntimeRegistry>,
    /// Audit sink (interface da Etapa 1; implementação real
    /// entra na Etapa 5+).
    pub audit: Arc<dyn AuditSink>,
    /// Wall-clock default (60s). Caller pode override via
    /// `max_wall_clock_ms` no `tool_call.args`.
    pub default_wall_clock: Duration,
}

impl FilesExecToolBase {
    /// Construtor padrão. Wall-clock default = 60s, output cap
    /// = 10 MB (const em `output.rs`).
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn new(
        resolver: Arc<SecurityJailResolver>,
        runtimes: Arc<RuntimeRegistry>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            resolver,
            runtimes,
            audit,
            default_wall_clock: Duration::from_secs(60),
        }
    }

    /// Resolve o wall-clock do `tool_call.args.max_wall_clock_ms`
    /// (clamped 1s..=600s) ou usa o default da base.
    pub fn wall_clock_for(&self, args: &Value) -> Duration {
        let ms = args
            .get("max_wall_clock_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_wall_clock.as_millis() as u64);
        let ms = ms.clamp(1_000, 600_000);
        Duration::from_millis(ms)
    }

    /// Verifica o `PermissionSet` da execução. v1 simplificado:
    /// se `python`/`node` é `None`, retorna `PermissionDenied`.
    /// Caso contrário, `Ok`. A Etapa 5+ adiciona `ApprovalModal`.
    pub fn check_permission(&self, args: &Value, ctx: &ToolContext) -> Result<(), ExecError> {
        // v1: olha `ctx.permissions` se o `ToolContext` tiver.
        // O `ToolContext` atual (Fase de Ligação) NÃO tem
        // `permissions` — esse campo é responsabilidade do
        // `RunExecutor` (Etapa 4 da Fase 3) que valida via
        // `validate_tool_call`. Aqui, a Etapa 4 confia que a
        // validação do `RunExecutor` já checou.
        //
        // Para a Etapa 4 v1, o gate é simples: o `tool_call`
        // precisa ter **algum** arg executor (`code`/`path`/`module`).
        // Se não tem, retorna `InvalidArgs` antes do spawn.
        let has_arg = args.get("code").is_some()
            || args.get("path").is_some()
            || args.get("module").is_some();
        if !has_arg {
            return Err(ExecError::InvalidArgs(
                "tool_call precisa de `code` OU `path` OU `module` (ver input_schema)".to_string(),
            ));
        }
        let _ = ctx; // ctx reservado para Etapa 5+
        Ok(())
    }

    /// Verifica o `PermissionSet` global. Diferente do
    /// `check_permission` (por tool_call), este é o gate
    /// "default deny" do ADR-0034 D1: se o user desligou
    /// `python` no settings, **toda** invocação é bloqueada.
    ///
    /// v1: aceita tudo (o gate real é via `validate_tool_call`
    /// do `RunExecutor`). Retorna `Ok(())` por enquanto.
    /// Etapa 5+ lê o `PermissionSet` real.
    pub fn check_global_permission(&self) -> Result<(), ExecError> {
        // v1: sempre Ok. O gate real é responsabilidade do
        // `validate_tool_call` no `RunExecutor` (Etapa 4 da
        // Fase 3), que olha o `PermissionSet` e a allowlist
        // por exec tool.
        Ok(())
    }
}

/// Erros do `exec.*` tools. Cada variante carrega o `tool_id`
/// + mensagem PT-BR. Mapeado para `ToolResult::err` no `execute`.
#[derive(Debug, Error)]
pub enum ExecError {
    /// `PermissionSet::python` (ou `node`) é `None` — user desligou.
    #[error("permission denied: {0} (user desligou a ferramenta)")]
    PermissionDenied(&'static str),
    /// Args inválidos (faltando `code`/`path`/`module`, ou tipo errado).
    #[error("argumentos invalidos: {0}")]
    InvalidArgs(String),
    /// Runtime não encontrado (ex.: `runtime=python-3.13.0` mas
    /// só temos `python-3.12.4` registrado).
    #[error("runtime '{0}' nao encontrado")]
    UnknownRuntime(String),
    /// `SecurityJailResolver::spawn` falhou.
    #[error("spawn falhou: {0}")]
    SpawnFailed(String),
    /// Processo terminou com exit code não-zero.
    #[error("exec falhou (exit code {code}): {stderr}")]
    NonZeroExit { code: i32, stderr: String },
    /// Wall-clock excedido.
    #[error("wall-clock excedido (>{0}s)")]
    WallClockExceeded(u64),
    /// Cancelamento (Etapa 4+).
    #[error("cancelado pelo user")]
    Cancelled,
}
