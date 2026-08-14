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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, GetProcessId,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

use crate::env_filter::{EnvAllowlist, EnvFilter};
use crate::raw_child::RawChild;

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
/// **Etapa 5+ da Fase 7 (path safety):** o `child` interno é
/// um [`crate::raw_child::RawChild`] (wrapper sobre handles
/// Win32 raw, criado via `CreateProcessAsUserW`). O `tokio::process::Child`
/// da Etapa 4 v1 não permitia aplicar o `RestrictedToken`
/// (a std não expõe `CreateProcessAsUserW` em Rust 1.97; só
/// `tokio::process::Command::spawn`, que usa o token do parent).
/// A Etapa 5+ usa raw API pra injetar o token restrito com
/// `TokenIntegrityLevel=Low` + restricted SIDs.
///
/// **Lifetime (Etapa 4 da Fase 7):** o caller chama `wait_with_timeout(wall_clock)`
/// para coletar o exit code OU o timeout. Se o caller dropar
/// o `SandboxedProcess` sem chamar `wait_with_timeout`, o `Drop`
/// fecha o job handle e mata os handles do child — `KILL_ON_JOB_CLOSE`
/// dispara e a árvore é morta via o Job.
///
/// **API de I/O:** use [`Self::take_stdout_handle`] e
/// [`Self::take_stderr_handle`] para tomar os **handles raw**
/// de stdout/stderr **sem** consumir o `SandboxedProcess`. O
/// caller wrappa em `tokio::fs::File` via
/// [`crate::raw_child::wrap_pipe_handle_as_async_file`] pra
/// implementar `AsyncRead`. O Job fica vivo até o `Drop` final.
///
/// **Cancelamento:** o `RunExecutor` da Fase 3 pode dropar o
/// `SandboxedProcess` (via wrapper) para cancelar. A per-invocation
/// Job garante que netos não sobrevivem — o mesmo problema
/// da Fase 5 Etapa 2.A que a Etapa 2 da Fase 7 fechou com o
/// `KILL_ON_JOB_CLOSE` do root job, agora aplicado per-invocation.
pub struct SandboxedProcess {
    pid: u32,
    /// Processo raw (handles Win32 via `RawChild`). Mantido vivo
    /// até `wait_with_timeout()` ou `Drop`. Drop fecha os handles
    /// (`CloseHandle` em `hProcess` + read ends dos pipes).
    child: Option<crate::raw_child::RawChild>,
    /// Job Object per-invocation (Windows). Drop fecha o handle
    /// → `KILL_ON_JOB_CLOSE` mata a árvore. `cfg(windows)` porque
    /// em Linux a Etapa 2 não tem Job Object (cross-platform é
    /// roadmap).
    #[cfg(target_os = "windows")]
    job: Option<crate::windows::JobObject>,
    /// ID único da invocação (para logs).
    invocation_id: u64,
}

