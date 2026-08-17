//! `FilesExecShellTool` — executa um comando de uma lista fechada
//! sob sandbox (Etapa 2b da Fase 8, ADR-0044; originalmente Etapa 7
//! da Fase 7, ADR-0031 + ADR-0034 D3).
//!
//! Ver spec `docs/architecture/exec-tools-specification.md`
//! §"`FilesExecShellTool`".
//!
//! ## O desenho, em uma frase
//!
//! **O `cmd.exe` não resolve programa.** Quem resolve é o
//! `frederico_security::exec_patterns::plan_command`, que devolve
//! ou um builtin (executado via `cmd.exe /d /v:off /c`) ou um
//! executável do `System32` **por caminho absoluto** (spawn direto,
//! sem shell nenhum no meio). Argumentos viajam como `argv`, nunca
//! como texto pra um shell reinterpretar.
//!
//! A v1 fazia o oposto — entregava o command string inteiro pro
//! `cmd.exe /c` — e o ADR-0037 mediu o custo: `echo x & <qualquer
//! coisa>` executava as duas metades, porque o `cmd.exe` trata `&`
//! como separador e a validação olhava só o primeiro token. Duas
//! consequências vieram junto, e as duas estão fechadas agora:
//!
//! 1. **Contrabando por separador** — fechado recusando
//!    metacaracteres antes do spawn (`SHELL_METACHARACTERS`).
//! 2. **Sequestro de binário pelo diretório corrente** — o
//!    `cmd.exe` procura no CWD antes do `PATH`, e o CWD do filho é
//!    o workspace, onde o `files.write` escreve. Plantar `find.bat`
//!    lá e pedir `find alfa arquivo.txt` executava o arquivo
//!    plantado (medido, ADR-0044). Fechado porque o caminho é
//!    absoluto e não há busca.
//!
//! ## Diferenças pra `exec.python`/`exec.node`
//!
//! - **Sem `RuntimeRegistry`**: os programas são do próprio Windows
//!   (não runtimes portáteis pinned por SHA-256). Resolvidos sob
//!   `%SystemRoot%\System32\` — `SystemRoot` é
//!   `EnvAllowlist::REQUIRED` (Etapa 6+1), então o path é estável.
//! - **`risk_level: Critical`** (não `High`) — `Critical` é o único
//!   nível que força `ApprovalRequest.mandatory = true` mesmo sem
//!   UI de escopo (ver `validate.rs::with_mandatory_for_risk`).
//! - **Validação sempre ativa**, não gateada por
//!   `PermissionSet::terminal`: o `ToolContext` que chega no
//!   `execute()` não carrega o `PermissionSet` da run (é
//!   responsabilidade do `validate_tool_call` no `RunExecutor`, que
//!   ainda não lê `permissions.terminal` — ver
//!   `crates/tool-registry/src/validate.rs` Passo 5). Validar
//!   sempre é consistente com o teto do projeto:
//!   `PermissionSet::allow_all()` já fixa
//!   `terminal: TerminalMode::Allowlist` — não existe variante "sem
//!   restrição".
//!
//! ## O que continua não protegido
//!
//! Os programas da lista leem arquivos, e o filho roda com
//! integridade baixa — que **restringe escrita, não leitura**. Um
//! `type C:\caminho\fora\do\workspace.txt` lê o arquivo. É a lacuna
//! de "read-up" que o `security-threat-model.md` já nomeia, comum a
//! `exec.python` e `exec.node` (um script Python faz o mesmo
//! `open()`), e fechá-la é trabalho de filtro no nível de processo,
//! não desta ferramenta. Fixada em teste
//! (`e2e_exec_shell_hardened.rs::documented_limit_child_can_read_outside_workspace`)
//! e declarada no `SECURITY.md`.

