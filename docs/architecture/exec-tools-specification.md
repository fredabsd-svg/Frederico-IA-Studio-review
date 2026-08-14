<!--
Estado: implementado
Verificado contra o código em: 2026-08-14
Fase correspondente: 7
-->

> Spec criado na Etapa 1 da Fase 7 (PR de planejamento, 2026-08-08), atualizado na **Etapa 4** (2026-08-08) e na **Etapa 7** (2026-08-14, PR `fase-7-etapa-7-exec-shell`). Especificação das 3 ferramentas `exec.*` — todas ✅ implementadas: `exec.python`, `exec.node` (Etapa 4) e `exec.shell` (Etapa 7 — a numeração real das etapas divergiu do planejamento original da Etapa 1; a rede do sandbox fechou primeiro como Etapa 6/6+1, `exec.shell` fechou depois como Etapa 7; ver nota em `windows-sandbox-design.md`). **Etapa 4 fechou**: `FilesExecPythonTool` + `FilesExecNodeTool` in-process em `crates/tool-registry/src/exec/`; integração com `SecurityJailResolver` (Etapa 2) + `RuntimeRegistry` (Etapa 3) via `Arc<...>` na `FilesExecToolBase`; **per-invocation Job Object**; `wait_with_timeout` dentro do `collect_output`; aprovação obrigatória via `validate_tool_call` Passo 9. **Etapa 7 fechou**: `FilesExecShellTool` (`shell.rs`, ~250L) + `frederico_security::exec_patterns` (denylist + allowlist, `~180L`). Diferenças do plano original documentadas na seção "`FilesExecShellTool`" abaixo — a mais relevante é que a denylist/allowlist são aplicadas **incondicionalmente** (não gateadas por `PermissionSet::terminal`), porque o `ToolContext` que chega no `Tool::execute` não carrega o `PermissionSet` da run (isso é responsabilidade do `validate_tool_call` no `RunExecutor`, que hoje só implementa o Passo 5 para `file_read` — os demais eixos, incluindo `terminal`/`python`/`node`, são bumpados no `PermissionSet` mas ainda não lidos por nenhum gate em runtime; ver `crates/tool-registry/src/validate.rs`).

# Especificação das Ferramentas `exec.*`

> **Contexto:** a Fase 7 introduz 3 ferramentas de execução no `ToolRegistry`: `exec.python` (Etapa 4), `exec.node` (Etapa 4), e `exec.shell` (Etapa 7). Cada uma invoca um binário (Python, Node, ou shell do Windows) sob o `SecurityJailResolver` (ADR-0031 + ADR-0036), com aprovação por escopo (ADR-0034), e audit (R1 do threat model). A especificação segue o contrato do `ToolManifest` (Fase 3 Etapa 2) e o modelo de `PermissionSet` (Fase 3 Etapa 3).

## Visão geral

As 3 ferramentas compartilham uma **infraestrutura comum** (`FilesExecTool` trait privado) e divergem na **política de aprovação**: `exec.shell` é sempre `OneExecution` (`risk_level: Critical`, que força `ApprovalRequest.mandatory = true` — ver `validate.rs::with_mandatory_for_risk`); `exec.python`/`exec.node` também exigem aprovação a cada invocação hoje (`risk_level: High`), porque o mecanismo de **cache de aprovação por escopo** (`OneTurn`+) descrito no ADR-0034 D2 **ainda não existe em código** — `RunExecutor::handle_tool_call` sempre chama `validate_tool_call` com `approval: None` (nenhuma decisão anterior é reusada, pra nenhuma tool). O campo `ApprovalScope` no `tool-registry` (`Once`/`Run`/`Project`, em `approval.rs`) é o modelo de dados pro futuro cache; o cache em si é trabalho de Fase 8.

