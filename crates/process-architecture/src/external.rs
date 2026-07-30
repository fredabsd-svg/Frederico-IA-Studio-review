//! `WorkerManager::spawn_external` — abre um worker sidecar via
//! `tokio::process::Command`, faz o handshake sobre named pipes
//! (Windows) e devolve o par `(WorkerManager, WorkerHandle)` na
//! mesma forma do `spawn_in_process` (Fase 5, Etapa 2B — fecha a
//! pendência registrada em `docs/modules/process-architecture.md`
//! §"Pendências para a próxima sessão" item 1).
//!
//! ## Fluxo (resumo do `spawn_external`)
//!
//! 1. Constrói o env do filho via [`build_worker_env`] (allowlist
//!    explícita — o env do pai **não** é lido, regra do
//!    `process-architecture.md` §Invariantes).
//! 2. `tokio::process::Command` com `stdin = null`, `stdout` e
//!    `stderr` piped. `CREATE_NO_WINDOW` no Windows (evita flash
//!    de janela do cmd). Spawna o filho, mantendo o `Child` no
//!    `WorkerManager` (pro `shutdown` chamar `kill()` se preciso).
//! 3. Lê a **primeira linha** do stdout com timeout 10s — espera
//!    `READY <pipe_name>`. Esse é o handshake da inversão
//!    (ADR-0017 §Decisão 2): o **worker** cria o
//!    `NamedPipeServer`, gera o nome, e anuncia via stdout; o
//!    **app** lê e usa o nome pra conectar.
//! 4. Spawna uma task de **stderr pump** (best-effort): lê
//!    linha a linha e loga via `tracing::warn!`. Crash do worker
//!    fica visível nos logs do app.
//! 5. `connect_pipe_client(name)` (síncrono, em `spawn_blocking`
//!    pra não bloquear o runtime) + `client.ready(READABLE |
//!    WRITABLE).await` (pós-connect — `ready()` no client
//!    retorna imediato, é o método certo aqui, ver
//!    `windows_pipes.rs` §"connect() no server, ready() no
//!    client").
//! 6. Envelopa o `NamedPipeClient` em `shared_pipe_pair` →
//!    `WindowsPipeReader`/`Writer` e segue o mesmo handshake
//!    `worker.hello` / `app.ack` do `spawn_in_process`.
//! 7. Devolve `(WorkerManager, WorkerHandle)` — `WorkerHandle` é
//!    indistinguível do que o `spawn_in_process` devolve.
//!
//! ## Falhas e cleanup
//!
//! O padrão é **early return** com cleanup explícito: se algo
//! falhar entre o spawn e o handshake completo, o `child` é
//! morto (`kill` + `wait`) e o erro retornado. **Não** usamos
//! `map_err` com `.await` dentro porque a closure do `map_err`
//! não é `async` — `kill` e `wait` exigem `.await`. O helper
//! [`cleanup_child`] faz o `kill+wait` best-effort e é chamado
//! em cada path de erro antes do `return`.
//!
//! Após o handshake completo, o `child` é **anexado** ao
//! `WorkerManager` via [`WorkerManager::attach_child`] (privado
//! ao crate). O `shutdown` cuida do `wait`+`kill` aí.
//!
//! ## PATH do pai (exceção documentada)
//!
//! O `process-architecture.md` §Invariantes diz: "Variáveis de
//! ambiente do processo pai não são herdadas pelos workers"
//! (regra dura — `OPENAI_API_KEY` no pai nunca vaza pro
//! worker). **Mas** o `PATH` do pai é injetado automaticamente
//! se o `ExternalSpawnConfig.env` não tem um `PATH` explícito.
//! Razão: workers reais (PowerShell, Python, qualquer binário
//! do sistema) precisam de `PATH` pra resolver DLLs e
//! executáveis adjacentes. `PATH` **não é segredo** — é só
//! lista de diretórios. A invariante "segredos do pai não
//! vazam" continua valendo: o `build_worker_env` é pura, não
//! lê o env do pai, e o `env_clear` no `Command` zera tudo **antes**
//! do `envs(env)`. A única coisa que volta é `PATH` (lido do
//! pai no momento do spawn, no Rust — não no env do pai
//! diretamente). Documentado aqui pra ficar explícito.
//!
//! ## Gate Windows
//!
//! O módulo inteiro é `#[cfg(windows)]` — named pipes são
//! Windows. Em outras plataformas o `spawn_external`
//! simplesmente não existe. O `lib.rs` re-exporta
//! `ExternalSpawnConfig` com o mesmo `#[cfg(windows)]`, e o
//! `WorkerManager::spawn_external` no `manager.rs` também é
//! `#[cfg(windows)]`.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader, Interest};
use tokio::process::{Child, Command};
use uuid::Uuid;

