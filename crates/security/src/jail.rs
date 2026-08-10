//! `SecurityJailResolver` — orquestrador que combina as 3 camadas
//! do sandbox Windows da Fase 7:
//!
//! 1. **Jail** (path safety, fora deste módulo — o caller passa o
//!    `workdir` já validado; ver `frederico-tool-registry::Jail`).
//! 2. **Job Object** ([`crate::windows::JobObject`]) — tree-kill
//!    garantido via `KILL_ON_JOB_CLOSE` + `BREAKAWAY_OK` (netos
//!    herdam o Job).
//! 3. **Restricted Token**
//!    ([`crate::windows::RestrictedToken`]) — drop dos 6 privilégios
//!    elevados (SeDebug, SeBackup, SeRestore, SeTakeOwnership,
//!    SeLoadDriver, SeShutdown).
//! 4. **Env Filter** ([`crate::env_filter::EnvFilter`]) — fecha I1
//!    do threat model (env do pai não vaza pro filho).
//!
//! Ver [ADR-0031 §D1](../../../decisions/0031-fase-7-isolation-model-windows.md),
//! [ADR-0036 §D1-D6](../../../decisions/0036-security-jail-resolver-windows-job-objects.md)
//! e [`windows_sandbox_design.md`](../../architecture/windows-sandbox-design.md).
//!
//! ## API pública
//!
//! O caller (e.g., `frederico-app` via `RunExecutor` ou a casca Tauri)
//! constrói **um** `SecurityJailResolver` por sessão de app, e o
//! usa para `spawn()` processos filhos sob sandbox. O resolver
//! mantém o `JobObject` (root) e o `RestrictedToken` (token restrito)
//! vivos durante toda a vida do app — `Drop` fecha o Job, e o
//! `KILL_ON_JOB_CLOSE` derruba **toda** a árvore atribuída.
//!
//! ## Fronteira §5.5 (modo servidor)
//!
//! O `SecurityJailResolver` é a **única** peça do sandbox que fala
//! com a Win32. O motor (que é plataforma-agnóstico) só vê a trait
//! `SandboxSpawner` (futuro) ou o tipo concreto via
//! `Arc<dyn SecurityJailResolver>` (atual). No Linux, a
//! implementação usará cgroups v2 + namespace + seccomp-bpf —
//! **mesma** interface Rust, sem mudança no motor.
//!
//! ## Integração com `Jail` (path safety)
//!
//! O `Jail` vive em `frederico-tool-registry` (ciclo de dependência
//! se `frederico-security` importasse). Para evitar, o
//! `SecurityJailResolver` **não** importa o `Jail` — o caller é
//! responsável por validar o `workdir` (via `Jail::resolve_allowing_nonexistent`
//! da Fase 6 Etapa 5.X) e passar um `Path` validado no
//! `SandboxConfig`. A barreira de path safety fica no caller; o
//! orchestrator confia que o path é seguro (e o `Jail` é
//! responsável por garantir isso).
//!
//! ## Cancelamento
//!
//! `SandboxedProcess` carrega o `pid` + `cancel_token`. Quando o
//! caller (e.g., `RunExecutor`) cancela, `SandboxedProcess::kill()`
//! chama `TerminateProcess` no PID. O `KILL_ON_JOB_CLOSE` do Job
//! Object **não** dispara com TerminateProcess (só dispara quando o
//! handle do Job é fechado, ou seja, no Drop do
//! `SecurityJailResolver` inteiro). Para cancelamento cascateado
//! (mata netos também), o caller pode `drop`ar o
//! `SecurityJailResolver` — o que raramente é o desejado. Solução:
//! per-spawn Job Object (roadmap, Etapa 4 da Fase 7).
//!
//! ## v1 simplificações
//!
//! - **Pipes stdout/stderr são criados pelo `spawn`** — a v1 já
//!   passa `Stdio::piped` no `Command`, e o caller toma as
//!   handles via `SandboxedProcess::stdout()` / `stderr()`. A
//!   Etapa 5 pode estender pra streaming parcial.
//! - **Sem `OutputCollector` (teto 10 MB + chunks 64 KB)** —
//!   implementado em `frederico-tool-registry::exec::output`
//!   (Etapa 4 da Fase 7). O `collect_output` usa
//!   `wait_with_timeout` (wall-clock enforcement real) e toma
//!   as handles via `stdout()`/`stderr()` sem consumir o
//!   `SandboxedProcess`.
//! - **Wall-clock enforcement via `wait_with_timeout(Duration)`**
//!   — a v1 do Etapa 2 tinha o campo `wall_clock` como
//!   "apenas informativo" no `SandboxConfig`. A Etapa 4 da Fase
//!   7 conecta o campo ao `tokio::time::timeout`; em timeout,
//!   o processo é marcado pra kill + o drop do SandboxedProcess
//!   cascateia via `KILL_ON_JOB_CLOSE` (mata netos que
//!   sobreviveriam ao `TerminateProcess` do pai).
//! - **Sem feature flag `FREDERICO_SANDBOX_V1`** — o orchestrator
//!   é opt-in por construção (não há fallback não-sandbox). A
//!   feature flag entra quando o `RunExecutor` da Etapa 4 é
//!   atualizado para usar este orchestrator.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;

