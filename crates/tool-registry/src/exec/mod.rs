//! Ferramentas `exec.*` — `exec.python` e `exec.node` (Etapa 4 da
//! Fase 7), `exec.shell` (Etapa 7, denylist/allowlist sempre
//! ativas, ADR-0034 D3).
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
//! ## Comportamento (Etapa 4 da Fase 7)
//!
//! - **Wall-clock enforcement real** — `wait_with_timeout` dentro
//!   do `collect_output`. O `SandboxConfig.wall_clock` deixou
//!   de ser "apenas informativo".
//! - **Cancelamento cascateado per-invocation** — cada `spawn`
//!   cria um Job Object novo. Drop fecha o handle e mata a
//!   árvore (filho + netos + bisnetos).
//! - **Aprovação obrigatória** — `requires_user_approval(true)`
//!   no manifesto. O `validate_tool_call` Passo 9 é o gate; sem
//!   `ApprovalDecision` aprovada, retorna `ApprovalRequired`.
//! - **Sem rede** — `pip install` falha (degradação declarada).
//!   Etapa 7 da Fase 7 implementa o proxy local.
//! - **Sem `exec_patterns.rs`** — auto-approval por code sem
//!   padrão perigoso é Etapa 5+ (só pro `exec.shell`).
//! - **Audit sink via trait `AuditSink`** — `NoopAuditSink` em
//!   v1; `DbAuditSink` é trabalho da Etapa 5+ da Fase 3.

#![allow(missing_docs)]

mod node;
mod output;
mod python;
mod shell;

pub use node::FilesExecNodeTool;
pub use output::{MAX_OUTPUT_BYTES, OUTPUT_CHUNK_SIZE};
pub use python::FilesExecPythonTool;
pub use shell::FilesExecShellTool;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{RunId, ToolId};
use frederico_runtimes::RuntimeRegistry;
use frederico_security::jail::SecurityJailResolver;
use frederico_security::network::{NetworkAllowlist, NetworkAuditSink};
use serde_json::Value;
use thiserror::Error;

use crate::audit::AuditSink;
use crate::manifest::ToolManifest;
use crate::tools::{Tool, ToolContext};

/// Constrói o catálogo default de `exec.*` tools (`exec.python` +
/// `exec.node`) com as dependências compartilhadas. Retorna
/// `Vec<Arc<dyn Tool>>` pronto pra `build_tool_registry` em
/// `frederico-app`.
///
/// **Por que uma função pública (não `FilesExecToolBase::new`):**
/// o `FilesExecToolBase` é `pub(crate)` (detalhe de
/// implementação). Expor `build_default_exec_tools` esconde esse
/// detalhe e impede que callers construam `FilesExecToolBase`
/// em configs inconsistentes (ex.: resolver sem runtimes).
///
/// **Fail-soft na plataforma:** o `Arc<SecurityJailResolver>`
/// pode ser construído em Linux (retorna `platform_supported=false`).
/// Os tools são registrados mesmo assim — o `validate_tool_call`
/// detecta a indisponibilidade do runtime e o `RunExecutor`
/// recusa a chamada (degradação declarada, mesma regra do
/// `document-worker` da Etapa 2.A).
///
/// **Network proxy (Etapa 6 da Fase 7, ADR-0033):** o penúltimo
/// arg é o `NetworkAllowlist` (default deny). O último é o
/// `NetworkAuditSink` — injetado de fora (mesmo padrão do
/// `AuditSink` de arquivo), **não** construído aqui. `tool-registry`
/// não depende de `frederico-storage` (limite arquitetural
/// deliberado — ver doc de `DbNetworkAuditSink`); o caller (a
/// casca, que já tem o `Database`) constrói o
/// `DbNetworkAuditSink` real e passa como trait object. Um único
/// sink é compartilhado entre todas as invocações — cada
/// `NetworkAccessEntry` carrega o `run_id` da invocação que a
/// gerou (estampado por `start_network_proxy` a cada chamada),
/// então o sink não precisa (e não deve) guardar um `run_id`
/// fixo — ver `network_audit_sink.rs::append_sync`.
#[must_use]
pub fn build_default_exec_tools(
    resolver: Arc<SecurityJailResolver>,
    runtimes: Arc<RuntimeRegistry>,
    audit: Arc<dyn AuditSink>,
    network_allowlist: NetworkAllowlist,
    network_audit: Arc<dyn NetworkAuditSink>,
) -> Vec<Arc<dyn Tool>> {
    let base = FilesExecToolBase::new(resolver, runtimes, audit, network_allowlist, network_audit);
    // `exec.shell` **NÃO** entra (ADR-0037, 2026-08-16). A
    // allowlist de comandos que justificava a ferramenta é
    // contornável por qualquer separador do `cmd.exe`: `is_allowed`
    // valida só o primeiro token e `build_args` entrega o command
    // string inteiro pro `cmd.exe /c`, então `echo x & <qualquer
    // coisa>` passa. Provado em
    // `crates/e2e/tests/e2e_exec_shell_out_of_catalog.rs` e em
    // `frederico_security::exec_patterns::tests::allowlist_is_defeated_by_cmd_exe_command_separators`.
    // Mesma regra aplicada a `exec.python`/`exec.node` na Etapa 5+ e
    // ao `dns_intercept` na Etapa 6: capacidade incompleta é
    // capacidade indisponível.
    //
    // O código de `shell.rs` fica no crate (não é deletado como o
    // `dns_intercept` foi): o conserto é conhecido — recusar
    // separadores de shell antes do spawn — e a ferramenta volta
    // ao catálogo quando isso, mais o problema dos binários MSYS2
    // sob integridade baixa, estiver resolvido. Ver ADR-0037
    // §"Consequências".
    vec![
        Arc::new(FilesExecPythonTool::new(base.clone())),
        Arc::new(FilesExecNodeTool::new(base)),
    ]
}

