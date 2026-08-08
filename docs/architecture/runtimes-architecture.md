<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-08
Fase correspondente: 7
-->

> Spec criado na Etapa 1 da Fase 7 (este PR de planejamento, 2026-08-08). Especificação da Etapa 3 da Fase 7 — `crates/runtimes/` com Python + Node portáteis. O estado é `parcialmente implementado` (não `especificado`) porque a Fase 7 já está `em andamento` no `docs/status.md` (regra da trava do §1.13): o planejamento cobre o `Runtime` trait, `RuntimeRegistry` com bootstrap idempotente, `manifest.json` com SHA-256 pinned, layout de diretórios, e o trade-off explícito "runtime separado do workspace". **Sem código de produção** — a Etapa 3 da Fase 7 implementa; o carimbo `Verificado contra o código em` ganha a data do merge.

# Runtimes Embutidos (Python + Node portáteis)

> **Contexto:** a Fase 7 introduz `exec.python` e `exec.node` (Etapas 4 e 6). Esses tools invocam um binário de Python/Node que precisa estar **dentro do app** — não como dependência de instalação do usuário. O `PROMPT MESTRE` §22.4 fixa a regra: "Python e Node como pacotes gerenciados, não como dependência de instalação do usuário. Não alterar `PATH` global."

## Visão geral

O **crate `frederico-runtimes`** (novo, Etapa 3 da Fase 7) gerencia 2 runtimes portáteis:

- **Python 3.12+** (build oficial CPython, `Windows embeddable package`).
- **Node 20+** (build oficial Node.js, distribuição zip portable).

Cada runtime vive em `%LOCALAPPDATA%\FredericoAIStudio\runtimes\<name>\<version>\` após bootstrap. O bootstrap é **idempotente** (segunda chamada é no-op), **offline-capable** (download com retry; cache local), e **versionado** (cada runtime tem um `runtime.toml` com `version`, `source_url`, `sha256`, `bootstrap_at`).

A localização é **separada** do workspace do sandbox (`%LOCALAPPDATA%\FredericoAIStudio\workspaces\<id>`), mas o filho do sandbox **lê** o runtime via `PATH` no env filtered (D5 do ADR-0031: `EnvAllowlist::REQUIRED` inclui `PATH` apontando pro runtime portátil). O filho **não escreve** no diretório de runtime — só o `frederico-runtimes` (no app) escreve, e só durante o bootstrap.

## Decisões tomadas

- **Sem `PATH` global alterado** (`PROMPT MESTRE` §5.2). O `PATH` injetado no env do filho aponta **apenas** para o runtime portátil + o `PATH` mínimo do Windows (`System32`). O `PATH` do usuário (que tem o Python/Node que ele possa ter instalado) **não** é herdado.
- **Bootstrap offline-first**: o `frederico-runtimes` checa o cache local primeiro; baixa via HTTPS com retry exponencial (3 tentativas, 1s/2s/4s) só se o cache está ausente ou corrompido (SHA-256 mismatch).
- **Versionamento por runtime.toml** (formato TOML versionado). Mudar a versão = bump no `runtime.toml` + nova pasta + (opcional) limpeza da versão antiga (configurável: `keep_n_versions`, default 2).
- **Validação pós-bootstrap**: cada runtime roda um `--version` (Python: `python.exe --version`, Node: `node.exe --version`) e um teste mínimo (Python: `python.exe -c "import sys; assert sys.version_info >= (3, 12)"`, Node: `node.exe -e "console.log(process.versions.node)"`). Falha aborta o bootstrap.
- **Sem alteração do sandbox de processo**: os runtimes são binários **normais** que rodam sob o `SecurityJailResolver` (Etapa 2). Eles não ganham primitivas especiais.
- **Source URL pinned**: o `runtime.toml` declara `source_url` e `sha256`; o bootstrap **não** aceita URL alternativa sem bump do TOML (que vira migration `0039_runtimes_manifest.sql` quando entrar).

## Contrato previsto

### `Runtime` (trait)

```rust
pub trait Runtime: Send + Sync {
    fn id(&self) -> &RuntimeId;             // "python-3.12.4", "node-20.16.0"
    fn version(&self) -> &str;
    fn executable(&self) -> &Path;          // absolute path para python.exe / node.exe
    fn home_dir(&self) -> &Path;            // diretório raiz do runtime (onde fica python.exe)
    fn site_packages(&self) -> Option<&Path>;  // Some(python), None(node — usa node_modules)
    fn env_vars(&self) -> &[(String, String)];  // PYTHONHOME, PYTHONPATH, NODE_PATH, etc.
    fn bootstrap_if_needed(&self) -> Result<(), BootstrapError>;
    fn validate(&self) -> Result<(), ValidationError>;  // --version + sanity check
}
```

### `RuntimeRegistry`

```rust
pub struct RuntimeRegistry {
    runtimes: HashMap<RuntimeId, Arc<dyn Runtime>>,
    config: RuntimeConfig,
}

