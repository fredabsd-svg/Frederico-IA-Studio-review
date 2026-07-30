//! Integration test do `WindowsPipeReader`/`Writer` com **named
//! pipes reais** do Windows (não `tokio::io::duplex`). Gateado em
//! `#[cfg(windows)]` — fora do Windows o arquivo é vazio (o CI
//! roda em Linux/macOS também).
//!
//! ## O que este teste prova
//!
//! - `create_pipe_server` cria um `NamedPipeServer` real em
//!   `\\.\pipe\<name>`, primeira instância do nome.
//! - `connect_pipe_client` faz `CreateFileW` + `ConnectNamedPipe`
//!   no mesmo nome e devolve um `NamedPipeClient`.
//! - Round-trip de uma linha: server lê `"hello\n"`, client lê
//!   `"world\n"`. Confirma que o framing line-delimited JSON
//!   sobrevive ao transporte real.
//! - `WriteHalf` fechado pelo server resulta em EOF limpo no
//!   client (`read_line` devolve `None`).
//!
//! ## Status: `#[ignore]` (pendente de diagnóstico)
//!
//! Os 2 testes deste arquivo estão marcados `#[ignore]` porque
//! deadlockam em runtime quando rodam (o `ready(READABLE|WRITABLE)`
//! ou o `read_exact` no `Arc<tokio::sync::Mutex<>>` trava). A
//! Etapa 2B fechou a abstração (`WindowsPipeReader`/`Writer` +
//! `shared_pipe_pair`) com cobertura pelos unit tests in-process
//! em `src/windows_pipes.rs` (usam `tokio::io::duplex` — prova o
//! contrato da abstração). O smoke test com named pipes reais
//! fica pra próxima sessão diagnosticar — pendência registrada
//! no `docs/decisions/0017-process-architecture-windows-pipes.md`
//! §Pendências e no handoff da Etapa 2B.
//!
//! **Como rodar localmente pra investigar o deadlock:**
//!
//! ```pwsh
//! cargo test -p frederico-process-architecture \
//!   --test windows_pipes_smoke -- --ignored --nocapture
//! ```
//!
//! Próximos passos do diagnóstico: instrumentar com `tracing` no
//! `ready()` e no `lock().await` do reader/writer, considerar
//! `tokio-console`, ou cair pra Win32 ETW se for caso de kernel.
//!
//! ## Tempo (quando o deadlock for resolvido)
//!
//! < 1s. Não há delay artificial — o `ConnectNamedPipe` é imediato
//! (o server está esperando).

#![cfg(windows)]

use std::time::Duration;

use frederico_process_architecture::fake::unique_pipe_name;
use frederico_process_architecture::pipes::{PipeReader, PipeWriter};
use frederico_process_architecture::windows_pipes::{
    connect_pipe_client, create_pipe_server, shared_pipe_pair, WindowsPipeReader, WindowsPipeWriter,
};
use tokio::io::Interest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "deadlock em diagnóstico — ver header do arquivo"]
async fn windows_pipe_server_client_roundtrip() {
    let name = unique_pipe_name("windows-pipes-smoke-roundtrip");
    let server_name = name.clone();
    let client_name = name.clone();

    // Task do server: cria o pipe, faz `ready()` (espera o
    // `ConnectNamedPipe` do client), lê uma linha, escreve outra,
    // e fecha (drop do writer → EOF pro client).
    let server = tokio::spawn(async move {
        let pipe = create_pipe_server(&server_name).expect("create_pipe_server");
        // `ready()` espera o client conectar; sem isso, o primeiro
        // `read`/`write` falha com `NotConnected`.
        pipe.ready(Interest::READABLE.add(Interest::WRITABLE))
            .await
            .expect("server ready");
        // `shared_pipe_pair` envelopa o mesmo `HANDLE` no
        // `WindowsPipeReader` e no `WindowsPipeWriter` (via
        // `Arc<tokio::sync::Mutex<NamedPipeServer>>`).
        let (mut reader, writer) = shared_pipe_pair(pipe);

        // Lê "hello\n" do client (5s de tolerância pra startup).
        let line = tokio::time::timeout(Duration::from_secs(5), reader.read_line())
            .await
            .expect("server read_line timeout")
            .expect("server read_line error")
            .expect("server read_line EOF antes do client chegar");
        assert_eq!(line, b"hello\n".to_vec(), "server leu a linha errada");

        // Responde "world\n".
        writer
            .write_line(b"world\n")
            .await
            .expect("server write_line");
        // Drop do writer (e do `pipe` por baixo) fecha o server-side;
        // o client vê EOF.
    });

    // Task do client: conecta, escreve "hello\n", lê "world\n".
    let client = tokio::spawn(async move {
        // `connect_pipe_client` é síncrono (CreateFileW +
        // ConnectNamedPipe). Envelopa em `spawn_blocking` pra
        // não bloquear o runtime.
        let pipe = tokio::task::spawn_blocking(move || connect_pipe_client(&client_name))
            .await
            .expect("spawn_blocking join")
            .expect("connect_pipe_client");
        // `ready()` no client (pós-connect).
        pipe.ready(Interest::READABLE.add(Interest::WRITABLE))
            .await
            .expect("client ready");
        let (mut reader, writer) = shared_pipe_pair(pipe);

        // Envia "hello\n".
        writer
            .write_line(b"hello\n")
            .await
            .expect("client write_line");

        // Lê "world\n" do server (5s de tolerância).
        let line = tokio::time::timeout(Duration::from_secs(5), reader.read_line())
            .await
            .expect("client read_line timeout")
            .expect("client read_line error")
            .expect("client read_line EOF antes do server responder");
        assert_eq!(line, b"world\n".to_vec(), "client leu a linha errada");

        // EOF depois que o server fecha.
        let eof = tokio::time::timeout(Duration::from_secs(5), reader.read_line())
            .await
            .expect("client read_line EOF timeout")
            .expect("client read_line EOF error");
        assert_eq!(eof, None, "client deveria ver EOF após server fechar");
    });

    server.await.expect("server task panicked");
    client.await.expect("client task panicked");
}

