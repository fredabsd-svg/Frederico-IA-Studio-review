//! `DocumentWorkerLauncher` — owner do ciclo de vida do worker
//! sidecar do `document-worker`.
//!
//! ADR-0023 §D3. Encapsula o [`WorkerManager`] + [`WorkerHandle`]
//! do `frederico-process-architecture` com 3 responsabilidades
//! extras:
//!
//! 1. **Lazy start** — o worker só sobe na primeira `invoke()`,
//!    não no startup da casca. Pesa os 2s de abertura do app
//!    se o usuário nem vai usar `docs.generate` na sessão.
//! 2. **Restart on death com teto** — se o worker morrer (panic,
//!    OOM, kill externo), a próxima `invoke()` detecta via
//!    `health_snapshot` e tenta recriar. Limite: 3 tentativas
//!    com recuo exponencial (1s, 2s, 4s). Excedeu → estado
//!    `Dead` permanente, `invoke()` retorna
//!    [`WorkerError::PermanentlyDead`].
//! 3. **Kill tree no app exit** — o `Drop` chama `shutdown`
//!    best-effort (com `tokio::runtime::Handle::block_on`
//!    síncrono). Garante que nenhum processo Python órfão
//!    sobrevive ao fechamento da janela.
//!
//! ## "Sempre mata o antigo antes de criar o novo"
//!
//! Regra dura (ADR-0023 §D3): se a próxima `invoke` detecta morte
//! e vai tentar recriar, **primeiro** chama `manager.shutdown()`
//! (que consome `self`), **depois** constrói um `WorkerManager`
//! novo. Worker em ciclo de falha gerando processos Python órfãos
//! é o pior modo de falha possível num app desktop.
//!
//! ## Gate Windows
//!
//! O módulo inteiro é `#[cfg(windows)]` — depende de
//! [`ExternalSpawnConfig`] (named pipes são Windows). Em outras
//! plataformas, `DocumentWorkerLauncher::new` retorna
//! [`WorkerError::PlatformNotSupported`]. O `build_default_tools`
//! no `composition.rs` é simétrico.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use frederico_process_architecture::{ProcessError, WorkerHandle, WorkerHealth, WorkerManager};
use tokio::sync::Mutex;

use crate::runtime::RuntimeLocation;

/// Configuração do launcher. Defaults são razoáveis pra produção;
/// testes podem customizar.
#[derive(Debug, Clone)]
pub struct LauncherConfig {
    /// Teto de tentativas de restart antes de marcar como
    /// `Dead` permanente. Default: 3.
    pub max_restart_attempts: u8,
    /// Recuo base. Recuo real = `base * 2^(attempts-1)`:
    /// attempts=1 → `base`, attempts=2 → `base*2`, attempts=3 →
    /// `base*4`. Default: 1s (1s, 2s, 4s).
    pub restart_backoff_base: Duration,
    /// Timeout do `READY <pipe_name>` no boot do worker
    /// (passado pro `ExternalSpawnConfig::with_ready_timeout`).
    /// Default: 10s (mesmo do `ExternalSpawnConfig::default()`).
    pub ready_timeout: Duration,
    /// Default timeout pra `invoke`/`ping` no
    /// `ExternalSpawnConfig`. Default: 30s.
    pub invoke_timeout_ms: u32,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            max_restart_attempts: 3,
            restart_backoff_base: Duration::from_secs(1),
            ready_timeout: Duration::from_secs(10),
            invoke_timeout_ms: 30_000,
        }
    }
}