use std::time::Duration;

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_security::exec_patterns::{plan_command, CommandRejection, ShellProgram};
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
    /// De volta ao catálogo pelo ADR-0044 (Etapa 2b da Fase 8),
    /// depois de ter saído pelo ADR-0037. O que mudou entre os
    /// dois: o `cmd.exe` deixou de resolver o programa. Ver o doc
    /// do módulo.
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
                    "description": "Comando de inspecao do workspace. Programas aceitos: cd, dir, echo, type, ver, vol (builtins do cmd.exe) e fc, findstr, more, sort, tree (System32). NAO e um shell: sem pipe, sem redirecionamento (`>`), sem encadeamento (`&`, `&&`), sem expansao de variavel (`%VAR%`) — comando com qualquer um desses caracteres e recusado. Aspas duplas agrupam um argumento com espacos."
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
                "Executa um comando de inspecao do workspace sob sandbox, a partir de uma lista fechada de 11 programas read-only (cd, dir, echo, type, ver, vol, fc, findstr, more, sort, tree). Nao interpreta sintaxe de shell: pipe, redirecionamento, encadeamento e expansao de variavel sao recusados antes do spawn. O programa e resolvido por caminho absoluto, nunca por busca no PATH ou no diretorio corrente. Sempre requer aprovacao por invocacao (ADR-0034 D3) — nunca reusa aprovacao anterior.",
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

    /// Resolve `%SystemRoot%\System32`. `SystemRoot` é
    /// `EnvAllowlist::REQUIRED` (Etapa 6+1) — se ausente do
    /// ambiente do processo pai, é configuração de SO quebrada,
    /// não um caso a degradar silenciosamente.
    fn system32_dir() -> Result<std::path::PathBuf, ExecError> {
        let system_root = std::env::var("SystemRoot").map_err(|_| {
            ExecError::SpawnFailed(
                "variavel de ambiente SystemRoot ausente — nao consigo resolver System32"
                    .to_string(),
            )
        })?;
        Ok(std::path::Path::new(&system_root).join("System32"))
    }

    /// Do command string cru ao par `(programa, argv)` pronto pro
    /// `SandboxConfig`. **É aqui que o `cmd.exe` deixa de ser
    /// resolvedor**: quem escolhe o binário é a lista fechada do
    /// `exec_patterns`, e o caminho devolvido é sempre absoluto.
    ///
    /// - **Builtin** (`dir`, `type`, …) não é arquivo: roda dentro
    ///   do `cmd.exe`, invocado como
    ///   `cmd.exe /d /v:off /c <nome> <args…>`. O `/d` pula o
    ///   `AutoRun` do registro (`HKCU\…\Command Processor\AutoRun`
    ///   é execução arbitrária a cada `cmd /c`); o `/v:off` desliga
    ///   a expansão atrasada, que também é ligável pelo registro.
    ///   Os argumentos continuam sendo `argv` — o `build_cmdline`
    ///   do `SecurityJailResolver` é quem os cita, não nós.
    /// - **`System32`** (`findstr`, `sort`, …) é spawn **direto** do
    ///   caminho absoluto. Sem `cmd.exe`, sem busca por `PATH`, sem
    ///   busca pelo diretório corrente — os metacaracteres já foram
    ///   recusados, e mesmo que escapassem não teriam quem os
    ///   interpretasse.
    ///
    /// Recusa é sempre pré-spawn: nenhum Job Object ou processo é
    /// criado para um comando recusado.
    fn plan(&self, args: &Value) -> Result<(std::path::PathBuf, Vec<String>), ExecError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::InvalidArgs("tool_call precisa de `command`".to_string()))?;

        let planned = plan_command(command).map_err(|rejection| match rejection {
            CommandRejection::Denylisted(pattern) => ExecError::CommandDenied(pattern.to_string()),
            CommandRejection::Metacharacter(c) => ExecError::CommandHasShellSyntax(c),
            CommandRejection::NotAllowed(token) => ExecError::CommandNotInAllowlist(token),
            other @ (CommandRejection::Empty | CommandRejection::UnbalancedQuote) => {
                ExecError::InvalidArgs(other.to_string())
            }
        })?;

        let system32 = Self::system32_dir()?;
        match planned.program {
            ShellProgram::Builtin { name } => {
                let mut argv = vec![
                    "/d".to_string(),
                    "/v:off".to_string(),
                    "/c".to_string(),
                    name.to_string(),
                ];
                argv.extend(planned.args);
                Ok((system32.join("cmd.exe"), argv))
            }
            ShellProgram::System32 { name, file_name } => {
                let program = system32.join(file_name);
                if !program.exists() {
                    // Degradação declarada, não silenciosa: a lista
                    // é fechada, então isto é instalação do Windows
                    // sem o componente — não entrada do usuário.
                    return Err(ExecError::SpawnFailed(format!(
                        "'{name}' esta na allowlist mas {} nao existe nesta instalacao do Windows",
                        program.display()
                    )));
                }
                Ok((program, planned.args))
            }
        }
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

    /// **O programa varia nesta ferramenta**, diferente de
    /// `exec.python`/`exec.node`, onde o binário é fixo e só os
    /// args mudam. O trait só devolve args, então quem manda aqui é
    /// [`FilesExecShellTool::plan`], que devolve o par completo; este
    /// método existe para satisfazer o trait e devolve a metade dos
    /// argumentos. Usar só ele perderia o programa resolvido — por
    /// isso o `execute()` chama `plan` direto.
    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError> {
        self.plan(args).map(|(_program, argv)| argv)
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

        // Toda a validacao (denylist, metacaracteres, tokenizacao,
        // resolucao do programa) acontece aqui, ANTES do spawn —
        // nenhum Job Object/processo e criado pra um comando ja
        // recusado (defesa em profundidade + eficiencia, mesmo
        // racional do `check_permission` de python/node).
        let (program, tool_args) = match self.plan(arguments) {
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
            SandboxConfig::new(program.clone(), tool_args, ctx.jail.root().to_path_buf());
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
        ToolResult::ok(tool_id, output_json(&raw), vec![program])
    }
}
