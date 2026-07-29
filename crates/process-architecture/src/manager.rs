//! `WorkerManager` — modelo de ator (ADR-0015).
//!
//! A Etapa 2A original usou `Arc<Mutex<Box<dyn Pipe>>>` partilhado
//! entre o `invoke`/`ping` e a task de leitura de background. O
//! `MutexGuard` ficava segurado durante `tx.send().await` (no
//! write) e `rx.recv().await` (no read) — deadlock clássico de
//! lock segurado em `.await`. Pior: o mesmo design quebrava
//! **duas invocações concorrentes** (a segunda `invoke` esperava
//! o guard, a primeira segurava o guard esperando a response).
//!
//! ## Decisão: ator, não mutex
//!
//! 1. **Uma task é dona exclusiva do pipe** (`Box<dyn PipeReader>`
//!    + `Box<dyn PipeWriter>` movidos pra dentro dela).
//! 2. **`invoke`/`ping`/`shutdown` mandam comandos por um
//!    `mpsc::Sender<ManagerCommand>` interno** (o
//!    `state.command_tx`), junto com um `oneshot::Sender` pra
//!    receber a resposta.
//! 3. **A task do ator** mantém um
//!    `HashMap<RequestId, PendingResponse>` indexado pelo
//!    `request_id` que **ela mesma gera** quando recebe o comando.
//!    Quando uma `IpcMessage` chega, o `request_id` da mensagem
//!    casa com o do pending, e o `oneshot::Sender` correspondente
//!    é despachado.
//! 4. **Concorrência de `invoke`s paralelos** cai de graça — cada
//!    invoke tem seu próprio `oneshot`. O `request_id` correlaciona
//!    request com response sem serializar no caller.
//! 5. **`std::sync::Mutex<HashMap<...>>` é o único lock do design**,
//!    e ele é usado em **operações pontuais** (`insert`/`remove`)
//!    que **não seguram `.await`**. É o que o `-D
//!    clippy::await_holding_lock` (no `verify.ps1` e no
//!    `ci.yml`) enforça.
//!
//! ## Handshake
//!
//! O `WorkerManager::spawn_in_process` faz o handshake **síncrono**
//! antes de spawnar o ator:
//!
//! 1. O fake server envia um `worker.hello` com o manifesto no
//!    boot (modelando o que o worker real faz ao subir).
//! 2. O manager lê esse `hello`, gera um `WorkerAuth` (UUID v4), e
//!    responde com `app.ack` carregando o token.
//! 3. Daí em diante, toda `tool.invoke` carrega o token — o
//!    `FakeWorker` valida contra o auth que recebeu.
//!
//! A task do ator só entra em cena **depois** do handshake — ela
//! não precisa se preocupar com o `hello` inicial. Concorrência
//! começa a partir das `invoke`/`ping`/`shutdown` vindas do caller.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, RwLock};
use uuid::Uuid;

use crate::error::ProcessError;
use crate::fake::FakeWorkerConfig;
use crate::pipes::{PipeReader, PipeWriter};
use crate::protocol::{
    IpcMessage, IpcOp, RequestId, WorkerAuth, WorkerHealthSnapshot, WorkerId, WorkerManifest,
};

/// Comando que o caller (`WorkerHandle`) envia ao ator. O ator
/// gera o `request_id` internamente (evita carregar UUID pelo
/// canal pra reduzir barulho) e registra a pending ANTES de
/// escrever no pipe — assim, mesmo que a response chegue
/// "rápido demais", o dispatch encontra o `oneshot::Sender`.
enum ManagerCommand {
    /// Executa uma `tool.invoke` e devolve o payload do
    /// `worker.tool.result` (ou um erro se a response for
    /// `worker.error` ou se o worker cair).
    Invoke {
        /// Payload da `tool.invoke` (opaco — o schema vem do
        /// `op` específico da tool, documentado no
        /// `shared-contracts`).
        payload: Value,
        /// Canal de resposta — o ator envia `Ok(payload do
        /// tool.result)` ou `Err(...)`.
        reply: oneshot::Sender<Result<Value, ProcessError>>,
    },
    /// Envia `app.ping` e devolve o payload do `worker.pong`.
    Ping {
        /// Canal de resposta — o ator envia `Ok(payload do
        /// pong)` ou `Err(...)`.
        reply: oneshot::Sender<Result<Value, ProcessError>>,
    },
    /// Envia `app.shutdown` gracioso. O worker morre; o `read_line`
    /// do ator devolve `None`; o loop termina e o pending de
    /// shutdown é despachado com `Ok(Value::Null)`. O caller
    /// (`WorkerManager::shutdown`) ignora o payload — a
    /// confirmação real de que o worker morreu vem do
    /// `actor_task.await`, não do oneshot.
    Shutdown {
        /// Canal de resposta — despachado com `Ok(Value::Null)`
        /// quando o loop do ator termina. **O caller ignora o
        /// payload**; é só pra acordar o `WorkerManager::shutdown`
        /// se ele estiver esperando o `rx.await`.
        reply: oneshot::Sender<Result<Value, ProcessError>>,
    },
}