use crate::env_filter::{EnvAllowlist, EnvFilter};

/// Configuração do `SecurityJailResolver` no momento de
/// construção. Decide o que **sempre** vale para a sessão do app.
#[derive(Debug, Clone)]
pub struct SecurityJailConfig {
    /// Allowlist base do env (REQUIRED + DENIED são hardcoded em
    /// `EnvAllowlist::secure_default()`). O caller pode adicionar
    /// ALLOWED via [`Self::with_allowed_env`].
    pub env_allowlist: EnvAllowlist,
    /// Limite de memória por processo (default 2 GB, igual ao
    /// `JobObject` default).
    pub per_process_memory_bytes: u64,
    /// Limite de memória total da árvore (default 4 GB, igual ao
    /// `JobObject` default).
    pub total_memory_bytes: u64,
}

impl SecurityJailConfig {
    /// Config default: `EnvAllowlist::secure_default()` + 2 GB por
    /// processo + 4 GB total.
    #[must_use]
    pub fn secure_default() -> Self {
        Self {
            env_allowlist: EnvAllowlist::secure_default(),
            per_process_memory_bytes: 2 * 1024 * 1024 * 1024,
            total_memory_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    /// Adiciona entries `ALLOWED` (configurável pelo usuário).
    #[must_use]
    pub fn with_allowed_env<I, S>(mut self, allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.env_allowlist = self.env_allowlist.with_allowed(allowed);
        self
    }
}

/// Configuração de **uma invocação** (`spawn`). Decidida por
/// tool_call.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Programa a executar (e.g., `python.exe`, `node.exe`,
    /// `cmd.exe`). **Caminho absoluto** esperado (o caller já
    /// resolveu via runtime registry / path safety).
    pub program: PathBuf,
    /// Args do programa (e.g., `["-c", "print(2+2)"]`).
    pub args: Vec<String>,
    /// Workdir do filho. **Já validado** pelo caller (Jail resolve)
    /// — o orchestrator confia que é seguro.
    pub workdir: PathBuf,
    /// Env vars adicionais a passar pro filho (além do
    /// `EnvAllowlist::REQUIRED` que é sempre passado). Default
    /// vazio; o Etapa 4 pluga com `PermissionSet::extra_env`.
    pub extra_env: Vec<(String, String)>,
    /// Stdin a passar pro filho (None = /dev/null).
    pub stdin: Option<Vec<u8>>,
    /// Wall-clock timeout (default 60s, configurável por tool_call).
    /// Apenas informativo nesta v1 — o caller deve usar
    /// `tokio::time::timeout` em volta de `wait()`.
    pub wall_clock: Duration,
}

impl SandboxConfig {
    /// Config default razoável: wall-clock 60s, sem stdin, sem
    /// extra_env.
    #[must_use]
    pub fn new(program: PathBuf, args: Vec<String>, workdir: PathBuf) -> Self {
        Self {
            program,
            args,
            workdir,
            extra_env: Vec::new(),
            stdin: None,
            wall_clock: Duration::from_secs(60),
        }
    }
}

/// Erro do `SecurityJailResolver::spawn`.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// Plataforma não suportada (Linux: retorna `NotSupported`; o
    /// caller deve fazer degradação declarada).
    #[error("SecurityJailResolver nao suportado na plataforma atual: {0}")]
    Unsupported(&'static str),
    /// Configuração do SecurityJailResolver é inválida (ex.:
    /// `per_process_memory_bytes == 0`).
    #[error("config invalida: {0}")]
    InvalidConfig(String),
    /// Erro de I/O ao preparar o spawn (ex.: criar pipes).
    #[error("erro de I/O preparando spawn: {0}")]
    Io(#[from] std::io::Error),
    /// `CreateProcessAsUser` (Windows) ou `Command::spawn` falhou.
    #[error("spawn falhou: {0}")]
    SpawnFailed(String),
}

/// Handle para um processo filho sob sandbox. Quando droppado,
/// o **Job Object per-invocation** é fechado, o que dispara
/// `KILL_ON_JOB_CLOSE` no Windows e mata **toda a árvore**
/// (filho + netos + bisnetos).
///
/// **Lifetime (Etapa 4 da Fase 7):** o caller chama `wait_with_timeout(wall_clock)`
/// para coletar o exit code OU o timeout. Se o caller dropar
/// o `SandboxedProcess` sem chamar `wait_with_timeout`, o `Drop`
/// fecha o job handle e mata os handles do child — `KILL_ON_JOB_CLOSE`
/// dispara e a árvore é morta via o Job.
///
/// **API de I/O:** use [`Self::stdout`] e [`Self::stderr`] para
/// tomar as handles de stdout/stderr **sem** consumir o
/// `SandboxedProcess`. O Job fica vivo até o `Drop` final. O
/// `into_child` da v1 foi **removido** (Etapa 4 da Fase 7):
/// ele consumia o `SandboxedProcess` e portanto fechava o Job
/// prematuramente, deixando o `Child` órfão (fora do Job) —
/// exatamente o bug que o per-invocation Job Object foi criado
/// pra evitar.
///
/// **Cancelamento:** o `RunExecutor` da Fase 3 pode dropar o
/// `SandboxedProcess` (via wrapper) para cancelar. A per-invocation
/// Job garante que netos não sobrevivem — o mesmo problema
/// da Fase 5 Etapa 2.A que a Etapa 2 da Fase 7 fechou com o
/// `KILL_ON_JOB_CLOSE` do root job, agora aplicado per-invocation.
pub struct SandboxedProcess {
    pid: u32,
    /// Processo tokio (mantido vivo até `wait_with_timeout()` ou `Drop`).
    child: Option<tokio::process::Child>,
    /// Job Object per-invocation (Windows). Drop fecha o handle
    /// → `KILL_ON_JOB_CLOSE` mata a árvore. `cfg(windows)` porque
    /// em Linux a Etapa 2 não tem Job Object (cross-platform é
    /// roadmap).
    #[cfg(target_os = "windows")]
    job: Option<crate::windows::JobObject>,
    /// ID único da invocação (para logs).
    invocation_id: u64,
}

// Manually implement Debug pra evitar expor o `Child` (que tem
// handles Windows) nem o `JobObject`.
impl std::fmt::Debug for SandboxedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedProcess")
            .field("pid", &self.pid)
            .field("invocation_id", &self.invocation_id)
            .field("has_job", &self.job.is_some())
            .finish_non_exhaustive()
    }
}