/// Trait `FilesExecTool` — método compartilhado por Python e
/// Node. Privado ao crate (os tools concretos são expostos
/// como `Arc<dyn Tool>` via `build_default_exec_tools`).
pub(crate) trait FilesExecTool: Send + Sync {
    /// ID canônico da ferramenta (`exec.python`, `exec.node`).
    fn tool_id(&self) -> ToolId;

    /// Manifesto da ferramenta (input/output schemas, risk_level, etc.).
    fn manifest(&self) -> &ToolManifest;

    /// Resolve o binário (Python, Node) via `RuntimeRegistry`.
    /// Retorna o ID do runtime (ex.: `python-3.12.4`).
    fn resolve_runtime_id<'a>(&self, args: &'a Value) -> Result<&'a str, ExecError>;

    /// Monta os args do `CreateProcess` a partir dos args da
    /// tool_call. Implementação diferente por tool:
    /// - Python: `-c "<code>"` / `<path>` / `-m <module>`
    /// - Node:   `<path>` / `-e "<code>"` / `-m <module>`
    fn build_args(&self, args: &Value) -> Result<Vec<String>, ExecError>;

    /// Default approval scope (ADR-0034 D2). v1: Etapa 4 não tem
    /// UI, retorna `OneExecution` por default (conservador; UI
    /// da Etapa 5 refina).
    #[allow(dead_code)] // Etapa 5+ usa quando a UI plugar.
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
/// audit + wall-clock + output cap + network proxy).
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
    /// Allowlist de hostnames pro proxy de rede (Etapa 6 da
    /// Fase 7, ADR-0033). **Deny-by-default**: vazio = tudo
    /// bloqueado. O `execute()` de cada tool inicia o proxy
    /// por invocação, com essa allowlist.
    pub network_allowlist: NetworkAllowlist,
    /// Timeout por request no proxy (HTTP ou CONNECT). Default
    /// 5s (mesmo do `ProxyConfig::default()`). Curto porque
    /// o Job Object do sandbox é o watchdog real.
    pub network_request_timeout: Duration,
    /// Sink de audit do proxy de rede (Etapa 6+1 da Fase 7).
    /// Injetado de fora — `NoopNetworkAuditSink` em testes,
    /// `DbNetworkAuditSink` na casca real. Um único sink
    /// compartilhado entre todas as invocações; cada
    /// `NetworkAccessEntry` carrega o `run_id` da chamada que a
    /// gerou (não o sink).
    pub network_audit: Arc<dyn NetworkAuditSink>,
}