/// O que está esperando uma response do worker.
struct PendingResponse {
    /// `Ok(Value)` no caso de sucesso, `Err(ProcessError)` em
    /// falha (incluindo `Timeout` se o caller desistir antes).
    reply: oneshot::Sender<Result<Value, ProcessError>>,
    /// Tag que diz ao `handle_incoming` que tipo de response é
    /// esperada (`Pong` pra Ping, `ToolResult` pra Invoke, vazio
    /// pra Shutdown).
    kind: PendingKind,
}

/// O que o pending está esperando do worker. `Shutdown` é
/// especial — não há response esperada; a "response" é o
/// `read_line` devolver `None` (EOF) e o loop do ator terminar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    /// Espera `worker.tool.result` ou `worker.error`.
    Invoke,
    /// Espera `worker.pong`.
    Ping,
    /// Não espera response — o "fim" vem do EOF.
    Shutdown,
}

/// Estado partilhado entre o `WorkerHandle` e a task do ator.
///
/// **`std::sync::Mutex` no `pending`** é proposital: as operações
/// dentro do lock (`insert`/`remove`) são síncronas e curtas, e o
/// `MutexGuard` **nunca** segura um `.await`. Isso é o que o
/// `-D clippy::await_holding_lock` (no `verify.ps1` e no
/// `ci.yml`) enforça em tempo de compilação.
struct WorkerState {
    /// Canal do caller pro ator — o `WorkerHandle` clona o
    /// `Sender` e manda comandos. Quando **todos** os clones são
    /// dropados, o `Receiver` no ator devolve `None` e o loop
    /// termina (cleanup path).
    command_tx: mpsc::Sender<ManagerCommand>,
    /// Tabela de requests em voo. `Mutex<HashMap<...>>` —
    /// `std::sync::Mutex` (não `tokio::sync::Mutex`) porque o
    /// lock é **síncrono** e curto. Segurar o `MutexGuard`
    /// através de um `.await` violaria a trava de CI; aqui ele
    /// é liberado antes de qualquer `.await`.
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    /// Saúde observada (atualizada pelo ator a cada `pong`
    /// recebido, ou marcada `Unhealthy` em shutdown/EOF).
    health: Arc<RwLock<WorkerHealthSnapshot>>,
    /// Token de auth (imutável depois do handshake; o ator inclui
    /// em todo `app.ping`/`app.shutdown`/`tool.invoke`).
    auth: WorkerAuth,
    /// Worker ID (do manifesto do `worker.hello`).
    worker_id: WorkerId,
    /// Manifesto (imutável depois do handshake).
    manifest: Arc<WorkerManifest>,
}

/// Handle de uso — o que o caller carrega. `Clone` (vários handles
/// pra o mesmo worker são permitidos; o `Arc<WorkerState>`
/// partilha o estado). É a **única coisa** que o caller toca
/// depois do `WorkerManager::spawn_in_process`.
#[derive(Clone)]
pub struct WorkerHandle {
    state: Arc<WorkerState>,
    /// Timeout default pra `invoke` (sem timeout explícito).
    /// 30s é folgado o suficiente pra tools reais (Word, Excel,
    /// PDF) sem pendurar indefinidamente em caso de worker
    /// travado.
    default_invoke_timeout: Duration,
}

impl WorkerHandle {
    /// Executa uma `tool.invoke` e devolve o payload do
    /// `worker.tool.result` (ou erro).
    ///
    /// # Erros
    /// - `ProcessError::Timeout` se o worker não responder
    ///   dentro de `timeout` (default 30s).
    /// - `ProcessError::Protocol` se a response for
    ///   `worker.error` (código preservado em `message`).
    /// - `ProcessError::Transport` se o canal pro ator cair
    ///   (ator já terminou — provavelmente `shutdown` foi
    ///   chamado).
    pub async fn invoke(&self, payload: Value) -> Result<Value, ProcessError> {
        self.invoke_with_timeout(payload, self.default_invoke_timeout)
            .await
    }