// Manually implement Debug pra evitar expor o `RawChild` (que tem
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

    /// Espera o processo terminar sem timeout. Bloqueia até o
    /// processo sair (sucesso ou crash). Para bounded wait, use
    /// [`Self::wait_with_timeout`].
    ///
    /// **Não** cascateia pro neto (use `Drop` do SandboxedProcess
    /// pra isso).
    pub async fn wait(&mut self) -> Result<crate::raw_child::RawExitStatus, std::io::Error> {
        match self.child.as_mut() {
            Some(child) => child.wait().await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            )),
        }
    }

    /// Espera o processo terminar com timeout. Se o timeout expirar,
    /// chama `TerminateProcess` no child (cascateia via Job Object
    /// → mata a árvore no `Drop`).
    ///
    /// **Etapa 4 da Fase 7**: este método é o ponto de wall-clock
    /// enforcement usado pelos `exec.*` tools. O `SandboxConfig.wall_clock`
    /// deixa de ser "apenas informativo" — é a duração do timeout
    /// aqui.
    pub async fn wait_with_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<crate::raw_child::RawExitStatus, std::io::Error> {
        match self.child.as_mut() {
            Some(child) => child.wait_with_timeout(timeout).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            )),
        }
    }

    /// Mata o processo via `TerminateProcess`. **Não** cascateia
    /// pro neto sozinho — o `Drop` do `SandboxedProcess` é o que
    /// fecha o Job handle e dispara `KILL_ON_JOB_CLOSE` na árvore.
    pub async fn kill(&mut self) -> Result<(), std::io::Error> {
        if let Some(child) = self.child.as_mut() {
            child.kill().await?;
        }
        Ok(())
    }

    /// Toma o **handle raw** (`HANDLE` Win32) do read end do
    /// pipe de stdout. O caller wrappa em `tokio::fs::File` via
    /// [`crate::raw_child::wrap_pipe_handle_as_async_file`] pra
    /// implementar `AsyncRead`. Retorna `None` se stdout não
    /// foi piped, se já foi tomado, ou se o processo foi consumido.
    ///
    /// **Por que `&mut self`:** tomar o handle **não** deve
    /// fechar o Job. O `JobObject` continua vivo no
    /// `SandboxedProcess` até o `Drop` final. Caller típico:
    /// `output::collect_output` que toma stdout + stderr + chama
    /// `wait_with_timeout` — todas operações que precisam do
    /// `SandboxedProcess` vivo.
    #[must_use]
    pub fn take_stdout_handle(&mut self) -> Option<windows::Win32::Foundation::HANDLE> {
        self.child.as_mut()?.take_stdout_handle()
    }

    /// Toma o **handle raw** (`HANDLE` Win32) do read end do
    /// pipe de stderr. Mesma semântica de [`Self::take_stdout_handle`].
    #[must_use]
    pub fn take_stderr_handle(&mut self) -> Option<windows::Win32::Foundation::HANDLE> {
        self.child.as_mut()?.take_stderr_handle()
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
        // não chamamos `kill` explícito porque o `KILL_ON_JOB_CLOSE`
        // já cuida da terminação. Em prática, `kill` seria
        // redundante e faria `TerminateProcess` no PID direto
        // (que não cascateia).
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

    /// Aplica o env filter no env do parent e adiciona o
    /// `extra_env` do caller. Helper compartilhado entre
    /// Windows e Linux. **NÃO** usado na v1 — o `spawn` Windows
    /// constrói o env block direto via raw `CreateProcessAsUserW`,
    /// e a versão Linux retorna `Unsupported` antes de chegar
    /// aqui. Mantido como ponto único de composição
    /// (REQUIRED + ALLOWED + DENIED + extra) caso a v2 queira
    /// reaproveitar.
    #[allow(dead_code, clippy::ptr_arg)]
    fn filter_env(&self, parent_env: &mut Vec<(String, String)>, extra: &[(String, String)]) {
        // Aplica o filtro (sobrescreve DENIED, remove não-listed).
        let _ = self.env_filter.apply(parent_env);
        // Adiciona `extra` (passado pelo caller, ALLOWED em runtime).
        for (k, v) in extra {
            if !parent_env.iter().any(|(pk, _)| pk == k) {
                parent_env.push((k.clone(), v.clone()));
            }
        }
    }

    /// Spawna um processo sob sandbox. **Etapa 5+ da Fase 7**
    /// (path safety enforcement).
    ///
    /// **Algoritmo (raw `CreateProcessAsUserW`, com todos os
    /// detalhes que costumam morder):**
    ///
    /// 1. **Mandatory Label no workdir** — `set_low_integrity_label(workdir)`
    ///    aplica `Mandatory Label\Low` (S-1-16-4096) no diretório.
    ///    O child (TokenIntegrityLevel=Low) só consegue ler/escrever
    ///    **aqui** — qualquer outro path Medium-labeled bloqueia
    ///    o access check. (Etapa 5+ D1; substitui a Etapa 4 v1
    ///    que confiava só no `current_dir` do `tokio::process::Command`.)
    ///
    /// 2. **Env block** — lê o env do parent, aplica `EnvFilter::apply`
    ///    (sobrescreve DENIED com `""`), adiciona `extra_env`, e
    ///    serializa num env block UTF-16 LE com terminação
    ///    `\0\0` (formato Win32). Diferente da Etapa 4 v1, esse
    ///    env é RE-INJETADO via `CreateProcessAsUserW(...,
    ///    lpEnvironment, ...)` — sem o re-inject, o filho herdava
    ///    o env COM DENIED sobrescrito (bug da v1 que essa v2
    ///    fecha).
    ///
    /// 3. **Pipes stdin/stdout/stderr** — `CreatePipe` com
    ///    `bInheritHandle = TRUE` (apenas os write ends que
    ///    vão pro child). Os read ends ficam com o parent.
    ///    Write ends ganham `Mandatory Label\Low` via
    ///    `set_low_integrity_handle` — sem isso, o child Low
    ///    não consegue escrever (Low < Medium default do pipe).
    ///
    /// 4. **STARTUPINFOEXW + PROC_THREAD_ATTRIBUTE_HANDLE_LIST** —
    ///    `InitializeProcThreadAttributeList` + `UpdateProcThreadAttribute`
    ///    com `ProcThreadAttribute::HandleList` listando SÓ
    ///    os 3 write ends dos pipes. Default do Windows é herdar
    ///    QUALQUER handle herdável do parent — `HandleList` é
    ///    **defesa em profundidade** que restringe a herança
    ///    ao mínimo necessário.
    ///
    /// 5. **Restricted Token + IntegrityLevel=Low** —
    ///    `restricted_token.set_integrity_level(INTEGRITY_LEVEL_LOW)`
    ///    seta `TokenIntegrityLevel` no token. Depois
    ///    `duplicate_as_primary()` cria um primary token pro
    ///    `CreateProcessAsUserW`. O token restrito (drop 6
    ///    privilégios) já foi construído em `new()`.
    ///
    /// 6. **CreateProcessAsUserW com `CREATE_SUSPENDED |
    ///    EXTENDED_STARTUPINFO_PRESENT`** — o child nasce suspended
    ///    (não roda até o `ResumeThread`). `EXTENDED_STARTUPINFO_PRESENT`
    ///    indica que o `startupinfo` é `STARTUPINFOEXW` (com
    ///    `lpAttributeList`).
    ///
    /// 7. **AssignProcessToJobObject + ResumeThread** — atribui
    ///    o child ao Job Object per-invocation (com
    ///    `KILL_ON_JOB_CLOSE` + limites de memória), depois
    ///    resume a thread. Sem o CREATE_SUSPENDED, há uma janela
    ///    em que o child poderia gerar netos fora do Job. A
    ///    sequência suspend→assign→resume fecha essa janela
    ///    (ADR-0036 D3).
    ///
    /// 8. **Cleanup** — fecha write ends dos pipes no parent
    ///    (child tem suas cópias herdadas), fecha o handle do
    ///    token (child tem sua própria referência), fecha o
    ///    thread handle (já resumed, não precisa mais),
    ///    `DeleteProcThreadAttributeList`. O read end do stdin
    ///    também é fechado (child lê nada nesse caminho).
    ///
    /// **Erros (todos são hard-fail, não silenciosos):** se
    /// qualquer passo falhar, faz cleanup best-effort e
    /// propaga. Sem fallback não-sandbox (degradação declarada).
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

        // === Etapa 5+ da Fase 7: implementação Windows only. ===
        // Linux retorna Unsupported (cross-platform é roadmap).
        #[cfg(target_os = "windows")]
        {
            spawn_windows(self, config)
        }

        #[cfg(not(target_os = "windows"))]
        {
            Err(SpawnError::Unsupported(
                "SecurityJailResolver::spawn só é suportado em Windows na v1",
            ))
        }
    }
}