impl RuntimeRegistry {
    pub fn new(config: RuntimeConfig) -> Result<Self, RegistryError>;
    pub fn get(&self, id: &RuntimeId) -> Option<Arc<dyn Runtime>>;
    pub fn all(&self) -> Vec<Arc<dyn Runtime>>;
    pub fn bootstrap_all(&self) -> Result<BootstrapReport, BootstrapError>;
    pub fn cleanup_old_versions(&self, keep_n: usize) -> Result<usize, CleanupError>;
}
```

### `RuntimeConfig`

```rust
pub struct RuntimeConfig {
    /// Diretório base dos runtimes. Default: `%LOCALAPPDATA%\FredericoAIStudio\runtimes\`.
    pub install_root: PathBuf,
    /// Manter N versões anteriores após bump. Default 2.
    pub keep_n_versions: usize,
    /// Permitir download pela rede. Default true; em air-gapped vira false (bootstrap só via cache).
    pub allow_download: bool,
    /// URL custom de mirror (opcional). Se Some, substitui `source_url` do `runtime.toml`.
    pub mirror_url: Option<String>,
    /// Timeout de download (default 5 min).
    pub download_timeout: Duration,
}
```

### `BootstrapReport`

```rust
pub struct BootstrapReport {
    pub bootstrapped: Vec<RuntimeId>,   // rodou bootstrap
    pub cached: Vec<RuntimeId>,          // já estava em cache
    pub failed: Vec<(RuntimeId, BootstrapError)>,
    pub total_duration: Duration,
    pub bytes_downloaded: u64,
}
```

## Comportamento de bootstrap

```text
┌─ bootstrap_if_needed(runtime) ─────────────────────────┐
│                                                        │
│  1. Compute target_dir = install_root/<id>/<version>/  │
│  2. If target_dir exists:                              │
│       a. Read manifest.json from target_dir            │
│       b. If sha256 of binaries == expected: return Ok  │
│       c. Else: delete target_dir, fall through         │
│  3. If !allow_download: return Err(OfflineRequired)    │
│  4. Download from source_url (or mirror_url)           │
│       a. Validate sha256 of downloaded archive         │
│       b. Extract to target_dir                         │
│       c. Write manifest.json with sha256 + timestamp   │
│  5. validate(runtime):                                │
│       a. Run `<runtime> --version`                    │
│       b. Run sanity check (--version, import sys)      │
│       c. If both Ok: return Ok                        │
│       d. Else: return Err(ValidationFailed)            │
│                                                        │
└────────────────────────────────────────────────────────┘
```

**Idempotência**: passo 2 garante que bootstrap repetido é no-op (sem rede, sem extraction) se o cache está válido.

**Resiliência**: passo 4c-5d garante que um download corrompido ou extração parcial não deixa o runtime em estado "meio instalado". Se validate falha, `target_dir` é deletado, e o próximo bootstrap tenta de novo.

## Localização dos runtimes

```text
%LOCALAPPDATA%\FredericoAIStudio\
├── runtimes\                          ← RuntimeConfig::install_root
│   ├── python\
│   │   ├── 3.12.4\
│   │   │   ├── python.exe             ← Runtime::executable
│   │   │   ├── python312.dll
│   │   │   ├── python312.zip
│   │   │   ├── Lib\
│   │   │   ├── tcl\
│   │   │   └── manifest.json          ← { version, source_url, sha256, bootstrap_at }
│   │   └── 3.11.9\                    ← versão anterior (mantida por keep_n_versions)
│   │       └── ...
│   └── node\
│       ├── 20.16.0\
│       │   ├── node.exe
│       │   ├── node_modules\
│       │   └── manifest.json
│       └── 20.15.1\                   ← versão anterior
│           └── ...
└── workspaces\                         ← workspaces dos sandboxes
    ├── <conversation_id_1>\
    └── <conversation_id_2>\
