//! Output collection para `exec.*` tools.
//!
//! Coleta stdout + stderr em chunks de 64 KB com teto total
//! de 10 MB por invocação (constante). Excedeu → trunca com
//! `truncated: true` no output. **Etapa 4 da Fase 7**: o
//! `collect_output` consome `&mut SandboxedProcess` (não
//! `Child` consumido). Isso é o que permite:
//!
//! 1. **Wall-clock real** — `wait_with_timeout(wall_clock)`
//!    é chamado **dentro** do `tokio::join!` junto com a
//!    leitura dos streams. Se o processo não sair em
//!    `wall_clock`, `wait_with_timeout` retorna
//!    `TimedOut` → `start_kill` no child + drop do
//!    `SandboxedProcess` cascateia `KILL_ON_JOB_CLOSE` na
//!    árvore (mata netos). A v1 da Etapa 2 só checava
//!    `elapsed > wall_clock` **depois** de stdout/stderr
//!    fecharem — defesa fraca (se o processo segura stdout
//!    aberto, o wall-clock nunca dispara).
//! 2. **Cancelamento cascateado** — se o caller drop o
//!    `SandboxedProcess` (cancelamento do Run), o Job handle
//!    fecha imediatamente, independente do estado do
//!    `collect_output`.
//!
//! Ver [`docs/architecture/exec-tools-specification.md`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/architecture/exec-tools-specification.md)
//! §"Output collection e limites".

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;

use frederico_security::jail::SandboxedProcess;
use frederico_security::raw_child::{wrap_pipe_handle_as_async_file, RawExitStatus};

/// Teto total de output (stdout + stderr somados) por invocação.
/// 10 MB é o limite do spec §"Output collection e limites".
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Tamanho do chunk de leitura (64 KB). Acumulado em `Vec<u8>`
/// e checado contra `MAX_OUTPUT_BYTES` a cada chunk.
pub const OUTPUT_CHUNK_SIZE: usize = 64 * 1024;

/// Output bruto coletado. A Tool converte em `ToolResult::output`
/// (JSON) no `execute`.
#[derive(Debug)]
pub(crate) struct RawOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub truncated: bool,
    pub bytes_stdout: usize,
    pub bytes_stderr: usize,
    pub duration_ms: u64,
}