use crate::env_allowlist::{build_worker_env, EnvEntry};
use crate::error::ProcessError;
use crate::manager::WorkerManager;
use crate::pipes::{PipeName, PipeReader, PipeWriter};
use crate::protocol::{IpcMessage, IpcOp, WorkerAuth, WorkerManifest};
use crate::windows_pipes::{connect_pipe_client, shared_pipe_pair};

/// Configuração do `WorkerManager::spawn_external`.
///
/// Lida com o caminho "abre um worker sidecar de verdade"
/// (vs. `spawn_in_process` que usa o fake `mpsc`).
#[derive(Debug, Clone)]
pub struct ExternalSpawnConfig {
    /// Comando (caminho do executável ou nome no PATH do pai — o
    /// `tokio::process::Command` resolve via PATH normal do
    /// Windows). Ex.: `"C:\workers\document-worker\python.exe"`,
    /// `"document-worker.exe"`.
    pub command: String,
    /// Argumentos passados ao comando. Ex.: `["--pipe"]` ou
    /// `["workers/document-worker/document-worker.py"]`.
    pub args: Vec<String>,
    /// Allowlist de env pro worker. **NÃO** inclui o env do pai
    /// (regra do `process-architecture.md` §Invariantes — o
    /// `build_worker_env` é a função pura que aplica a regra).
    pub env: Vec<EnvEntry>,
    /// Diretório de trabalho do filho. `None` herda o cwd do app
    /// (regra de cwd é diferente da regra de env — cwd é só um
    /// path, não carrega segredos).
    pub cwd: Option<PathBuf>,
    /// Token de auth pré-definido. `None` (default) gera UUID v4.
    /// Mesmo comportamento do `WorkerSpawnConfig::auth_token` —
    /// fica aqui (não no `WorkerSpawnConfig`) porque o
    /// `spawn_external` não usa `WorkerSpawnConfig` (esse é
    /// específico do fake in-process).
    pub auth_token: Option<WorkerAuth>,
    /// Timeout default pra `invoke`/`ping`. Default: 30s.
    pub default_timeout_ms: u32,
    /// Timeout pra esperar a linha `READY <name>` no stdout do
    /// filho. Default: 10s. Se o worker travar antes de anunciar
    /// o pipe, falha com `ProcessError::Platform` (não
    /// `Timeout` — o tempo é o de boot, não o de uma `invoke`).
    pub ready_timeout: Duration,
}

impl Default for ExternalSpawnConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            auth_token: None,
            default_timeout_ms: 30_000,
            ready_timeout: Duration::from_secs(10),
        }
    }
}

impl ExternalSpawnConfig {
    /// Constrói com `command` setado.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ..Self::default()
        }
    }

    /// Define os args (substitui).
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Adiciona um env entry (allowlist).
    #[must_use]
    pub fn with_env(mut self, entries: &[EnvEntry]) -> Self {
        self.env = entries.to_vec();
        self
    }

    /// Define o cwd do filho.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Define um token de auth pré-definido (útil pra testes).
    #[must_use]
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(WorkerAuth::new(token.into()));
        self
    }

    /// Define o timeout do READY (default 10s).
    #[must_use]
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }
}

