# 0036 — `SecurityJailResolver` com Job Objects do Windows (carry-over da Etapa 7 REMOVIDA da Fase de Ligação)

## Contexto

A Fase de Ligação (entre Fase 5 e Fase 6) planejou, na Etapa 7 original, um `SecurityJailResolver` em `crates/security/src/jail.rs` com **Job Objects do Windows** para garantir tree-kill do child quando o parent morre — mesmo em `kill -9` (que o Windows simula como `TerminateProcess` sem cleanup). A Etapa 7 foi **removida** da Fase de Ligação com a justificativa registrada em `docs/releases/fase-ligacao/README.md`:

> "**Etapa 7 (`SecurityJailResolver` modo desenvolvedor) REMOVIDA** desta fase. Depende da Fase 7 do PROMPT MESTRE; uma fase de ligação que depende de fase futura nunca fecha. Virou pendência nomeada dentro da Fase 7 (criar `SecurityJailResolver` em `crates/security/src/jail.rs` com Job Objects do Windows pra garantir kill-tree do child quando o parent morre, mesmo em kill -9; substituir `FileSystemJailResolver` no `setup` da casca)."

A Fase 7 do PROMPT MESTRE começa agora (Etapa 1, este PR). A pendência da Fase de Ligação vira **trabalho de Etapa 2** desta fase, e este ADR formaliza a decisão de implementação.