| Ferramenta | Runtime | Etapa | Approval (hoje) | Sandbox | Network default |
|---|---|---|---|---|---|
| `exec.python` | Python portátil (Etapa 3) | 4 | Toda invocação (cache de escopo é roadmap) | Job Object + Restricted Token + env zeroed | deny-by-default (ADR-0033), allowlist via perfil TOML (Etapa 7) |
| `exec.node` | Node portátil (Etapa 3) | 4 | Toda invocação (cache de escopo é roadmap) | Job Object + Restricted Token + env zeroed | deny-by-default (ADR-0033), allowlist via perfil TOML (Etapa 7) |
| `exec.shell` | `cmd.exe` (built-in Windows) | 7 | Toda invocação, sem exceção (`risk_level: Critical`, ADR-0034 D3) | Job Object + Restricted Token + env zeroed + Denylist/Allowlist de comandos (sempre ativas) | deny-by-default (ADR-0033), allowlist via perfil TOML (Etapa 7) |

As 3 vivem em `crates/tool-registry/src/exec/`:
- `mod.rs` — `FilesExecTool` trait + `FilesExecToolBase` (camada comum de audit, cancelamento, output collection).
- `python.rs` — `FilesExecPythonTool` (Etapa 4).
- `node.rs` — `FilesExecNodeTool` (Etapa 4).
- `shell.rs` — `FilesExecShellTool` (Etapa 7).

Tamanho estimado: ~400 linhas cada.

## Decisões tomadas

- **Default deny** (D1 do ADR-0034): as 3 ferramentas nascem com `PermissionSet::python = None` / `node = None` / `terminal = None`. Usuário liga explicitamente.
- **Aprovação por escopo** (D2 do ADR-0034): `OneExecution` / `OneTurn` / `OneSession` / `OneProject` / `Forever`. Default da UI: `OneTurn` para python/node, `OneExecution` para shell.
- **Comando exato é exibido** (D5 do ADR-0034): a UI mostra a string literal que vai para `CreateProcess`, com botão "Aprovar" desabilitado até rolar até o fim.
- **Sandbox = Job Object + Restricted Token + env zeroed** (D1 do ADR-0031): as 3 ferramentas spawnam via `SecurityJailResolver::spawn()`. O `Jail` é a barreira primária de path safety (Fase 6 Etapa 5.X) e é aplicado no `workdir` antes do spawn.
- **Cancelamento cascateia** (Fase 3 Etapa 4.x): o `CancellationToken` do `Run` é passado pro `SandboxedProcess`, que cascateia pro PID via `TerminateProcess` (que dispara `KILL_ON_JOB_CLOSE` e mata a árvore).
- **Output limitado**: stdout + stderr coletados em chunks de até 64 KB, com teto total de 10 MB por invocação. Excedeu o teto → trunca com aviso `OUTPUT_TRUNCATED` no audit.
- **Wall-clock default 60s**, configurável por tool (`max_wall_clock_ms` no tool_call args). Excedeu → mata a árvore (mesmo mecanismo do cancelamento).
- **Network deny-by-default** (ADR-0033): o `NetworkAllowlist` da invocação começa vazio. A Etapa 4 da Fase 7 não **proíbe** o tool_call de pedir rede (porque `pip install` precisa), mas a primeira tentativa de acesso à rede **bloqueia** e abre modal de "permitir host X" (Etapa 7 da Fase 7 implementa a UI).
- **Audit completo** (D6 do ADR-0034 + R1 do threat model): toda invocação registra `kind`, `tool`, `args`, `runtime`, `workdir`, `exit_code`, `duration_ms`, `output_truncated`, `bytes_stdout`, `bytes_stderr`, `approval_scope`, `network_attempts` no `DbAuditSink`.

## Contrato previsto

### `FilesExecTool` (trait privado, base comum)

```rust
#[async_trait]
trait FilesExecTool: Tool {
    /// Resolve o `Runtime` (Python, Node) ou aponta pro `cmd.exe` (shell).
    fn resolve_runtime(&self, call: &ToolCall) -> Result<RuntimeSpec, ResolveError>;
    
    /// Monta os args do `CreateProcess` a partir do `ToolCall`.
    /// Implementação diferente por tool (Python: `-c` ou `-m`; Node: `-e` ou path; Shell: command string).
    fn build_args(&self, call: &ToolCall, runtime: &RuntimeSpec) -> Result<Vec<String>, BuildArgsError>;
    
    /// Política de approval default por escopo. Diferente por tool.
    fn default_approval_scope(&self) -> ApprovalScope;
    
    /// Pós-processa a saída (trunca, detecta erros, formatta para o modelo).
    fn postprocess_output(&self, raw: RawOutput) -> Result<ToolOutput, PostprocessError>;
}
```

