//! Worker simulado in-process (sem spawn real, sem named pipes).
//!
//! Implementa o `Pipe` trait com `tokio::sync::mpsc` channels, e
//! expõe uma função [`spawn_fake_worker`] que cria o par
//! (client-side, server-side) e roda o server numa `tokio::spawn`.
//!
//! O server entende só os opcodes essenciais:
//! - `worker.hello` → ecoa o `worker.ack` com token fixo
//!   `"fake-token"` + atualiza saúde pra `Ok`
//! - `app.ping` → `worker.pong`
//! - `app.shutdown` → fecha
//! - `tool.invoke` → devolve `{"ok": true, "echo": <payload>}` no
//!   `tool.result` (suficiente pra testar o `WorkerManager::invoke`
//!   end-to-end sem o worker real)
//!
//! Testes E2E em `tests/fake_worker.rs` validam:
//! - handshake: `hello` → `ack` (com auth) → próximas mensagens
//!   carregam o token
//! - invoke: `tool.invoke` round-trip preserva payload
//! - timeout: invoke com `timeout_ms = 0` falha com `ProcessError::Timeout`
//! - shutdown: `app.shutdown` fecha limpo
//! - env allowlist: variável `OPENAI_API_KEY` injetada no test
//!   runner **não** aparece no env recebido pelo fake worker
//!
//! **Por que existe:** `testing-strategy.md` §"Dados de teste"
//! lista "Worker simulado: implementa o envelope IPC em processo,
//! sem spawn real". Esse é o fake.

use std::collections::BTreeMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::env_allowlist::EnvEntry;
use crate::error::ProcessError;
use crate::pipes::{Pipe, PipeName};
use crate::protocol::{
    CompatibilityInfo, Dependency, IpcMessage, IpcOp, WorkerAuth, WorkerHealth, WorkerId,
    WorkerManifest,
};

/// Buffer de leitura com busca por `\n`.
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

/// Half-client do fake (usado pelo `WorkerManager`).
pub struct FakePipeClient {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    line_buf: LineBuffer,
    closed: bool,
}

impl FakePipeClient {
    fn new(tx: mpsc::Sender<Vec<u8>>, rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            tx,
            rx,
            line_buf: LineBuffer::default(),
            closed: false,
        }
    }
}

#[async_trait]
impl Pipe for FakePipeClient {
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, ProcessError> {
        // Drena o line_buf primeiro.
        if let Some(line) = self.line_buf.flush_eof() {
            // `flush_eof` só retorna se tiver algo sem `\n`; vamos
            // checar se por acaso havia um `\n` que perdemos.
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

    async fn write_line(&mut self, line: &[u8]) -> Result<(), ProcessError> {
        if self.closed {
            return Err(ProcessError::Transport {
                message: "fake client fechado".to_string(),
            });
        }
        self.tx
            .send(line.to_vec())
            .await
            .map_err(|_| ProcessError::Transport {
                message: "fake server caiu".to_string(),
            })?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), ProcessError> {
        self.closed = true;
        Ok(())
    }
}

/// Resultado do [`spawn_fake_worker`].
pub struct FakeWorkerHandle {
    /// Lado client (o que o `WorkerManager` segura).
    pub client: FakePipeClient,
    /// `JoinHandle` da task do server (pra testes que querem esperar
    /// a morte limpa).
    pub server_task: tokio::task::JoinHandle<()>,
    /// Snapshot da saúde atualizada pelo server.
    pub health: std::sync::Arc<tokio::sync::RwLock<crate::protocol::WorkerHealth>>,
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
}

impl Default for FakeWorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: WorkerId::new("fake-worker"),
            version: "0.1.0".to_string(),
            capabilities: vec!["fake.invoke".to_string()],
            env: BTreeMap::new(),
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
}

/// Spawna o fake worker e devolve o handle (client + task).
pub fn spawn_fake_worker(config: FakeWorkerConfig) -> FakeWorkerHandle {
    let (client_tx, mut server_rx) = mpsc::channel::<Vec<u8>>(64);
    let (server_tx, client_rx) = mpsc::channel::<Vec<u8>>(64);

    let health = std::sync::Arc::new(tokio::sync::RwLock::new(WorkerHealth::Ok));
    let health_clone = health.clone();

    let server_task = tokio::spawn(async move {
        let mut line_buf = LineBuffer::default();
        let mut auth: Option<WorkerAuth> = None;
        loop {
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
                    let manifest = WorkerManifest {
                        worker_id: config.worker_id.clone(),
                        version: config.version.clone(),
                        capabilities: config.capabilities.clone(),
                        dependencies: vec![Dependency {
                            name: "fake-runtime".to_string(),
                            version: "0.1.0".to_string(),
                            source: Some("fake".to_string()),
                        }],
                        health: WorkerHealth::Ok,
                        compatibility: CompatibilityInfo {
                            min_os: "any".to_string(),
                            arch: "any".to_string(),
                            min_runtime: "fake".to_string(),
                        },
                    };
                    // O server **não** responde com `worker.hello`
                    // — quem chama `hello` é o próprio worker no
                    // boot. Aqui o server só registra o token de
                    // auth que o app envia no `app.ack`.
                    let _ = manifest;
                }
                IpcOp::Ack => {
                    auth = msg.auth.clone();
                }
                IpcOp::Ping => {
                    let pong = IpcMessage {
                        protocol_version: IpcMessage::current_protocol_version(),
                        request_id: msg.request_id,
                        op: IpcOp::Pong,
                        payload: serde_json::json!({"status": "ok", "env": config.env}),
                        auth: None,
                    };
                    let line = pong.encode_line().expect("encode pong");
                    if server_tx.send(line).await.is_err() {
                        return;
                    }
                }
                IpcOp::Shutdown => {
                    *health_clone.write().await = WorkerHealth::Unhealthy;
                    return;
                }
                IpcOp::ToolInvoke => {
                    // Verifica token se configurado.
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
                            "env_received": config.env,
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

    let client = FakePipeClient::new(client_tx, client_rx);
    FakeWorkerHandle {
        client,
        server_task,
        health,
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