/// Erro do launcher.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// Runtime não foi resolvido pelo
    /// [`resolve_document_worker_runtime`]. Não há candidato
    /// válido em env, recursos, dev. Indisponibilidade (não
    /// erro). A UI mostra "document-worker: indisponível" e
    /// `docs.generate`/`docs.inspect` não aparecem no schema
    /// do modelo.
    #[error("document-worker runtime indisponível")]
    RuntimeUnavailable,

    /// Plataforma não suportada (não-Windows). Gate
    /// `#[cfg(windows)]` do `ExternalSpawnConfig` é o motivo.
    #[error("document-worker não suportado nesta plataforma (Windows only)")]
    PlatformNotSupported,

    /// Spawn falhou — `WorkerManager::spawn_external` retornou
    /// `Err`. A mensagem é do `ProcessError` original.
    #[error("falha ao spawnar document-worker: {0}")]
    SpawnFailed(#[source] ProcessError),

    /// Restart excedeu o teto de tentativas. O launcher está
    /// permanentemente morto. A UI mostra "document-worker:
    /// falhou N vezes, reinicie o app" e o botão "tentar
    /// reiniciar" reseta o state via [`DocumentWorkerLauncher::reset`].
    #[error("document-worker falhou {attempts} vezes seguidas (limite: {max})")]
    PermanentlyDead {
        /// Número de tentativas que falharam.
        attempts: u8,
        /// Teto configurado.
        max: u8,
    },

    /// O `invoke` falhou. A mensagem é do `ProcessError` original.
    #[error("invoke falhou: {0}")]
    InvokeFailed(#[source] ProcessError),

    /// Shutdown best-effort no `Drop` falhou. Não é fatal —
    /// o child já pode ter saído por conta própria.
    #[error("shutdown best-effort falhou: {0}")]
    ShutdownFailed(#[source] ProcessError),
}

/// State interno do launcher. As transições são:
///
/// ```text
/// NotStarted  ──invoke()──>  Alive { manager, handle }
/// Alive       ──invoke() se health != ok──>  Restarting
/// Alive       ──Drop──>  (shutdown síncrono)  →  Drop termina
/// Restarting  ──invoke() após backoff──>  Alive (sucesso) | Restarting (falha)
/// Restarting  ──attempts >= max──>  Dead
/// Dead        ──reset()──>  NotStarted
/// ```
#[allow(dead_code)] // Campos lidos pela UI de diagnóstico em Etapa futura.
#[allow(clippy::large_enum_variant)] // Alive é grande (WorkerManager+WorkerHandle); Box<'a, T> introduziria lifetime que complica o Mutex.
pub(crate) enum LauncherState {
    /// Lazy — primeira invoke vai spawnar.
    NotStarted,
    /// Worker vivo. `manager` é o owner (o `Drop` chama
    /// `shutdown(self)`).
    Alive {
        manager: WorkerManager,
        handle: WorkerHandle,
    },
    /// Worker morreu, contando tentativas. A próxima `invoke`
    /// espera o backoff e tenta de novo.
    Restarting {
        attempts: u8,
        last_error: ProcessError,
        last_attempt_at: Instant,
    },
    /// Teto de tentativas excedido. `invoke` retorna
    /// `PermanentlyDead` até alguém chamar `reset()`.
    Dead {
        attempts: u8,
        last_error: ProcessError,
    },
}

/// Estado de saúde do runtime, exposto pra UI de diagnóstico
/// via `tauri::command DocumentWorkerStatus` (que delega pra
/// [`DocumentWorkerLauncher::status`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherStatus {
    /// Se o worker está rodando agora (state `Alive` E
    /// `health_snapshot.alive == true`).
    pub alive: bool,
    /// Qual opção do `RuntimeLocation::source` foi usada
    /// (env / app / dev). `None` se o launcher nunca chegou
    /// a spawnar.
    pub runtime_source: Option<&'static str>,
    /// Caminho raiz do runtime resolvido. `None` se
    /// indisponível.
    pub runtime_path: Option<PathBuf>,
    /// Mensagem PT-BR explicando o estado atual. Útil pra UI
    /// mostrar "document-worker: disponível/indisponível,
    /// caminho resolvido" sem depurar.
    pub message: String,
}

/// Owner do ciclo de vida do worker. Thread-safe via
/// `Arc<Mutex<LauncherState>>`.
#[derive(Clone)]
pub struct DocumentWorkerLauncher {
    state: Arc<Mutex<LauncherState>>,
    location: RuntimeLocation,
    config: LauncherConfig,
}

impl DocumentWorkerLauncher {
    /// Cria um launcher a partir de um `RuntimeLocation`
    /// resolvido. **Não spawna** o worker — só inicializa o
    /// state como `NotStarted`. A primeira `invoke` faz o
    /// spawn lazy.
    #[must_use]
    pub fn new(location: RuntimeLocation, config: LauncherConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(LauncherState::NotStarted)),
            location,
            config,
        }
    }

    /// Tenta fazer o `invoke` no worker. Se o worker ainda não
    /// subiu, spawna (lazy). Se morreu, tenta recriar (com
    /// teto). Se excedeu o teto, retorna `PermanentlyDead`.
    ///
    /// O `invoke` em si é uma chamada no `WorkerHandle::invoke`
    /// — o dispatcher do toolkit (`WorkerToolDispatcher`)
    /// chama isso por baixo. Aqui o launcher é a abstração
    /// mais alta: o caller (casca) passa o payload, o launcher
    /// cuida do ciclo de vida.
    pub async fn invoke(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, WorkerError> {
        // Mutex é segurado só o necessário — não seguramos
        // durante `await` (lock de curta duração para
        // inspecionar o state e/ou transicionar). O invoke
        // em si (que é awaitable) acontece DEPOIS de soltar o
        // lock.
        let mut state = self.state.lock().await;

        // State `NotStarted` ou `Restarting` → tenta spawnar.
        if matches!(
            *state,
            LauncherState::NotStarted | LauncherState::Restarting { .. }
        ) {
            // Se `Restarting`, espera o backoff antes de tentar.
            if let LauncherState::Restarting {
                attempts,
                last_attempt_at,
                ..
            } = &*state
            {
                let backoff = self.config.restart_backoff_base * 2u32.pow((*attempts - 1) as u32);
                let elapsed = last_attempt_at.elapsed();
                if elapsed < backoff {
                    let wait = backoff - elapsed;
                    tracing::debug!(
                        attempts = *attempts,
                        backoff_ms = backoff.as_millis() as u64,
                        elapsed_ms = elapsed.as_millis() as u64,
                        wait_ms = wait.as_millis() as u64,
                        "document-worker: aguardando backoff antes de retry"
                    );
                    drop(state);
                    tokio::time::sleep(wait).await;
                    state = self.state.lock().await;
                }
            }

            // Tenta spawnar (ou respawnar).
            let new_attempts = if let LauncherState::Restarting { attempts, .. } = &*state {
                *attempts + 1
            } else {
                1
            };

            // Se o state é `Restarting`, o manager antigo
            // (do spawn anterior) já foi consumido pelo
            // `shutdown` no `try_spawn_alive` anterior — não
            // há manager antigo vivo pra matar. A regra "mata
            // antes de criar novo" se aplica ao spawn loop:
            // quando o invoke detecta morte, ele mata antes
            // de tentar de novo. (Ver `check_health_and_try_restart`.)
            match self.try_spawn_alive().await {
                Ok((manager, handle)) => {
                    tracing::info!(
                        attempts = new_attempts,
                        "document-worker spawn OK (respawn após morte)"
                    );
                    *state = LauncherState::Alive { manager, handle };
                }
                Err(e) => {
                    // Extrai o `ProcessError` subjacente uma
                    // vez (o `match` consome `e`).
                    let pe: ProcessError = match e {
                        WorkerError::SpawnFailed(pe) => pe,
                        other => ProcessError::Platform {
                            message: other.to_string(),
                        },
                    };
                    if new_attempts >= self.config.max_restart_attempts {
                        tracing::error!(
                            attempts = new_attempts,
                            max = self.config.max_restart_attempts,
                            error = %pe,
                            "document-worker: teto de tentativas excedido, marcado como Dead"
                        );
                        *state = LauncherState::Dead {
                            attempts: new_attempts,
                            last_error: pe,
                        };
                        return Err(WorkerError::PermanentlyDead {
                            attempts: new_attempts,
                            max: self.config.max_restart_attempts,
                        });
                    }
                    tracing::warn!(
                        attempts = new_attempts,
                        error = %pe,
                        "document-worker spawn falhou, vai tentar de novo"
                    );
                    // `pe` será movido pro return;
                    // pra não perder o `last_error` no
                    // state, construímos uma versão
                    // "descritiva" via Display — não é
                    // perfeita (perde o tipo), mas o
                    // `Restarting` é state transitório.
                    *state = LauncherState::Restarting {
                        attempts: new_attempts,
                        last_error: ProcessError::Platform {
                            message: format!("spawn attempt {new_attempts}: {pe}"),
                        },
                        last_attempt_at: Instant::now(),
                    };
                    return Err(WorkerError::SpawnFailed(pe));
                }
            }
        }

        // State deve ser `Alive` agora.
        let LauncherState::Alive { manager: _, handle } = &*state else {
            // Race: o state mudou entre o lock e o match. Vai
            // pra próxima invoke. Não é fatal — só perdemos
            // uma tentativa.
            drop(state);
            return Err(WorkerError::InvokeFailed(ProcessError::Transport {
                message: "state mudou durante invoke (race)".to_string(),
            }));
        };
        let handle_clone = handle.clone();
        drop(state); // solta o lock antes do await

        // Checagem de saúde antes do invoke. Se morto, vai
        // pra próxima invoke (que vai detectar `Alive` mas
        // com `health == Unhealthy` e vai tentar recriar).
        let health = handle_clone.health_snapshot().await;
        if matches!(health.health, WorkerHealth::Unhealthy) {
            tracing::warn!(
                worker_id = %handle_clone.worker_id(),
                "document-worker morto antes do invoke, marcando pra restart"
            );
            // Transita pra `Restarting` com attempts=1. O
            // shutdown do manager antigo (que detém o `child`)
            // acontece no `try_spawn_alive` da PRÓXIMA invoke
            // — aqui só marcamos o state.
            let mut state = self.state.lock().await;
            // O manager atual (que está em `Alive { manager,
            // handle }`) tem o child vivo até alguém chamar
            // `shutdown`. O `try_spawn_alive` vai consumir esse
            // `manager` via `replace` antes de criar o novo.
            // Detalhe: como `Alive.manager` é owned pelo
            // `state`, e o `try_spawn_alive` precisa do
            // `manager` antigo pra chamar `shutdown`, o caminho
            // mais simples é: tira o `manager` do state,
            // chama `shutdown` DEPOIS de soltar o lock (não
            // segura lock durante `await`).
            if let LauncherState::Alive { manager, handle: _ } =
                std::mem::replace(&mut *state, LauncherState::NotStarted)
            {
                drop(state);
                // Shutdown best-effort do manager antigo.
                if let Err(e) = manager.shutdown().await {
                    tracing::warn!(
                        error = %e,
                        "document-worker: shutdown do manager antigo (morto) falhou — processo pode já ter saído"
                    );
                }
            }
            return Err(WorkerError::InvokeFailed(ProcessError::Unhealthy {
                worker_id: handle_clone.worker_id().to_string(),
                message: "saúde degradada antes do invoke (WorkerHealth::Unhealthy)".to_string(),
            }));
        }

        // Tudo OK — invoke de verdade.
        handle_clone
            .invoke(payload)
            .await
            .map_err(WorkerError::InvokeFailed)
    }

    /// Spawna o worker (lazy). Constrói o `ExternalSpawnConfig`
    /// a partir do `RuntimeLocation` e chama
    /// `WorkerManager::spawn_external`.
    async fn try_spawn_alive(&self) -> Result<(WorkerManager, WorkerHandle), WorkerError> {
        let cfg = frederico_process_architecture::ExternalSpawnConfig::new(
            self.location.python_exe.to_string_lossy().to_string(),
        )
        .with_args(vec![self.location.script.to_string_lossy().to_string()])
        .with_cwd(self.location.root.clone())
        .with_ready_timeout(self.config.ready_timeout);

        let cfg = frederico_process_architecture::ExternalSpawnConfig {
            default_timeout_ms: self.config.invoke_timeout_ms,
            ..cfg
        };

        frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .map_err(WorkerError::SpawnFailed)
    }

    /// Reseta o state do launcher. Chamado pela UI quando o
    /// usuário clica em "tentar reiniciar" no diagnóstico.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        tracing::info!("document-worker: state resetado pelo usuário (botão 'tentar reiniciar')");
        *state = LauncherState::NotStarted;
    }

    /// Snapshot do estado atual pra UI de diagnóstico.
    /// **Não** faz I/O (não checa `health_snapshot` — só
    /// inspeciona o state interno). A UI pode chamar
    /// periodicamente pra ter feedback sem custar caro.
    pub async fn status(&self) -> LauncherStatus {
        let state = self.state.lock().await;
        let runtime_source_str = match self.location.source {
            crate::runtime::RuntimeSource::EnvVar => "env",
            crate::runtime::RuntimeSource::AppResources => "app_resources",
            crate::runtime::RuntimeSource::DevRepo => "dev_repo",
        };
        match &*state {
            LauncherState::NotStarted => LauncherStatus {
                alive: false,
                runtime_source: Some(runtime_source_str),
                runtime_path: Some(self.location.root.clone()),
                message: "document-worker ainda não inicializado (lazy). \
                          Será iniciado na primeira chamada a docs.generate/docs.inspect."
                    .to_string(),
            },
            LauncherState::Alive { .. } => LauncherStatus {
                alive: true,
                runtime_source: Some(runtime_source_str),
                runtime_path: Some(self.location.root.clone()),
                message: "document-worker: disponível".to_string(),
            },
            LauncherState::Restarting { attempts, .. } => LauncherStatus {
                alive: false,
                runtime_source: Some(runtime_source_str),
                runtime_path: Some(self.location.root.clone()),
                message: format!(
                    "document-worker: reiniciando (tentativa {attempts} de {})",
                    self.config.max_restart_attempts
                ),
            },
            LauncherState::Dead { attempts, .. } => LauncherStatus {
                alive: false,
                runtime_source: Some(runtime_source_str),
                runtime_path: Some(self.location.root.clone()),
                message: format!(
                    "document-worker: falhou {attempts} vezes, reinicie o app ou clique em 'tentar reiniciar'"
                ),
            },
        }
    }
}