### `FilesExecPythonTool`

```rust
pub struct FilesExecPythonTool {
    base: FilesExecToolBase,
    runtimes: Arc<RuntimeRegistry>,
    jail_resolver: Arc<SecurityJailResolver>,
    permission_checker: Arc<PermissionChecker>,
}

#[async_trait]
impl Tool for FilesExecPythonTool {
    fn id(&self) -> ToolId { ToolId("exec.python") }
    fn namespace(&self) -> &str { "exec" }
    fn version(&self) -> SemVer { SemVer::new(0, 1, 0) }
    fn display_name(&self) -> &str { "Executar Python" }
    fn description(&self) -> &str {
        "Executa código Python sob sandbox. Args: code (string, obrigatório) OU path (relativo ao workdir) OU module (string, -m). Opcional: stdin (string), runtime (default 'python-3.12.4'). Wall-clock default 60s; max 600s."
    }
    fn input_schema(&self) -> JsonSchema { json_schema!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "description": "Código Python a executar via python -c" },
            "path": { "type": "string", "description": "Caminho para script .py (relativo ao workdir)" },
            "module": { "type": "string", "description": "Módulo a executar via python -m" },
            "stdin": { "type": "string", "description": "Input a passar pro stdin do processo" },
            "runtime": { "type": "string", "description": "ID do runtime (default: python-3.12.4)" },
            "max_wall_clock_ms": { "type": "integer", "minimum": 1000, "maximum": 600000 }
        },
        "oneOf": [
            { "required": ["code"] },
            { "required": ["path"] },
            { "required": ["module"] }
        ]
    }) }
    fn output_schema(&self) -> JsonSchema { json_schema!({
        "type": "object",
        "properties": {
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "exit_code": { "type": "integer" },
            "duration_ms": { "type": "integer" },
            "truncated": { "type": "boolean" }
        }
    }) }
    fn risk_level(&self) -> RiskLevel { RiskLevel::High }
    fn requires_user_approval(&self) -> bool { true }  // ADR-0034 D1
    fn cancellable(&self) -> bool { true }
    fn timeout_ms(&self) -> u32 { 60_000 }
    fn category(&self) -> ToolCategory { ToolCategory::CodeExecution }
    fn capabilities(&self) -> Vec<String> { vec!["exec.python".to_string()] }
    
    async fn execute(&self, call: ToolCall, ctx: &ValidationContext) -> Result<ToolResult, ToolError> {
        // 1. Check permission (ADR-0034 D1: default deny)
        if ctx.permissions.python == RuntimePermission::None {
            return Err(ToolError::PermissionDenied("python"));
        }
        
        // 2. Check approval (ADR-0034 D2)
        let approval = self.permission_checker.check_approval(&call, ctx)?;
        
        // 3. Resolve runtime
        let runtime = self.runtimes.get(&RuntimeId::from_model_str(call.args.get("runtime").map(String::as_str).unwrap_or("python-3.12.4"))?)?
            .ok_or(ToolError::UnknownRuntime(...))?;
        
        // 4. Build args
        let args = self.build_args(&call, &runtime.spec())?;
        // Se call.args["code"] = "print(2+2)", args = ["-c", "print(2+2)"]
        // Se call.args["path"] = "scripts/hello.py", args = ["scripts/hello.py"]
        // Se call.args["module"] = "pytest", args = ["-m", "pytest"]
        
        // 5. Build SandboxConfig (ADR-0031)
        let workdir = self.jail_resolver.file_system_jail.resolve_allowing_nonexistent(call.workdir.as_ref().unwrap_or(&".".to_string()))?;
        let config = SandboxConfig {
            tool: self.id().clone(),
            permissions: ctx.permissions.clone(),
            workdir: workdir.clone(),
            args,
            wall_clock: Duration::from_millis(call.args.get("max_wall_clock_ms").and_then(|v| v.as_u64()).unwrap_or(60_000)),
            env: runtime.env_vars().to_vec(),
            stdin: call.args.get("stdin").and_then(|s| s.as_str().map(String::as_bytes)).map(|b| b.to_vec()),
            ..Default::default()
        };
        
        // 6. Spawn under sandbox
        let mut process = self.jail_resolver.spawn(config)?;
        
        // 7. Collect output (with cancellation)
        let output = tokio::select! {
            output = collect_output(&mut process, MAX_OUTPUT_BYTES) => output,
            _ = ctx.cancel_token.cancelled() => {
                process.kill().await?;
                return Err(ToolError::Cancelled);
            }
        };
        
        // 8. Postprocess
        let result = self.postprocess_output(output)?;
        
        // 9. Audit
        self.base.audit_sink.record(AuditEntry {
            kind: "exec_python",
            tool: self.id().clone(),
            runtime: runtime.id().to_string(),
            args: process.invoked_args.clone(),  // comando exato (D5 do ADR-0034)
            workdir: workdir.to_string(),
            exit_code: result.exit_code,
            duration_ms: result.duration.as_millis() as u64,
            bytes_stdout: result.bytes_stdout,
            bytes_stderr: result.bytes_stderr,
            truncated: result.truncated,
            approval_scope: approval.scope,
            approved_by: approval.approved_by,
            ...
        })?;
        
        Ok(ToolResult { output: result.into_json(), .. })
    }
}
```