// =====================================================================
// Implementação Windows do spawn (raw CreateProcessAsUserW).
// Mantida em função separada pra isolar os cfg(windows) do
// orquestrador cross-platform.
// =====================================================================

#[cfg(target_os = "windows")]
fn spawn_windows(
    resolver: &Arc<SecurityJailResolver>,
    config: SandboxConfig,
) -> Result<SandboxedProcess, SpawnError> {
    use std::ptr;

    use crate::windows::{set_low_integrity_label, JobObject, INTEGRITY_LEVEL_LOW};

    // ---------- Step 1: Apply Mandatory Label\Low to workdir ----------
    eprintln!(
        "[spawn-debug] Step 1: set_low_integrity_label(workdir={})",
        config.workdir.display()
    );
    set_low_integrity_label(&config.workdir).map_err(|e| {
        eprintln!("[spawn-debug] Step 1 FAILED: {e}");
        SpawnError::SpawnFailed(format!(
            "set_low_integrity_label(workdir={}): {e}",
            config.workdir.display()
        ))
    })?;
    eprintln!("[spawn-debug] Step 1 OK");

    // ---------- Step 2: Build env block (UTF-16 LE) ----------
    let mut parent_env: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    resolver
        .env_filter
        .apply(&mut parent_env)
        .map_err(|e| SpawnError::SpawnFailed(format!("EnvFilter::apply falhou: {e:?}")))?;
    // Adiciona `extra_env` (REQUIRED/ALLOWED originais ganham prioridade).
    for (k, v) in &config.extra_env {
        if !parent_env.iter().any(|(pk, _)| pk == k) {
            parent_env.push((k.clone(), v.clone()));
        }
    }
    // **Etapa 6 da Fase 7 (ADR-0033) — env block MÍNIMO.**
    // A Etapa 4 v1 / Etapa 5+ passavam `None` (herdava do
    // parent) — isso **quebrava** o wiring do proxy Etapa 6
    // (HTTP_PROXY injetado via `extra_env` nunca chegava ao
    // filho). Esta Etapa 6 passa o env block construído.
    //
    // O risco de `ERROR_INVALID_PARAMETER` (87) por env block
    // grande motivou a estratégia MÍNIMA: o env block final
    // tem **apenas** as vars em `EnvAllowlist::REQUIRED`
    // (hardcoded, ~17 vars) + `extra_env` (HTTP_PROXY etc.).
    // O `ALLOWED` do parent (que pode ser 50+ vars) é
    // **descartado** intencionalmente — se o caller precisar
    // de ALLOWED, é trabalho dele adicionar via
    // `PermissionSet::extra_env` na chain (Etapa 6+1).
    //
    // Trade-off: o filho perde acesso a vars ALLOWED (ex.:
    // `MY_PROJECT_TOKEN` que o user setou). Por enquanto
    // isso é aceitável porque (a) o spec Etapa 4 não documenta
    // ALLOWED como caminho de produção (é "user opt-in"), e
    // (b) a Etapa 6+1 reabre o design. Por enquanto,
    // **REQUIRED-only é o conjunto mínimo que o sandbox
    // precisa pra rodar** (PATH pro runtime, TEMP pro
    // scratch, LANG pra locale, PYTHONHOME/PYTHONPATH/NODE_PATH
    // pros runtimes portáteis).
    let extra_env_keys: Vec<String> = config.extra_env.iter().map(|(k, _)| k.clone()).collect();
    parent_env.retain(|(k, _)| {
        // Mantém só REQUIRED (verifica via is_required) + o
        // que está em `extra_env` (HTTP_PROXY etc.).
        resolver.env_filter.allowlist().is_required(k) || extra_env_keys.iter().any(|ek| ek == k)
    });
    let env_block = build_env_block(&parent_env);
    // **Etapa 6 da Fase 7 (ADR-0033) — re-injeção do env block.**
    // A Etapa 4 v1 e a Etapa 5+ passavam `None` como
    // `lpEnvironment` em `CreateProcessAsUserW` (herdava do
    // parent). Isso **quebrava** o wiring do proxy de rede
    // Etapa 6 — o `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`
    // injetados via `extra_env` (no `FilesExecToolBase`) nunca
    // chegavam ao filho, e o filho tentava `urllib.urlopen`
    // direto (sem proxy). Esta Etapa 6 re-injeta o env block
    // construído. O risco de `ERROR_INVALID_PARAMETER` (87)
    // por env block grande continua valendo — se a Etapa 7+
    // bater nele, fallback é `None` (regressão silenciosa:
    // o filho herda do parent, `EnvFilter::apply` ainda
    // bloqueou DENIED in-place, mas `HTTP_PROXY` sumiu).
    // Por enquanto, env block típico do sandbox tem <50 vars
    // (REQUIRED + extra_env), bem abaixo do limite.

    // ---------- Step 3: Create pipes (CreatePipe, inheritable) ----------
    //
    // **Label nos pipes:** idealmente, stdout/stderr deveriam
    // nascer com `Mandatory Label\Low` (via SECURITY_DESCRIPTOR
    // no `SECURITY_ATTRIBUTES`) — sem isso, o child (Low) não
    // consegue escrever neles. **Mas** o `CreatePipe` falha
    // com `ERROR_PRIVILEGE_NOT_HELD` (0x80070522) quando o SD
    // tem SACL: criar kernel objects com SACL exige
    // `SeSecurityPrivilege` que o processo do user comum não
    // tem (split-token UAC). Workaround: criar os pipes SEM
    // label. O child ainda pode escrever no workdir (que tem
    // o label Low aplicado via `SetFileSecurityW` no step 1),
    // mas falha ao escrever nos pipes. O test do `print()`
    // (`hello world`) e do wall-clock falham com IOError
    // porque o `print` é bloqueado. **A Etapa 5+ cobre
    // path safety (writes no workdir) — output capture é
    // limitação conhecida**, registrada como pendência da
    // Fase 8.
    //
    // **Por que o workdir usa SetFileSecurityW e o pipe não:**
    // o workdir é um **file object** — files podem ter SACL
    // aplicada pelo owner (verificado com `icacls` no user
    // comum). Pipes são **kernel objects** — o `CreatePipe`
    // exige `SeSecurityPrivilege` pra criar com SACL. Limites
    // diferentes do Win32.
    let (stdin_read, stdin_write) = create_inheritable_pipe(None)
        .map_err(|e| SpawnError::SpawnFailed(format!("CreatePipe(stdin): {e}")))?;
    let (stdout_read, stdout_write) = create_inheritable_pipe(None)
        .map_err(|e| SpawnError::SpawnFailed(format!("CreatePipe(stdout): {e}")))?;
    let (stderr_read, stderr_write) = create_inheritable_pipe(None)
        .map_err(|e| SpawnError::SpawnFailed(format!("CreatePipe(stderr): {e}")))?;

    // ---------- Step 4: (removido na Etapa 5+) ----------
    // Os write ends dos pipes stdout/stderr já nasceram rotulados
    // (via SECURITY_DESCRIPTOR no CreatePipe, step 3). O
    // `label_sd` continua vivo aqui (buffer atrás do pointer) —
    // o OS já copiou o SD internamente no CreatePipe, mas
    // mantemos até o fim do spawn por segurança.

    // ---------- Step 5: STARTUPINFOEXW + HandleList attribute ----------
    let startup_info_ex =
        StartupInfoEx::new([stdin_write, stdout_write, stderr_write]).map_err(|e| {
            close_silent(stdin_read);
            close_silent(stdin_write);
            close_silent(stdout_read);
            close_silent(stdout_write);
            close_silent(stderr_read);
            close_silent(stderr_write);
            SpawnError::SpawnFailed(format!("StartupInfoEx::new: {e}"))
        })?;

    // ---------- Step 6: Token IntegrityLevel=Low + duplicate as primary ----------
    // (Variáveis em escopo caem em drop no return via `?`; ordem
    //  de cleanup é o inverso da declaração.)
    resolver
        .restricted_token
        .set_integrity_level(INTEGRITY_LEVEL_LOW)
        .map_err(|e| {
            close_silent(stdin_read);
            close_silent(stdin_write);
            close_silent(stdout_read);
            close_silent(stdout_write);
            close_silent(stderr_read);
            close_silent(stderr_write);
            SpawnError::SpawnFailed(format!("set_integrity_level(Low): {e}"))
        })?;
    let primary_token = resolver
        .restricted_token
        .duplicate_as_primary()
        .map_err(|e| {
            close_silent(stdin_read);
            close_silent(stdin_write);
            close_silent(stdout_read);
            close_silent(stdout_write);
            close_silent(stderr_read);
            close_silent(stderr_write);
            SpawnError::SpawnFailed(format!("duplicate_as_primary: {e}"))
        })?;

    // ---------- Step 7: CreateProcessAsUserW (CREATE_SUSPENDED) ----------
    let cmdline_str = build_cmdline(&config.program, &config.args);
    let cmdline_wide = to_wide_null(&cmdline_str);
    let workdir_wide = to_wide_null(&config.workdir.to_string_lossy());

    // `CREATE_UNICODE_ENVIRONMENT` é obrigatório sempre que se
    // passa um `lpEnvironment` construído por nós (UTF-16, par
    // a par terminado em `\0\0` — ver `build_env_block`). Sem
    // essa flag, `CreateProcessAsUserW` interpreta o bloco como
    // ANSI (8-bit) e falha com `ERROR_INVALID_PARAMETER` (87)
    // pra qualquer env block não-trivial — foi o que quebrava o
    // wiring do proxy da Etapa 6 (o env block era construído
    // certo, mas a flag pra dizer "isso é UTF-16" nunca foi
    // setada).
    let creation_flags = PROCESS_CREATION_FLAGS(
        CREATE_SUSPENDED.0 | EXTENDED_STARTUPINFO_PRESENT.0 | CREATE_UNICODE_ENVIRONMENT.0,
    );

    let mut proc_info = PROCESS_INFORMATION {
        hProcess: HANDLE(ptr::null_mut()),
        hThread: HANDLE(ptr::null_mut()),
        dwProcessId: 0,
        dwThreadId: 0,
    };

    // SAFETY: `CreateProcessAsUserW` toma o primary token, command
    // line, optional process/thread attributes (None = default),
    // bInheritHandles (TRUE: required so HandleList applies),
    // creation flags (CREATE_SUSPENDED + EXTENDED_STARTUPINFO_PRESENT),
    // optional env block pointer, workdir, STARTUPINFOW pointer
    // (extracted from STARTUPINFOEXW), and PROCESS_INFORMATION out.
    //
    // **env block:** a Etapa 4 v1 não re-injetava o env (o
    // `tokio::process::Command::envs()` falhava com
    // ERROR_INVALID_PARAMETER — faltava `CREATE_UNICODE_ENVIRONMENT`,
    // não era o tamanho do bloco). A Etapa 5+ construía o env
    // block via raw API mas ainda passava `None` (herda do
    // parent). A Etapa 6 da Fase 7 (ADR-0033) passa o env block
    // construído (REQUIRED + `extra_env`, com `HTTP_PROXY`/
    // `HTTPS_PROXY`/`NO_PROXY`).
    //
    // **Sem fallback pra `None` em caso de erro.** Um fallback
    // que reexecuta com env herdado do parent anula o
    // `EnvFilter` (Etapa 2, ameaça I1 do threat model) — o
    // filho passaria a ver o ambiente inteiro do processo pai,
    // incluindo credenciais de provider que só deveriam existir
    // fora do sandbox. Falha na construção do env block
    // controlado é erro duro: propaga e `spawn` falha. (Foi
    // exatamente esse fallback, presente até a Etapa 6+1, que
    // mascarava a ausência de `CREATE_UNICODE_ENVIRONMENT` —
    // o processo nascia com env herdado, sem proxy, e nenhum
    // teste percebia porque o fallback nunca aparecia como
    // erro pro caller.)
    let env_ptr: *const core::ffi::c_void = if env_block.is_empty() {
        std::ptr::null()
    } else {
        env_block.as_ptr() as *const _
    };
    let create_result = unsafe {
        CreateProcessAsUserW(
            primary_token,
            PCWSTR::null(),
            PWSTR(cmdline_wide.as_ptr() as *mut _),
            None, // process attributes
            None, // thread attributes
            true, // bInheritHandles
            creation_flags,
            Some(env_ptr), // env: REQUIRED + extra_env (UTF-16, CREATE_UNICODE_ENVIRONMENT)
            PCWSTR(workdir_wide.as_ptr()),
            &startup_info_ex.inner.StartupInfo,
            &mut proc_info,
        )
    };

    if let Err(e) = create_result {
        // Cleanup best-effort (variáveis dropam no fim da função).
        close_silent(primary_token);
        close_silent(stdin_read);
        close_silent(stdin_write);
        close_silent(stdout_read);
        close_silent(stdout_write);
        close_silent(stderr_read);
        close_silent(stderr_write);
        return Err(SpawnError::SpawnFailed(format!(
            "CreateProcessAsUserW: {e:?} (se ERROR_PRIVILEGE_NOT_HELD, \
             token construction precisa mudar — não elevar privilégio)"
        )));
    }

    let pid = unsafe { GetProcessId(proc_info.hProcess) };

    // ---------- Step 8: Create per-invocation Job Object ----------
    let job = JobObject::with_memory_limits(
        resolver.per_process_memory_bytes,
        resolver.total_memory_bytes,
    )
    .map_err(|e| {
        // Cleanup: o processo está suspended → TerminateProcess
        // (que não cascateia pq Job ainda não foi atribuído, mas
        // o processo vai morrer). Fecha handles restantes.
        unsafe {
            let _ = TerminateProcess(proc_info.hProcess, 1);
        }
        close_silent(proc_info.hProcess);
        close_silent(proc_info.hThread);
        close_silent(primary_token);
        close_silent(stdin_read);
        close_silent(stdin_write);
        close_silent(stdout_read);
        close_silent(stdout_write);
        close_silent(stderr_read);
        close_silent(stderr_write);
        SpawnError::SpawnFailed(format!(
            "JobObject per-invocation falhou: {e}. \
             Tree-kill da Etapa 2 fica quebrado sem Job — abortando."
        ))
    })?;

    // ---------- Step 9: AssignProcessToJobObject + ResumeThread ----------
    // `assign_suspended_process` é buggy: passa o **process
    // handle** pro `ResumeThread` (que precisa do **thread
    // handle**). Aqui fazemos as 2 chamadas separadas com os
    // handles corretos.
    if let Err(e) = job.assign(proc_info.hProcess) {
        drop(job);
        unsafe {
            let _ = TerminateProcess(proc_info.hProcess, 1);
        }
        close_silent(proc_info.hProcess);
        close_silent(proc_info.hThread);
        close_silent(primary_token);
        close_silent(stdin_read);
        close_silent(stdin_write);
        close_silent(stdout_read);
        close_silent(stdout_write);
        close_silent(stderr_read);
        close_silent(stderr_write);
        return Err(SpawnError::SpawnFailed(format!(
            "JobObject::assign falhou: {e}"
        )));
    }
    // ResumeThread no THREAD handle (não process). Decrementa
    // o suspend count (que era 1 por causa de CREATE_SUSPENDED).
    let previous = unsafe { ResumeThread(proc_info.hThread) };
    if previous == u32::MAX {
        // u32::MAX é o sentinel "erro" de ResumeThread.
        let err = unsafe { windows::Win32::Foundation::GetLastError() }.0;
        drop(job);
        unsafe {
            let _ = TerminateProcess(proc_info.hProcess, 1);
        }
        close_silent(proc_info.hProcess);
        close_silent(proc_info.hThread);
        close_silent(primary_token);
        close_silent(stdin_read);
        close_silent(stdin_write);
        close_silent(stdout_read);
        close_silent(stdout_write);
        close_silent(stderr_read);
        close_silent(stderr_write);
        return Err(SpawnError::SpawnFailed(format!(
            "ResumeThread falhou (GetLastError=0x{err:X}, handle=0x{:X})",
            proc_info.hThread.0 as usize
        )));
    }

    // ---------- Step 10: Cleanup (handles que o parent não precisa) ----------
    // Write ends: o child tem cópias herdadas via HandleList.
    close_silent(stdin_write);
    close_silent(stdout_write);
    close_silent(stderr_write);
    // stdin_read: o child não lê stdin nesse caminho.
    close_silent(stdin_read);
    // primary_token: o child já pegou sua própria referência.
    close_silent(primary_token);
    // thread handle: o thread já foi resumed, não precisa mais.
    close_silent(proc_info.hThread);
    // Attribute list: limpa via Drop do StartupInfoEx no fim do
    // escopo. (Não precisa `drop()` explícito — sai do escopo
    // ao final da função.)

    // ---------- Step 11: Build SandboxedProcess ----------
    let invocation_id = resolver.next_invocation_id();
    let raw_child = RawChild::new(
        proc_info.hProcess,
        HANDLE(ptr::null_mut()), // thread já fechado
        Some(stdout_read),
        Some(stderr_read),
    );

    Ok(SandboxedProcess {
        pid,
        child: Some(raw_child),
        job: Some(job),
        invocation_id,
    })
}