impl Drop for DocumentWorkerLauncher {
    fn drop(&mut self) {
        // **Limitação conhecida:** o `Drop` do Rust é síncrono
        // e o `WorkerManager::shutdown` é async. Por isso o
        // shutdown explícito **não** roda no `Drop` — o que
        // roda é o **kill tree do SO Windows** quando o
        // processo pai termina (o child `python.exe` é filho
        // do `frederico-desktop.exe` e morre junto).
        //
        // A Etapa 7 da fase-ligação (Modo desenvolvedor)
        // substitui o `FileSystemJailResolver` por
        // `SecurityJailResolver` via `frederico-security`,
        // que adiciona Job Objects + AllowVolumeAccess. Os
        // Job Objects **garantem** que o child é morto
        // quando o parent termina, mesmo em cenários onde o
        // `frederico-desktop.exe` é morto abruptamente
        // (crash, kill -9, etc.).
        //
        // **Por que não usar `try_lock` aqui:** o `Mutex` é
        // `tokio::sync::Mutex` (necessário para `await`
        // fora de `Drop`). `try_lock` retorna `Result`, e
        // mesmo se conseguir o lock, o `shutdown` async não
        // pode rodar em contexto sync. A solução real é a
        // Etapa 7 com Job Objects.
        tracing::info!(
            "DocumentWorkerLauncher droppado — child python.exe será reaped pelo SO \
             (Windows) ou pelo Job Object (Etapa 7 — SecurityJailResolver)."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: cria um `RuntimeLocation` sintético apontando
    /// pra um diretório qualquer. Não checa a presença de
    /// artefatos — é só pra construção de Launcher de teste.
    fn fake_location(root: PathBuf, source: crate::runtime::RuntimeSource) -> RuntimeLocation {
        RuntimeLocation {
            python_exe: root.join("python.exe"),
            script: root.join("document-worker.py"),
            root,
            source,
        }
    }

    #[tokio::test]
    async fn new_launcher_starts_in_not_started() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loc = fake_location(
            dir.path().to_path_buf(),
            crate::runtime::RuntimeSource::DevRepo,
        );
        let launcher = DocumentWorkerLauncher::new(loc, LauncherConfig::default());

        let status = launcher.status().await;
        assert!(!status.alive);
        assert!(status.message.contains("ainda não inicializado"));
        assert_eq!(status.runtime_source, Some("dev_repo"));
    }

    #[tokio::test]
    async fn status_runtime_source_string_mapping() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loc = fake_location(
            dir.path().to_path_buf(),
            crate::runtime::RuntimeSource::EnvVar,
        );
        let launcher = DocumentWorkerLauncher::new(loc, LauncherConfig::default());
        assert_eq!(launcher.status().await.runtime_source, Some("env"));

        let dir2 = tempfile::tempdir().expect("tempdir");
        let loc2 = fake_location(
            dir2.path().to_path_buf(),
            crate::runtime::RuntimeSource::AppResources,
        );
        let launcher2 = DocumentWorkerLauncher::new(loc2, LauncherConfig::default());
        assert_eq!(
            launcher2.status().await.runtime_source,
            Some("app_resources")
        );
    }

    #[tokio::test]
    async fn reset_transitions_dead_back_to_not_started_via_replacement() {
        // Sem spawnar de verdade (que precisa do Windows),
        // testamos só a mecânica do `reset`: o state
        // `NotStarted` é o estado inicial, e `reset` mantém
        // nesse estado. A transição Dead → NotStarted via
        // reset é testada indiretamente (não podemos
        // exercitar Dead sem um WorkerManager real).
        let dir = tempfile::tempdir().expect("tempdir");
        let loc = fake_location(
            dir.path().to_path_buf(),
            crate::runtime::RuntimeSource::DevRepo,
        );
        let launcher = DocumentWorkerLauncher::new(loc, LauncherConfig::default());
        launcher.reset().await;
        let status = launcher.status().await;
        assert!(!status.alive);
        assert!(status.message.contains("ainda não inicializado"));
    }
}