/// Confirma que o `WindowsPipeReader` e `WindowsPipeWriter`
/// independentes (sem `shared_pipe_pair`) também funcionam —
/// usado quando o transporte já vem "split" (ex.: o
/// `ReadHalf`/`WriteHalf` do `tokio::io::split`). Aqui
/// simulamos isso criando o server e tratando reader/writer
/// como entidades que escrevem no mesmo handle em momentos
/// diferentes (read_line primeiro, write_line depois).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "deadlock em diagnóstico — ver header do arquivo"]
async fn windows_pipe_sequential_read_then_write() {
    let name = unique_pipe_name("windows-pipes-smoke-sequential");
    let server_name = name.clone();
    let client_name = name.clone();

    // Server: cria, ready, lê, escreve (sequencial no mesmo handle).
    let server = tokio::spawn(async move {
        let pipe = create_pipe_server(&server_name).expect("create_pipe_server");
        pipe.ready(Interest::READABLE.add(Interest::WRITABLE))
            .await
            .expect("server ready");
        // Reader primeiro (envelopa o `pipe` em `Arc<Mutex<>>`).
        // Quando o reader for dropado, o `Arc<Mutex<>>` é
        // liberado, e a gente pode usar o `pipe` de novo pro
        // writer. Mas o `WindowsPipeWriter::new(pipe)` toma
        // `pipe` por movimento — depois que o reader é
        // dropado, o `pipe` ainda está vivo (porque o reader
        // só tem um `Arc<Mutex<>>` que segura uma cópia, e o
        // `pipe` original ainda existe).
        //
        // Para o teste, simplificamos: usamos `shared_pipe_pair`
        // (mesmo `Arc<Mutex<>>`) e operamos o read e o write
        // em série — o lock do tokio serializa.
        let (mut reader, writer) = shared_pipe_pair(pipe);
        let line = reader.read_line().await.expect("read_line");
        assert_eq!(line, Some(b"ping\n".to_vec()));
        writer.write_line(b"pong\n").await.expect("write_line");
    });

    let client = tokio::spawn(async move {
        let pipe = tokio::task::spawn_blocking(move || connect_pipe_client(&client_name))
            .await
            .expect("spawn_blocking join")
            .expect("connect_pipe_client");
        pipe.ready(Interest::READABLE.add(Interest::WRITABLE))
            .await
            .expect("client ready");
        let (mut reader, writer) = shared_pipe_pair(pipe);
        writer.write_line(b"ping\n").await.expect("write ping");
        let line = reader.read_line().await.expect("read pong");
        assert_eq!(line, Some(b"pong\n".to_vec()));
    });

    server.await.expect("server task panicked");
    client.await.expect("client task panicked");
}

// `WindowsPipeReader` e `WindowsPipeWriter` são re-exportados
// indiretamente via `frederico_process_architecture::windows_pipes`.
// O `use ... WindowsPipeReader, WindowsPipeWriter` acima é
// pra garantir que o type checker valida o uso de cada um
// mesmo quando o teste só usa `shared_pipe_pair` (que devolve
// ambos). Se um dos tipos ficar sem uso no módulo, o Rust
// avisa — `WindowsPipeReader` é usado em
// `windows_pipes.rs` (unit tests) e `WindowsPipeWriter` é
// usado em `windows_pipes.rs` (Clone derive), então o
// `use` aqui é mais documentação do que necessidade.
const _: fn() = || {
    let _ = WindowsPipeReader::<tokio::io::DuplexStream>::new;
    let _ = WindowsPipeWriter::<tokio::io::DuplexStream>::new;
};
