//! Worker simulado in-process (sem spawn real, sem named pipes).
//!
//! O fake worker é o "lado server" usado pelos testes de
//! integração do `WorkerManager` (em `tests/fake_worker.rs`).
//! Implementa [`PipeReader`] e [`PipeWriter`] (as duas metades do
//! transporte) sobre `tokio::sync::mpsc` channels — o `WorkerManager`
//! monta o ator com o `FakePipeReader` + um clone do
//! `FakePipeWriter`, sem nenhum `Mutex`.
//!
//! ## Protocolo implementado pelo server
//!
//! 1. **Boot**: o server envia um `worker.hello` com o manifesto
//!    (gerado a partir de [`FakeWorkerConfig`]). É o `WorkerManager`
//!    que recebe esse `hello`, gera o `WorkerAuth`, e responde
//!    com `app.ack` carregando o token.
//! 2. **`app.ack`** → o server salva o `WorkerAuth` recebido.
//!    Toda `tool.invoke` posterior é validada contra esse token.
//! 3. **`app.ping`** → `worker.pong` com `status: "ok"` e
//!    `env_received: <env do fake>`. Atualiza `health` pra `Ok`
//!    no primeiro pong.
//! 4. **`app.shutdown`** → server marca `health = Unhealthy` e
//!    termina o loop (causa `read_line` no `WorkerManager` a
//!    devolver `None`).
//! 5. **`tool.invoke`** → server valida o token (se já viu o
//!    `app.ack`) e responde `tool.result` com `{ok: true, echo:
//!    <payload>, env_received: <env>}` — payload preservado
//!    ponta-a-ponta, mais o env que o server enxergou
//!    (testes injetam o `FakeWorkerConfig.env` pra provar que a
//!    allowlist do `env_allowlist` é respeitada: o `OPENAI_API_KEY`
//!    que o test runner tem nunca aparece aqui).
//!
//! ## Por que existe
//!
//! `testing-strategy.md` §"Dados de teste" lista "Worker simulado:
//! implementa o envelope IPC em processo, sem spawn real". Esse é
//! o fake. A Etapa 2B substitui o `mpsc` interno por named pipes
//! reais (Windows), sem mudar a API pública do fake.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{mpsc, RwLock};

use crate::env_allowlist::EnvEntry;
use crate::error::ProcessError;
use crate::pipes::{PipeName, PipeReader, PipeWriter};
use crate::protocol::{
    CompatibilityInfo, Dependency, IpcMessage, IpcOp, WorkerAuth, WorkerHealth,
    WorkerHealthSnapshot, WorkerId, WorkerManifest,
};

/// Buffer de leitura com busca por `\n`. Reusa o helper que a
/// versão antiga (`Pipe` único) já tinha — continua sendo o
/// algoritmo certo: append no buffer, devolve a primeira linha
/// completa quando encontra `\n`, devolve o resto no `flush_eof`
/// quando o canal fecha.
#[derive(Debug, Default)]
struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    /// Alimenta o buffer com mais bytes; devolve `Some(line)` se
    /// encontrou `\n`, ou `None` se precisa de mais.
    fn feed(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            Some(line)
        } else {
            None
        }
    }

    /// EOF — devolve o que sobrou (sem `\n`) ou `None` se vazio.
    fn flush_eof(&mut self) -> Option<Vec<u8>> {
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

/// Metade de leitura do fake (consumida pela task do ator).
pub struct FakePipeReader {
    rx: mpsc::Receiver<Vec<u8>>,
    line_buf: LineBuffer,
}

impl FakePipeReader {
    /// Constrói a partir do `Receiver` que veio do
    /// `spawn_fake_worker`.
    #[must_use]
    pub fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            line_buf: LineBuffer::default(),
        }
    }
}

#[async_trait]
impl PipeReader for FakePipeReader {
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, ProcessError> {
        // Drena o line_buf primeiro.
        if let Some(line) = self.line_buf.flush_eof() {
            if line.contains(&b'\n') {
                let pos = line.iter().position(|&b| b == b'\n').unwrap();
                let head = line[..=pos].to_vec();
                let tail = line[pos + 1..].to_vec();
                if !tail.is_empty() {
                    self.line_buf.buf = tail;
                }
                return Ok(Some(head));
            }
            return Ok(Some(line));
        }
        loop {
            match self.rx.recv().await {
                Some(chunk) => {
                    if let Some(line) = self.line_buf.feed(&chunk) {
                        return Ok(Some(line));
                    }
                }
                None => return Ok(None), // EOF
            }
        }
    }
}