### `FilesExecNodeTool` (mesma forma, `node` no lugar de `python`)

Diferenças:
- `runtime.id() = "node-20.16.0"` por default.
- Args: `node <script.js>` ou `node -e "<code>"` ou `node -m <module>`.
- `category` = `ToolCategory::CodeExecution`.
- `description` menciona "Node.js" explicitamente.

### `FilesExecShellTool` (Etapa 7 — fechada, diferenças do plano original nomeadas abaixo)

Implementado em `crates/tool-registry/src/exec/shell.rs` + `frederico_security::exec_patterns`:

- **Sempre `OneExecution`**: `default_approval_scope()` retorna `ApprovalScope::OneExecution`. **Diferença do plano:** não existe hoje um `permission_checker.check_approval` que "rejeita tentativa de aumentar o escopo" — porque não existe *nenhum* mecanismo de escopo de aprovação persistente ainda (ver "Visão geral" acima). Na prática, `exec.shell` já se comporta como `OneExecution` simplesmente porque **toda** tool que exige aprovação hoje pede aprovação nova a cada chamada — a garantia "nunca reusa aprovação anterior" é automática, não uma checagem ativa.
- **Denylist de comandos destrutivos**: `build_args` chama `frederico_security::exec_patterns::denylist_hit` (substring case-insensitive contra o command string inteiro) **antes** do spawn. Lista real (`SHELL_DENYLIST`): `rm -rf`, `del /f /s /q`, `remove-item -recurse -force`, `format`, `diskpart`, `bcdedit`, `reg delete`, `net user`, `net localgroup`, `cipher /w`, `sfc /scannow`. Match → `Err(ExecError::CommandDenied(pattern))` antes de qualquer `SecurityJailResolver::spawn`. **Limitação documentada** (não escondida — fixada em teste `shell_denylist_hit_documents_split_flag_bypass`): o match é substring literal, não um parser de shell; `rm -r -f` (flags separadas) não casa `rm -rf`. A barreira real contra dano ao host continua sendo o Jail (Mandatory Label\Low) + Restricted Token.
- **Allowlist sempre ativa** (não opcional, e não é `TerminalMode::Allowlist(Vec<String>)` — esse enum é **flat**, sem payload, no código real; `permission.rs:82-90`): `frederico_security::exec_patterns::is_allowed` checa o **primeiro token** contra `SHELL_ALLOWLIST_DEFAULT` (`ls`, `cat`, `head`, `tail`, `grep`, `find`, `wc`, `pwd`, `echo`). Sem match → `Err(ExecError::CommandNotInAllowlist(token))`. **`git status`/`git log`/`git diff`/`git show`** (2 tokens) continuam pendência nomeada do ADR-0034 §"Pendências" — a allowlist v1 só reconhece o primeiro token, não pares.
- **Por que a allowlist é incondicional, não gateada por `PermissionSet::terminal`:** o `ToolContext` (`crates/tool-registry/src/tools/mod.rs`) não carrega `PermissionSet` — só `conversation_id`/`run_id`/`message_id`/`jail`. Ler `permissions.terminal` exigiria estender o `ToolContext` ou o `RunExecutor` (fora do escopo da Etapa 7). Aplicar a allowlist sempre é consistente com o teto do projeto: `PermissionSet::allow_all()` já fixa `terminal: TerminalMode::Allowlist` — não existe variante "sem restrição" no enum.
- **Comando passado como string única**: `cmd.exe /c "<command>"` (sem shell intermediário, o que evita `cmd injection` via `&&`).
- **`risk_level` = `Critical`**: shell é a ferramenta mais permissiva do registry.
- **Sem `RuntimeRegistry`**: `cmd.exe` é resolvido via `%SystemRoot%\System32\cmd.exe` (não um runtime portátil pinned — `resolve_runtime_id` retorna um valor informativo `"system-cmd"`, não consultado).