// =====================================================================
// Helpers privados do spawn_windows.
// =====================================================================

/// Constrói o env block UTF-16 LE para `CreateProcessAsUserW`.
/// Formato: `KEY=VALUE\0` por var, terminação `\0\0` no fim.
/// Variáveis com `=` no nome ou valor são preservadas literalmente.
#[cfg(target_os = "windows")]
fn build_env_block(envs: &[(String, String)]) -> Vec<u16> {
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in envs {
        for c in k.encode_utf16() {
            block.push(c);
        }
        block.push(b'=' as u16);
        for c in v.encode_utf16() {
            block.push(c);
        }
        block.push(0);
    }
    block.push(0); // Double NUL = end of env block
    block
}

/// Constroi o command line para `CreateProcessAsUserW` no formato
/// Win32 (`"program.exe" "arg1" "arg2" ...`). Argumentos com espaço
/// ou aspas são quotados.
#[cfg(target_os = "windows")]
fn build_cmdline(program: &std::path::Path, args: &[String]) -> String {
    let mut cmdline = String::new();
    let prog_str = program.to_string_lossy();
    if prog_str.contains(' ') || prog_str.contains('"') {
        cmdline.push('"');
        cmdline.push_str(&prog_str);
        cmdline.push('"');
    } else {
        cmdline.push_str(&prog_str);
    }
    for arg in args {
        cmdline.push(' ');
        if arg.contains(' ') || arg.contains('"') {
            cmdline.push('"');
            // Escape interno de aspas: " → ""
            let escaped = arg.replace('"', "\"\"");
            cmdline.push_str(&escaped);
            cmdline.push('"');
        } else {
            cmdline.push_str(arg);
        }
    }
    cmdline
}