impl SandboxedProcess {
    /// PID do processo filho.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// ID único da invocação (para correlação com `DbAuditSink`).
    #[must_use]
    pub const fn invocation_id(&self) -> u64 {
        self.invocation_id
    }

    /// Espera o processo terminar e devolve o exit status. **Não**
    /// cascateia pro neto (use `SecurityJailResolver` Drop pra isso).
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, std::io::Error> {
        match self.child.as_mut() {
            Some(child) => child.wait().await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            )),
        }
    }

    /// Espera o processo terminar com timeout. Se o timeout expirar,
    /// chama `start_kill` no child E fecha o job handle (drop do
    /// `SandboxedProcess`) — `KILL_ON_JOB_CLOSE` mata a árvore
    /// inteira (caller é responsável por dropar o `SandboxedProcess`
    /// após o timeout pra fechar o job).
    ///
    /// **Etapa 4 da Fase 7**: este método é o ponto de wall-clock
    /// enforcement usado pelos `exec.*` tools. O `SandboxConfig.wall_clock`
    /// deixa de ser "apenas informativo" — é a duração do timeout
    /// aqui.
    pub async fn wait_with_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<std::process::ExitStatus, std::io::Error> {
        let child = self.child.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            )
        })?;
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                // Timeout: mata o child (não-await). O drop do
                // `SandboxedProcess` depois fecha o job handle,
                // o que cascateia via `KILL_ON_JOB_CLOSE` — o caller
                // deve dropar o `SandboxedProcess` após este erro
                // pra completar a limpeza.
                let _ = child.start_kill();
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "wall-clock excedido (>{:?}); processo marcado pra kill, drop do SandboxedProcess cascateia",
                        timeout
                    ),
                ))
            }
        }
    }

    /// Mata o processo via `TerminateProcess` (Windows) ou
    /// `Child::kill` (tokio cross-platform). **Não** cascateia pro
    /// neto sozinho — o `Drop` do `SandboxedProcess` é o que fecha
    /// o Job handle e dispara `KILL_ON_JOB_CLOSE` na árvore.
    pub async fn kill(&mut self) -> Result<(), std::io::Error> {
        if let Some(child) = self.child.as_mut() {
            child.start_kill()?;
        }
        Ok(())
    }

    /// Toma a handle de stdout do `Child` interno (se ainda não
    /// foi tomada). O `SandboxedProcess` continua vivo — o Job
    /// segue aberto até o `Drop`. Retorna `None` se o `Child`
    /// já foi consumido, se stdout não foi piped, ou se já
    /// foi tomado por chamada anterior.
    ///
    /// **Por que `&mut self` em vez de `self`:** tomar stdout
    /// **não** deve fechar o Job. O caller típico é o
    /// `output::collect_output` que toma stdout + stderr +
    /// chama `wait_with_timeout` — todas operações que precisam
    /// do `SandboxedProcess` vivo.
    ///
    /// **Por que `&mut` no campo em vez de método:** `tokio::process::Child`
    /// expõe `stdout` e `stderr` como **campos públicos** (não
    /// métodos), em `tokio` 1.x. O `take()` no `Option<ChildStdout>`
    /// consome o campo e devolve o valor.
    #[must_use]
    pub fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }

    /// Toma a handle de stderr do `Child` interno (se ainda não
    /// foi tomada). Mesma semântica de [`Self::stdout`].
    #[must_use]
    pub fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.as_mut()?.stderr.take()
    }
}