```

**Separação** entre `runtimes/` e `workspaces/`: o runtime é **read-only** durante execução normal (só o bootstrap escreve), enquanto o workspace é o scratch space do filho do sandbox. O `Jail` da Fase 6 Etapa 5.X protege o workspace; o runtime é acessado via `PATH` (D5 do ADR-0031) mas o filho **não** está confinado ao runtime — pode ler qualquer arquivo que o usuário consegue ler (limitação do Restricted Token, ADR-0031 D4).

## Manifest (`manifest.json`)

```json
{
  "runtime_id": "python-3.12.4",
  "version": "3.12.4",
  "source_url": "https://www.python.org/ftp/python/3.12.4/python-3.12.4-embed-amd64.zip",
  "source_sha256": "1d2b89c2e3...",
  "archive_size_bytes": 12345678,
  "bootstrap_at": "2026-08-08T12:34:56Z",
  "validated": true,
  "validation_output": "Python 3.12.4"
}
```

O `manifest.json` é o que `bootstrap_if_needed` consulta no passo 2b para validar o cache sem rodar `--version` (rápido, 1 leitura de arquivo). O `validated: true` só é setado depois que o validate do passo 5 passa.

## Integração com o sandbox (Etapa 4 da Fase 7)

A Etapa 4 (exec tools) consome o `RuntimeRegistry`:

```rust
// Em crates/execution-engine/src/exec/python_tool.rs
pub struct PythonExecTool {
    jail_resolver: Arc<SecurityJailResolver>,
    runtimes: Arc<RuntimeRegistry>,
    permission_checker: Arc<PermissionChecker>,
}