## `ToolManifest` (registro no `ToolRegistry`)

Cada uma das 3 ferramentas é registrada com o `ToolManifest` da Fase 3 Etapa 2:

```rust
ToolManifest {
    id: ToolId("exec.python"),
    namespace: "exec".to_string(),
    version: SemVer::new(0, 1, 0),
    display_name: "Executar Python".to_string(),
    description: "...".to_string(),
    input_schema: ...,
    output_schema: ...,
    category: ToolCategory::CodeExecution,
    capabilities: vec!["exec.python".to_string()],
    risk_level: RiskLevel::High,
    requires_network: false,  // depende do que o script faz; o tool em si não requer
    requires_file_read: false,
    requires_file_write: false,
    requires_process_execution: true,
    requires_user_approval: true,
    supported_platforms: vec![Platform::Windows],
    supported_provider_modes: vec![ProviderMode::NativeTools],
    timeout_ms: 60_000,
    cancellable: true,
    availability: Availability::Disabled,  // até o usuário ligar (ADR-0034 D1)
    health_message: Some("Disponível. Requer aprovação por invocação.".to_string()),
    worker_id: None,  // executa no app principal via SecurityJailResolver
}
```

## Aprovação por escopo (ADR-0034 D2)

A Etapa 4 implementa o `ApprovalModal` (UI) + `PermissionChecker::check_approval` (backend). A interação é:

1. Modelo emite tool_call `exec.python` com `code: "import os; os.listdir()"`.
2. `RunExecutor` valida via `validate_tool_call` (10 passos da Fase 3 Etapa 2).
3. `FilesExecPythonTool::execute` é chamado.
4. `permission_checker.check_approval`:
   - Olha o `PermissionSet` da execução + do `assistant` + do `project` + do `user` + do pai (subagente).
   - Se o `python` está `None` em **qualquer** camada, retorna `Err(PermissionDenied("python"))` — usuário precisa ligar o toggle antes.
   - Se o `python` está `Sandboxed` e o tool_call tem um `code` que casa um padrão "perigoso" (regex simples: `os.system`, `subprocess.run`, `shutil.rmtree`, `__import__`, `eval`, `exec`, `open("C:\\...")`), abre modal com diff do comando + radio de escopo.
   - Se o `python` está `Sandboxed` e o `code` é "seguro" (não casa padrão), aprovação **automática** com escopo `OneExecution` (default conservador).
   - Usuário escolhe escopo: `OneExecution` / `OneTurn` / `OneSession` / `OneProject` / `Forever`.
   - Modal mostra comando exato (D5 do ADR-0034): "vai executar: `python -c \"import os; os.listdir()\"`" com syntax highlight.
5. Decisão é gravada no `DbAuditSink` (`kind: 'approval_granted'`, `scope`, `actor: 'user'`).
6. `execute` continua com o spawn.