    /// Mesmo [`invoke`], mas com timeout customizado. Útil pra
    /// testes que querem provar que um invoke lento falha com
    /// `Timeout` em vez de pendurar.
    pub async fn invoke_with_timeout(
        &self,
        payload: Value,
        timeout: Duration,
    ) -> Result<Value, ProcessError> {
        let (tx, rx) = oneshot::channel();
        let cmd = ManagerCommand::Invoke { payload, reply: tx };
        self.state
            .command_tx
            .send(cmd)
            .await
            .map_err(|_| ProcessError::Transport {
                message: "ator não está mais aceitando commands".to_string(),
            })?;
        let fut = async move {
            rx.await.map_err(|_| ProcessError::Transport {
                message: "ator cancelou o invoke antes de responder".to_string(),
            })?
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => Err(ProcessError::Timeout {
                worker_id: self.state.worker_id.to_string(),
                timeout_ms: timeout.as_millis() as u32,
            }),
        }
    }

    /// Envia `app.ping` e devolve o payload do `worker.pong`
    /// (que inclui `status` e `env_received` no caso do fake).
    pub async fn ping(&self) -> Result<Value, ProcessError> {
        self.ping_with_timeout(self.default_invoke_timeout).await
    }

    /// Mesmo [`ping`], mas com timeout customizado.
    pub async fn ping_with_timeout(&self, timeout: Duration) -> Result<Value, ProcessError> {
        let (tx, rx) = oneshot::channel();
        let cmd = ManagerCommand::Ping { reply: tx };
        self.state
            .command_tx
            .send(cmd)
            .await
            .map_err(|_| ProcessError::Transport {
                message: "ator não está mais aceitando commands".to_string(),
            })?;
        let fut = async move {
            rx.await.map_err(|_| ProcessError::Transport {
                message: "ator cancelou o ping antes de responder".to_string(),
            })?
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => Err(ProcessError::Timeout {
                worker_id: self.state.worker_id.to_string(),
                timeout_ms: timeout.as_millis() as u32,
            }),
        }
    }

    /// Snapshot atual da saúde observada. **Não** é um healthcheck
    /// ativo (chame [`ping`](Self::ping) pra forçar um); é a
    /// última que o ator registrou.
    pub async fn health_snapshot(&self) -> WorkerHealthSnapshot {
        self.state.health.read().await.clone()
    }

    /// Worker ID (do manifesto do `worker.hello`).
    #[must_use]
    pub fn worker_id(&self) -> &WorkerId {
        &self.state.worker_id
    }

    /// Manifesto do worker (imutável depois do handshake).
    #[must_use]
    pub fn manifest(&self) -> &WorkerManifest {
        &self.state.manifest
    }
}

/// Configuração de spawn do worker.
#[derive(Debug, Clone)]
pub struct WorkerSpawnConfig {
    /// Timeout default pra `invoke`/`ping` (sem timeout
    /// explícito). Default: 30s.
    pub default_timeout_ms: u32,
    /// Token de auth pré-definido. `None` (default) gera um UUID
    /// v4 novo. Útil pra testes que querem um token conhecido
    /// (ex.: `"test-token"`) em vez de um aleatório.
    pub auth_token: Option<WorkerAuth>,
}

impl Default for WorkerSpawnConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
            auth_token: None,
        }
    }
}