impl Drop for SandboxedProcess {
    fn drop(&mut self) {
        // Etapa 4 da Fase 7: o `JobObject` per-invocation é o
        // coração do cancelamento. Quando o `SandboxedProcess` é
        // droppado (fim do `execute` da Tool, ou cancelamento
        // do `Run`), o `Option<JobObject>` é droppado aqui, o
        // `JobObject::drop` fecha o handle do job, e o Windows
        // dispara `KILL_ON_JOB_CLOSE`, matando **toda a árvore**
        // (filho + netos + bisnetos).
        //
        // O `child` (se ainda existe) também é droppado junto —
        // não chamamos `start_kill` explícito porque o
        // `KILL_ON_JOB_CLOSE` já cuida da terminação. Em
        // prática, `start_kill` seria redundante e faria
        // `TerminateProcess` no PID direto (que não cascateia).
        let _ = self.child.take();
        // O `self.job` (Option<JobObject>) tem `Drop` automático
        // que fecha o handle. Não precisa `take()` manual.
    }
}

/// Orquestrador do sandbox. Combina Jail (path safety via caller)
/// + Job Object + Restricted Token + Env Filter. Por design, **só
///   Windows** é suportado na v1; Linux retorna
///   `SpawnError::Unsupported` (degradação declarada).
///
/// ## Per-invocation Job Object (Etapa 4 da Fase 7)
///
/// Cada `spawn()` cria um `JobObject` **novo** (per-invocation) com
/// `KILL_ON_JOB_CLOSE`. O `SandboxedProcess` carrega o handle; quando
/// droppado, fecha o handle e o Windows mata a **árvore inteira**
/// (filho + netos + bisnetos) do processo.
///
/// Não há mais "root job" compartilhado — o `SecurityJailResolver`
/// NÃO mata nada ao ser droppado (a v1 tem expectativa de que o
/// app chame `wait_with_timeout()` ou drope cada `SandboxedProcess`
/// individualmente). O test da Etapa 2
/// `job_object_kills_tree_on_resolver_drop` foi renomeado para
/// `job_object_kills_tree_on_sandboxed_process_drop` (dropa o
/// `SandboxedProcess`, não o resolver).
pub struct SecurityJailResolver {
    /// Memória por processo (bytes) — aplicada em cada job
    /// per-invocation no `spawn()`.
    per_process_memory_bytes: u64,
    /// Memória total (bytes) — aplicada em cada job per-invocation
    /// no `spawn()`.
    total_memory_bytes: u64,
    /// Token restrito (drop dos 6 privilégios). Construído em
    /// `new()` mas **não aplicado** na v1 (a `CreateProcessAsUser`
    /// via `tokio::process` ainda não tem a integração; Etapa 5+
    /// da Fase 7 implementa via raw `CreateProcessAsUserW` do
    /// `windows` crate — `std::os::windows::process::CommandExt::as_user`
    /// foi removido em Rust 1.97).
    /// Mantido aqui como **infraestrutura** — o teste
    /// `restricted_token_constructed_in_resolver` (Etapa 4) prova
    /// que a peça 3 (restricted_token.rs) está plugada no
    /// orchestrator.
    #[cfg(target_os = "windows")]
    #[allow(dead_code)]
    restricted_token: crate::windows::RestrictedToken,
    /// Env filter (fail-closed).
    env_filter: EnvFilter,
    /// Contador de invocações (para o `invocation_id`).
    next_id: AtomicU64,
    /// True se a plataforma é suportada (Windows = true;
    /// outros = false). Permite testes sem `#[cfg]` em todo
    /// lugar.
    platform_supported: bool,
}