/// Metade de escrita do fake. **É `Clone`** — o `WorkerManager`
/// carrega um clone e a task do fake fica com o original. A
/// `mpsc::Sender` por baixo já é concorrente (vários writers
/// mandam pro mesmo canal sem lock).
#[derive(Clone)]
pub struct FakePipeWriter {
    tx: mpsc::Sender<Vec<u8>>,
    closed: Arc<AtomicBool>,
}

impl FakePipeWriter {
    /// Constrói a partir do `Sender` que veio do
    /// `spawn_fake_worker`.
    #[must_use]
    pub fn new(tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            tx,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl PipeWriter for FakePipeWriter {
    async fn write_line(&self, line: &[u8]) -> Result<(), ProcessError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProcessError::Transport {
                message: "fake writer fechado".to_string(),
            });
        }
        self.tx
            .send(line.to_vec())
            .await
            .map_err(|_| ProcessError::Transport {
                message: "fake reader caiu".to_string(),
            })?;
        Ok(())
    }

    async fn close(&self) -> Result<(), ProcessError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

/// Resultado do [`spawn_fake_worker`].
pub struct FakeWorkerHandle {
    /// Metade de leitura (entra na task do ator).
    pub reader: FakePipeReader,
    /// Metade de escrita (clone dela entra no `WorkerHandle`).
    pub writer: FakePipeWriter,
    /// `JoinHandle` da task do server (pra testes que querem esperar
    /// a morte limpa — `handle.server_task.await?`).
    pub server_task: tokio::task::JoinHandle<()>,
    /// Snapshot da saúde observada pelo server.
    pub health: Arc<RwLock<WorkerHealthSnapshot>>,
    /// Manifesto que o server anunciou no `worker.hello` de boot.
    /// O `WorkerManager` recebe esse manifesto antes de montar o
    /// seu próprio `WorkerManifest` interno.
    pub manifest: WorkerManifest,
}

/// Configuração do fake worker.
#[derive(Debug, Clone)]
pub struct FakeWorkerConfig {
    /// Worker ID reportado no manifesto.
    pub worker_id: WorkerId,
    /// Versão reportada.
    pub version: String,
    /// Capacidades.
    pub capabilities: Vec<String>,
    /// Env que o server enxerga (testes de allowlist injetam aqui).
    pub env: BTreeMap<String, String>,
    /// Atraso artificial antes de responder `tool.invoke`/`ping`/
    /// `shutdown`, em milissegundos. `None` (default) responde
    /// síncrono. Usado pelos testes de **timeout**: o server
    /// dorme `slow_response_ms` antes de mandar a response,
    /// fazendo o `WorkerHandle::invoke_with_timeout` falhar
    /// com `ProcessError::Timeout`.
    pub slow_response_ms: Option<u64>,
}

impl Default for FakeWorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: WorkerId::new("fake-worker"),
            version: "0.1.0".to_string(),
            capabilities: vec!["fake.invoke".to_string()],
            env: BTreeMap::new(),
            slow_response_ms: None,
        }
    }
}

impl FakeWorkerConfig {
    /// Define env entries (substitui tudo).
    #[must_use]
    pub fn with_env(mut self, entries: &[EnvEntry]) -> Self {
        self.env.clear();
        for (k, v) in entries {
            self.env.insert(k.clone(), v.clone());
        }
        self
    }

    /// Define o atraso artificial antes de cada response (em ms).
    #[must_use]
    pub fn with_slow_response(mut self, ms: u64) -> Self {
        self.slow_response_ms = Some(ms);
        self
    }
}