/// `WorkerManager` — factory e owner do ciclo de vida do worker.
///
/// É o **dono** do `JoinHandle` da task do ator e do `JoinHandle`
/// da task do fake server. O caller tipicamente faz:
///
/// ```ignore
/// let (manager, handle) = WorkerManager::spawn_in_process(
///     FakeWorkerConfig::default(),
///     WorkerSpawnConfig::default(),
/// )
/// .await?;
///
/// let result = handle.invoke(json!({"path": "..."})).await?;
/// let _ = handle.ping().await?;
///
/// manager.shutdown().await?;
/// ```
///
/// Se o `WorkerManager` for **dropado** sem `shutdown` explícito,
/// o `Drop` faz best-effort: dropar o `request_tx` faz a task do
/// ator terminar; mas o `JoinHandle` fica em background até o
/// shutdown real (EOF do pipe). É leak, não bug — o processo está
/// saindo mesmo.
pub struct WorkerManager {
    /// `JoinHandle` da task do **fake server** (lado worker do
    /// fake). O `shutdown` espera ela terminar pra confirmar que
    /// o worker morreu limpo.
    server_task: Option<tokio::task::JoinHandle<()>>,
    /// `JoinHandle` da task do **ator** (lado manager). O
    /// `shutdown` espera ela terminar depois de receber o
    /// `app.shutdown` e detectar EOF.
    actor_task: Option<tokio::task::JoinHandle<()>>,
    /// `command_tx` mantido **além** do `state` do `handle`. O
    /// `shutdown` clona esse `Sender` e manda `ManagerCommand::
    /// Shutdown`. Manter aqui (em vez de no `handle`) garante
    /// que o `shutdown` consegue mandar o comando mesmo se o
    /// `handle` foi dropado.
    command_tx: mpsc::Sender<ManagerCommand>,
}

impl WorkerManager {
    /// Spawna um worker in-process (fake) e faz o handshake.
    ///
    /// Devolve `(WorkerManager, WorkerHandle)`. O `WorkerManager`
    /// é o **owner** do ciclo de vida (chame `shutdown` pra
    /// encerrar limpo); o `WorkerHandle` é o **cliente** (use
    /// `invoke`/`ping`).
    ///
    /// # Erros
    /// - `ProcessError::Protocol` se o `worker.hello` enviado pelo
    ///   fake no boot não decodificar (improvável — o fake é
    ///   Rust, mesmo código de encode/decode).
    /// - `ProcessError::Transport` se a escrita do `app.ack`
    ///   falhar (improvável — o canal `mpsc` foi acabado de criar).
    pub async fn spawn_in_process(
        config: FakeWorkerConfig,
        spawn: WorkerSpawnConfig,
    ) -> Result<(WorkerManager, WorkerHandle), ProcessError> {
        let timeout = Duration::from_millis(spawn.default_timeout_ms as u64);
        let auth = spawn
            .auth_token
            .unwrap_or_else(|| WorkerAuth::new(Uuid::new_v4().to_string()));

        // 1. Spawna o fake server. Ele envia `worker.hello` no
        //    boot — modelando o que o worker real faz ao subir.
        let fake = crate::fake::spawn_fake_worker(config);

        // 2. Lê o `worker.hello` enviado pelo fake. **Sem timeout
        //    aqui** — o `mpsc::channel` do fake é finito (cap 64),
        //    e o `hello` é a primeira mensagem. Se a leitura
        //    falhar, é bug do fake, não do worker.
        let mut hello_reader = fake.reader;
        let hello_line = hello_reader
            .read_line()
            .await
            .map_err(|e| ProcessError::Transport {
                message: format!("leitura do `worker.hello` falhou: {e}"),
            })?
            .ok_or_else(|| ProcessError::Transport {
                message: "fake fechou antes de enviar `worker.hello`".to_string(),
            })?;
        let (hello_msg, _) =
            IpcMessage::decode_line(&hello_line).map_err(|e| ProcessError::Protocol {
                message: format!("decode do `worker.hello` falhou: {e}"),
            })?;
        if hello_msg.op != IpcOp::Hello {
            return Err(ProcessError::Protocol {
                message: format!(
                    "primeira mensagem do fake deveria ser `worker.hello`, veio {:?}",
                    hello_msg.op
                ),
            });
        }
        let manifest: WorkerManifest =
            serde_json::from_value(hello_msg.payload).map_err(|e| ProcessError::Protocol {
                message: format!("manifesto inválido: {e}"),
            })?;
        let worker_id = manifest.worker_id.clone();

        // 3. Envia `app.ack` com o token. O fake server salva o
        //    token no seu `auth` interno e valida toda
        //    `tool.invoke` subsequente contra ele.
        let ack = IpcMessage::ack(hello_msg.request_id, auth.clone());
        let ack_line = ack.encode_line().map_err(|e| ProcessError::Protocol {
            message: format!("encode do `app.ack` falhou: {e}"),
        })?;
        let ack_writer = fake.writer.clone();
        ack_writer
            .write_line(&ack_line)
            .await
            .map_err(|e| ProcessError::Transport {
                message: format!("write do `app.ack` falhou: {e}"),
            })?;

        // 4. Constrói o estado partilhado.
        let (command_tx, command_rx) = mpsc::channel::<ManagerCommand>(64);
        let health = fake.health.clone();
        let state = Arc::new(WorkerState {
            command_tx: command_tx.clone(),
            pending: Mutex::new(HashMap::new()),
            health,
            auth,
            worker_id,
            manifest: Arc::new(manifest),
        });

        // 5. Spawna a task do **ator** — fica com o `reader` e a
        //    `writer` (movidos). A partir daqui, o ator é o único
        //    dono do pipe.
        let actor_state = state.clone();
        let actor_writer: Box<dyn PipeWriter> = Box::new(fake.writer);
        let actor_reader: Box<dyn PipeReader> = Box::new(hello_reader);
        let actor_task = tokio::spawn(run_actor(
            actor_reader,
            actor_writer,
            command_rx,
            actor_state,
        ));

        let handle = WorkerHandle {
            state: state.clone(),
            default_invoke_timeout: timeout,
        };

        Ok((
            WorkerManager {
                server_task: Some(fake.server_task),
                actor_task: Some(actor_task),
                command_tx,
            },
            handle,
        ))
    }