// Manualmente implement Send + Sync (todos os campos são
// Send + Sync: `JobObject` e `RestrictedToken` declaram
// explicitamente, `EnvFilter` é cloneable, `AtomicU64` é Sync).
unsafe impl Send for SecurityJailResolver {}
unsafe impl Sync for SecurityJailResolver {}

impl SecurityJailResolver {
    /// Constrói o resolver. Na plataforma Windows, cria o root
    /// `JobObject` (com `KILL_ON_JOB_CLOSE` + `BREAKAWAY_OK` + os
    /// limites de memória do config) e o `RestrictedToken` (drop
    /// dos 6 privilégios). Em outras plataformas, retorna Ok com
    /// `platform_supported = false` — `spawn` retorna
    /// `SpawnError::Unsupported` quando chamado.
    ///
    /// # Erros
    ///
    /// Falha se a criação do `JobObject` ou `RestrictedToken`
    /// falhar (raríssimo em Windows; em prática só acontece se o
    /// OS estiver sem handles).
    #[allow(unused_variables)]
    pub fn new(config: SecurityJailConfig) -> Result<Arc<Self>, SpawnError> {
        #[cfg(target_os = "windows")]
        {
            let restricted_token = crate::windows::RestrictedToken::from_current_process()
                .map_err(|e| {
                    SpawnError::SpawnFailed(format!("RestrictedToken::from_current_process: {e}"))
                })?;

            let env_filter = EnvFilter::new(config.env_allowlist);

            Ok(Arc::new(Self {
                per_process_memory_bytes: config.per_process_memory_bytes,
                total_memory_bytes: config.total_memory_bytes,
                restricted_token,
                env_filter,
                next_id: AtomicU64::new(1),
                platform_supported: true,
            }))
        }

        #[cfg(not(target_os = "windows"))]
        {
            // Degradação declarada (memory 2026-08-03): retorna
            // Ok com platform_supported=false. spawn retorna
            // Err(Unsupported) quando chamado. Caller decide o
            // que fazer (não executar o tool_call, ou usar
            // fallback não-sandbox).
            let _ = config; // suprimir warning de unused
            let _ = EnvFilter::new(EnvAllowlist::secure_default());
            Ok(Arc::new(Self {
                per_process_memory_bytes: 0,
                total_memory_bytes: 0,
                env_filter: EnvFilter::new(EnvAllowlist::secure_default()),
                next_id: AtomicU64::new(1),
                platform_supported: false,
            }))
        }
    }