**Padrões "perigoso"** (regex) ficam em `crates/security/src/exec_patterns.rs` (~200 linhas, versionado, editável pelo usuário avançado). Lista inicial conservadora: `os.system`, `subprocess.run`, `subprocess.Popen`, `shutil.rmtree`, `__import__`, `eval(`, `exec(`, `open(["']C:\\`, `open(["']/etc/`, `compile(`, `importlib`. A Etapa 4 implementa; a Fase 8 pode estender.

**Aprovação automática** (sem modal) **só** se (a) o `code` não casa padrão perigoso **e** (b) o `python` está `Sandboxed` (não `Unrestricted`). `Unrestricted` **sempre** pede modal — é o escape hatch, exige opt-in consciente.

## Cancelamento (Fase 3 Etapa 4.x + D3 do ADR-0036)

O `CancellationToken` do `Run` é passado pro `SandboxedProcess::cancel_token` (D6 do ADR-0036). Quando o `Run` é cancelado (botão "Parar" do usuário, timeout, budget estourado):

1. `ctx.cancel_token.cancelled()` dispara.
2. O `tokio::select!` no `execute` cancela o `collect_output`.
3. `process.kill().await?` chama `TerminateProcess` no PID.
4. O `Job Object` (`JobHandle::close`) fecha o handle → `KILL_ON_JOB_CLOSE` derruba a **árvore inteira** (filho + netos + processos em `pip install` etc.).
5. `DbAuditSink` registra `kind: 'exec_cancelled'`, `pid`, `tree_size_at_kill` (quantos processos foram mortos).

**Teste de regressão** (regra do user): `crates/e2e/tests/e2e_exec_cancellation.rs::cancel_kills_grandchildren` — spawna `python -m pip install numpy` (que cria netos: pip → python → compilador C), cancela no meio, afirma que **todos** os processos estão mortos em < 2s. Sem `KILL_ON_JOB_CLOSE`, o neto (compilador) sobrevive.

## Network deny-by-default (ADR-0033) — fechado

**Histórico real (diferente do plano original desta seção):** a Etapa 4 não tinha proxy nem firewall de processo — a rede do child era simplesmente a do host, sem filtro nenhum (lacuna nomeada em `SECURITY.md`). O que fechou essa lacuna foi um **proxy HTTP/CONNECT local** (`127.0.0.1:<porta efêmera>`), não um firewall no nível de processo Windows (WFP) — implementado na Etapa 6 (mecanismo, `frederico_security::network`) e ligado no caminho real de `exec.python`/`exec.node`/`exec.shell` na Etapa 6+1/7 (`FilesExecToolBase::start_network_proxy`).

**Como funciona hoje:** cada invocação de `exec.python`/`exec.node`/`exec.shell` sobe o proxy, injeta `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` no env filtrado do child. Sem host na allowlist, toda request recebe `502 Bad Gateway` — `pip install` falha com esse erro (não `WSAEACCES`), o que é honesto sobre o mecanismo real (proxy, não firewall).

**Allowlist carregada do perfil TOML** (Etapa 7, `PermissionSet.network_allowlist: Vec<String>`, campo novo em `permission.rs` + `permission_loader.rs`): a casca resolve o perfil do usuário ∩ projeto (`~/.config/frederico/profiles/default.toml` + `./.frederico/project.toml`) antes de construir o `ExecDeps`, e usa o `network_allowlist` efetivo (interseção fail-closed dos dois layers) pra montar o `NetworkAllowlist` do proxy. **Limitação nomeada:** só user+project — o layer de assistant não é aplicado aqui porque exige um `assistant_id` que não existe no momento do boot do processo (o `ExecDeps` é construído uma vez, process-wide, não por conversa). Refinar por assistant é Fase 8, quando o proxy virar per-run.

Ver `SECURITY.md` §"Rede" para as lacunas conhecidas do mecanismo (bypass por socket raw, HTTP/3, janela de DNS leakage).

## Output collection e limites

