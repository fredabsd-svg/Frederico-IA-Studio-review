//! `FilesExecShellTool` — executa comandos via `cmd.exe` sob
//! sandbox (Etapa 7 da Fase 7, ADR-0031 + ADR-0034 D3).
//!
//! Ver spec `docs/architecture/exec-tools-specification.md`
//! §"`FilesExecShellTool`".
//!
//! ## Diferenças pra `exec.python`/`exec.node`
//!
//! - **Sem `RuntimeRegistry`**: `cmd.exe` é built-in do Windows
//!   (não um runtime portátil pinned por SHA-256). Resolvido via
//!   `%SystemRoot%\System32\cmd.exe` — `SystemRoot` já é
//!   `EnvAllowlist::REQUIRED` (Etapa 6+1) então o path é estável.
//! - **`risk_level: Critical`** (não `High`) — shell é a ferramenta
//!   mais permissiva do registro; `Critical` é o único nível que
//!   força `ApprovalRequest.mandatory = true` mesmo sem UI de
//!   escopo (ver `validate.rs::with_mandatory_for_risk`).
//! - **Denylist + allowlist sempre ativas** (`frederico_security::exec_patterns`):
//!   o comando passa por `denylist_hit` (recusa dura, antes do
//!   spawn) e depois por `is_allowed` contra `SHELL_ALLOWLIST_DEFAULT`
//!   (só binários read-only). **Por que incondicional, e não
//!   gateado por `PermissionSet::terminal`:** o `ToolContext` que
//!   chega no `execute()` não carrega o `PermissionSet` da run
//!   (é responsabilidade do `validate_tool_call` no `RunExecutor`,
//!   que hoje não lê `permissions.terminal` — ver
//!   `crates/tool-registry/src/validate.rs` Passo 5). Aplicar a
//!   allowlist sempre, em vez de "só quando `TerminalMode::Allowlist`",
//!   é consistente com o teto do projeto: `PermissionSet::allow_all()`
//!   já fixa `terminal: TerminalMode::Allowlist` (nunca "sem
//!   restrição" — não existe essa variante). Refinar por
//!   `PermissionSet` real é trabalho de Fase 8 quando o
//!   `ToolContext` carregar permissões.
//! - **Comando como string única**: `cmd.exe /c "<command>"`, sem
//!   shell intermediário adicional (evita reintroduzir a
//!   superfície de injection que o `/c` puro já minimiza).

use std::time::Duration;

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_security::exec_patterns::{
    denylist_hit, first_token, is_allowed, SHELL_ALLOWLIST_DEFAULT,
};
use frederico_security::jail::SandboxConfig;
use serde_json::{json, Value};

use crate::exec::output::{collect_output, output_json};
use crate::exec::{ApprovalScope, ExecError, FilesExecTool, FilesExecToolBase};
use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `exec.shell`.
pub struct FilesExecShellTool {
    pub manifest: ToolManifest,
    pub(crate) base: FilesExecToolBase,
}

impl FilesExecShellTool {
    #[must_use]
    pub(crate) fn new(base: FilesExecToolBase) -> Self {
        Self {
            manifest: Self::build_manifest(),
            base,
        }
    }

    fn input_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Comando executado via `cmd.exe /c \"<command>\"`. Passado como string unica (sem shell intermediario)."
                },
                "max_wall_clock_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": 600000,
                    "description": "Wall-clock em ms (default 60000, max 600000)."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }))
    }

    fn output_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "stdout": {"type": "string"},
                "stderr": {"type": "string"},
                "exit_code": {"type": "integer"},
                "duration_ms": {"type": "integer"},
                "truncated": {"type": "boolean"},
                "bytes_stdout": {"type": "integer"},
                "bytes_stderr": {"type": "integer"}
            },
            "required": ["stdout", "stderr", "exit_code", "duration_ms", "truncated", "bytes_stdout", "bytes_stderr"]
        }))
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("exec.shell"), "exec")
            .version("0.1.0")
            .display_name("Executar comando de terminal")
            .description(
                "Executa um comando via cmd.exe sob sandbox. Denylist de comandos destrutivos (rm -rf, format, etc.) sempre ativa; allowlist de binarios read-only (ls, cat, grep, ...) sempre ativa nesta v1. Sempre requer aprovacao por invocacao (ADR-0034 D3) — nunca reusa aprovacao anterior.",
            )
            .category(ToolCategory::Exec)
            .risk_level(RiskLevel::Critical)
            .capability("exec.shell")
            .requires_process_execution(true)
            .requires_user_approval(true)
            .timeout_ms(60_000)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .build()
            .expect("manifesto de exec.shell bem-formado")
    }

    /// Resolve `%SystemRoot%\System32\cmd.exe`. `SystemRoot` é
    /// `EnvAllowlist::REQUIRED` (Etapa 6+1) — se ausente do
    /// ambiente do processo pai, é configuração de SO quebrada,
    /// não um caso a degradar silenciosamente.
    fn cmd_exe_path() -> Result<std::path::PathBuf, ExecError> {
        let system_root = std::env::var("SystemRoot").map_err(|_| {
            ExecError::SpawnFailed(
                "variavel de ambiente SystemRoot ausente — nao consigo resolver cmd.exe"
                    .to_string(),
            )
        })?;
        Ok(std::path::Path::new(&system_root)
            .join("System32")
            .join("cmd.exe"))
    }
}