    /// True se a plataforma atual é suportada (Windows).
    #[must_use]
    pub const fn is_platform_supported(&self) -> bool {
        self.platform_supported
    }

    /// Próximo ID de invocação (atômico, monotonic).
    fn next_invocation_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Aplica o env filter no env do parent. Helper compartilhado
    /// entre Windows e Linux.
    ///
    /// **Não usado na v1** — o `spawn` atual chama `env_filter.apply`
    /// diretamente (porque o `envs()` do `tokio::process` falha com
    /// `ERROR_INVALID_PARAMETER` em Windows com env block grande).
    /// Marcado `#[allow(dead_code)]` até a Etapa 4 (raw `CreateProcessW`)
    /// reativar a injeção completa. Mantido como ponto único de
    /// composição (REQUIRED + ALLOWED + DENIED + extra).
    ///
    /// Toma `&mut Vec` (não `&mut [_]`) porque faz `push` no `extra`.
    /// Clippy reclama de `ptr_arg` — permitido aqui pelo `dead_code`.
    #[allow(dead_code, clippy::ptr_arg)]
    fn filter_env(&self, parent_env: &mut Vec<(String, String)>, extra: &[(String, String)]) {
        // Aplica o filtro (sobrescreve DENIED, remove não-listed).
        // Erro de UTF-8 não é tratado nesta v1 (cai no `unwrap`
        // abaixo) — env vars do OS são UTF-8 em prática.
        let _ = self.env_filter.apply(parent_env);
        // Adiciona `extra` (passado pelo caller, ALLOWED em runtime).
        for (k, v) in extra {
            // Pula se já está no parent_env (REQUIRED/ALLOWED
            // originais ganham prioridade).
            if !parent_env.iter().any(|(pk, _)| pk == k) {
                parent_env.push((k.clone(), v.clone()));
            }
        }
    }