#[async_trait]
impl Tool for PythonExecTool {
    async fn execute(&self, call: ToolCall, ctx: &ValidationContext) -> Result<ToolResult, ToolError> {
        // 1. Check approval (ADR-0034)
        let approval = self.permission_checker.check_approval(&call, ctx)?;
        
        // 2. Resolve runtime
        let runtime = self.runtimes.get(&RuntimeId::from_model_str(&call.args["runtime"])?)
            .ok_or(ToolError::UnknownRuntime(...))?;
        
        // 3. Build SandboxConfig (ADR-0031)
        let config = SandboxConfig {
            tool: self.id().clone(),
            permissions: ctx.permissions.clone(),
            workdir: self.jail_resolver.file_system_jail.resolve(...)?,
            args: parse_python_args(&call.args)?,
            wall_clock: Duration::from_secs(60),
            env: runtime.env_vars().to_vec(),
            stdin: call.args.get("stdin").and_then(|s| s.as_str().map(String::as_bytes)),
            ..Default::default()
        };
        
        // 4. Spawn under sandbox
        let mut process = self.jail_resolver.spawn(config)?;
        
        // 5. Collect output (with cancellation)
        let output = tokio::select! {
            output = collect_output(&mut process) => output,
            _ = ctx.cancel_token.cancelled() => {
                process.kill().await?;
                return Err(ToolError::Cancelled);
            }
        };
        
        // 6. Audit (R1 do threat model)
        self.audit_sink.record(AuditEntry {
            kind: "exec_python",
            tool: self.id().clone(),
            runtime: runtime.id().to_string(),
            args: config.args.clone(),
            exit_code: output.exit_code,
            duration_ms: output.duration.as_millis() as u64,
            approval_scope: approval.scope,
            ...
        })?;
        
        Ok(ToolResult { output, .. })
    }
}
```

O `runtime.env_vars()` é o que popula `EnvAllowlist::REQUIRED` (ADR-0031 D5): `PATH` (apontando pro runtime), `PYTHONHOME`, `PYTHONPATH`. A Etapa 4 implementa; a Etapa 2 do sandbox já consome o `EnvFilter` configurado.

## Comportamento esperado

### Primeiro launch do app (cache vazio)

```text
1. App inicia, RuntimeRegistry::bootstrap_all() roda
2. Python 3.12.4: download 12 MB, extract, validate → ~5s
3. Node 20.16.0: download 30 MB, extract, validate → ~8s
4. Total: ~13s no primeiro launch (rede), 0s nos subsequentes (cache hit)
5. App continua inicializando normalmente
```

### Launch com cache válido

```text
1. App inicia, RuntimeRegistry::bootstrap_all() roda
2. Python: cache hit (manifest.json presente, sha256 OK) → 1ms
3. Node: cache hit → 1ms
4. Total: ~2ms (só lê manifest.json)
```

### Launch com cache corrompido

```text
1. App inicia, RuntimeRegistry::bootstrap_all() roda
2. Python: cache hit, mas sha256 mismatch → delete, re-download
3. Node: cache hit, OK → 1ms
4. Total: ~5s (re-download Python)
```

### Launch air-gapped (sem rede)

```text
1. App inicia, RuntimeRegistry::bootstrap_all() roda
2. Python: cache hit, OK → 1ms
3. Node: cache miss, allow_download=false → Err(OfflineRequired)
4. App continua com Python funcional, Node reporta Unavailable
5. UI mostra "Python disponível, Node requer bootstrap (verifique conexão)"
```

## Não-objetivos

- **Build de Python/Node a partir do source.** Usa-se o build oficial (embeddable/zip portable).
- **Múltiplas versões simultâneas ativas** (v1 só permite 1 versão por runtime ativo). A `keep_n_versions` guarda versões antigas para rollback, mas só 1 está no `RuntimeRegistry::all()`.
- **Virtualenv.** A Etapa 4 (exec tools) consome o Python portátil com `--user` ou um diretório de deps isolado por workspace (Etapa 3.x roadmap). Sem `virtualenv`/`venv` na v1.
- **PyPI/npm install offline.** O `pip install` (Etapa 4) usa a rede do sandbox, que passa pelo proxy local (ADR-0033). Sem mirror PyPI/npm local na v1.
- **Auto-update dos runtimes.** Bump de versão é bump do `runtime.toml` + release. Sem auto-update silencioso (Fase 9 do roadmap).
- **Cross-platform.** Windows only (mesma restrição do sandbox). Linux é roadmap.

## Testes de regressão obrigatórios (regra do user: "teste de negação")

A Etapa 3 da Fase 7 entrega pelo menos:

| Teste | O que prova |
|---|---|
| `crates/runtimes/tests/python_bootstrap.rs::python_finds_no_path_to_user_installs` | O `PATH` do filho do sandbox (Python) **não** inclui paths do usuário (Documents, AppData/Roaming, etc.) — defesa contra o usuário ter um `python.exe` malicioso em `~/bin/` que hijacka o do app. |
| `crates/runtimes/tests/node_bootstrap.rs::node_finds_no_path_to_user_installs` | Mesma defesa para Node. |
| `crates/runtimes/tests/bootstrap_idempotent.rs::bootstrap_twice_is_noop` | Segundo `bootstrap_if_needed` em cache válido é no-op (1ms, sem rede). |
| `crates/runtimes/tests/bootstrap_offline.rs::offline_returns_error_for_missing_runtime` | Sem rede + sem cache = `Err(OfflineRequired)`, não panic. |
| `crates/runtimes/tests/manifest_corruption.rs::corrupted_manifest_triggers_redownload` | Manifest com sha256 errado = delete + re-download (não usa runtime corrompido). |

**Teste de negação** (regra do user, 2026-08-08): o `python_finds_no_path_to_user_installs` é o teste que **prova que o isolamento é real** — injeta um `python.exe` falso em `~/bin/python.exe` que printa "PWNED" quando invocado, roda o sandbox, afirma que o filho executa o Python do app (não o falso). Sem esse teste, o `PATH` filtrado é só promessa.

## Trade-offs explícitos

| Decisão | Custo | Ganho | Por quê |
|---|---|---|---|
| Bootstrap no primeiro launch | 13s de latência | Garantia de runtime presente | Sem dependência de instalação do usuário |
| Runtime separado do workspace | 2 paths (runtime + workspace) | Sandbox pode ser destruído sem perder runtime | Cleanup do workspace não afeta runtime |
| `PATH` filtrado (sem `~/bin/`) | Usuário não pode usar Python/Node do sistema para os tools da Fase 7 | Defesa contra hijack de PATH | Mesmo princípio do "ambiente do app nunca é herdado" do `PROMPT MESTRE` §22.5 |
| SHA-256 pinned | Bump de versão requer rebuild do `runtime.toml` | Defesa contra MITM de download | Source URL é HTTPS, mas SHA-256 é a verificação final |
| Sem virtualenv | `pip install --user` em vez de venv | Menor superfície (sem venv activate/deactivate) | venv é roadmap de Fase 8+ |
| Sem auto-update | Bump manual | Sem mudança silenciosa de runtime | Mesma regra do `no interruptor` do §19.6 |

## Decisões (a aprofundar antes da Etapa 3)

- **Versões iniciais pinned**: Python 3.12.4 (LTS mais recente no momento da Etapa 1) e Node 20.16.0 (LTS ativo). Sujeito a bump antes da Etapa 3.
- **Mirror custom**: o `mirror_url` da `RuntimeConfig` é uma **válvula de escape** para ambiente corporativo (mirror PyPI/npm interno). Default é None (usa `source_url` direto). Ativação requer UI de settings.
- **Cache compartilhado entre usuários**: a Etapa 3 v1 **não** compartilha cache entre usuários do Windows (cada usuário tem seu `%LOCALAPPDATA%`). Compartilhamento é roadmap (Fase 8+).

## Referências

- [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md) — modelo de isolamento, `EnvAllowlist::REQUIRED`
- [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) — escopo da Fase 7
- [ADR-0033](../decisions/0033-sandbox-network-policy.md) — rede do sandbox (proxy)
- [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) — `SecurityJailResolver::spawn` consome `Runtime::env_vars`
- [`windows-sandbox-design.md`](./windows-sandbox-design.md) — spec do sandbox
- [`exec-tools-specification.md`](./exec-tools-specification.md) — `exec.python` / `exec.node` que consomem este runtime
- `PROMPT MESTRE` §22.4 (Python/Node como pacotes gerenciados)
- [`docs/architecture/development-roadmap.md`](./development-roadmap.md) — Fase 7, Etapa 3
