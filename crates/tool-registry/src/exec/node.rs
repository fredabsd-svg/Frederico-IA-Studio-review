//! `FilesExecNodeTool` — executa Node sob sandbox (Etapa 4 da
//! Fase 7).
//!
//! Espelha `FilesExecPythonTool` com 2 diferenças:
//! - Runtime default: `node-20.16.0`.
//! - Args: `<path>` (script.js) ou `-e "<code>"` (não suporta
//!   `-m` no v1 — raro no Node).
//!
//! Mesmas garantias de wall-clock enforcement real + cancelamento
//! cascateado per-invocation + aprovação obrigatória via
//! `validate_tool_call` (Passo 9). Ver doc do `python.rs`
//! para os detalhes.

use std::time::Duration;

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_runtimes::RuntimeId;
use frederico_security::jail::SandboxConfig;
use serde_json::{json, Value};

use crate::exec::output::{collect_output, output_json};
use crate::exec::{ApprovalScope, ExecError, FilesExecTool, FilesExecToolBase};
use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `exec.node`.
pub struct FilesExecNodeTool {
    pub manifest: ToolManifest,
    pub(crate) base: FilesExecToolBase,
}

impl FilesExecNodeTool {
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
                "code": {
                    "type": "string",
                    "description": "Codigo JS via `node -e`. Mutuamente exclusivo com `path`."
                },
                "path": {
                    "type": "string",
                    "description": "Caminho do script .js (relativo ao workdir). Mutuamente exclusivo."
                },
                "runtime": {
                    "type": "string",
                    "description": "ID do runtime (default `node-20.16.0`).",
                    "default": "node-20.16.0"
                },
                "max_wall_clock_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": 600000,
                    "description": "Wall-clock em ms (default 60000, max 600000)."
                }
            },
            "anyOf": [
                {"required": ["code"]},
                {"required": ["path"]}
            ],
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
        ToolManifestBuilder::new(ToolId::new("exec.node"), "exec")
            .version("0.1.0")
            .display_name("Executar Node.js")
            .description(
                "Executa codigo Node.js sob sandbox. Args: code (`node -e`) OU path (script.js). Wall-clock default 60s. Sem rede ate Etapa 7. Requer aprovacao por invocacao (Etapa 4 v1).",
            )
            .category(ToolCategory::Exec)
            .risk_level(RiskLevel::High)
            .capability("exec.node")
            .requires_process_execution(true)
            .requires_user_approval(true)
            .timeout_ms(60_000)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .build()
            .expect("manifesto de exec.node bem-formado")
    }
}

impl FilesExecTool for FilesExecNodeTool {
    fn tool_id(&self) -> ToolId {
        ToolId::new("exec.node")
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn resolve_runtime_id<'a>(&self, args: &'a Value) -> Result<&'a str, ExecError> {
        Ok(args
            .get("runtime")
            .and_then(|v| v.as_str())
            .unwrap_or("node-20.16.0"))
    }

    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError> {
        if let Some(code) = args.get("code").and_then(|v| v.as_str()) {
            Ok(vec!["-e".to_string(), code.to_string()])
        } else if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            Ok(vec![path.to_string()])
        } else {
            Err(ExecError::InvalidArgs(
                "tool_call precisa de `code` OU `path`".to_string(),
            ))
        }
    }

    #[allow(dead_code)]
    fn default_approval_scope(&self) -> ApprovalScope {
        ApprovalScope::OneExecution
    }
}

#[async_trait]
impl Tool for FilesExecNodeTool {
    fn manifest(&self) -> &ToolManifest {
        FilesExecTool::manifest(self)
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &Value) -> ToolResult {
        let tool_id = self.tool_id();

        if let Err(e) = self.base.check_global_permission() {
            return ToolResult::err(tool_id, e.to_string());
        }
        if let Err(e) = self.base.check_permission(arguments, ctx) {
            return ToolResult::err(tool_id, e.to_string());
        }

        let runtime_id_str = match self.resolve_runtime_id(arguments) {
            Ok(s) => s,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };
        let runtime = match self.base.runtimes.get(&RuntimeId::new(runtime_id_str)) {
            Some(r) => r,
            None => {
                return ToolResult::err(
                    tool_id,
                    format!(
                        "runtime '{runtime_id_str}' nao registrado (Etapa 4 v1: so `node-20.16.0`)"
                    ),
                );
            }
        };

        let tool_args = match self.build_args(arguments) {
            Ok(a) => a,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let wall_clock = self.base.wall_clock_for(arguments);

        // Sobe o proxy de rede (Etapa 6 da Fase 7, ADR-0033).
        // Ver `python.rs::execute` para a justificativa completa
        // do RAII guard + ordem do drop.
        let proxy_guard = match self.base.start_network_proxy(ctx.jail.root(), ctx.run_id) {
            Ok(g) => g,
            Err(e) => {
                return ToolResult::err(tool_id, format!("start_network_proxy falhou: {e}"));
            }
        };

        let mut config = SandboxConfig::new(
            runtime.executable().to_path_buf(),
            tool_args,
            ctx.jail.root().to_path_buf(),
        );
        if proxy_guard.is_enabled() {
            config.extra_env = proxy_guard.extra_env();
        }

        let mut process = match self.base.resolver.spawn(config) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(tool_id, format!("spawn falhou: {e}")),
        };

        // Ver `python.rs::execute` para a justificativa completa
        // de `&mut SandboxedProcess` em vez de `Child` consumido.
        let raw = match collect_output(&mut process, wall_clock).await {
            Ok(r) => r,
            Err(e) => return ToolResult::err(tool_id, e),
        };

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
                format!("node exit code {}: {}", raw.exit_code, raw.stderr),
            );
        }
        ToolResult::ok(
            tool_id,
            output_json(&raw),
            vec![runtime.executable().to_path_buf()],
        )
    }
}