/// Spawna o fake worker e devolve o handle (reader + writer + task
/// + health + manifesto).
///
/// **Comportamento:** antes de começar o loop, o server envia um
/// `worker.hello` com o manifesto. Isso modela o que o worker real
/// faz ao subir (anuncia-se pro app). O `WorkerManager` consome
/// esse `hello`, gera o `WorkerAuth`, e responde com `app.ack` —
/// handshake completo.
pub fn spawn_fake_worker(config: FakeWorkerConfig) -> FakeWorkerHandle {
    // Capacidade 64 — mais que suficiente pra testes (mesma
    // capacidade da versão anterior).
    let (client_tx, mut server_rx) = mpsc::channel::<Vec<u8>>(64);
    let (server_tx, client_rx) = mpsc::channel::<Vec<u8>>(64);

    let manifest = WorkerManifest {
        worker_id: config.worker_id.clone(),
        version: config.version.clone(),
        capabilities: config.capabilities.clone(),
        dependencies: vec![Dependency {
            name: "fake-runtime".to_string(),
            version: "0.1.0".to_string(),
            source: Some("fake".to_string()),
        }],
        health: WorkerHealth::Unhealthy, // vira `Ok` no primeiro pong
        compatibility: CompatibilityInfo {
            min_os: "any".to_string(),
            arch: "any".to_string(),
            min_runtime: "fake".to_string(),
        },
    };

    let health = Arc::new(RwLock::new(WorkerHealthSnapshot {
        health: WorkerHealth::Unhealthy,
        last_check_at: Utc::now(),
        message: None,
    }));
    let health_clone = health.clone();

    let manifest_for_server = manifest.clone();
    let env_for_server = config.env.clone();

    let server_task = tokio::spawn(async move {
        let mut line_buf = LineBuffer::default();
        let mut auth: Option<WorkerAuth> = None;

        // **Boot**: envia `worker.hello` com o manifesto. É o
        // equivalente do "worker anunciou-se pro app" — sem isso,
        // o `WorkerManager` não tem como construir o seu próprio
        // `WorkerManifest` interno.
        let hello = IpcMessage::hello(manifest_for_server);
        let hello_line = hello.encode_line().expect("encode hello");
        if server_tx.send(hello_line).await.is_err() {
            // Reader caiu antes de subir — improvável, mas não
            // trava o server.
            return;
        }

        loop {
            // Lê a próxima linha do `server_rx`. O loop interno
            // espera mais chunks até encontrar um `\n`.
            let line = loop {
                match server_rx.recv().await {
                    Some(chunk) => {
                        if let Some(l) = line_buf.feed(&chunk) {
                            break l;
                        }
                    }
                    None => {
                        if let Some(remaining) = line_buf.flush_eof() {
                            break remaining;
                        }
                        return;
                    }
                }
            };

            let msg = match IpcMessage::decode_line(&line) {
                Ok((m, _)) => m,
                Err(e) => {
                    tracing::warn!(?e, "fake_worker: decode falhou");
                    continue;
                }
            };

            match msg.op {
                IpcOp::Hello => {
                    // O server já enviou o próprio `worker.hello`
                    // no boot; um `Hello` vindo do outro lado é
                    // protocolo errado — ignora.
                    tracing::debug!("fake_worker: Hello recebido após boot, ignorando");
                }
                IpcOp::Ack => {
                    auth = msg.auth.clone();
                }
                IpcOp::Ping => {
                    if let Some(ms) = config.slow_response_ms {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    let pong = IpcMessage {
                        protocol_version: IpcMessage::current_protocol_version(),
                        request_id: msg.request_id,
                        op: IpcOp::Pong,
                        payload: serde_json::json!({
                            "status": "ok",
                            "env_received": env_for_server,
                        }),
                        auth: None,
                    };
                    let line = pong.encode_line().expect("encode pong");
                    if server_tx.send(line).await.is_err() {
                        return;
                    }
                    // Marca saúde como `Ok` no primeiro pong.
                    let mut h = health_clone.write().await;
                    *h = WorkerHealthSnapshot {
                        health: WorkerHealth::Ok,
                        last_check_at: Utc::now(),
                        message: None,
                    };
                }
                IpcOp::Shutdown => {
                    let mut h = health_clone.write().await;
                    *h = WorkerHealthSnapshot {
                        health: WorkerHealth::Unhealthy,
                        last_check_at: Utc::now(),
                        message: Some("shutdown recebido".to_string()),
                    };
                    return;
                }
                IpcOp::ToolInvoke => {
                    if let Some(ms) = config.slow_response_ms {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    // Verifica token se o server já viu um `app.ack`.
                    if let Some(expected) = &auth {
                        match &msg.auth {
                            Some(got) if got == expected => {}
                            _ => {
                                let err = IpcMessage {
                                    protocol_version: IpcMessage::current_protocol_version(),
                                    request_id: msg.request_id,
                                    op: IpcOp::Error,
                                    payload: serde_json::json!({
                                        "code": "process_unauthorized",
                                        "message": "token ausente ou inválido",
                                    }),
                                    auth: None,
                                };
                                let line = err.encode_line().expect("encode err");
                                let _ = server_tx.send(line).await;
                                continue;
                            }
                        }
                    }
                    let result = IpcMessage {
                        protocol_version: IpcMessage::current_protocol_version(),
                        request_id: msg.request_id,
                        op: IpcOp::ToolResult,
                        payload: serde_json::json!({
                            "ok": true,
                            "echo": msg.payload,
                            "env_received": env_for_server,
                        }),
                        auth: None,
                    };
                    let line = result.encode_line().expect("encode result");
                    if server_tx.send(line).await.is_err() {
                        return;
                    }
                }
                _ => {
                    tracing::debug!(?msg.op, "fake_worker: op ignorado");
                }
            }
        }
    });

    let reader = FakePipeReader::new(client_rx);
    let writer = FakePipeWriter::new(client_tx);
    FakeWorkerHandle {
        reader,
        writer,
        server_task,
        health,
        manifest,
    }
}

/// Constrói um `PipeName` único pra teste (evita colisão entre
/// testes paralelos).
#[must_use]
pub fn unique_pipe_name(prefix: &str) -> PipeName {
    use uuid::Uuid;
    let id = Uuid::new_v4().simple().to_string();
    let name = format!("frederico-test-{prefix}-{id}");
    // O `unique_pipe_name` é só pra testes — se o nome for
    // rejeitado (improvável), panic é aceitável.
    PipeName::new(name).expect("nome de pipe gerado é válido")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn fake_reader_writer_split_roundtrip() {
        // Garante que as duas metades se conversam ponta-a-ponta
        // — a `PipeWriter` envia, a `PipeReader` lê. Sem o
        // `WorkerManager` no caminho.
        let mut handle = spawn_fake_worker(FakeWorkerConfig::default());
        // O server enviou `worker.hello` no boot — drena a linha
        // antes de testar o split.
        let hello_line = handle
            .reader
            .read_line()
            .await
            .expect("read hello")
            .expect("hello presente");
        let (hello, _) = IpcMessage::decode_line(&hello_line).expect("decode hello");
        assert_eq!(hello.op, IpcOp::Hello);

        // Envia `app.ack` com token.
        let ack = IpcMessage::ack(hello.request_id, WorkerAuth::new("t1"));
        handle
            .writer
            .write_line(&ack.encode_line().expect("encode ack"))
            .await
            .expect("write ack");

        // Envia `app.ping`.
        let ping_id = uuid::Uuid::new_v4();
        let ping = IpcMessage::ping(ping_id, Some(WorkerAuth::new("t1")));
        handle
            .writer
            .write_line(&ping.encode_line().expect("encode ping"))
            .await
            .expect("write ping");

        // Lê a resposta `worker.pong`.
        let pong_line = handle
            .reader
            .read_line()
            .await
            .expect("read pong")
            .expect("pong presente");
        let (pong, _) = IpcMessage::decode_line(&pong_line).expect("decode pong");
        assert_eq!(pong.op, IpcOp::Pong);
        assert_eq!(pong.request_id, ping_id);

        // Envia `app.shutdown`.
        let shutdown_id = uuid::Uuid::new_v4();
        let shutdown = IpcMessage::shutdown(shutdown_id, Some(WorkerAuth::new("t1")));
        handle
            .writer
            .write_line(&shutdown.encode_line().expect("encode shutdown"))
            .await
            .expect("write shutdown");

        // A próxima leitura devolve `None` (server caiu).
        let eof = handle.reader.read_line().await.expect("read eof");
        assert!(eof.is_none(), "server deveria ter fechado");

        // Espera a task do server terminar limpo.
        let _ = handle.server_task.await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_is_clone_and_idempotent_close() {
        // `PipeWriter` precisa ser `Clone` (o `WorkerManager`
        // carrega um clone) e `close` precisa ser idempotente.
        let handle = spawn_fake_worker(FakeWorkerConfig::default());
        let writer_a = handle.writer.clone();
        let writer_b = handle.writer.clone();
        // Consome o `worker.hello` pra não deixar o server
        // pendurado.
        let mut reader = handle.reader;
        let _ = reader.read_line().await.expect("read hello");

        // `close` em qualquer um fecha a flag compartilhada.
        writer_a.close().await.expect("close a");
        writer_b.close().await.expect("close b (idempotente)");
        // Write após `close` falha com `Transport`.
        let res = writer_a
            .write_line(b"qualquer\n")
            .await
            .expect_err("write após close tem que falhar");
        assert!(matches!(res, ProcessError::Transport { .. }));
    }
}
