//! `FilesExecPythonTool` — executa Python sob sandbox (Etapa 4
//! da Fase 7, ADR-0031 + ADR-0036).
//!
//! Ver spec `docs/architecture/exec-tools-specification.md`
//! §"`FilesExecPythonTool`".
//!
//! ## Comportamento (Etapa 4 da Fase 7)
//!
//! - **Wall-clock enforcement real** — `wait_with_timeout`
//!   dentro do `collect_output` (concorrente com leitura dos
//!   streams). Estoura → kill do child + drop do SandboxedProcess
//!   cascateia `KILL_ON_JOB_CLOSE` na árvore.
//! - **Cancelamento cascateado per-invocation** — cada `spawn`
//!   cria um Job Object novo. Drop fecha o handle e mata a
//!   árvore (filho + netos + bisnetos).
//! - **Aprovação obrigatória** — `requires_user_approval(true)`
//!   no manifesto. O `validate_tool_call` Passo 9 é o gate;
//!   sem `ApprovalDecision` aprovada, retorna `ApprovalRequired`
//!   e o `execute` nem é chamado.
//! - **Sem `exec_patterns.rs`** — auto-approval por code sem
//!   padrão perigoso é Etapa 5+ (só faz sentido pro `exec.shell`).
//! - **Audit mínimo** — 1 entrada por invocação, sem
//!   `kind`/`runtime`/campos extras (o `AuditEntry` da Fase 3
//!   não tem).

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

/// A ferramenta `exec.python`.
pub struct FilesExecPythonTool {
    pub manifest: ToolManifest,
    pub(crate) base: FilesExecToolBase,
}

impl FilesExecPythonTool {
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
                    "description": "Codigo Python via `python -c`. Mutuamente exclusivo com `path` e `module`."
                },
                "path": {
                    "type": "string",
                    "description": "Caminho do script .py (relativo ao workdir). Mutuamente exclusivo."
                },
                "module": {
                    "type": "string",
                    "description": "Modulo via `python -m`. Mutuamente exclusivo."
                },
                "runtime": {
                    "type": "string",
                    "description": "ID do runtime (default `python-3.12.4`).",
                    "default": "python-3.12.4"
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
                {"required": ["path"]},
                {"required": ["module"]}
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
        ToolManifestBuilder::new(ToolId::new("exec.python"), "exec")
            .version("0.1.0")
            .display_name("Executar Python")
            .description(
                "Executa codigo Python sob sandbox. Args: code (`python -c`) OU path OU module (`python -m`). Wall-clock default 60s. Sem rede ate Etapa 7. Requer aprovacao por invocacao (Etapa 4 v1).",
            )
            .category(ToolCategory::Exec)
            .risk_level(RiskLevel::High)
            .capability("exec.python")
            .requires_process_execution(true)
            .requires_user_approval(true)
            .timeout_ms(60_000)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .build()
            .expect("manifesto de exec.python bem-formado")
    }
}

impl FilesExecTool for FilesExecPythonTool {
    fn tool_id(&self) -> ToolId {
        ToolId::new("exec.python")
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn resolve_runtime_id<'a>(&self, args: &'a Value) -> Result<&'a str, ExecError> {
        Ok(args
            .get("runtime")
            .and_then(|v| v.as_str())
            .unwrap_or("python-3.12.4"))
    }

    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError> {
        if let Some(code) = args.get("code").and_then(|v| v.as_str()) {
            Ok(vec!["-c".to_string(), code.to_string()])
        } else if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            Ok(vec![path.to_string()])
        } else if let Some(module) = args.get("module").and_then(|v| v.as_str()) {
            Ok(vec!["-m".to_string(), module.to_string()])
        } else {
            Err(ExecError::InvalidArgs(
                "tool_call precisa de `code` OU `path` OU `module`".to_string(),
            ))
        }
    }

    #[allow(dead_code)]
    fn default_approval_scope(&self) -> ApprovalScope {
        ApprovalScope::OneExecution
    }
}

#[async_trait]
impl Tool for FilesExecPythonTool {
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
                    format!("runtime '{runtime_id_str}' nao registrado (Etapa 4 v1: so `python-3.12.4`)"),
                );
            }
        };

        let tool_args = match self.build_args(arguments) {
            Ok(a) => a,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let wall_clock = self.base.wall_clock_for(arguments);

        // Sobe o proxy de rede (Etapa 6 da Fase 7, ADR-0033).
        // O guard é RAII — drop no fim do `execute()` chama
        // `frederico_security::network::shutdown` (fecha o
        // listener Tokio) e remove o `proxy.port`. **Segurar
        // o guard até depois de `collect_output` é obrigatório**
        // — drop prematuro = o filho perde a saída de rede
        // (a request TCP falha com "connection reset" mid-exec).
        let proxy_guard = match self.base.start_network_proxy(ctx.jail.root(), ctx.run_id) {
            Ok(g) => g,
            Err(e) => {
                return ToolResult::err(tool_id, format!("start_network_proxy falhou: {e}"));
            }
        };

        // Injeta `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` no
        // `SandboxConfig::extra_env` (Etapa 4 v1 já tem o
        // campo, o `EnvFilter` da Etapa 2/Etapa 6 valida que
        // `HTTP_PROXY` está em REQUIRED antes de deixar passar).
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
            Err(e) => {
                return ToolResult::err(tool_id, format!("spawn falhou: {e}"));
            }
        };

        // `collect_output` recebe `&mut SandboxedProcess` (não
        // `Child` consumido). Isso é o que permite wall-clock
        // real (`wait_with_timeout` dentro do `tokio::join!`)
        // E mantém o Job Object per-invocation vivo durante a
        // execução — drop do `process` no fim do escopo fecha
        // o handle do Job (mata a árvore se sobrar neto).
        let raw = match collect_output(&mut process, wall_clock).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    tool_id = %tool_id,
                    error = %e,
                    "exec.python: collect_output falhou (spawn ou wait falhou)"
                );
                return ToolResult::err(tool_id, e);
            }
        };
        // `process` dropa aqui. Como `wait_with_timeout` já
        // coletou o exit status, o `Drop` do child é no-op
        // (processo já morto). O Drop do Job é no-op também
        // (handle fechado sem ninguém atribuído vivo).

        // `proxy_guard` dropa aqui — encerra o listener Tokio
        // e remove o `proxy.port`. **Não dropar antes** (drop
        // prematuro = filho perde a saída de rede mid-exec).

        // Audit mínimo: serializa o output como `result_json`.
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
                format!("python exit code {}: {}", raw.exit_code, raw.stderr),
            );
        }
        ToolResult::ok(
            tool_id,
            output_json(&raw),
            vec![runtime.executable().to_path_buf()],
        )
    }
}