/// Coleta stdout + stderr + exit code de um [`SandboxedProcess`]
/// vivo. Aplica o teto de 10 MB (truncando com `truncated: true`
/// no output) e o wall-clock (mata o processo se exceder).
///
/// **Por que `&mut SandboxedProcess` (e não `Child` consumido):**
/// o `collect_output` precisa:
///
/// 1. Tomar stdout/stderr via `sandboxed.stdout()` / `stderr()`.
/// 2. Chamar `sandboxed.wait_with_timeout(wall_clock)` **dentro**
///    do `tokio::join!` junto com a leitura dos streams — wall-clock
///    real (não "apenas informativo" como na v1 da Etapa 2).
///
/// Se o `SandboxedProcess` fosse consumido (estilo `into_child`),
/// o `Drop` rodaria no momento do consumo e fecharia o Job
/// handle — `KILL_ON_JOB_CLOSE` dispararia prematuramente,
/// deixando o `Child` órfão (fora do Job) e quebrando o
/// tree-kill. Por isso a v2 da Etapa 4 toma stdout/stderr via
/// `Option<ChildStdout>` e mantém o `SandboxedProcess` vivo até
/// o `Drop` final.
///
/// **Comportamento em wall-clock excedido:** `wait_with_timeout`
/// retorna `Err(TimedOut)` (já chamou `start_kill` no child).
/// O `SandboxedProcess` continua vivo nesta função — quando o
/// caller dropa (depois do `Err`), o Job handle fecha e mata
/// netos via `KILL_ON_JOB_CLOSE`. A função retorna `Err` com
/// mensagem `"wall-clock excedido (>{wall_clock:?})"`.
///
/// **Comportamento em exit normal:** os streams podem saturar
/// (atingir `MAX_OUTPUT_BYTES`) e fechar antes do `wait`. O
/// `truncated: true` sinaliza isso; o resto do output (se
/// houver) está perdido — teto é teto.
pub(crate) async fn collect_output(
    sandboxed: &mut SandboxedProcess,
    wall_clock: std::time::Duration,
) -> Result<RawOutput, String> {
    let start = std::time::Instant::now();

    // Toma as handles ANTES do join. A Etapa 5+ da Fase 7
    // mudou a API: o `SandboxedProcess` retorna **HANDLEs raw**
    // (Win32), que o caller wrappa em `tokio::fs::File` via
    // `wrap_pipe_handle_as_async_file` pra implementar `AsyncRead`.
    // Motivo: o `tokio::process::Child` da Etapa 4 v1 não
    // permitia aplicar o `RestrictedToken` (a std não expõe
    // `CreateProcessAsUserW` em Rust 1.97). A Etapa 5+ usa
    // raw `CreateProcessAsUserW` (com `HANDLE_LIST` +
    // `CREATE_SUSPENDED` + `TokenIntegrityLevel=Low` +
    // `Mandatory Label\Low` no workdir/pipes). Resultado:
    // handle raw, wrappear é responsabilidade do caller.
    let stdout_handle = sandboxed.take_stdout_handle().ok_or_else(|| {
        "stdout nao piped (SecurityJailResolver::spawn deveria ter feito CreatePipe)".to_string()
    })?;
    let stdout = wrap_pipe_handle_as_async_file(stdout_handle)
        .map_err(|e| format!("wrap_pipe_handle_as_async_file(stdout): {e}"))?;
    let stderr_handle = sandboxed
        .take_stderr_handle()
        .ok_or_else(|| "stderr nao piped (mesma razão de stdout)".to_string())?;
    let stderr = wrap_pipe_handle_as_async_file(stderr_handle)
        .map_err(|e| format!("wrap_pipe_handle_as_async_file(stderr): {e}"))?;

    // Concorrência: 3 futures. O wait_with_timeout é o gate —
    // em timeout, ele já fez kill no child (via
    // `RawChild::wait_with_timeout` → `TerminateProcess`). O
    // caller dropa o SandboxedProcess depois → Job fecha →
    // KILL_ON_JOB_CLOSE na árvore.
    let (stdout_bytes, stderr_bytes, wait_res) = tokio::join!(
        read_stream_to_cap(stdout, MAX_OUTPUT_BYTES),
        read_stream_to_cap(stderr, MAX_OUTPUT_BYTES),
        sandboxed.wait_with_timeout(wall_clock),
    );

    // Wall-clock excedido → erro (SandboxedProcess ainda vivo;
    // caller dropará → Job handle fecha → netos morrem).
    if let Err(ref e) = wait_res {
        if e.kind() == std::io::ErrorKind::TimedOut {
            return Err(format!("wall-clock excedido (>{wall_clock:?}): {e}"));
        }
    }

    // `RawExitStatus` (Etapa 5+) expõe o `code` como campo público
    // (`u32`), não como método `.code() -> Option<i32>`. Convenção
    // C: 0 = sucesso; diferente de 0 = erro. Não há "no status"
    // (RawChild garante que sempre lê o exit code se o wait
    // succeeded).
    let exit_status: RawExitStatus = wait_res.map_err(|e| format!("wait falhou: {e}"))?;
    let exit_code = exit_status.code as i32;
    let duration_ms = start.elapsed().as_millis() as u64;

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

    Ok(RawOutput {
        bytes_stdout: stdout_bytes.len(),
        bytes_stderr: stderr_bytes.len(),
        truncated: stdout_bytes.len() >= MAX_OUTPUT_BYTES || stderr_bytes.len() >= MAX_OUTPUT_BYTES,
        stdout: stdout_str,
        stderr: stderr_str,
        exit_code,
        duration_ms,
    })
}

/// Lê um `tokio::fs::File` (wrappado de um `HANDLE` de pipe) em
/// chunks até o teto. Retorna o `Vec<u8>` acumulado (truncado se
/// excedeu o teto).
///
/// A Etapa 5+ da Fase 7 troca `ChildStdout`/`ChildStderr` (que
/// vinham de `tokio::process::Child`) por `tokio::fs::File`
/// (wrappado de `HANDLE` raw do `CreatePipe`).
async fn read_stream_to_cap<R>(reader: R, max: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf_reader = BufReader::with_capacity(OUTPUT_CHUNK_SIZE, reader);
    let mut acc = Vec::new();
    let mut chunk = vec![0u8; OUTPUT_CHUNK_SIZE];
    loop {
        match buf_reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if acc.len() + n > max {
                    let remaining = max.saturating_sub(acc.len());
                    acc.extend_from_slice(&chunk[..remaining]);
                    break;
                }
                acc.extend_from_slice(&chunk[..n]);
            }
            Err(_) => break,
        }
    }
    acc
}

/// Constrói o JSON de output padrão do `exec.*` (sucesso).
/// Caller adiciona campos extras se precisar.
pub(crate) fn output_json(raw: &RawOutput) -> serde_json::Value {
    json!({
        "stdout": raw.stdout,
        "stderr": raw.stderr,
        "exit_code": raw.exit_code,
        "duration_ms": raw.duration_ms,
        "truncated": raw.truncated,
        "bytes_stdout": raw.bytes_stdout,
        "bytes_stderr": raw.bytes_stderr,
    })
}
