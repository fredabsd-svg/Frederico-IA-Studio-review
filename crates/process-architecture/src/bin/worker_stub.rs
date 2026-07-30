//! Stub binário do worker sidecar — usado pelos integration
//! tests E2E do `WorkerManager::spawn_external` em
//! `tests/external_worker.rs`. Implementa o **protocolo
//! completo** sobre `tokio::net::windows::named_pipe` (mesma
//! stack do manager, sem dependência de Python ou PowerShell).
//!
//! **Por que Rust e não PowerShell como stub dos tests E2E?**
//!
//! A Etapa 2B entregou um stub em PowerShell 5.1
//! (`tests/stubs/worker-stub.ps1`) que funciona bem em
//! Windows local (< 500ms pro handshake completo). Mas no
//! Windows Server 2022 runner do GitHub Actions (PR #11, runs
//! #30541220400 / #30541686745 / #30542253439 / #30542716235),
//! o PowerShell leva **mais de 30 segundos** só pra fazer
//! cold-start + imprimir `READY <name>` — tornando o test
//! E2E flaky/lento. Substituir o stub por um binário Rust
//! elimina essa variabilidade (cold-start Rust ~50ms).
//!
//! O PowerShell stub continua em `tests/stubs/worker-stub.ps1`
//! como **referência** e pra uso em smoke tests manuais —
//! `tests/smoke_local.ps1` (Etapa 2B+X) usa o PowerShell
//! stub pra provar o ciclo standalone sem o Rust.
//!
//! ## Protocolo
//!
//! Mesmo do stub PowerShell — `IpcMessage` line-delimited JSON,
//! 8 opcodes estáveis em snake_case com prefixo de direção.
//! Detalhes em `crates/process-architecture/src/protocol.rs`.
//!
//! ## Protocolo implementado pelo server
//!
//! 1. **Boot:** gera `pipe_name` único (UUID curto), cria
//!    `NamedPipeServer` (`first_pipe_instance(true)`), imprime
//!    `READY <pipe_name>` no stdout (sem newline buffering
//!    porque o Cargo faz flush automaticamente em child procs
//!    quando o stdout é piped), espera `ConnectNamedPipe`.
//! 2. Envia `worker.hello` com manifesto versionado (de
//!    `manifest-stub.json` ao lado, ou hardcoded).
//! 3. Loop: lê linhas JSON, dispatch por `op`:
//!    - `app.ack` → salva o token de auth.
//!    - `app.ping` → `worker.pong` com `status: "ok"`,
//!      `env_received: {}`.
//!    - `app.shutdown` → fecha o pipe e sai (sem response —
//!      o manager detecta EOF).
//!    - `tool.invoke` → valida token (se já temos um) e
//!      responde `tool.result` com `ok: true, echo: <payload>`.
//! 4. EOF / shutdown → fecha o handle e exit 0.
//!
//! ## Build & invoke
//!
//! O Cargo compila esse binário automaticamente quando o test
//! o referencia via `env!("CARGO_BIN_EXE_worker_stub")`. O
//! test setup é:
//!
//! ```ignore
//! let bin = env!("CARGO_BIN_EXE_worker_stub");
//! let cfg = ExternalSpawnConfig::new(bin)
//!     .with_args(vec!["--manifest".to_string(),
//!                      "tests/stubs/manifest-stub.json".to_string()]);
//! ```

use std::process::ExitCode;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Interest};
use tokio::net::windows::named_pipe::ServerOptions;