impl FilesExecToolBase {
    /// Construtor padrão. Wall-clock default = 60s, output cap
    /// = 10 MB (const em `output.rs`), network proxy deny-by-default
    /// (allowlist vazia = tudo bloqueado).
    #[must_use]
    pub(crate) fn new(
        resolver: Arc<SecurityJailResolver>,
        runtimes: Arc<RuntimeRegistry>,
        audit: Arc<dyn AuditSink>,
        network_allowlist: NetworkAllowlist,
        network_audit: Arc<dyn NetworkAuditSink>,
    ) -> Self {
        Self {
            resolver,
            runtimes,
            audit,
            default_wall_clock: Duration::from_secs(60),
            network_allowlist,
            network_request_timeout: Duration::from_secs(5),
            network_audit,
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

    /// Sobe o proxy de rede pro filho do sandbox (Etapa 6 da
    /// Fase 7, ADR-0033), escreve `<workdir>/.frederico/proxy.port`
    /// com a porta atribuída pelo OS, e monta as env vars
    /// `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` que entram no
    /// `SandboxConfig::extra_env` do `spawn`.
    ///
    /// **Por que helper único:** o `execute()` do Python e o
    /// do Node são idênticos nesse pedaço. Centralizar evita
    /// divergência (ex.: esquecer de escrever o `proxy.port`,
    /// ou usar `127.0.0.1` no `NO_PROXY` mas esquecer o
    /// `localhost` — o filho que faz `urllib.urlopen("http://
    /// localhost:8000/")` precisa do `localhost` na
    /// `NO_PROXY` pra não loopar pelo proxy).
    ///
    /// **Lifecycle:** o retorno [`NetworkProxyGuard`] dropa o
    /// proxy no fim do escopo (chama `frederico_security::network::shutdown`).
    /// Caller segura a variável até o `collect_output` retornar.
    /// Drop prematuro = o filho perde a saída de rede (a request
    /// TCP falha com "connection reset"). Caller **deve** segurar
    /// o guard até depois de `collect_output`.
    ///
    /// **Sem degradação:** o proxy HTTP/HTTPS é obrigatório desde
    /// a Etapa 7 (feature flag removida, ADR-0033 §D7). Se
    /// `start_proxy` falhar (loopback indisponível — improvável),
    /// este método retorna `Err(ExecError::SpawnFailed(...))` e o
    /// caller (`execute()` de cada tool) aborta a chamada — não
    /// existe mais um caminho de "rede aberta sem proxy".
    ///
    /// **Audit sink:** `self.network_audit` (Etapa 6+1) —
    /// injetado de fora (ver doc de `build_default_exec_tools`),
    /// `Noop` em testes, `DbNetworkAuditSink` na casca real. O
    /// proxy funciona deny-by-default mesmo com sink `Noop` — só
    /// não fica registrado.
    ///
    /// **`run_id` tipado, não `&str`:** a `RunId` tem `Display`
    /// customizado (`"RunId(<uuid>)"`, não o uuid puro — pensado
    /// pra debug/log, não pra round-trip). Passar `RunId` aqui
    /// (em vez de `caller.to_string()`) e formatar o uuid puro
    /// só neste um lugar evita que essa string com o wrapper
    /// `RunId(...)` vá parar no `network_audit.run_id` e quebre
    /// o `Uuid::parse_str` do lado do `DbNetworkAuditSink`
    /// (silenciosamente — o parse falha, o `run_id` gravado vira
    /// `NULL`, e nada avisa).
    pub fn start_network_proxy(
        &self,
        workdir: &std::path::Path,
        run_id: RunId,
    ) -> Result<NetworkProxyGuard, ExecError> {
        // Cria o `<workdir>/.frederico/` se não existe. O workdir
        // tem low integrity (Etapa 5+), mas o parent (este processo)
        // tem user integrity, então pode escrever.
        let proxy_dir = workdir.join(".frederico");
        std::fs::create_dir_all(&proxy_dir).map_err(|e| {
            ExecError::SpawnFailed(format!(
                "não conseguiu criar {}: {e} (workdir não é writable?)",
                proxy_dir.display()
            ))
        })?;

        // Inicia o proxy com o sink real (injetado). O uuid puro
        // (não o `Display` de `RunId`) é o que vai pro
        // `NetworkAccessEntry.run_id` — ver doc acima.
        let config = frederico_security::network::ProxyConfig {
            allowlist: self.network_allowlist.clone(),
            request_timeout: self.network_request_timeout,
        };
        let handle = frederico_security::network::start_proxy(
            config,
            self.network_audit.clone(),
            Some(run_id.0.to_string()),
        )
        .map_err(|e| ExecError::SpawnFailed(format!("start_proxy falhou: {e}")))?;

        // Escreve `<workdir>/.frederico/proxy.port` com a porta.
        let port_path = proxy_dir.join("proxy.port");
        let port = handle.port; // extrai a porta antes de mover handle no error path
        if let Err(e) = std::fs::write(&port_path, port.to_string()) {
            // Rollback: derruba o proxy que acabamos de subir.
            frederico_security::network::shutdown(handle);
            return Err(ExecError::SpawnFailed(format!(
                "não conseguiu escrever {}: {e}",
                port_path.display()
            )));
        }

        // Monta as env vars que entram no SandboxConfig::extra_env.
        // `NO_PROXY=127.0.0.1,localhost` é obrigatório (D6 do
        // ADR-0033) — sem isso, o filho que faz
        // `requests.get("http://127.0.0.1:8000/")` (auto-loop
        // comum em testes) passa pelo proxy, que faz CONNECT
        // pra 127.0.0.1:8000, que é o próprio listener, que…
        // loop infinito.
        let proxy_url = format!("http://127.0.0.1:{port}");
        let extra_env = vec![
            ("HTTP_PROXY".to_string(), proxy_url.clone()),
            ("HTTPS_PROXY".to_string(), proxy_url),
            ("NO_PROXY".to_string(), "127.0.0.1,localhost".to_string()),
        ];

        tracing::info!(
            %run_id,
            port = port,
            allowlist = ?self.network_allowlist.allowed,
            "proxy de rede iniciado (Etapa 6 / ADR-0033)"
        );

        Ok(NetworkProxyGuard {
            handle: Some(handle),
            port_path,
            extra_env,
        })
    }
}

/// RAII guard do proxy de rede. Drop chama
/// `frederico_security::network::shutdown` (que fecha o
/// listener via `oneshot::Sender`). O `proxy.port` é removido
/// do workdir no Drop (cleanup best-effort — se o filho
/// crashou, o workdir também pode estar gone, e o `remove_file`
/// pode falhar; nesse caso, o `recover_stale_runs` da Etapa 5.x
/// é o caminho secundário).
///
/// O proxy HTTP/HTTPS é sempre obrigatório desde a Etapa 7 (a
/// feature flag `FREDERICO_NETWORK_PROXY_V1` que permitia
/// desligá-lo foi removida — ADR-0033 §D7).
///
/// **Sem DNS intercept.** A Etapa 7 tentou wirear um responder DNS
/// (`dns_proxy` + `dns_intercept::set_dns_intercept` via `netsh` na
/// interface Loopback) e reverteu — verificação manual end-to-end
/// (2026-08-14, `nslookup` sem servidor explícito, comparando
/// antes/durante/depois do `netsh`) provou que o Windows **não**
/// consulta a interface Loopback pra resolver DNS de verdade; ele
/// usa os adaptadores de rede reais. O mecanismo não entregava
/// nenhuma proteção, exigia Admin, e mexia na config de DNS da
/// máquina do usuário à toa — removido por inteiro (regra "capacidade
/// incompleta é capacidade indisponível", mesma regra aplicada à
/// Etapa 5+ quando `exec.python`/`exec.node` saíram do catálogo).
/// Ver `SECURITY.md` §"Rede" e ADR-0033 §D1/§Pendências.
pub(crate) struct NetworkProxyGuard {
    /// Sempre `Some` quando `start_network_proxy` retorna `Ok`
    /// (o proxy HTTP/HTTPS é obrigatório).
    handle: Option<frederico_security::network::ProxyHandle>,
    /// Path do `proxy.port` escrito no workdir. `Drop` tenta
    /// remover (best-effort).
    port_path: std::path::PathBuf,
    /// Env vars pro `SandboxConfig::extra_env`.
    extra_env: Vec<(String, String)>,
}

impl NetworkProxyGuard {
    /// `true` se o proxy HTTP/HTTPS está ativo. Caller usa isso
    /// pra decidir se deve ou não loggar warning no audit do
    /// tool.
    #[must_use]
    pub(crate) fn is_enabled(&self) -> bool {
        self.handle.is_some()
    }

    /// Env vars pro `SandboxConfig::extra_env`.
    #[must_use]
    pub(crate) fn extra_env(&self) -> Vec<(String, String)> {
        self.extra_env.clone()
    }
}

impl Drop for NetworkProxyGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            frederico_security::network::shutdown(handle);
        }
        // Best-effort: remove o `proxy.port`. Se o workdir já
        // não existe (filho crashou e cleanup removeu), o erro
        // é silencioso — o `recover_stale_runs` da Etapa 5.x
        // é o caminho secundário.
        if !self.port_path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.port_path);
        }
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
    /// `exec.shell`: comando bate um padrão da
    /// `frederico_security::exec_patterns::SHELL_DENYLIST` (Etapa 7,
    /// ADR-0034 D3). Recusado antes do spawn.
    #[error("comando recusado pela denylist (padrao: {0})")]
    CommandDenied(String),
    /// `exec.shell`: primeiro token do comando não está na
    /// `SHELL_ALLOWLIST_DEFAULT` (Etapa 7). Recusado antes do spawn.
    #[error("comando '{0}' nao esta na allowlist")]
    CommandNotInAllowlist(String),
}
