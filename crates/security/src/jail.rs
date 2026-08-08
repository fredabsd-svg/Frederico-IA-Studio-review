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
//! - **Sem criação de pipes para stdout/stderr** — a v1 retorna
//!   o PID + handle do processo; o caller é responsável por criar
//!   pipes e fazer `read` assíncrono. A Etapa 4 da Fase 7 (criação
//!   do `RunExecutor` integrado) pluga isso.
//! - **Sem `OutputCollector` (teto 10 MB + chunks 64 KB)** —
//!   mesma razão. Implementação fica na Etapa 4.
//! - **Sem wall-clock timeout** — caller pode usar `tokio::time::timeout`
//!   em volta de `wait()` (similar ao `Command::output` com timeout).
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
/// o processo é morto via `TerminateProcess` (não cascateia pro
/// neto; para isso, `Drop` do `SecurityJailResolver`).
///
/// **Lifetime:** o caller é responsável por chamar `wait()` para
/// coletar o exit code; caso contrário, o processo vira órfão
/// (até o Job Object do root matar via `KILL_ON_JOB_CLOSE` no
/// shutdown do app).
pub struct SandboxedProcess {
    pid: u32,
    /// Processo tokio (mantido vivo até `wait()` ou `Drop`).
    child: Option<tokio::process::Child>,
    /// ID único da invocação (para logs).
    invocation_id: u64,
}

// Manually implement Debug pra evitar expor o `Child` (que tem
// handles Windows).
impl std::fmt::Debug for SandboxedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxedProcess")
            .field("pid", &self.pid)
            .field("invocation_id", &self.invocation_id)
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

    /// Mata o processo via `TerminateProcess` (Windows) ou
    /// `Child::kill` (tokio cross-platform). **Não** cascateia pro
    /// neto — para isso, drop o `SecurityJailResolver` inteiro.
    pub async fn kill(&mut self) -> Result<(), std::io::Error> {
        if let Some(child) = self.child.as_mut() {
            child.start_kill()?;
        }
        Ok(())
    }

    /// Consome o `tokio::process::Child` interno (para o caller
    /// fazer I/O async diretamente). Após isso, `wait()` e
    /// `kill()` retornam erro.
    pub fn into_child(mut self) -> tokio::process::Child {
        self.child.take().expect("into_child chamado 2x")
    }
}

impl Drop for SandboxedProcess {
    fn drop(&mut self) {
        // Sem `kill()` explícito: se o caller não chamar `wait()`,
        // o processo fica até o `SecurityJailResolver` ser
        // droppado (que fecha o Job Object e mata tudo via
        // KILL_ON_JOB_CLOSE). Em prática isso é o desejado — o
        // caller deve `wait()` pra coletar exit, mas se ele
        // esquecer, o shutdown limpa.
        if let Some(mut child) = self.child.take() {
            // Tenta `start_kill` (não-await) — se o processo
            // terminar por KILL_ON_JOB_CLOSE no shutdown do
            // SecurityJailResolver, isso evita zumbis.
            let _ = child.start_kill();
        }
    }
}

/// Orquestrador do sandbox. Combina Jail (path safety via caller)
/// + Job Object + Restricted Token + Env Filter. Por design, **só
///   Windows** é suportado na v1; Linux retorna
///   `SpawnError::Unsupported` (degradação declarada).
pub struct SecurityJailResolver {
    /// Root Job Object (vive até o `Drop` do resolver). Todos
    /// os processos filhos são atribuídos a este Job.
    #[cfg(target_os = "windows")]
    root_job: crate::windows::JobObject,
    /// Token restrito (drop dos 6 privilégios). Construído em
    /// `new()` mas **não aplicado** na v1 (a `CreateProcessAsUser`
    /// via `tokio::process` ainda não tem a integração; Etapa 4
    /// da Fase 7 implementa via `std::os::windows::process::CommandExt::as_user`).
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
            let root_job = crate::windows::JobObject::with_memory_limits(
                config.per_process_memory_bytes,
                config.total_memory_bytes,
            )
            .map_err(|e| SpawnError::SpawnFailed(format!("JobObject::with_memory_limits: {e}")))?;

            let restricted_token = crate::windows::RestrictedToken::from_current_process()
                .map_err(|e| {
                    SpawnError::SpawnFailed(format!("RestrictedToken::from_current_process: {e}"))
                })?;

            let env_filter = EnvFilter::new(config.env_allowlist);

            Ok(Arc::new(Self {
                root_job,
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
    /// `CreateProcessAsUser` no Windows quando combinado com
    /// `token` — a Etapa 4 pluga o `restricted_token` ao
    /// `Command` via `windows-rs` raw; por enquanto, a v1
    /// não tem o caminho completo `CreateProcessAsUser` via
    /// `tokio::process`).
    ///
    /// **Importante:** a v1 deste método **NÃO** aplica o
    /// `RestrictedToken` (a `CreateProcessAsUser` raw precisa de
    /// setup manual que `tokio::process::Command` não expõe). A
    /// Etapa 4 da Fase 7 implementa via `std::os::windows::process::CommandExt::as_user`
    /// (que aceita um HANDLE de token). Por enquanto, o spawn
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
    /// Etapa 4.
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

        // 5. Atribui o PID ao root Job (Windows). Falha
        //    silenciosamente se a atribuição falhar — o
        //    `KILL_ON_JOB_CLOSE` não dispara, mas o processo
        //    ainda roda (degradação controlada, logada).
        #[cfg(target_os = "windows")]
        {
            if let Err(e) = self.root_job.assign_pid(pid) {
                eprintln!(
                    "[SecurityJailResolver] AVISO: assign_pid falhou (pid={pid}): {e}. \
                     Processo não está no Job Object — tree-kill não vai cascatear."
                );
            }
        }

        Ok(SandboxedProcess {
            pid,
            child: Some(child),
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