```rust
const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;  // 10 MB
const OUTPUT_CHUNK_SIZE: usize = 64 * 1024;          // 64 KB

async fn collect_output(process: &mut SandboxedProcess, max_bytes: usize) -> RawOutput {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut truncated = false;
    
    loop {
        tokio::select! {
            chunk = process.stdout.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        if stdout.len() + bytes.len() > max_bytes {
                            stdout.truncate(max_bytes);
                            truncated = true;
                            break;
                        }
                        stdout.extend_from_slice(&bytes);
                    }
                    Some(Err(e)) => {
                        stderr.extend_from_slice(format!("\n[stream error: {}]", e).as_bytes());
                        break;
                    }
                    None => break,  // stdout fechou
                }
            }
            chunk = process.stderr.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        if stderr.len() + bytes.len() > max_bytes {
                            stderr.truncate(max_bytes);
                            truncated = true;
                            // continua lendo stdout até fechar
                        }
                        stderr.extend_from_slice(&bytes);
                    }
                    _ => {}  // ignore stderr errors durante leitura de stdout
                }
            }
            exit = process.wait() => {
                return RawOutput { exit_code: exit?, stdout, stderr, truncated, ... };
            }
        }
    }
    
    let exit_code = process.wait().await?;
    RawOutput { exit_code, stdout, stderr, truncated, ... }
}
```

O limite de 10 MB é por invocação, não por tool_call total (o tool_call pode ter múltiplas invocações, cada uma com seu teto). O `truncated: true` no output avisa o modelo, que pode decidir split em invocações menores.

## Não-objetivos

- **REPL interativo** (enviar comando, ler output, enviar outro). A Etapa 4 implementa apenas "1 invocação = 1 execução completa, retorna tudo". REPL é roadmap.
- **Streaming de output para o modelo durante a execução** (modelo vê output parcial antes do exit). A Etapa 4 coleta tudo e devolve no fim. Streaming é roadmap de Fase 8+ (com UI de "running..." + progresso).
- **Background execution** (filho continua rodando depois do tool_call retornar). A Etapa 4 bloqueia até o exit. Background é roadmap.
- **`exec.javascript`** (Node + browser-like). Fora de escopo.
- **`exec.bash`** (WSL/git-bash). Sem WSL na v1; git-bash é Fase 8 com Git.
- **`exec.powershell`**. Considerado, mas o command string via `powershell -Command` tem superfície de injection maior que `cmd /c`. A Etapa 6 entra com `cmd.exe`; PowerShell é roadmap.
- **Sandbox de GPU** (defeito clássico para ML). Roadmap.

## Testes de regressão obrigatórios (regra do user: "teste de negação")

A Etapa 4 da Fase 7 entrega:

| Teste | O que prova |
|---|---|
| `crates/e2e/tests/e2e_exec_python_under_sandbox.rs::child_cannot_write_outside_workspace` | `python -c "open('C:/Windows/System32/drivers/etc/hosts', 'w').write('pwned')"` é bloqueado pelo Jail antes do Python rodar. |
| `crates/e2e/tests/e2e_exec_python_under_sandbox.rs::child_env_does_not_inherit_api_keys` | Fecha I1 do threat model — o Python filho **não** vê `OPENAI_API_KEY` do parent. |
| `crates/e2e/tests/e2e_exec_python_under_sandbox.rs::grandchild_survives_parent_kill9` | Mesmo teste do ADR-0036 D6, mas via `exec.python` (que cria netos via `subprocess`). |
| `crates/e2e/tests/e2e_approval_display.rs::approved_command_matches_actual_invocation` | Fecha D5 do ADR-0034 — UI mostra `python -c "print(2+2)"`, executor roda `python -c "print(2+2)"`, byte-a-byte. |
| `crates/e2e/tests/e2e_exec_cancellation.rs::cancel_kills_grandchildren` | Cancelar no meio de `pip install` mata o compilador C que o `pip` invocou. |
| `crates/e2e/tests/e2e_exec_shell_denylist.rs::rm_rf_is_rejected_by_denylist` (Etapa 7 ✅) | `exec.shell` com `rm -rf /` é rejeitado pela Denylist antes do spawn (+ `del_f_s_q_is_rejected_by_denylist`). |
| `crates/e2e/tests/e2e_exec_shell_allowlist.rs::echo_allowlisted_command_executes` + `curl_not_in_allowlist_is_blocked` (Etapa 7 ✅) | `echo` (allowlist) executa de ponta a ponta; `curl` (fora da allowlist) é recusado antes do spawn. **Trocado `ls` por `echo`** no teste real — `ls` não é builtin do `cmd.exe` (só existe via Git for Windows/WSL, não garantido no CI); `echo` prova o mesmo gate sem depender de PATH externo. |