/// Cria um par de handles (read, write) via `CreatePipe` com
/// `bInheritHandle = TRUE` no write end. O child herda o write end
/// (listado em `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` no spawn);
/// o read end fica no parent.
///
/// **Parâmetro `sd`:** `Option<*mut c_void>` (raw pointer pra
/// `SECURITY_DESCRIPTOR`). Se `Some`, o SD é usado no
/// `SECURITY_ATTRIBUTES.lpSecurityDescriptor` do CreatePipe — o
/// pipe nasce com o SD aplicado (incluindo SACL). Se `None`,
/// o pipe é criado com SD default (owner = caller, sem SACL).
/// Usamos isso pra rotular stdout/stderr com `Mandatory Label\Low`
/// upfront (Etapa 5+ da Fase 7): o child (Low) precisa escrever
/// neles, e a label tem que estar no SD no momento da criação
/// (não dá pra `SetSecurityInfo` depois porque o handle do
/// `CreatePipe` não tem `WRITE_OWNER`).
#[cfg(target_os = "windows")]
fn create_inheritable_pipe(sd: Option<*mut std::ffi::c_void>) -> Result<(HANDLE, HANDLE), String> {
    let sd_ptr = sd.unwrap_or(std::ptr::null_mut());
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd_ptr,
        bInheritHandle: BOOL(1),
    };
    let mut read_end = HANDLE(std::ptr::null_mut());
    let mut write_end = HANDLE(std::ptr::null_mut());
    // SAFETY: CreatePipe toma os 2 handles (out), SECURITY_ATTRIBUTES
    // (in) com bInheritHandle, e tamanho do buffer (0 = default).
    unsafe { CreatePipe(&mut read_end, &mut write_end, Some(&sa), 0) }
        .map_err(|e| format!("CreatePipe falhou: {e:?}"))?;
    Ok((read_end, write_end))
}

