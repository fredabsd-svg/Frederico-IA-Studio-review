<!--
Estado: implementado
Verificado contra o código em: 2026-08-14
Fase correspondente: 7
-->

> Aprofundado na **Etapa 1 da Fase 7** (PR de planejamento, 2026-08-08), atualizado na **Etapa 2** (PR `fase-7-etapa-2-primitivas-sandbox`, 2026-08-08, 4 primitivas Rust implementadas em `crates/security/src/`), na **Etapa 3** (PR `fase-7-etapa-3-runtimes-embutidos`, 2026-08-08, novo crate `frederico-runtimes` com Python 3.12.4 + Node 20.16.0 portáteis), e na **Etapa 4** (PR `fase-7-etapa-4-exec-python-node`, 2026-08-08, `exec.python` + `exec.node` no `ToolRegistry` com wall-clock enforcement real + cancelamento cascateado per-invocation + aprovação obrigatória honrada), e na **Etapa 5+** (PR `fase-7-etapa-5-token-restrito`, 2026-08-10, **path safety enforcement** via `Mandatory Label\Low` no workdir + `TokenIntegrityLevel=Low` no child + raw `CreateProcessAsUserW` com `CREATE_SUSPENDED` + `AssignProcessToJobObject` per-invocation + reativação do catálogo `exec.python`/`exec.node` no `ToolRegistry` com `python/node: None → Sandboxed` no `PermissionSet`). Aprofundamento guiado pelo ADR-0031 (modelo de isolamento), ADR-0033 (política de rede) e ADR-0036 (SecurityJailResolver). O estado é `parcialmente implementado` (não `especificado`) porque a Fase 7 já está `em andamento` no `docs/status.md` (regra da trava do §1.13). **Etapa 2**: `EnvFilter` (env allowlist com subenum REQUIRED/ALLOWED/DENIED), `JobObject` (`KILL_ON_JOB_CLOSE` + limites de memória, **sem** `JOB_OBJECT_LIMIT_BREAKAWAY_OK` — default do Windows já garante que netos herdam o Job), `RestrictedToken` (drop 6 privilégios elevados), `SecurityJailResolver` (orquestrador que combina as 3 camadas). **Etapa 3**: `frederico-runtimes` (Python 3.12.4 + Node 20.16.0 com SHA-256 pinned). **Etapa 4**: `FilesExecPythonTool` + `FilesExecNodeTool` in-process em `crates/tool-registry/src/exec/` consumindo `SecurityJailResolver` + `RuntimeRegistry`; **per-invocation Job Object** (cada `spawn` cria Job novo, hard-fail em falha de criação/atribuição); `SandboxedProcess::wait_with_timeout` chamado **dentro** do `collect_output` (wall-clock real, não mais "apenas informativo"); aprovação obrigatória via `validate_tool_call` Passo 9. Pendência: `RestrictedToken` construído mas não aplicado via spawn (`CommandExt::as_user` exige refactor maior; Etapa 5+ raw `CreateProcessW`); `BREAKAWAY_OK` doc do ADR-0036 §D2 precisa ser corrigido (flag é INVERTIDA do que o original sugere — ela PERMITE netos escaparem via `CREATE_BREAKAWAY_FROM_JOB`, não que herdem; a Etapa 4 mantém ausente que é o correto, mas o doc está errado). **Etapas 5-7 ainda não iniciadas** (em 2026-08-08, antes da Etapa 5+). **Etapa 5+ fechada em 2026-08-10** com `RestrictedToken` aplicado via raw `CreateProcessAsUserW` (não mais só construído), `TokenIntegrityLevel=Low` + `Mandatory Label\Low` no workdir (via `SetFileSecurityW(LABEL_SECURITY_INFORMATION)`, sem `SeSecurityPrivilege`), per-invocation Job Object com `AssignProcessToJobObject` + `ResumeThread` separado. Pendências Etapa 5+ nomeadas: **read-up** (Mandatory Label só bloqueia write-up; child pode ler Medium-labeled paths — solução: SID restritivo próprio + DACL custom, Fase 8+), **rede** (sandbox não bloqueia rede; read-up + open net = exfiltração — solução: proxy local, Fase 7 Etapa 7), **pipe labels** (`CreatePipe` com SACL no SD exige `SeSecurityPrivilege`, falha com `ERROR_PRIVILEGE_NOT_HELD` 0x80070522 — pipes criados sem label, child escreve porque anônimo sem label passa o check). Detalhes em [`/SECURITY.md`](../../SECURITY.md) §"O que essa combinação NÃO protege". **Etapa 6 fechada em 2026-08-13** (PR #51, ADR-0033): proxy de rede local (`frederico-security::network`, TCP forward HTTP + tunnel CONNECT sem MITM) + `dns_intercept` (Windows) + `DbNetworkAuditSink`, deny-by-default. **Etapa 6+1, mesmo PR:** wiring real no `exec.python`/`exec.node` (`FilesExecToolBase::start_network_proxy`), fechando 4 causas-raiz que mascaravam o wiring (falta de `CREATE_UNICODE_ENVIRONMENT`, fallback silencioso pro env do pai, `SystemRoot`/`windir` faltando no `EnvAllowlist::REQUIRED`, `run_id` errado no audit sink). **Etapa 7 fechada em 2026-08-14** (PR `fase-7-etapa-7-exec-shell`): `exec.shell` (`FilesExecShellTool`) com denylist+allowlist de comandos sempre ativas (`frederico_security::exec_patterns`), `risk_level: Critical`; e a allowlist de rede passou a carregar do perfil TOML do usuário ∩ projeto (`PermissionSet.network_allowlist`, novo campo) em vez de hardcoded vazia — ver `docs/architecture/exec-tools-specification.md` §"Network deny-by-default" pro contrato completo. **Nota sobre a numeração das etapas:** a tabela "Etapas da Fase 7" abaixo, escrita na Etapa 1 (planejamento), previa Etapa 6 = `exec.shell` e Etapa 7 = rede — a ordem real executada foi invertida (rede fechou primeiro). A tabela foi corrigida pra refletir a ordem real; o número de cada etapa é o que apareceu nos commits/PRs reais, não o plano original.

# Design do Sandbox Windows

> **Alterado em relação ao plano original:** o cabeçalho deste spec dizia `Fase correspondente: 3`. A Fase 3 (Motor de execução e ferramentas) fechou entregando o `Jail` (Fase 6 Etapa 5.X, PR #25) — o ponto único de normalização de caminho que cobre a ameaça I3 — mas **não** o sandbox descrito aqui. O `development-roadmap.md` posiciona o sandbox na Fase 7, e é essa a fase correspondente correta. A troca desfaz a violação da trava do §1.13, que acusava um spec `especificado` cuja fase já estava concluída. A Etapa 1 da Fase 7 (este PR) conserta o cabeçalho e aprofunda o spec.

## Visão geral

O **sandbox do Modo Desenvolvedor** isola a execução de ferramentas perigosas (`exec.python`, `exec.node`, `exec.shell`, `files.write`, `files.edit`) em uma camada de processo Windows. A Fase 7 fecha quando as 3 camadas combinadas (Jail + Job Object + Restricted Token + env zeroed, ADR-0031 D1) estão implementadas, testadas com **teste de negação** (regra do user, 2026-08-08), e integradas ao `RunExecutor` da Fase 3.

A combinação é **deliberada**, não cumulativa: cada camada cobre uma classe de ameaça diferente, e juntas formam o mínimo que cobre as ameaças documentadas no `security-threat-model.md` para as ferramentas que a Fase 7 introduz. O `PROMPT MESTRE` §22 fixa a restrição: **sem Docker, sem WSL, sem `PATH` global alterado** — primitivas do Windows.

A Fase 6 já entregou a **barreira primária** de path safety (Etapa 5.X, PR #25): o `Jail` do `frederico-tool-registry` (`crates/tool-registry/src/workspace.rs`) rejeita `..`/absoluto/UNC/letra de unidade/symlink, e é invocado em **toda** operação de filesystem antes de qualquer I/O. A Fase 7 **adiciona** camadas (processo, privilégio, env, rede) — não substitui.

## Decisões tomadas

- **Sem Docker, sem WSL, sem `PATH` global alterado** (`PROMPT MESTRE` §5.2, §22). Isolamento via primitivas do Windows.
- **Workspace em `%LOCALAPPDATA%\FredericoAIStudio\workspaces\`** (`PROMPT MESTRE` §22.2) — agente não acessa diretamente documentos pessoais, área de trabalho, credenciais, navegador, registro, pastas de sistema, outros projetos.
- **Acesso externo ao workspace só com**: seleção pelo usuário, concessão de permissão, definição de leitura/escrita, registro, possibilidade de revogação (`PROMPT MESTRE` §22.3). Mecanismo: `ApprovalScope` no `PermissionSet` (ADR-0034).
- **Python e Node como pacotes gerenciados** (Etapa 3 da Fase 7, `runtimes-architecture.md`), não como dependência de instalação do usuário (`PROMPT MESTRE` §22.4). Não alterar `PATH` global.
- **Env do processo filho zerado e reconstruído por allowlist**; ambiente do app nunca é herdado (`PROMPT MESTRE` §22.5, ADR-0031 D5, ADR-0033 D2). A allowlist tem subenum `REQUIRED` (não-editável: `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `PATH` do runtime portátil, `TEMP`, `TMP`, `LANG`, `LC_ALL`, `PYTHONHOME`, `PYTHONPATH`, `NODE_PATH`, `HOME`, `USERPROFILE`) e `ALLOWED` (configurável pelo usuário, versionada).
- **Teste automatizado obrigatório**: nenhuma variável de ambiente de processo do sandbox contém valor de credencial cadastrada (`PROMPT MESTRE` §22.5, ADR-0031 D5). O teste está em `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets`.
- **Rede do sandbox só através de proxy local do app**, com allowlist (deny-by-default) e registro de URLs visível ao usuário na conversa (`PROMPT MESTRE` §22.5, ADR-0033).
- **Comandos aprovados são exibidos ao usuário exatamente como serão executados, sem abreviação** (`PROMPT MESTRE` §22.5 final, ADR-0034 D5). A invariante tem teste em `crates/e2e/tests/e2e_approval_display.rs::approved_command_matches_actual_invocation`.
- **Sandbox é opt-in por feature flag** durante a Etapa 2-6 da Fase 7 (`FREDERICO_SANDBOX_V1`, ADR-0031 D7). A Etapa 7 (Fase 7 concluída) remove a flag.

## As 3 camadas (ADR-0031 D1)

| Camada | Função | Ferramentas que usam | Implementação |
|---|---|---|---|
| **Jail** (Fase 6 Etapa 5.X) | Barreira primária de path safety | todas as de filesystem | `crates/tool-registry/src/workspace.rs::Jail` (existente) |
| **Job Object** | Tree-kill garantido (mesmo em `kill -9`), limite de memória por processo e total | `exec.python`, `exec.node`, `exec.shell` | `crates/security/src/windows/job_object.rs` (novo, Etapa 2) |
| **Restricted Token** | Descarte de 6 privilégios elevados (SeDebug, SeBackup, SeRestore, SeTakeOwnership, SeLoadDriver, SeShutdown) | `exec.python`, `exec.node`, `exec.shell` | `crates/security/src/windows/restricted_token.rs` (novo, Etapa 2) |
| **Env zeroed + allowlist** | Fecha `I1` do threat model (env leak) | `exec.python`, `exec.node`, `exec.shell` | `crates/security/src/env_filter.rs` (novo, Etapa 2) |
| **Proxy local de rede** | Rede do sandbox passa por allowlist + log visível (fecha SSRF reverso + DNS exfiltration) | `exec.python`, `exec.node`, quando precisam de rede (pip install, npm install) | `crates/security/src/network.rs` + `crates/security/src/dns_intercept.rs` (novos, Etapa 7 da Fase 7) |

A Etapa 2 da Fase 7 implementa as 4 camadas de processo (Job Object, Restricted Token, Env Filter, e o orquestrador `SecurityJailResolver`). A Etapa 7 da Fase 7 (rede) fecha o proxy local — o `exec.python`/`exec.node` da Etapa 4 já funciona **sem rede** nesse meio tempo (degradação declarada: `pip install` falha com mensagem clara, `python main.py` que não precisa de rede funciona normal).

`files.write` / `files.edit` / `files.list` **não** rodam sob Job Object + Restricted Token (não há spawn de processo). Usam só o `Jail` + protocolo atômico (ADR-0035 D1) + backup (D3) + audit (D6).

`AppContainer` (a primitiva mais forte do Windows) é **adiada** para Fase 8+ (ADR-0031 D6) — quebra rotinas comuns de Python/Node, e o custo de fazer funcionar é desproporcional ao ganho.

## Contrato previsto

### `SandboxConfig` (entrada do `SecurityJailResolver::spawn`)

```rust
struct SandboxConfig {
    /// Ferramenta que está sendo executada. Cada uma tem um nível default de sandbox.
    tool: ToolId,
    /// Permissões herdadas do `PermissionSet` da execução. Subagente ⊆ pai.
    permissions: PermissionSet,
    /// Allowlist de rede efetiva (do `NetworkAllowlist` global + escopo da execução).
    network_allowlist: NetworkAllowlist,
    /// Workdir (validado pelo Jail antes de qualquer coisa).
    workdir: CanonicalPath,
    /// Args a passar para o executável (do tool_call, validado pelo `validate_tool_call`).
    args: Vec<String>,
    /// Timeout de wall-clock. Default 60s; configurável por tool.
    wall_clock: Duration,
    /// Limite de memória por processo (default 2 GB) e total (default 4 GB).
    memory_limits: MemoryLimits,
    /// Variáveis de ambiente adicionais permitidas (além do `EnvAllowlist::REQUIRED`).
    /// Vazio por default; a Etapa 4 da Fase 7 pluga com `PermissionSet::extra_env`.
    extra_env: Vec<(String, String)>,
    /// Stdin a passar pro filho (None = /dev/null).
    stdin: Option<Vec<u8>>,
}

struct MemoryLimits {
    per_process_bytes: u64,   // default 2 GB
    total_bytes: u64,          // default 4 GB (soma da árvore)
}

struct NetworkAllowlist {
    entries: Vec<NetworkEntry>,  // hostname + ttl (OneExecution, OneSession, Forever)
    default: AllowOrDeny,         // Deny (nada passa se a allowlist está vazia)
}

enum AllowOrDeny { Allow, Deny }
```

### `SandboxedProcess` (saída do `SecurityJailResolver::spawn`)

```rust
struct SandboxedProcess {
    /// PID do processo filho (atribuído ao Job Object antes de ResumeThread).
    pid: u32,
    /// Handle do Job Object específico desse spawn (o `JobHandle` é droppado no fim, e o `KILL_ON_JOB_CLOSE` dispara).
    job_handle: JobHandle,
    /// Streams async para I/O (stdout, stderr) — coletados pela Etapa 4 da Fase 7.
    stdout: BoxStream<Vec<u8>>,
    stderr: BoxStream<Vec<u8>>,
    /// Cancelamento (cascateia do `CancellationToken` do `Run`).
    cancel_token: CancellationToken,
}

impl Drop for SandboxedProcess {
    fn drop(&mut self) {
        // Fecha o job_handle → KILL_ON_JOB_CLOSE derruba a árvore
        // (mesmo que o pai tenha crashado — o handle vive no app, não no filho)
    }
}
```

### `SecurityJailResolver` (orquestrador)

```rust
pub struct SecurityJailResolver {
    file_system_jail: FileSystemJailResolver,  // PR #25, Fase 6 Etapa 5.X
    root_job: JobObject,                         // vive até o app morrer
    env_filter: EnvFilter,                       // EnvAllowlist::REQUIRED + ALLOWED
    active_jobs: Mutex<HashMap<u32, JobHandle>>,
    next_id: AtomicU64,
}

impl SecurityJailResolver {
    pub fn new(file_system_jail: FileSystemJailResolver) -> Result<Arc<Self>, JailError>;
    pub fn spawn(&self, config: SandboxConfig) -> Result<SandboxedProcess, SpawnError>;
    pub fn cancel(&self, pid: u32) -> Result<(), CancelError>;
    pub fn alive_pids(&self) -> Vec<u32>;
    pub fn cleanup_orphans(&self) -> Result<u32, CleanupError>;  // Etapa 7 da Fase 7
}

impl Drop for SecurityJailResolver {
    fn drop(&mut self) {
        // Fecha active_jobs (KILL_ON_JOB_CLOSE) e root_job.
        // Garante tree-kill no shutdown normal, panic, e TerminateProcess.
    }
}
```

## Comportamento por tipo de execução

| Tool | Jail | Job Object | Restricted Token | Env filter | Proxy rede | Approval |
|---|---|---|---|---|---|---|
| `files.read` | sim | — | — | — | — | nunca |
| `files.list` | sim | — | — | — | — | nunca (dentro do workspace) |
| `files.write` / `files.edit` | sim | — | — | — | — | D2 do ADR-0034 (escopo `OneTurn` default) |
| `exec.python` | sim (workdir) | sim | sim | sim | sim (quando precisa) | D2 do ADR-0034 (escopo `OneTurn` default) |
| `exec.node` | sim (workdir) | sim | sim | sim | sim (quando precisa) | D2 do ADR-0034 (escopo `OneTurn` default) |
| `exec.shell` | sim (workdir) | sim | sim | sim | sim (quando precisa) | D3 do ADR-0034 (sempre `OneExecution`) |
| `web.fetch` / `web.search` | — | — | — | — | sim | nunca (tool de leitura, sem side-effect) |

`files.write` e `files.edit` rodam **dentro do processo do app** (sem spawn, sem sandbox de processo) — ADR-0035 D7. O ganho de segurança do sandbox de processo **não se aplica** (não há execução de código arbitrário, só `std::fs::write` em Rust). A barreira é Jail + atomicidade + backup + audit.

## Não-objetivos

- **Sandboxing de GPU.** Fora de escopo (uso sensível não previsto na v1).
- **Anti-debug, anti-tamper.** O sandbox não tenta defender contra o usuário que tenta ativamente bypassar (root, kernel debugger, etc.) — é defesa contra o **filho do sandbox**, não contra o usuário.
- **"Browser sandbox" completo.** O `browser-worker` é separado, fora do sandbox principal (mesma lógica da Fase 5).
- **Suporte a sandbox em macOS/Linux na v1.** A interface Rust é simétrica (linux retorna `Err(NotSupported)` com degradação declarada, mesma regra da Etapa 2.A da Fase 5). Implementação Windows é a única da v1.
- **AppContainer** (a primitiva mais forte de isolamento) é adiada para Fase 8+ (ADR-0031 D6). Razão: quebra rotinas comuns de Python/Node.
- **Sandboxing de certificate pinning bypass** (filho que ignora `HTTPS_PROXY` e conecta via `socket.socket` raw). Documentado como lacuna no `security-threat-model.md`; defesa via `Windows Defender Application Control` é roadmap de Fase 8+.
- **HTTP/3 (QUIC)** — o proxy fala TCP+TLS; filhos que tentam QUIC bypassam. Documentado como lacuna.

## Trade-offs explícitos (a parte "NÃO protege" do sandbox)

A REGRA 1.1 e a honestidade do `SECURITY.md` exigem que o documento diga **o que o sandbox não protege**. O sandbox da Fase 7 é defesa em profundidade contra **ameaças documentadas no threat model**, não garantia absoluta:

| Cenário | Protege? | Por quê |
|---|---|---|
| Filho lê arquivo fora do workspace | **sim** | Jail (Fase 6 Etapa 5.X) + filesystem do OS |
| Filho cria neto que sobrevive ao `kill -9` do app | **sim** | Job Object (D2 do ADR-0031, D6 do ADR-0036) |
| Filho herda privilégio de admin | **não** | Restricted Token descarta 6 privilégios, mas **outros** privilégios (SeNetworkLogonRight, SeInteractiveLogonRight etc.) permanecem. Defesa contra elevação requer AppContainer (Fase 8+) ou conta de usuário separada (Fase 8+). |
| Filho lê credencial em cache de DLL/TLS | **parcialmente** | Env filter zera env (D5 do ADR-0031, ADR-0036 D5), mas a credencial pode estar em (a) estrutura de adapter em memória, (b) TLS handshake cache, (c) `~/.netrc` do usuário. Defesa em profundidade: ferramentas que lidam com credencial não passam pelo sandbox (worker de provider, Fase 2). |
| Filho conecta direto via `socket.socket(AF_INET, SOCK_STREAM)` raw, ignorando `HTTPS_PROXY` | **não** | Sem firewall no nível de processo (WDAC é Fase 8+). O filho pode chamar `connect()` direto, bypassando o proxy. **Lacuna documentada.** |
| Filho tenta QUIC | **não** | Proxy é TCP+TLS. QUIC bypassa. **Lacuna documentada.** |
| Filho **lê** arquivo Medium-labeled fora do workdir (read-up) | **não** | Mandatory Label\Low só bloqueia **write-up** (`NO_WRITE_UP`). Child (Low) **consegue ler** paths Medium (default do filesystem): `%LOCALAPPDATA%\frederico.db`, `%USERPROFILE%\Documents`, etc. DACL permite o user ler seus próprios arquivos; Mandatory Label só nega **escrita** se o token < label. **Lacuna documentada** — Etapa 5+ (Fase 7) + roadmap Fase 8 (SID restritivo próprio + DACL custom fecha). |
| Filho escreve em pipe anônimo stdout/stderr sem label | **sim** (aceitável) | Pipes criados por `CreatePipe` **sem** `Mandatory Label\Low` no SACL. Tentativa de criar pipe **com** label falha com `ERROR_PRIVILEGE_NOT_HELD` (0x80070522) — kernel objects com SACL exigem `SeSecurityPrivilege` no caller, e o app roda sem UAC. Decisão Etapa 5+: pipes sem label (Medium default) — child (Low) escreve porque anônimo sem label passa o Mandatory Label check (objetos sem label não são comparados). Risco: child pode floodar stdout/stderr; não é escalada, é comunicação. **Lacuna documentada** — fechamento exige service Windows rodando como SYSTEM (Fase 8+). |
| Filho exfiltra via DNS | **sim** | `netsh dns set` força DNS via proxy, que valida hostname **antes** de resolver (ADR-0033 D5). |
| Filho lê `SAM` (Windows Security Account Manager) | **sim** | SeBackup removido (Restricted Token, ADR-0036 D4). |
| Filho faz `rm -rf /` no host | **não** (dentro do sandbox: sim) | O filho roda sob usuário limitado + Restricted Token, mas `rm -rf` no workdir (que é o workspace) **funciona** (é o diretório do usuário). Defesa: `exec.shell` com `Denylist` (Etapa 6 da Fase 7) proíbe; `exec.python` requer aprovação. |
| Usuário malicioso bypassa o sandbox | **não** | Sandbox é defesa contra o **filho**, não contra o usuário. Usuário admin pode matar o app, debugar o processo, editar config. **Esperado e documentado.** |

A frase que entra no `security-threat-model.md` §"Sandbox: o que protege e o que NÃO protege" é literal: **"O sandbox da Fase 7 é defesa em profundidade contra as ameaças I1, I2, I3, e a classe 'filho malicioso/invadido' das ameaças STRIDE. Não é sandbox de contêiner (Docker/runc), não é VPN, não é firewall. Lacunas explícitas: bypass de proxy via socket raw, HTTP/3, certificate pinning bypass, privilégios SeNetworkLogonRight. Mitigações estão em roadmap de Fase 8+."**

## Decisões (a aprofundar antes da Etapa 2)

Nenhuma nova. As decisões da Fase 7 Etapa 1 estão nos 6 ADRs (0031, 0032, 0033, 0034, 0035, 0036). Este spec é o **ponto de entrada** para o código da Etapa 2 em diante, e referencia cada ADR pela sigla.

## Etapas da Fase 7 (referência — números refletem a ordem REAL de execução, não o plano original da Etapa 1)

| Etapa | Status (em 2026-08-14) | Próxima | Bloqueia | Foco |
|---|---|---|---|---|
| Etapa 1 — Planejamento | **concluída** | Etapa 2 | nenhuma | 6 ADRs + 2 specs novos + 4 specs atualizados + fase-7/README + status + CHANGELOG |
| Etapa 2 — Primitivas do sandbox | **concluída** (PR `fase-7-etapa-2-primitivas-sandbox`, 2026-08-08) | Etapa 3 | nenhuma | `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 2 testes de integração `tree_kill.rs`. 37 tests verdes |
| Etapa 3 — Runtimes embutidos | **concluída** (PR `fase-7-etapa-3-runtimes-embutidos`, 2026-08-08) | Etapa 4 | nenhuma | `crates/runtimes/` com Python 3.12.4 + Node 20.16.0 portáteis, `RuntimeRegistry::bootstrap_all` idempotente, SHA-256 pinned |
| Etapa 4 — `exec.python` / `exec.node` no registro | **concluída** (PR `fase-7-etapa-4-exec-python-node`, 2026-08-08) | Etapa 5 | Etapa 2 + Etapa 3 | `FilesExecPythonTool` + `FilesExecNodeTool`, wall-clock real, cancelamento cascateado, aprovação obrigatória |
| Etapa 5 — `files.write` / `files.edit` / `files.list` | **concluída** (PR `fase-7-etapa-5-files-write-list`, 2026-08-08; **Etapa 5+** path safety, PR `fase-7-etapa-5-token-restrito`, 2026-08-10) | Etapa 6 | Etapa 2 | `FilesWriteTool` + `FilesEditTool` + `FilesListTool` + `Mandatory Label\Low` + `TokenIntegrityLevel=Low` via raw `CreateProcessAsUserW`. Reativou `exec.python`/`exec.node` no `ToolRegistry` |
| Etapa 6 — Rede do sandbox (proxy local) | **concluída** (PR #51, 2026-08-13, ADR-0033) | Etapa 6+1 | Etapa 2 | `crates/security/src/network.rs` + `dns_intercept.rs` + `DbNetworkAuditSink`, deny-by-default, HTTPS via `CONNECT` sem MITM |
| Etapa 6+1 — Wiring do proxy em `exec.python`/`exec.node` | **concluída** (mesmo PR #51, 2026-08-13) | Etapa 7 | Etapa 6 | `FilesExecToolBase::start_network_proxy`; fechou 4 causas-raiz que mascaravam o wiring (env block UTF-16, fallback silencioso, `SystemRoot` faltando no `EnvAllowlist`, `run_id` errado no audit) |
| Etapa 7 — `exec.shell` com denylist/allowlist + allowlist de rede via perfil | **concluída** (PR `fase-7-etapa-7-exec-shell`, 2026-08-14) | — (Fase 7 fecha) | Etapa 2 + Etapa 4 + Etapa 6+1 | `FilesExecShellTool` + `frederico_security::exec_patterns` (denylist+allowlist sempre ativas, não gateadas por `PermissionSet` — ver `exec-tools-specification.md`); `PermissionSet.network_allowlist` novo campo, carregado do perfil TOML user∩project na casca |

Regra para todas as etapas de 2 a 7: **pelo menos um teste de negação por etapa** (regra do user, 2026-08-08). Sandbox se prova impedindo, não funcionando.

## Mapa de E2E planejado por etapa

| Etapa | Teste de negação (principal) | Twin determinístico (PR) | Noturno (`#[ignore]`) |
|---|---|---|---|
| 2 | `crates/security/tests/tree_kill.rs::fase5_etapa2a_incomplete_kill_parent_does_not_kill_grandchild` (controle negativo — regressão da Fase 5) + `crates/security/tests/tree_kill.rs::job_object_kills_tree_on_resolver_drop` (controle positivo — fix) | mesmo arquivo, em `cargo test -p frederico-security` | — |
| 2 | (pendente) `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets` (fecha `I1` do threat model) — **deferido para Etapa 4**: v1 não re-injecta o env filtrado (limitação do `tokio::process::Command::envs()` em Windows com env block grande) | — | — |
| 2 | (pendente) `crates/security/tests/restricted_token.rs::python_runs_under_restricted_token` — **deferido para Etapa 4**: v1 constrói o token mas não aplica via `CreateProcessAsUser` (precisa de `CommandExt::as_user`, fora do `tokio::process` plain) | — | — |
| 2 | (pendente) `crates/security/tests/job_object_setup.rs::process_runs_under_memory_limit` — **deferido para Etapa 5 ou 6**: testar com processo leve (Python idle) não prova o limite; testar com alocação pesada é flaky no CI | — | — |
| 3 | `crates/runtimes/tests/python_bootstrap.rs::python_env_vars_do_not_include_user_paths` (controle negativo — PATH injetado não contém hijack patterns) + `crates/runtimes/tests/bootstrap_idempotent.rs::bootstrap_twice_is_noop` (controle positivo — 2ª chamada >5x mais rápida) + `crates/runtimes/tests/manifest_corruption.rs::corrupted_manifest_triggers_redownload` (controle positivo — SHA-256 mismatch deleta + re-download) | mesmo arquivo, em `cargo test -p frederico-runtimes` | — |
| 3 | `crates/runtimes/tests/node_bootstrap.rs::node_env_vars_do_not_include_user_paths` (controle negativo — mesma defesa para Node) + `crates/runtimes/tests/bootstrap_offline.rs::offline_returns_error_for_missing_runtime` (controle positivo — `allow_download=false` + cache vazio = `Err(OfflineRequired)`, não panic) | mesmo arquivo | — |
| 4 | `crates/e2e/tests/e2e_exec_python_under_sandbox.rs::child_cannot_write_outside_workspace` | mesmo arquivo | — |
| 4 | `crates/e2e/tests/e2e_approval_display.rs::approved_command_matches_actual_invocation` | mesmo arquivo | — |
| 5 | `crates/e2e/tests/e2e_atomic_write.rs::crash_between_write_and_rename_leaves_original_intact` | mesmo arquivo | — |
| 5 | `crates/e2e/tests/e2e_overwrite_backup.rs::overwrite_creates_backup_with_previous_content` | mesmo arquivo | — |
| 6/6+1 | `crates/e2e/tests/e2e_network_proxy.rs` (7 testes) + `e2e_network_proxy_wired_into_exec_python.rs` + `e2e_network_proxy_wired_into_exec_node.rs` | mesmo arquivo | — |
| 7 | `crates/e2e/tests/e2e_exec_shell_denylist.rs::rm_rf_is_rejected_by_denylist` + `e2e_exec_shell_allowlist.rs::curl_not_in_allowlist_is_blocked`/`echo_allowlisted_command_executes` | mesmo arquivo | — |

A Etapa 1 (este PR) preenche o **plano** no `status.md` (coluna `E2E de cobertura` com `pending` para cada teste, e a Etapa 2 em diante atualiza o plano com `path::fn_name` real quando implementar — gate `check-e2e-gate.ps1` passa a validar consistência).

## Referências

- [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md) — modelo de isolamento (3 camadas combinadas, AppContainer adiado)
- [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) — escopo da Fase 7 vira só execução isolada
- [ADR-0033](../decisions/0033-sandbox-network-policy.md) — política de rede (deny-by-default, proxy local, log visível)
- [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md) — política de aprovação de escrita/exec
- [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) — semântica de sobrescrita (atômico, backup, audit)
- [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) — `SecurityJailResolver` com Job Objects (carry-over da Fase de Ligação)
- [`security-threat-model.md`](./security-threat-model.md) — modelo STRIDE, I1, I2, I3, e a parte "NÃO protege" do sandbox
- [`process-architecture.md`](./process-architecture.md) — como o sandbox se relaciona com workers sidecar (Fase 5)
- [`tool-permission-model.md`](./tool-permission-model.md) — `PermissionSet` da Fase 3 Etapa 3 (ganha campo `sandbox: SandboxLevel` na Etapa 2)
- [`docs/architecture/runtimes-architecture.md`](./runtimes-architecture.md) — Python + Node portáteis (Etapa 3)
- [`docs/architecture/exec-tools-specification.md`](./exec-tools-specification.md) — `exec.python`, `exec.node`, `exec.shell` (Etapa 4, 6)
- [`docs/architecture/development-roadmap.md`](./development-roadmap.md) — Fase 7 com escopo novo, Fase 8 absorve Git/GitHub
- `PROMPT MESTRE` §22 (execução local sem Docker), §22.5 (segredos e rede)
- [`docs/releases/fase-7/README.md`](../releases/fase-7/README.md) — narrativa de processo da fase