    /// Shutdown gracioso. Envia `app.shutdown`, espera a task do
    /// ator terminar (ela detecta EOF do pipe e sai do loop) e
    /// espera a task do fake server terminar (ela saiu no
    /// `Shutdown`).
    ///
    /// # Erros
    /// - `ProcessError::Transport` se o canal pro ator já estiver
    ///   fechado (ator caiu sozinho).
    /// - `ProcessError::JoinError` embrulhado se uma das tasks
    ///   panicou.
    pub async fn shutdown(mut self) -> Result<(), ProcessError> {
        // Envia o comando de shutdown. O ator escreve
        // `app.shutdown`, e o fake server morre depois de
        // processar. O `read_line` do ator devolve `None` (EOF) e
        // o loop termina — o pending de Shutdown é despachado com
        // `Ok(())`.
        let (tx, rx) = oneshot::channel();
        let cmd = ManagerCommand::Shutdown { reply: tx };
        self.command_tx
            .send(cmd)
            .await
            .map_err(|_| ProcessError::Transport {
                message: "ator não está mais aceitando commands".to_string(),
            })?;
        rx.await.map_err(|_| ProcessError::Transport {
            message: "ator cancelou o shutdown antes de responder".to_string(),
        })??;

        // Espera a task do ator terminar.
        if let Some(task) = self.actor_task.take() {
            task.await.map_err(|e| ProcessError::Transport {
                message: format!("ator panicou: {e}"),
            })?;
        }
        // Espera a task do fake server terminar (ela já saiu
        // quando viu o `Shutdown`).
        if let Some(task) = self.server_task.take() {
            let _ = task.await;
        }
        Ok(())
    }
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        // Best-effort: dropar o `command_tx` faz o `command_rx`
        // do ator devolver `None`, e o loop termina. Não é await
        // (Drop não pode ser async) — confiamos no EOF do pipe
        // pra realmente encerrar. O `JoinHandle` da task do ator
        // fica em background até o processo sair; o `fake.server_task`
        // também.
        //
        // Se o caller quer garantir shutdown limpo, deve chamar
        // `shutdown().await` explicitamente.
    }
}

// ---------------------------------------------------------------------------
// Ator
// ---------------------------------------------------------------------------