use frederico_process_architecture::protocol::{
    IpcMessage, IpcOp, RequestId, WorkerAuth, WorkerHealth, WorkerId, WorkerManifest,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let manifest_path = args
        .iter()
        .position(|a| a == "--manifest")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "tests/stubs/manifest-stub.json".to_string());

    let manifest = match load_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("worker_stub: load_manifest({manifest_path}) failed: {e}");
            return ExitCode::from(1);
        }
    };
    let worker_id = manifest.worker_id.clone();
    eprintln!("worker_stub: starting {worker_id} {}", manifest.version);

    // 1. Gera nome único pro pipe.
    let pipe_name = format!("frederico-stub-{}-{}", worker_id, uuid_short());
    let pipe_path = format!(r"\\.\pipe\{pipe_name}");

    // 2. Cria o `NamedPipeServer`. `first_pipe_instance(true)`
    //    garante que essa é a primeira instância do nome.
    let server = match ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_path)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("worker_stub: CreateNamedPipe({pipe_path}) failed: {e}");
            return ExitCode::from(2);
        }
    };

    // 3. Anuncia o pipe pro app via stdout. `print!` com
    //    `flush()` garante que a linha chega antes do
    //    `connect()` bloquear esperando o app.
    println!("READY {pipe_name}");
    use std::io::Write as _;
    std::io::stdout().flush().expect("flush stdout");

    // 4. Espera o app conectar. `connect()` envelopa o
    //    `ConnectNamedPipe` Win32 (Etapa 2B continuação —
    //    PR #10).
    if let Err(e) = server.connect().await {
        eprintln!("worker_stub: ConnectNamedPipe failed: {e}");
        return ExitCode::from(3);
    }
    eprintln!("worker_stub: client connected on {pipe_path}");

    // 5. Envelopa em reader/writer. `tokio::io::split` divide
    //    o NamedPipeServer (que implementa AsyncRead +
    //    AsyncWrite) em metades separadas — mesmo padrão do
    //    `WindowsPipeReader`/`Writer` no manager.
    let (mut server_read, mut server_write) = tokio::io::split(server);
    let mut reader = BufReader::new(&mut server_read);

    // 6. Envia `worker.hello` imediatamente após o connect.
    let hello_id = uuid_v4();
    let hello = IpcMessage::hello(manifest.clone());
    if let Err(e) = write_message(&mut server_write, &hello).await {
        eprintln!("worker_stub: write worker.hello failed: {e}");
        return ExitCode::from(4);
    }
    eprintln!("worker_stub: sent worker.hello (request_id={hello_id})");

    // 7. Loop: lê linhas do pipe e dispatcha.
    let mut auth_token: Option<WorkerAuth> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("worker_stub: read_line failed: {e}");
                break;
            }
        };
        if n == 0 {
            eprintln!("worker_stub: peer closed (EOF)");
            break;
        }

        let msg = match IpcMessage::decode_line(line.as_bytes()) {
            Ok((m, _)) => m,
            Err(e) => {
                eprintln!("worker_stub: decode failed: {e} (line ignored)");
                continue;
            }
        };

        match msg.op {
            IpcOp::Ack => {
                auth_token = msg.auth.clone();
                eprintln!("worker_stub: app.ack received (auth saved)");
            }
            IpcOp::Ping => {
                let pong = IpcMessage {
                    protocol_version: IpcMessage::current_protocol_version(),
                    request_id: msg.request_id,
                    op: IpcOp::Pong,
                    payload: serde_json::json!({
                        "status": "ok",
                        "env_received": {},
                    }),
                    auth: None,
                };
                if let Err(e) = write_message(&mut server_write, &pong).await {
                    eprintln!("worker_stub: write pong failed: {e}");
                    break;
                }
            }
            IpcOp::Shutdown => {
                eprintln!("worker_stub: app.shutdown received, exiting");
                break;
            }
            IpcOp::ToolInvoke => {
                // Valida o token se já temos um (depois do
                // handshake).
                if let Some(expected) = &auth_token {
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
                            if let Err(e) = write_message(&mut server_write, &err).await {
                                eprintln!("worker_stub: write err failed: {e}");
                                break;
                            }
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
                        "env_received": {},
                    }),
                    auth: None,
                };
                if let Err(e) = write_message(&mut server_write, &result).await {
                    eprintln!("worker_stub: write tool.result failed: {e}");
                    break;
                }
            }
            other => {
                eprintln!("worker_stub: ignoring op {other:?}");
            }
        }
    }

    eprintln!("worker_stub: {worker_id} exiting");
    ExitCode::SUCCESS
}

/// Carrega o manifesto de um arquivo JSON.
fn load_manifest(path: &str) -> Result<WorkerManifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read_to_string: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("serde_json::from_str: {e}"))
}

/// Escreve uma `IpcMessage` como JSON line-delimited no
/// writer. Flush explícito porque `tokio::io::split` no
/// `NamedPipeServer` não tem AutoFlush (não é `StreamWriter`).
async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &IpcMessage,
) -> Result<(), String> {
    let line = msg.encode_line().map_err(|e| format!("encode_line: {e}"))?;
    writer
        .write_all(&line)
        .await
        .map_err(|e| format!("write_all: {e}"))?;
    writer.flush().await.map_err(|e| format!("flush: {e}"))?;
    Ok(())
}

/// UUID v4 curto (12 chars hex) — só pra uniqueness do
/// `pipe_name`. Não é usado pra `request_id` (que usa UUID v4
/// completo via `uuid::Uuid::new_v4`).
fn uuid_short() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:012x}")
}

fn uuid_v4() -> RequestId {
    uuid::Uuid::new_v4()
}

/// Stub silencioso — o `connect()` do server precisa ser
/// aguardado de forma compatível com o `read_line` no mesmo
/// task. `tokio::io::split` divide `NamedPipeServer` em
/// reader + writer; ambos compartilham o mesmo handle, mas
/// são usados separadamente pra evitar conflito de borrow.
#[allow(dead_code)]
fn _interest_marker() -> Interest {
    // Mantém o import usado (cargo clippy reclama de
    // unused_imports se a `Interest` não for referenciada em
    // lugar nenhum).
    Interest::READABLE
}

/// Placeholder — o stub usa `chrono::Utc::now` no manifesto
/// hardcoded mas não precisa do `chrono` runtime (a data do
/// `WorkerHealthSnapshot` é responsabilidade do manager, não
/// do worker). Mantém o import alive pro `Cargo.toml` se
/// alguém quiser usar.
#[allow(dead_code)]
fn _chrono_marker() -> Duration {
    std::time::Duration::from_secs(0)
}

/// Placeholder similar — o stub recebe `WorkerHealth` no
/// manifesto hardcoded mas não precisa do tipo em runtime
/// (só no manifesto). Mantém o import alive.
#[allow(dead_code)]
fn _health_marker(_h: WorkerHealth) -> WorkerId {
    WorkerId::new("worker_stub")
}