O **problema concreto** que o `SecurityJailResolver` resolve é o carry-over da Etapa 2.A da Fase de Ligação (PR #22, ADR-0023): o `DocumentWorkerLauncher` (Python sidecar para kits de documento) **não tem tree-kill confiável**. Se o app morre com `kill -9` (cenário de crash, OU Windows Update reiniciando o serviço, OU usuário matando o app pelo Task Manager), o Python continua rodando, deixa handles abertos em arquivos temporários do workspace, e na próxima execução o sandbox tem resíduo. O mesmo problema afeta os **filhos do sandbox da Fase 7** (Etapa 2): `exec.python` spawna um `python.exe` que pode criar netos (e.g. `pip install` spawna `pip`, que spawna `python` de novo). Sem tree-kill, qualquer um dos netos pode sobreviver ao `kill -9` do app.

A primitiva certa no Windows é `Job Object` com `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — quando o **handle** do Job é fechado (e o handle do Job vive no app principal, não no filho), o OS derruba a árvore inteira. Esse é o mecanismo que sobrevive a `TerminateProcess` do parent (o `TerminateProcess` fecha os handles, então o `KILL_ON_JOB_CLOSE` dispara).

A Etapa 2 da Fase de Ligação (PR #22) **tentou** o tree-kill via `Child::kill()` do `tokio::process`, que envia `TerminateProcess` ao PID direto. Esse mecanismo é **incompleto** porque:

1. `Child::kill()` mata o PID, mas o OS já pode ter spawned netos que **não** são netos do PID original (em Windows, netos criados via `CreateProcess` sem `JobObject` herdado não recebem o `kill`).
2. A janela entre `CreateProcess` retornar e o app "conseguir" matar o filho é uma race condition — em crash, o app não tem tempo de chamar `Child::kill()`.
3. O `tokio::process` no Windows usa `Job Object` por default para **alguns** flags, mas não para `KILL_ON_JOB_CLOSE` (que precisa de setup explícito via `win32job` ou `windows` crate).

O `SecurityJailResolver` da Fase 7 Etapa 2 fecha esses 3 buracos de uma vez, com a primitiva certa (`Job Object` + `AssignProcessToJobObject` **antes** do `CreateProcess` retornar, com a janela de race coberta por `JOB_OBJECT_LIMIT_BREAKAWAY_OK`).

## Decisões

### D1 — `SecurityJailResolver` é a substituição do `FileSystemJailResolver` no `setup` da casca

A `casca Tauri` (`apps/desktop/src-tauri/src/main.rs`) atualmente chama `FileSystemJailResolver::new` na inicialização para resolver o `Jail` por conversa. A Etapa 2 da Fase 7 substitui por `SecurityJailResolver::new` (novo, em `crates/security/src/jail.rs`), que **encapsula** o `FileSystemJailResolver` e adiciona 3 capacidades:

1. **Job Object por spawn** (tree-kill): cada filho do sandbox recebe um `JobHandle` registrado no `SecurityJailResolver`. Quando o app morre (qualquer causa), o destructor do `SecurityJailResolver` fecha todos os `JobHandle`s → `KILL_ON_JOB_CLOSE` derruba a árvore.

2. **Restricted Token** (D4 do ADR-0031): o filho é spawnado com token que descarta privilégios elevados. O setup é feito no `SecurityJailResolver::spawn()`, que retorna o `Child` + `JobHandle` + `SandboxContext` (com o token, env filtrado, workdir).

3. **Env filtering** (D5 do ADR-0031): o env do filho é zerado e reconstruído por allowlist (`EnvAllowlist::REQUIRED` + `EnvAllowlist::ALLOWED` + `EnvAllowlist::DENIED`).

A separação é: `FileSystemJailResolver` continua existindo (Fase 6 Etapa 5.X, PR #25, é a barreira primária de path safety), e o `SecurityJailResolver` é a **camada de processo** que envolve o `FileSystemJailResolver`. As 3 capacidades são **aditivas** — uma `files.write` que não precisa de sandbox de processo (D7 do ADR-0035) usa só `FileSystemJailResolver`; um `exec.python` usa `SecurityJailResolver::spawn(...)` com as 3 capacidades.

### D2 — `Job Object` configurado com `KILL_ON_JOB_CLOSE` + `BREAKAWAY_OK`

A Etapa 2 da Fase 7 implementa `JobObject::new()` em `crates/security/src/windows/job_object.rs` (novo, ~150 linhas):

```rust
pub struct JobObject(HANDLE);  // HANDLE é NonNull, sentinela != INVALID_HANDLE_VALUE

impl JobObject {
    pub fn new() -> Result<Self, JobError> {
        let h = unsafe { CreateJobObjectW(None, None) };
        if h.is_null() { return Err(JobError::CreateFailed(...)); }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_BREAKAWAY_OK       // permite que netos criados pelo filho herdem o Job
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY      // limite de memória por processo
            | JOB_OBJECT_LIMIT_JOB_MEMORY;         // limite de memória total da árvore
        info.ProcessMemoryLimit = 2 * 1024 * 1024 * 1024;  // 2 GB por processo
        info.JobMemoryLimit = 4 * 1024 * 1024 * 1024;       // 4 GB total
        let ok = unsafe {
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 { return Err(JobError::SetInfoFailed(...)); }
        Ok(Self(h))
    }

    pub fn assign(&self, process_handle: HANDLE) -> Result<(), JobError> { ... }
    pub fn assign_pid(&self, pid: u32) -> Result<(), JobError> { ... }  // via OpenProcess + assign
}
```

`Drop` para `JobObject` chama `CloseHandle(self.0)`. Quando o `JobObject` é droppado (no destructor do `SecurityJailResolver`, que roda no panic do app, no `kill -9`, no shutdown normal), o handle é fechado → `KILL_ON_JOB_CLOSE` derruba a árvore.

`BREAKAWAY_OK` é o flag que **permite** que netos criados pelo filho herdem o Job. Sem ele, `subprocess.Popen(...)` em Python falha com "access denied" (o filho não pode criar neto sob o Job). A Etapa 2 da Fase 7 documenta: o `BREAKAWAY_OK` é o que faz `pip install` funcionar — o `pip` é neto do Python, e o `pip` invoca outro Python para compilar wheels nativas (`.pyd`).

### D3 — Atribuição acontece **antes** do `CreateProcess` retornar

A janela de race condition do `Child::kill()` da Etapa 2.A da Fase de Ligação é fechada por **atribuição preventiva**:

1. App chama `CreateProcessW(...)` com flag `CREATE_SUSPENDED`.
2. App chama `JobObject::assign(process_handle)` no `HANDLE` retornado por `CreateProcessW`.
3. App chama `ResumeThread(process_handle)`.
4. App registra o `pid` no `SecurityJailResolver::active_jobs: HashMap<u32, JobHandle>`.

A janela entre passo 1 e 2 é **zero** (o processo está suspended, não roda). A janela entre passo 2 e 3 é **zero** (o Job Object já contém o PID suspended, e qualquer spawn que o filho fizesse após `ResumeThread` herdaria o Job via `BREAKAWAY_OK`). A janela entre passo 3 e 4 é **zero** no sentido prático (o `HashMap::insert` é sync, e o OS já tem o Job configurado).

Se o app crasha entre passo 1 e 3: o processo suspended é órfão, o Job Object é droppado no destructor do `SecurityJailResolver` (que vive no `app_state` da casca Tauri, que é droppado no panic do Tauri), e o OS mata o suspended. Sem a atribuição, o suspended viraria zumbi (não executa nada, mas segura o handle do arquivo `.exe` carregado em memória).

A Etapa 2 da Fase 7 implementa via `win32job` crate (binding leve) ou `windows` crate (mais verboso, mas sem dependência nova — `windows = "0.58"` já está no `Cargo.toml` da Fase 2 Hardening 1).

### D4 — `RestrictedToken` configurado com `SaferLevel::Disallowed` + 6 SIDs negados

A Etapa 2 da Fase 7 implementa `RestrictedToken::new()` em `crates/security/src/windows/restricted_token.rs` (novo, ~200 linhas):

```rust
pub struct RestrictedToken {
    handle: HANDLE,
    original_handle: HANDLE,  // do pai, antes do filtro
}

impl RestrictedToken {
    pub fn drop_privileges(&mut self, privileges_to_disable: &[LPCWSTR]) -> Result<(), TokenError> {
        // GetTokenInformation(TokenPrivileges) -> lista de privilégios do token
        // Para cada privilege em privileges_to_disable, marca como SE_PRIVILEGE_REMOVED
        // AdjustTokenPrivileges(...)
    }

    pub fn deny_sids(&mut self, sids_to_deny: &[PSID]) -> Result<(), TokenError> {
        // Cria lista de deny-only SIDs
        // SetTokenInformation(TokenRestrictedSids, ...)
    }
}
```

Lista de privilégios a remover (D4 do ADR-0031):

- `SeDebugPrivilege` — impede que o filho atache a outro processo (incluindo o pai) para inspecionar memória.
- `SeBackupPrivilege` — impede leitura de arquivos de sistema (SAM, SECURITY) via API de backup.
- `SeRestorePrivilege` — impede escrita em arquivos de sistema.
- `SeTakeOwnershipPrivilege` — impede "roubar" ownership de arquivo de sistema.
- `SeLoadDriverPrivilege` — impede carregar driver (vetor clássico de rootkit).
- `SeShutdownPrivilege` — impede shutdown/reboot do host (defesa contra `exec.shell` malicioso).

SIDs a negar (defesa em profundidade):

- `BUILTIN\Administrators` (S-1-5-32-544) — não roda como admin.
- `Everyone` (S-1-1-0) — em deny-only, vira effective "no access" para recursos que dão `Everyone` apenas.

**Teste de regressão** (regra do user: "teste de negação"): `crates/security/tests/restricted_token.rs::python_runs_under_restricted_token` — spawna `python.exe -c "import os; print(os.getuid() if hasattr(os, 'getuid') else 'windows')"`, afirma que o uid efetivo **não** é o de admin. Sem o token restricted, o filho roda com os privilégios do usuário (que pode ser admin em algumas instalações).

### D5 — Env filtering é fail-closed (D5 do ADR-0031, executado aqui)

A Etapa 2 da Fase 7 implementa `EnvFilter::apply()` em `crates/security/src/env_filter.rs` (novo, ~150 linhas):

```rust
pub struct EnvFilter {
    allowlist: EnvAllowlist,  // { required: Vec<&'static str>, allowed: Vec<&'static str>, denied: Vec<String> }
}

impl EnvFilter {
    pub fn apply(&self, parent_env: &[(String, String)]) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(self.allowlist.required.len() + self.allowlist.allowed.len());
        // 1. Required vars (sempre passam, sem chance de override do usuário).
        for key in &self.allowlist.required {
            if let Some((_, v)) = parent_env.iter().find(|(k, _)| k == key) {
                out.push((key.to_string(), v.clone()));
            } else if let Some(default) = self.default_for(key) {
                out.push((key.to_string(), default.to_string()));
            }
        }
        // 2. Allowed vars (passam se presentes, sem override do usuário).
        for key in &self.allowlist.allowed {
            if let Some((_, v)) = parent_env.iter().find(|(k, _)| k == key) {
                out.push((key.to_string(), v.clone()));
            }
        }
        // 3. Vars sensíveis (sobrescreve com string vazia antes de remover, defesa contra cache de libc).
        for key in &self.allowlist.denied {
            if let Some(_) = parent_env.iter().find(|(k, _)| k == key) {
                // Não inclui na saída, mas já foi sobrescrita com "" no parent_env
                // (a sobrescrita é feita in-place, no vetor pai, antes do filtro, pra
                // qualquer cache de getenv que olhe a string original ver "")
            }
        }
        out
    }
}
```

A sobrescrita in-place **antes** do filtro é o que fecha a porta de "filho vê o valor antigo via cache de libc". Sem ela, um filho que fez `getenv("OPENAI_API_KEY")` antes do fork ainda vê o valor antigo; com ela, o filho vê `""`.

`EnvAllowlist::required` é populado pelo construtor do `EnvFilter` (hardcoded em `crates/security/src/config.rs`): `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` (do ADR-0033), `PATH` (apontando pro runtime portátil, Etapa 3), `TEMP`, `TMP`, `LANG`, `LC_ALL`, `PYTHONHOME`, `PYTHONPATH`, `NODE_PATH`, `HOME`, `USERPROFILE`. **Não editável pelo usuário.**

`EnvAllowlist::denied` é hardcoded com os segredos comuns: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GITHUB_TOKEN`, `GH_TOKEN`, `*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PRIVATE_KEY`, `DATABASE_URL`. Match por **sufixo de nome** (`*_TOKEN` casa `GITHUB_TOKEN`, `READ_TOKEN`, etc.) e match exato para os específicos.

**Teste de regressão** (D5 do ADR-0031): `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets` injeta `OPENAI_API_KEY=test-secret-XXX` no env do app, spawna `python.exe -c "import os; print(os.environ.get('OPENAI_API_KEY', 'EMPTY'))"` via `SecurityJailResolver::spawn()`, afirma que imprime `EMPTY`. **Esse é o teste que fecha `I1` do threat model**, e é o que a Fase 6 carregou como dependência da Fase 7.

### D6 — `SecurityJailResolver` tem `Drop` que fecha Jobs

```rust
pub struct SecurityJailResolver {
    file_system_jail: FileSystemJailResolver,
    job_object: JobObject,  // Root Job, vive até o app morrer
    env_filter: EnvFilter,
    active_jobs: Mutex<HashMap<u32, JobHandle>>,  // pids filhos
    next_id: AtomicU64,
}

impl Drop for SecurityJailResolver {
    fn drop(&mut self) {
        // Itera active_jobs, fecha cada um (KILL_ON_JOB_CLOSE dispara)
        for (_, handle) in self.active_jobs.lock().unwrap().drain() {
            handle.close();  // fecha o handle do Job específico desse spawn
        }
        // O job_object root é droppado automaticamente, fechando o handle dele
        // e disparando KILL_ON_JOB_CLOSE em qualquer filho ainda atribuído
    }
}
```

A invariante é: **toda árvore de filhos atribuída ao Job root é morta no drop do `SecurityJailResolver`**. O destructor roda no:

- Shutdown normal do app (`Drop` no `app_state` do Tauri).
- Panic do app (drop em unwind, com `RefUnwindSafe` para o que precisa ser droppable).
- `TerminateProcess` do app pelo OS (`kill -9` simula isso, o handle do Job é fechado pelo OS como parte do cleanup do processo).

**Teste de regressão** (regra do user): `crates/security/tests/tree_kill.rs::child_survives_parent_kill9` spawna um filho que cria um neto, mata o pai com `TerminateProcess` (via `Child::kill()`), afirma que **ambos** estão mortos em < 1s. Sem o Job Object, o neto sobrevive (a `tokio::process` não tem como matar netos via PID).

## Consequências

- `crates/security/src/windows/job_object.rs` (novo, ~150 linhas).
- `crates/security/src/windows/restricted_token.rs` (novo, ~200 linhas).
- `crates/security/src/env_filter.rs` (novo, ~150 linhas).
- `crates/security/src/jail.rs` (novo, ~300 linhas) — o `SecurityJailResolver` que orquestra os 3 acima + o `FileSystemJailResolver`.
- `apps/desktop/src-tauri/src/main.rs` substitui `FileSystemJailResolver::new(...)` por `SecurityJailResolver::new(...)` no `setup` da casca.
- `crates/execution-engine/src/run_executor.rs` ganha 1 hook: `executor.spawn_under_sandbox(cmd, args) -> Result<SandboxedProcess, SpawnError>`, que delega para `SecurityJailResolver::spawn()`. A Etapa 4 da Fase 7 (exec tools) usa.
- O `PermissionSet` da Fase 3 Etapa 3 ganha 1 campo novo: `sandbox: SandboxLevel { None, Soft, Strict }` (D1 do ADR-0031). A Etapa 4 da Fase 7 implementa.
- 4 testes novos em `crates/security/tests/`: `tree_kill.rs`, `restricted_token.rs`, `env_isolation.rs`, `job_object_setup.rs` (~600 linhas estimadas de teste). **Todos com teste de negação** (regra do user).
- A `pendência 1 do process-architecture.md` ("SecurityJailResolver com Job Objects para tree-kill") é **fechada** por este ADR + a Etapa 2.
- O `docs/architecture/process-architecture.md` ganha §"Camada de processo" linkando para este ADR e o `crates/security/src/jail.rs` (Etapa 2 da Fase 7).

## Alternativas consideradas

1. **Reusar `Child::kill()` da Fase 5** (que é o que a Etapa 2.A da Fase de Ligação fez). Rejeitado por (a) race condition documentada (D3), (b) não mata netos, (c) depende do app estar vivo para chamar `kill()` — crash bypassa o mecanismo.
2. **Job Object via `win32job` crate**. Considerado, mas rejeitado por adicionar dependência nova (`win32job` não é crate oficial Microsoft). O `windows = "0.58"` crate (já no `Cargo.toml` da Fase 2 Hardening 1) tem os bindings necessários, e a verbosidade é mitigada por testes.
3. **Sem Restricted Token** (só Job Object + env filter). Rejeitado por (a) não descarta privilégios, (b) `SeDebug` sozinho dá ao filho acesso à memória do pai, (c) D4 do ADR-0031 já decidiu que Restricted Token é a camada de drop de privilégios. Reduzir a Etapa 2 a Job Object + env filter é cortar a cobertura do ADR-0031.
4. **AppContainer** (em vez de Restricted Token). Rejeitado pelo ADR-0031 D6 — AppContainer quebra Python/Node. Restricted Token é o que mantém compatibilidade.
5. **Filter de env por regex case-insensitive** (em vez de match exato + sufixo). Rejeitado por (a) regex é mais lento, (b) regex casa strings demais (`_TOKEN_X` casaria `MY_TOKEN_X` se não for ancorado, mas com `^.*_TOKEN$` ancora pelo fim, e mesmo assim é mais frágil que match literal).

## Pendências

- **`linux.rs` do `SecurityJailResolver`** — a Etapa 2 implementa só Windows. Linux é `Err(NotSupported)` (degradação declarada, mesma regra da Etapa 2.A da Fase 5). Roadmap: cgroups v2 + namespace + seccomp-bpf, com a interface Rust espelhada. **Fora do escopo da v1** (PROMPT MESTRE §22 + windows-sandbox-design.md §"Não-objetivos").
- **`JobHandle` exposto ao `DbAuditSink`** — para que o log de auditoria carregue o ID do Job por execução, e o post-mortem de incidente possa referenciar o handle. A Etapa 2 implementa; o `DbAuditSink` ganha 1 campo `job_id: Option<u64>` (opcional, retrocompatível).
- **Limites de CPU** (`JOB_OBJECT_LIMIT_PROCESSOR`) — o D2 deste ADR configura só `JOB_OBJECT_LIMIT_PROCESS_MEMORY` + `JOB_OBJECT_LIMIT_JOB_MEMORY`. CPU é roadmap (precisa de `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` que tem semântica diferente por versão do Windows). A Etapa 7 (Fase 7 concluída) pode adicionar.
- **Cleanup de processos órfãos em execuções anteriores** — se o app crashou e o `JobHandle` não foi droppado (impossível pelo D6, mas defensivamente), o `Job Object` do OS ainda existe com o nome do processo. A Etapa 7 da Fase 7 implementa: no boot, lista Jobs ativos do OS (`QueryInformationJobObject` com `JobObjectBasicProcessIdList`) e mata os órfãos. Sem isso, jobs de execuções crashadas podem acumular até o OS reciclar (que é lento: o Job é freed quando o último processo atribuído morre).

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7, fecha a pendência da Fase de Ligação Etapa 7 REMOVIDA (registrada em `docs/releases/fase-ligacao/README.md` linhas 82-89). Validação pelo user (via `ask_user`): "tire Git/GitHub da Fase 7. Ele é de outra natureza." A presente Etapa 1 da Fase 7 herda a Etapa 7 da Fase de Ligação como Etapa 2 daqui (sandbox primitives), e o `SecurityJailResolver` é a peça que faltava para a Fase 5 Etapa 2.A ter tree-kill real.
