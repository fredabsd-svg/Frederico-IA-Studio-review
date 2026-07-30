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
//! ## `connect()` no server, `ready()` no client
//!
//! A Tokio 1.x expõe **dois métodos** no `NamedPipeServer`:
//!
//! - `pipe.connect().await` — espera o `ConnectNamedPipe` Win32
//!   (o client fazer `CreateFileW` no nome). É o método certo
//!   **antes** do primeiro read/write.
//! - `pipe.ready(interest).await` — espera o `Interest`
//!   solicitado ficar pronto. Num `NamedPipeServer` **pré-connect**
//!   ele trava esperando readiness que nunca chega (não há dados
//!   pra ler antes do connect, e `WRITABLE` fica preso esperando
//!   o próprio connect).
//!
//! O `NamedPipeClient` (depois de `ClientOptions::open`) já está
//! conectado — `ready(READABLE | WRITABLE)` retorna imediato.
//!
//! Trocar `ready(READABLE | WRITABLE)` por `connect()` no server
//! foi o fix do deadlock da Etapa 2B (Etapa 2B continuação).
//!
//! ## Como rodar
//!
//! ```pwsh
//! cargo test -p frederico-process-architecture --test windows_pipes_smoke
//! ```
//!
//! Roda sem `--ignored` (os 2 testes deste arquivo são parte da
//! suíte normal desde a Etapa 2B continuação).
//!
//! ## Tempo
//!
//! < 100ms total. O `ConnectNamedPipe` é imediato (o server está
//! esperando via `connect()` antes do client tentar abrir).

#![cfg(windows)]

use std::time::Duration;

use frederico_process_architecture::fake::unique_pipe_name;
use frederico_process_architecture::pipes::{PipeReader, PipeWriter};
use frederico_process_architecture::windows_pipes::{
    connect_pipe_client, create_pipe_server, shared_pipe_pair, WindowsPipeReader, WindowsPipeWriter,
};
use tokio::io::Interest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_pipe_server_client_roundtrip() {
    let name = unique_pipe_name("windows-pipes-smoke-roundtrip");
    let server_name = name.clone();
    let client_name = name.clone();

    // Task do server: cria o pipe, `connect()` (espera o
    // `ConnectNamedPipe` do client), lê uma linha, escreve outra,
    // e fecha (drop do writer → EOF pro client).
    let server = tokio::spawn(async move {
        let pipe = create_pipe_server(&server_name).expect("create_pipe_server");
        // `connect()` no `NamedPipeServer` (NÃO `ready()`) — é o
        // método que envelopa `ConnectNamedPipe` Win32. Ver
        // header do arquivo pra justificativa completa.
        pipe.connect().await.expect("server connect");
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
        // `ready()` no client (pós-connect). Diferente do server:
        // o client JÁ está conectado depois de `ClientOptions::open`,
        // então `ready(READABLE|WRITABLE)` é a forma correta de
        // esperar readiness (na prática retorna imediato, mas a
        // chamada explícita é o padrão recomendado pela Tokio).
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

/// Confirma que `read_line` e `write_line` no mesmo handle (sem
/// `shared_pipe_pair`) também funcionam — usado quando o transporte
/// já vem "split" (ex.: `ReadHalf`/`WriteHalf` do
/// `tokio::io::split`). Aqui simulamos isso criando o server e
/// tratando reader/writer como entidades que operam no mesmo
/// handle em momentos diferentes (read_line primeiro, write_line
/// depois) — o `Arc<tokio::sync::Mutex<>>` do `shared_pipe_pair`
/// serializa.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_pipe_sequential_read_then_write() {
    let name = unique_pipe_name("windows-pipes-smoke-sequential");
    let server_name = name.clone();
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        let pipe = create_pipe_server(&server_name).expect("create_pipe_server");
        // `connect()` no server (NÃO `ready()`).
        pipe.connect().await.expect("server connect");
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
        // `ready()` no client (pós-connect).
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