/// Constante Windows: `CREATE_NO_WINDOW` — evita que o
/// `Command::new("python.exe")` (e similares) faça flash de uma
/// janela de console quando rodar no app desktop. Mesma flag
/// usada pela `tokio::process::Command` no Rust idiomático.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Helper de cleanup: mata o child e espera o `wait` retornar
/// (best-effort — `kill`/`wait` podem falhar se o processo já
/// saiu; nesse caso, logamos `warn` e seguimos). Usado em todos
/// os paths de erro do `spawn_external` antes do `return Err`.
async fn cleanup_child(child: &mut Child, context: &str) {
    if let Err(e) = child.kill().await {
        tracing::warn!(?e, worker = %context, "cleanup: kill falhou (já saiu?)");
    }
    if let Err(e) = child.wait().await {
        tracing::warn!(?e, worker = %context, "cleanup: wait falhou");
    }
}

/// Implementação do `WorkerManager::spawn_external`. Vive aqui
/// (e não no `manager.rs`) pra manter o `manager.rs` focado no
/// modelo de ator — esta função é "só" o bootstrap do
/// transporte.
pub async fn spawn_external(
    config: ExternalSpawnConfig,
) -> Result<(WorkerManager, crate::WorkerHandle), ProcessError> {
    // 1. Constrói o env via allowlist. `build_worker_env` é pura
    //    e não lê o env do pai (regra §Invariantes).
    let mut env = build_worker_env(&config.env);

    // 1b. PATH do pai (exceção documentada no header do módulo).
    //     Só injeta se o caller não passou um PATH explícito.
    //     `PATH` não é segredo — é só lista de diretórios pra
    //     resolver binários/DLLs. Workers reais (PowerShell,
    //     Python) precisam.
    if !env.contains_key("PATH") {
        if let Ok(parent_path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), parent_path);
        }
    }
    // Mesmo pro SystemRoot no Windows (PowerShell crasha sem
    // `SystemRoot` apontando pro diretório do Windows).
    #[cfg(windows)]
    if !env.contains_key("SystemRoot") {
        if let Ok(sr) = std::env::var("SystemRoot") {
            env.insert("SystemRoot".to_string(), sr);
        }
    }

    // 2. Monta o Command. `stdin = null` (worker não lê input do
    //    app no boot — o app.ack vem pelo pipe). `stdout`/`stderr`
    //    piped (lê o READY do stdout; stderr vai pra log pump).
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(env);
    if let Some(cwd) = &config.cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // 3. Spawn. Falha aqui = binário não existe / sem permissão
    //    / cwd inválido. Não há child vivo nesse caso, então
    //    basta retornar o erro.
    let mut child: Child = cmd.spawn().map_err(|e| ProcessError::Platform {
        message: format!("spawn de `{}` falhou: {e}", config.command),
    })?;

    // 4. Stderr pump — best-effort. Lê linha a linha e loga via
    //    `tracing::warn!`. Crash do worker fica visível nos logs
    //    do app. A task termina quando o stderr fecha (worker
    //    morreu ou app shutdownou).
    if let Some(stderr) = child.stderr.take() {
        let mut err_reader = BufReader::new(stderr);
        let worker_label = config.command.clone();
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match err_reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        tracing::warn!(
                            worker = %worker_label,
                            stderr = %line.trim_end(),
                            "worker stderr"
                        );
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // 5. Lê a primeira linha do stdout com timeout 10s — espera
    //    `READY <pipe_name>`. Se o filho crashar antes de
    //    imprimir, o `read_line` retorna `Ok(0)` (EOF) — também
    //    é erro. Se travar, o timeout dispara.
    let stdout = child
        .stdout
        .take()
        .expect("stdout foi configurado como piped");
    let mut out_reader = BufReader::new(stdout);
    let mut line = String::new();
    let read_result =
        tokio::time::timeout(config.ready_timeout, out_reader.read_line(&mut line)).await;

    let ready_line = match read_result {
        Ok(Ok(0)) => {
            // EOF — filho fechou stdout sem mandar READY.
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Platform {
                message: format!("`{}` fechou stdout antes de enviar READY", config.command),
            });
        }
        Ok(Ok(_)) => line,
        Ok(Err(e)) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Platform {
                message: format!("leitura do READY de `{}` falhou: {e}", config.command),
            });
        }
        Err(_timeout) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Platform {
                message: format!(
                    "`{}` não enviou READY em {:?}",
                    config.command, config.ready_timeout
                ),
            });
        }
    };

    // 6. Parse `READY <pipe_name>`. O formato é fixo: primeira
    //    linha, dois tokens, segundo token é o `PipeName` (sem
    //    espaços, sem `\`, ≤ 200 chars — o `PipeName::new`
    //    valida).
    let pipe_name = match parse_ready_line(&ready_line) {
        Ok(n) => n,
        Err(e) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Protocol {
                message: format!("linha `READY` de `{}` inválida: {e}", config.command),
            });
        }
    };

    // 7. Conecta como client. `connect_pipe_client` é síncrono
    //    (CreateFileW + ConnectNamedPipe do lado kernel) —
    //    `spawn_blocking` pra não bloquear o runtime. O nome do
    //    pipe foi anunciado pelo worker no stdout, então o
    //    server já está escutando — `open` retorna rápido
    //    (tipicamente < 10ms).
    let pipe_name_for_connect = pipe_name.clone();
    let connect_result =
        tokio::task::spawn_blocking(move || connect_pipe_client(&pipe_name_for_connect)).await;

    let client = match connect_result {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Platform {
                message: format!("connect_pipe_client falhou: {e}"),
            });
        }
        Err(join_err) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Platform {
                message: format!("task de connect_pipe_client panicou: {join_err}"),
            });
        }
    };

    // 8. `ready()` no client (pós-connect). Retorna imediato
    //    porque o client já está conectado — é a chamada
    //    obrigatória da Tokio (a Tokio recomenda o `ready`
    //    explícito, ver `windows_pipes.rs` §"connect() no
    //    server, ready() no client").
    if let Err(e) = client.ready(Interest::READABLE | Interest::WRITABLE).await {
        cleanup_child(&mut child, &config.command).await;
        return Err(ProcessError::Platform {
            message: format!("ready do NamedPipeClient falhou: {e}"),
        });
    }

    // 9. Envelopa em WindowsPipeReader/Writer e segue o mesmo
    //    handshake do `spawn_in_process` (worker.hello →
    //    app.ack → monta o estado). O `Box<dyn>` é porque o
    //    ator quer abstração.
    let (reader_pipe, writer_pipe) = shared_pipe_pair(client);
    let mut actor_reader: Box<dyn PipeReader> = Box::new(reader_pipe);
    let writer_for_actor: Box<dyn PipeWriter> = Box::new(writer_pipe);

    // 10. Handshake: lê `worker.hello`, valida, gera
    //     `WorkerAuth`, envia `app.ack`. Mesma sequência do
    //     `spawn_in_process` (Etapa 2A). Se o `hello` chegar
    //     com manifesto inválido, matamos o filho.
    let auth = config
        .auth_token
        .unwrap_or_else(|| WorkerAuth::new(Uuid::new_v4().to_string()));

    let hello_line = match actor_reader.read_line().await {
        Ok(Some(l)) => l,
        Ok(None) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Transport {
                message: "filho fechou antes de enviar `worker.hello`".to_string(),
            });
        }
        Err(e) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Transport {
                message: format!("leitura do `worker.hello` falhou: {e}"),
            });
        }
    };

    let (hello_msg, _) = match IpcMessage::decode_line(&hello_line) {
        Ok(m) => m,
        Err(e) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Protocol {
                message: format!("decode do `worker.hello` falhou: {e}"),
            });
        }
    };

    if hello_msg.op != IpcOp::Hello {
        cleanup_child(&mut child, &config.command).await;
        return Err(ProcessError::Protocol {
            message: format!(
                "primeira mensagem do filho deveria ser `worker.hello`, veio {:?}",
                hello_msg.op
            ),
        });
    }

    let manifest: WorkerManifest = match serde_json::from_value(hello_msg.payload) {
        Ok(m) => m,
        Err(e) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Protocol {
                message: format!("manifesto inválido: {e}"),
            });
        }
    };
    let worker_id = manifest.worker_id.clone();

    // 11. Envia `app.ack`. O worker (Python ou outro) salva o
    //     token e valida toda `tool.invoke` subsequente.
    let ack = IpcMessage::ack(hello_msg.request_id, auth.clone());
    let ack_line = match ack.encode_line() {
        Ok(l) => l,
        Err(e) => {
            cleanup_child(&mut child, &config.command).await;
            return Err(ProcessError::Protocol {
                message: format!("encode do `app.ack` falhou: {e}"),
            });
        }
    };
    if let Err(e) = writer_for_actor.write_line(&ack_line).await {
        cleanup_child(&mut child, &config.command).await;
        return Err(ProcessError::Transport {
            message: format!("write do `app.ack` falhou: {e}"),
        });
    }

    // 12. Constrói o estado partilhado + spawna o ator (igual
    //     ao `spawn_in_process` Etapa 2A). O `health` inicial é
    //     `Unhealthy` — só vira `Ok` depois do primeiro `Pong`
    //     positivo.
    let timeout = Duration::from_millis(config.default_timeout_ms as u64);
    let (mut manager, handle) = crate::manager::assemble_actor_and_state(
        actor_reader,
        writer_for_actor,
        auth,
        worker_id,
        manifest,
        crate::manager::fresh_health_snapshot(),
        None, // server_task — não tem no spawn_external
        timeout,
    )?;

    // 13. "Presenteia" o manager com o `Child` — o `shutdown`
    //     usa pra `wait()` / `kill()`.
    manager.attach_child(child);

    Ok((manager, handle))
}

