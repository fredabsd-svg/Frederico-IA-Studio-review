//! Output collection para `exec.*` tools.
//!
//! Coleta stdout + stderr em chunks de 64 KB com teto total
//! de 10 MB por invocação (constante). Excedeu → trunca com
//! `truncated: true` no output. A Etapa 4 implementa a versão
//! mínima via `tokio::process::Child::wait`; a Etapa 5 da Fase 7
//! pode estender para streaming.
//!
//! Ver [`docs/architecture/exec-tools-specification.md`](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/architecture/exec-tools-specification.md)
//! §"Output collection e limites".

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::Child;

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

/// Coleta stdout + stderr + exit code do `Child` (depois de
/// `SecurityJailResolver::spawn().into_child()`). Aplica o teto
/// de 10 MB (truncando com `truncated: true` no output).
///
/// **v1 simplificação**: lê em chunks de `OUTPUT_CHUNK_SIZE` até
/// o processo fechar stdout/stderr. A Etapa 5 pode adicionar
/// streaming parcial (modelo vê output durante execução).
pub(crate) async fn collect_output(
    mut child: Child,
    wall_clock: std::time::Duration,
) -> Result<RawOutput, String> {
    let start = std::time::Instant::now();

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (stdout_bytes, stderr_bytes) = tokio::join!(
        read_stream_to_cap(stdout, MAX_OUTPUT_BYTES),
        read_stream_to_cap(stderr, MAX_OUTPUT_BYTES),
    );

    // Wall-clock check (defesa contra loop infinito).
    let elapsed = start.elapsed();
    if elapsed > wall_clock {
        // Mata o processo (defesa em profundidade; o `KILL_ON_JOB_CLOSE`
        // também fecha o app, mas queremos ser explícitos).
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(format!(
            "wall-clock excedido ({}s > {}s)",
            elapsed.as_secs(),
            wall_clock.as_secs()
        ));
    }

    // Espera exit code.
    let exit_status = child
        .wait()
        .await
        .map_err(|e| format!("wait falhou: {e}"))?;
    let exit_code = exit_status.code().unwrap_or(-1);
    let duration_ms = start.elapsed().as_millis() as u64;

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

    Ok(RawOutput {
        bytes_stdout: stdout_bytes.len(),
        bytes_stderr: stderr_bytes.len(),
        truncated: stdout_bytes.len() >= MAX_OUTPUT_BYTES
            || stderr_bytes.len() >= MAX_OUTPUT_BYTES,
        stdout: stdout_str,
        stderr: stderr_str,
        exit_code,
        duration_ms,
    })
}

/// Lê um `ChildStdout` ou `ChildStderr` em chunks até o teto.
/// Retorna o `Vec<u8>` acumulado (truncado se excedeu o teto).
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