## Trade-offs explícitos

| Decisão | Custo | Ganho | Por quê |
|---|---|---|---|
| Default `OneExecution` para shell | UX: usuário aprova toda invocação | Segurança: a fronteira entre `ls` e `rm -rf` é invisível | Denylist+Allowlist (Etapa 7) reduzem o risco por trás da aprovação, mas não eliminam a aprovação em si — nenhum mecanismo de cache de escopo existe ainda (roadmap Fase 8) |
| Network deny-by-default | `pip install`/`npm install` falham sem allowlist configurada | Sem rede, sem exfiltração | Proxy local (Etapa 6/6+1) + allowlist via perfil TOML (Etapa 7): usuário configura hosts explícitos, resto continua bloqueado |
| Output 10 MB | Memória do app | Sem DoS por output gigante | Teto negociável por tool_call |
| Wall-clock 60s default | Scripts longos quebram | Sem DoS por loop infinito | Configurável por `max_wall_clock_ms` |
| Cancelamento cascateia | Overhead de Job Object | Tree-kill garantido (mesmo em `kill -9`) | Mesma decisão do ADR-0036 |
| Sem streaming de output | Modelo não vê progresso | Simplicidade, audit confiável | Streaming é roadmap |
| `cmd.exe` em vez de PowerShell | Superfície de command string | PowerShell tem injection maior | PowerShell é roadmap |

## Decisões (fechadas na Etapa 4/7 — mantidas aqui como histórico)

- **Versão inicial do Python e Node** pinada em `runtimes-architecture.md` (Python 3.12.4, Node 20.16.0).
- **Padrões "perigoso"** (substring case-insensitive, não regex) implementados em `crates/security/src/exec_patterns.rs` (Etapa 7). Lista conservadora; refino com uso real e edição pelo usuário são roadmap (Fase 8).
- **Allowlist de shell** (Etapa 7, `SHELL_ALLOWLIST_DEFAULT`): `ls`, `cat`, `head`, `tail`, `grep`, `find`, `wc`, `pwd`, `echo`. Read-only, primeiro token apenas — `git status`/`git log`/etc. (2 tokens) continuam pendência do ADR-0034. **Aplicada incondicionalmente** (ver seção `FilesExecShellTool` acima).
- **Denylist de shell** (Etapa 7, `SHELL_DENYLIST`): `rm -rf`, `del /f /s /q`, `remove-item -recurse -force`, `format`, `diskpart`, `bcdedit`, `reg delete`, `net user`, `net localgroup`, `cipher /w`, `sfc /scannow`. Hardcoded; edição pelo usuário é roadmap.

## Referências

- [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md) — modelo de isolamento
- [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) — escopo da Fase 7
- [ADR-0033](../decisions/0033-sandbox-network-policy.md) — política de rede
- [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md) — política de aprovação
- [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) — semântica de sobrescrita (file ops)
- [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) — SecurityJailResolver
- [`runtimes-architecture.md`](./runtimes-architecture.md) — Python + Node portáteis
- [`windows-sandbox-design.md`](./windows-sandbox-design.md) — sandbox Windows
- [`tool-registry-specification.md`](./tool-registry-specification.md) — `ToolManifest` (Fase 3 Etapa 2)
- [`tool-permission-model.md`](./tool-permission-model.md) — `PermissionSet` (Fase 3 Etapa 3)
- [`security-threat-model.md`](./security-threat-model.md) — I1, I3, R1
- `PROMPT MESTRE` §22.5 (segredos e rede), §8 (permissões hierárquicas)
- [`docs/architecture/development-roadmap.md`](./development-roadmap.md) — Fase 7, Etapas 4 e 6