/// Parse da linha `READY <pipe_name>`. Retorna
/// `ProcessError::Protocol` se a linha não tiver o formato
/// esperado.
fn parse_ready_line(line: &str) -> Result<PipeName, String> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.splitn(2, ' ');
    let first = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("").trim();
    if first != "READY" {
        return Err(format!("esperava `READY <name>`, veio {:?}", trimmed));
    }
    if name.is_empty() {
        return Err("`READY` sem nome de pipe".to_string());
    }
    PipeName::new(name).map_err(|e| format!("nome de pipe inválido: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ready_line_accepts_valid() {
        let name = parse_ready_line("READY frederico-doc-12345\n").expect("parse");
        assert_eq!(name.as_str(), "frederico-doc-12345");
    }

    #[test]
    fn parse_ready_line_strips_crlf() {
        let name = parse_ready_line("READY my-pipe\r\n").expect("parse");
        assert_eq!(name.as_str(), "my-pipe");
    }

    #[test]
    fn parse_ready_line_rejects_wrong_prefix() {
        let err = parse_ready_line("HELLO foo").expect_err("HELLO não é READY");
        assert!(err.contains("READY"));
    }

    #[test]
    fn parse_ready_line_rejects_missing_name() {
        let err = parse_ready_line("READY").expect_err("READY sem nome");
        assert!(err.contains("sem nome"));
    }

    #[test]
    fn parse_ready_line_rejects_invalid_name_chars() {
        // `PipeName::new` rejeita `\\` e `/`.
        let err = parse_ready_line("READY bad\\name").expect_err("nome com \\");
        assert!(err.contains("inválido"));
    }
}