/// Loop principal do ator. Roda até:
/// - O `command_rx` fechar (todos os `WorkerHandle` foram
///   dropados sem `shutdown`), **ou**
/// - O `reader.read_line()` devolver `None` (EOF — o worker caiu
///   ou respondeu ao `app.shutdown`), **ou**
/// - O `reader.read_line()` devolver erro (transporte quebrou).
///
/// Em qualquer caso, o cleanup `drain_pending_with_error` é
/// chamado pra acordar os `oneshot::Receiver` em voo com uma
/// resposta definitiva (em vez de deixar eles pendurados).
async fn run_actor(
    mut reader: Box<dyn PipeReader>,
    writer: Box<dyn PipeWriter>,
    mut command_rx: mpsc::Receiver<ManagerCommand>,
    state: Arc<WorkerState>,
) {
    loop {
        tokio::select! {
            // `biased` garante que commands são processados antes
            // de responses — sem isso, um command poderia ficar
            // enfileirado enquanto responses se acumulam, e o
            // pending map cresceria.
            biased;

            cmd = command_rx.recv() => {
                match cmd {
                    Some(cmd) => handle_command(cmd, writer.as_ref(), &state).await,
                    None => {
                        // Todos os `WorkerHandle` foram dropados
                        // sem `shutdown` explícito — sai do loop
                        // pra não pendurar o `JoinHandle` em
                        // background.
                        tracing::debug!("ator: command_rx fechado, saindo");
                        break;
                    }
                }
            }

            line = reader.read_line() => {
                match line {
                    Ok(Some(l)) => handle_incoming(&l, &state).await,
                    Ok(None) => {
                        // EOF — o worker caiu ou respondeu ao
                        // `app.shutdown`. Loop termina; o
                        // `drain_pending` acorda os pendings.
                        tracing::debug!("ator: EOF do pipe, saindo");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(?e, "ator: erro de leitura, saindo");
                        break;
                    }
                }
            }
        }
    }

    // Cleanup — best-effort: derrama pendings pra que
    // `oneshot::Receiver::await` acorde com erro (ou `Ok(())` no
    // caso de Shutdown). Também fecha o writer.
    drain_pending_with_error(&state, "ator terminou");
    let _ = writer.close().await;
}

/// Processa um command vindo do `WorkerHandle`. Gera o
/// `request_id`, registra o pending **antes** de escrever no pipe
/// (pra que a response — mesmo que chegue "rápido demais" — já
/// encontre o `oneshot::Sender`), e escreve a `IpcMessage` no
/// pipe.
async fn handle_command(cmd: ManagerCommand, writer: &dyn PipeWriter, state: &WorkerState) {
    let request_id = Uuid::new_v4();
    let (msg, kind) = match &cmd {
        ManagerCommand::Invoke { payload, .. } => {
            let msg = IpcMessage::tool_invoke(request_id, state.auth.clone(), payload.clone());
            (msg, PendingKind::Invoke)
        }
        ManagerCommand::Ping { .. } => {
            let msg = IpcMessage::ping(request_id, Some(state.auth.clone()));
            (msg, PendingKind::Ping)
        }
        ManagerCommand::Shutdown { .. } => {
            let msg = IpcMessage::shutdown(request_id, Some(state.auth.clone()));
            (msg, PendingKind::Shutdown)
        }
    };

    let reply = match cmd {
        ManagerCommand::Invoke { reply, .. } => reply,
        ManagerCommand::Ping { reply, .. } => reply,
        ManagerCommand::Shutdown { reply, .. } => {
            // O `Shutdown` também usa o mesmo tipo
            // `oneshot::Sender<Result<Value, ProcessError>>` por
            // uniformidade do HashMap de pendings. O payload `Ok`
            // do Shutdown é `Value::Null` (ver
            // `drain_pending_with_error`); o
            // `WorkerManager::shutdown` ignora o payload — a
            // confirmação real de que o worker morreu vem do
            // `actor_task.await`.
            reply
        }
    };

    // Registra o pending **antes** de escrever — se a response
    // chegar antes do `insert` retornar, o `handle_incoming`
    // não vai encontrar o `request_id` e vai logar warning. É
    // impossível na prática (o `mpsc::channel` tem 64 slots, o
    // `command_tx.send` é await e o write do pipe depois é
    // await — single-threaded), mas a ordem é defensiva.
    {
        let mut p = state.pending.lock().expect("pending poisoned");
        p.insert(request_id, PendingResponse { reply, kind });
    }

    // Encode + write. Sem segurar o lock durante `.await`.
    let line = match msg.encode_line() {
        Ok(l) => l,
        Err(e) => {
            // Erro de encode (improvável). Remove o pending e
            // envia o erro pro caller.
            let mut p = state.pending.lock().expect("pending poisoned");
            if let Some(entry) = p.remove(&request_id) {
                let _ = entry.reply.send(Err(ProcessError::Protocol {
                    message: format!("encode falhou: {e}"),
                }));
            }
            return;
        }
    };
    if let Err(e) = writer.write_line(&line).await {
        // Erro de write. Remove o pending e envia o erro pro
        // caller.
        let mut p = state.pending.lock().expect("pending poisoned");
        if let Some(entry) = p.remove(&request_id) {
            let _ = entry.reply.send(Err(ProcessError::Transport {
                message: format!("write falhou: {e}"),
            }));
        }
    }
}