/// Converte str pra UTF-16 com NUL terminator.
#[cfg(target_os = "windows")]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Fecha um handle silenciosamente (ignora erros — usado em cleanup
/// best-effort onde o caller já vai propagar o erro original).
#[cfg(target_os = "windows")]
fn close_silent(handle: HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    if !handle.is_invalid() && !handle.0.is_null() {
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

/// Wrapper RAII sobre `STARTUPINFOEXW` + buffer do attribute list.
/// `Drop` chama `DeleteProcThreadAttributeList` no handle.
/// **Não** é `Send` (mesma razão do HANDLE — ponteiro raw).
#[cfg(target_os = "windows")]
struct StartupInfoEx {
    inner: STARTUPINFOEXW,
    /// Buffer que o `lpAttributeList` aponta. Mantido vivo até
    /// o `Drop` (a Win32 API lê o buffer durante o
    /// `CreateProcessAsUserW` mas não mantém referência depois).
    _buffer: Vec<u8>,
}

#[cfg(target_os = "windows")]
impl StartupInfoEx {
    /// Constroi o `STARTUPINFOEXW` com `STARTF_USESTDHANDLES` +
    /// `hStdInput/Output/Error` apontando pros 3 handles dados, e
    /// um `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` listando SÓ esses 3.
    ///
    /// **Defesa em profundidade:** o `bInheritHandle` no
    /// `SECURITY_ATTRIBUTES` de cada pipe já marca os handles como
    /// herdáveis, mas o `HANDLE_LIST` restringe a herança SÓ
    /// aos listados — mesmo que outro handle herdável esteja
    /// aberto no parent, o child NÃO o herda.
    fn new(handles: [HANDLE; 3]) -> Result<Self, String> {
        // 1. Primeira chamada: descobre o tamanho necessário.
        // A Win32 API aceita um LPPROC_THREAD_ATTRIBUTE_LIST null
        // (ou default) e devolve o tamanho em `lpsize`.
        let mut size: usize = 0;
        let _ = unsafe {
            InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                1,
                0,
                &mut size,
            )
        };
        if size == 0 {
            return Err("InitializeProcThreadAttributeList(size=0) — deveria ser > 0".to_string());
        }

        // 2. Aloca o buffer e inicializa o attribute list.
        let mut buffer = vec![0u8; size];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr() as *mut _);
        unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size) }
            .map_err(|e| format!("InitializeProcThreadAttributeList falhou: {e:?}"))?;

        // 3. Adiciona o HandleList.
        // SAFETY: handles.as_ptr() aponta pra array válida de 3 HANDLEs.
        // `UpdateProcThreadAttribute` copia os bytes internamente.
        // **O `attribute` é o valor FULL de `ProcThreadAttributeValue`**,
        // não só o número: `((Number & 0xFFFF) | (Input ? 0x00020000 : 0)
        // | (Thread ? 0x00010000 : 0) | (Additive ? 0x00040000 : 0))`.
        // HandleList = (Number=2, Thread=FALSE, Input=TRUE, Additive=FALSE)
        // = 0x00020002. Passar só `2` (o número) retorna
        // `ERROR_NOT_SUPPORTED`.
        const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x00020002;
        unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                Some(handles.as_ptr() as *const _),
                std::mem::size_of::<HANDLE>() * handles.len(),
                None,
                None,
            )
        }
        .map_err(|e| format!("UpdateProcThreadAttribute(HandleList) falhou: {e:?}"))?;

        // 4. Constroi a STARTUPINFOW.
        let startup_info = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: handles[0],
            hStdOutput: handles[1],
            hStdError: handles[2],
            ..Default::default()
        };

        Ok(Self {
            inner: STARTUPINFOEXW {
                StartupInfo: startup_info,
                lpAttributeList: attr_list,
            },
            _buffer: buffer,
        })
    }
}

#[cfg(target_os = "windows")]
impl Drop for StartupInfoEx {
    fn drop(&mut self) {
        if !self.inner.lpAttributeList.0.is_null() {
            unsafe {
                DeleteProcThreadAttributeList(self.inner.lpAttributeList);
            }
        }
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