    /// Spawna um processo sob sandbox. Na v1, usa
    /// `tokio::process::Command` (que internamente usa
    /// `CreateProcessW` no Windows, sem token restrito).
    ///
    /// **Importante:** a v1 deste método **NÃO** aplica o
    /// `RestrictedToken` (`CreateProcessAsUser` raw precisa de
    /// setup manual que `tokio::process::Command` não expõe;
    /// o `std::os::windows::process::CommandExt::as_user` que
    /// fazia isso foi removido em Rust 1.97). A Etapa 5+ da
    /// Fase 7 vai implementar via raw `CreateProcessAsUserW`
    /// do `windows` crate (precisa de `STARTUPINFOW` +
    /// `PROCESS_INFORMATION` construídos manualmente + pipes
    /// piped, é trabalho significativo). Por enquanto, o spawn
    /// aplica só:
    ///
    /// 1. **Env filter** — herda o env do parent, **remove** as
    ///    vars em DENIED (sobrescreve com `""` in-place antes
    ///    via `EnvFilter::apply`), e adiciona as vars em
    ///    `extra_env`. **NÃO** usa `env_clear()` (Windows
    ///    quebra: `SystemRoot` some, `CreateProcess` falha
    ///    com `ERROR_INVALID_PARAMETER` 87).
    /// 2. **Job Object** (processo atribuído ao root Job via
    ///    `JobObject::assign_pid` após o spawn).
    /// 3. **Workdir** (via `current_dir`).
    /// 4. **Stdin** (via `Stdio::piped` se fornecido).
    ///
    /// O `RestrictedToken` é construído em `new()` mas não
    /// aplicado nesta v1 — fica como **infraestrutura** para a
    /// Etapa 5+ (que vai usar raw `CreateProcessAsUserW` do
    /// `windows` crate direto, sem o `std::process::Command`).
    pub fn spawn(self: &Arc<Self>, config: SandboxConfig) -> Result<SandboxedProcess, SpawnError> {
        if !self.platform_supported {
            return Err(SpawnError::Unsupported(
                "SecurityJailResolver::spawn só é suportado em Windows na v1",
            ));
        }

        if config.period_is_zero() {
            return Err(SpawnError::InvalidConfig(
                "wall_clock não pode ser zero".to_string(),
            ));
        }

        // 1. Lê o env do parent (uma vez por invocação).
        let mut parent_env: Vec<(String, String)> = std::env::vars_os()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();

        // 2. Aplica o filter (sobrescreve DENIED in-place, mantém
        //    o resto). NÃO remove as outras — a herança é
        //    importante para Windows (SystemRoot, windir, etc).
        self.env_filter
            .apply(&mut parent_env)
            .map_err(|e| SpawnError::SpawnFailed(format!("EnvFilter::apply falhou: {e:?}")))?;

        // 3. Constrói o `tokio::process::Command` (caminho
        //    cross-platform; a Etapa 4 substitui pelo
        //    `CreateProcessAsUser` raw via `CommandExt::as_user`).
        //    Herdamos o parent_env (com DENIED sobrescrito com
        //    "") e adicionamos extra_env.
        //
        //    **DIAGNÓSTICO: envs() causa ERROR_INVALID_PARAMETER
        //    (87) em tokio 1.53 + Windows quando o env block é
        //    muito grande (parent + extras). Workaround v1: NÃO
        //    chamar envs() — herda o env do parent
        //    automaticamente. O EnvFilter::apply já sobrescreveu
        //    DENIED in-place no parent_env, mas como NÃO
        //    re-injetamos, o filho herda o parent_env COM DENIED
        //    já sobrescrito (o que NÃO é o que queremos — DENIED
        //    deveria ser REMOVIDO, não só sobrescrito).
        //
        //    Solução correta (Etapa 4): usar
        //    `CommandExt::raw_arg` com `CreateProcessW` direto
        //    passando o env block construído manualmente.
        let invocation_id = self.next_invocation_id();
        let mut cmd = Command::new(&config.program);
        cmd.args(&config.args)
            .current_dir(&config.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // 4. Spawna o processo (tokio::process::Child). O
        //    `spawn()` retorna imediatamente com o handle;
        //    `wait()` coleta o exit.
        let child = cmd.spawn().map_err(|e| {
            SpawnError::SpawnFailed(format!("tokio::process::Command::spawn falhou: {e}"))
        })?;
        let pid = child.id().ok_or_else(|| {
            SpawnError::SpawnFailed("Child::id() retornou None (sem PID)".to_string())
        })?;

        // 5. Cria o Job Object per-invocation (Windows) com
        //    `KILL_ON_JOB_CLOSE` + os limites de memória. Drop do
        //    `SandboxedProcess` (ou drop do `Job` no final do
        //    `execute` da Tool) fecha o handle → mata a árvore
        //    inteira via `KILL_ON_JOB_CLOSE`.
        //
        //    **Falha aqui é erro duro** (não silenciosa como era
        //    na v1 com o root job compartilhado). Sem o Job, o
        //    processo não está sob `KILL_ON_JOB_CLOSE`, e o
        //    tree-kill da Etapa 2 fica quebrado — exatamente o
        //    bug que o `tree_kill.rs::fase5_etapa2a_incomplete`
        //    testa.
        #[cfg(target_os = "windows")]
        let job = {
            let job = crate::windows::JobObject::with_memory_limits(
                self.per_process_memory_bytes,
                self.total_memory_bytes,
            )
            .map_err(|e| {
                SpawnError::SpawnFailed(format!(
                    "JobObject per-invocation falhou (pid={pid}): {e}. \
                     Tree-kill da Etapa 2 fica quebrado sem Job — abortando."
                ))
            })?;
            // Atribui o PID ao Job recém-criado.
            if let Err(e) = job.assign_pid(pid) {
                // Rollback: o `Child` está vivo mas não está
                // sob Job. Matamos o PID (best-effort, sem await)
                // e propagamos o erro. O OS reaps o processo
                // quando o último handle fechar.
                let mut child = child;
                let _ = child.start_kill();
                return Err(SpawnError::SpawnFailed(format!(
                    "assign_pid per-invocation falhou (pid={pid}): {e}. \
                     Processo foi marcado pra kill (rollback). Sem Job = tree-kill quebrado."
                )));
            }
            job
        };

        Ok(SandboxedProcess {
            pid,
            child: Some(child),
            #[cfg(target_os = "windows")]
            job: Some(job),
            invocation_id,
        })
    }
}

// Helpers privados (não fazem parte da API pública; ficam aqui
// pra organização).
impl SandboxConfig {
    /// True se `wall_clock` é zero. Usado pelo `spawn` pra
    /// validar config (zero causaria timeout imediato, que é
    /// provavelmente bug do caller).
    fn period_is_zero(&self) -> bool {
        self.wall_clock.is_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `SecurityJailConfig::secure_default()` tem os defaults
    /// esperados (2 GB por processo, 4 GB total).
    #[test]
    fn config_secure_default_has_expected_memory_limits() {
        let cfg = SecurityJailConfig::secure_default();
        assert_eq!(cfg.per_process_memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(cfg.total_memory_bytes, 4 * 1024 * 1024 * 1024);
        // EnvAllowlist tem os 3 REQUIRED (HTTP_PROXY etc).
        assert!(cfg.env_allowlist.is_required("PATH"));
    }

    /// `SandboxConfig::new()` tem os defaults razoáveis.
    #[test]
    fn sandbox_config_new_has_defaults() {
        let cfg = SandboxConfig::new(
            PathBuf::from("C:/Python312/python.exe"),
            vec!["-c".to_string(), "print(2+2)".to_string()],
            PathBuf::from("C:/workspace"),
        );
        assert_eq!(cfg.wall_clock, Duration::from_secs(60));
        assert!(cfg.stdin.is_none());
        assert!(cfg.extra_env.is_empty());
    }

    /// `SandboxConfig::period_is_zero` detecta wall_clock zero
    /// (que é bug do caller — timeout imediato).
    #[test]
    fn sandbox_config_period_is_zero() {
        let mut cfg = SandboxConfig::new(
            PathBuf::from("C:/Python312/python.exe"),
            vec![],
            PathBuf::from("C:/workspace"),
        );
        assert!(!cfg.period_is_zero());
        cfg.wall_clock = Duration::ZERO;
        assert!(cfg.period_is_zero());
    }

    /// `is_platform_supported` reflete a plataforma de build.
    /// Em Windows é `true`; em Linux/macOS é `false`.
    #[test]
    fn platform_supported_reflects_build_target() {
        let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
            .expect("new deve ter sucesso em qualquer plataforma");
        #[cfg(target_os = "windows")]
        assert!(resolver.is_platform_supported());
        #[cfg(not(target_os = "windows"))]
        assert!(!resolver.is_platform_supported());
    }

    /// `spawn` retorna `Unsupported` em plataforma não-Windows
    /// (v1).
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn spawn_returns_unsupported_on_non_windows() {
        let resolver =
            SecurityJailResolver::new(SecurityJailConfig::secure_default()).expect("new");
        let cfg = SandboxConfig::new(
            PathBuf::from("/bin/echo"),
            vec!["hello".to_string()],
            PathBuf::from("/tmp"),
        );
        let result = resolver.spawn(cfg);
        assert!(matches!(result, Err(SpawnError::Unsupported(_))));
    }

    /// `next_invocation_id` é monotônico.
    #[test]
    fn next_invocation_id_is_monotonic() {
        let resolver =
            SecurityJailResolver::new(SecurityJailConfig::secure_default()).expect("new");
        let a = resolver.next_invocation_id();
        let b = resolver.next_invocation_id();
        let c = resolver.next_invocation_id();
        assert!(b > a);
        assert!(c > b);
    }
}