/// Processa uma linha recebida do worker. Despacha pelo
/// `request_id` da mensagem.
async fn handle_incoming(line: &[u8], state: &WorkerState) {
    let msg = match IpcMessage::decode_line(line) {
        Ok((m, _)) => m,
        Err(e) => {
            tracing::warn!(?e, "ator: decode falhou, descartando linha");
            return;
        }
    };

    // Pega o pending correspondente. `remove` libera o lock
    // antes de qualquer `.await` subsequente.
    let entry = {
        let mut p = state.pending.lock().expect("pending poisoned");
        p.remove(&msg.request_id)
    };
    let Some(entry) = entry else {
        // Sem pending — provavelmente um `Pong` ou `ToolResult`
        // chegou **depois** do timeout do caller (race entre
        // `drain_pending_with_error` e a response tardia). É
        // benigno: loga e ignora.
        tracing::debug!(
            ?msg.op,
            ?msg.request_id,
            "ator: response sem pending correspondente (race com timeout)"
        );
        return;
    };

    let result: Result<Value, ProcessError> = match msg.op {
        IpcOp::Pong if entry.kind == PendingKind::Ping => Ok(msg.payload),
        IpcOp::ToolResult if entry.kind == PendingKind::Invoke => Ok(msg.payload),
        IpcOp::Error => {
            // O fake envia `{code, message}` no payload. Preserva
            // o `code` na mensagem de erro.
            let code = msg
                .payload
                .get("code")
                .and_then(|c| c.as_str())
                .unwrap_or("process_protocol_error")
                .to_string();
            Err(ProcessError::Protocol { message: code })
        }
        // Resposta chegou pro tipo errado de pending (ex.: um
        // `Pong` chegou pra um `Invoke`). Loga e responde com
        // erro — o caller não vai conseguir usar o payload.
        other => {
            tracing::warn!(
                pending_kind = ?entry.kind,
                received_op = ?other,
                "ator: response com `op` não casa com pending"
            );
            Err(ProcessError::Protocol {
                message: format!(
                    "response com `op` {:?} não casa com pending {:?}",
                    other, entry.kind
                ),
            })
        }
    };

    let _ = entry.reply.send(result);

    // Atualiza `health` se a response carrega info útil:
    // - `Pong` → mantém `Ok`.
    // - `Error` → marca `Degraded` (worker reportou erro mas
    //   não caiu).
    // - Outros → não mexe.
    if matches!(msg.op, IpcOp::Pong) {
        let mut h = state.health.write().await;
        *h = WorkerHealthSnapshot {
            health: crate::protocol::WorkerHealth::Ok,
            last_check_at: chrono::Utc::now(),
            message: None,
        };
    } else if matches!(msg.op, IpcOp::Error) {
        let mut h = state.health.write().await;
        *h = WorkerHealthSnapshot {
            health: crate::protocol::WorkerHealth::Degraded,
            last_check_at: chrono::Utc::now(),
            message: Some(format!(
                "worker reportou `worker.error` no request {}",
                msg.request_id
            )),
        };
    }
}

/// Acorda todos os pendings com uma resposta definitiva. Usado
/// quando o loop do ator termina (EOF, command_rx fechado, erro
/// de leitura).
fn drain_pending_with_error(state: &WorkerState, reason: &str) {
    let mut p = match state.pending.lock() {
        Ok(p) => p,
        Err(p) => p.into_inner(),
    };
    for (_id, entry) in p.drain() {
        let res = match entry.kind {
            // Shutdown sem response: o worker morreu (EOF) ou o
            // `command_tx` foi dropado. O `WorkerManager::shutdown`
            // vai acordar via o `JoinHandle`, não via este
            // pending. Mas se o ator sair por outra razão (ex.:
            // command_rx fechado porque o handle foi dropado), o
            // pending de Shutdown precisa acordar pra que
            // `WorkerManager::shutdown` não fique pendurado
            // esperando o `rx`.
            PendingKind::Shutdown => Ok(Value::Null),
            // Invoke/Ping pendurados sem response: erro de
            // transporte (ator caiu, EOF inesperado).
            PendingKind::Invoke | PendingKind::Ping => Err(ProcessError::Transport {
                message: reason.to_string(),
            }),
        };
        let _ = entry.reply.send(res);
    }
}