impl FilesExecTool for FilesExecShellTool {
    fn tool_id(&self) -> ToolId {
        ToolId::new("exec.shell")
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn resolve_runtime_id<'a>(&self, _args: &'a Value) -> Result<&'a str, ExecError> {
        // `exec.shell` nao usa `RuntimeRegistry` (cmd.exe e
        // built-in do SO, nao um runtime portatil pinned). O
        // valor e informativo (audit/log), o path real vem de
        // `cmd_exe_path`.
        Ok("system-cmd")
    }

    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::InvalidArgs("tool_call precisa de `command`".to_string()))?;

        if let Some(pattern) = denylist_hit(command) {
            return Err(ExecError::CommandDenied(pattern.to_string()));
        }
        if !is_allowed(command, SHELL_ALLOWLIST_DEFAULT) {
            let program = first_token(command).unwrap_or("").to_string();
            return Err(ExecError::CommandNotInAllowlist(program));
        }

        Ok(vec!["/c".to_string(), command.to_string()])
    }

    #[allow(dead_code)]
    fn default_approval_scope(&self) -> ApprovalScope {
        // ADR-0034 D3: exec.shell e SEMPRE OneExecution, sem
        // excecao — diferente de python/node, o escopo nao pode
        // ser aumentado pelo usuario na hora da aprovacao.
        ApprovalScope::OneExecution
    }
}

#[async_trait]
impl Tool for FilesExecShellTool {
    fn manifest(&self) -> &ToolManifest {
        FilesExecTool::manifest(self)
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &Value) -> ToolResult {
        let tool_id = self.tool_id();

        if let Err(e) = self.base.check_global_permission() {
            return ToolResult::err(tool_id, e.to_string());
        }

        // Denylist/allowlist checadas dentro de `build_args`,
        // ANTES do spawn — nenhum Job Object/processo e criado
        // pra um comando ja recusado (defesa em profundidade +
        // eficiencia, mesmo racional do `check_permission` de
        // python/node).
        let tool_args = match self.build_args(arguments) {
            Ok(a) => a,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let cmd_exe = match Self::cmd_exe_path() {
            Ok(p) => p,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let wall_clock = self.base.wall_clock_for(arguments);

        // Sobe o proxy de rede (Etapa 6 da Fase 7, ADR-0033) —
        // mesmo mecanismo de exec.python/exec.node. Guard RAII,
        // segurado ate depois do `collect_output` (ver doc de
        // `FilesExecToolBase::start_network_proxy`).
        let proxy_guard = match self.base.start_network_proxy(ctx.jail.root(), ctx.run_id) {
            Ok(g) => g,
            Err(e) => {
                return ToolResult::err(tool_id, format!("start_network_proxy falhou: {e}"));
            }
        };

        let mut config =
            SandboxConfig::new(cmd_exe.clone(), tool_args, ctx.jail.root().to_path_buf());
        if proxy_guard.is_enabled() {
            config.extra_env = proxy_guard.extra_env();
        }

        let mut process = match self.base.resolver.spawn(config) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult::err(tool_id, format!("spawn falhou: {e}"));
            }
        };

        let raw = match collect_output(&mut process, wall_clock).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    tool_id = %tool_id,
                    error = %e,
                    "exec.shell: collect_output falhou (spawn ou wait falhou)"
                );
                return ToolResult::err(tool_id, e);
            }
        };
        // `process` e `proxy_guard` dropam aqui (mesma ordem e
        // mesmo racional documentado em `python.rs`).

        let result_json =
            serde_json::to_string(&output_json(&raw)).unwrap_or_else(|_| "{}".to_string());
        let _ = self.base.audit.record(crate::audit::AuditEntry {
            tool_id: tool_id.clone(),
            tool_version: "0.1.0".to_string(),
            arguments_json: arguments.to_string(),
            result_ok: raw.exit_code == 0,
            result_json,
            duration: Duration::from_millis(raw.duration_ms),
        });

        if raw.exit_code != 0 {
            return ToolResult::err(
                tool_id,
                format!("comando exit code {}: {}", raw.exit_code, raw.stderr),
            );
        }
        ToolResult::ok(tool_id, output_json(&raw), vec![cmd_exe])
    }
}
