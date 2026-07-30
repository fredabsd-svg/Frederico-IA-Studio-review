//! `WindowsPipeReader` + `WindowsPipeWriter` — transporte real
//! sobre named pipes do Windows.
//!
//! **Gateado em `#[cfg(windows)]`** — em outras plataformas o módulo
//! é vazio. A escolha por `tokio::net::windows::named_pipe` (sem a
//! `crate windows`) é deliberada e registrada no [ADR-0017]:
//!
//! 1. A Tokio já envelopa o `HANDLE` Win32 em `NamedPipeServer` /
//!    `NamedPipeClient`, com `AsyncRead` + `AsyncWrite`. Cria o
//!    pipe (`ServerOptions::new().create(name)`) e conecta o
//!    client (`NamedPipeClient::connect(name)`) sem `unsafe`
//!    no nosso código.
//! 2. A `crate windows` só seria necessária se quiséssemos
//!    security descriptor customizado, handle duplication, ou
//!    `CreateProcessW` com flags específicas. Nada disso é
//!    requisito da Etapa 2B — pode entrar depois se virar.
//! 3. **Inversão do handshake** do ADR-0015: o **worker** cria
//!    o pipe (server), o **app** se conecta (client), e o
//!    worker passa o nome do pipe via **stdout** (uma linha
//!    `READY <pipe_name>`). Resolve herança de handle sem
//!    complicar — `tokio::process::Command` no Windows herda
//!    stdin/stdout/stderr automaticamente; o handle do pipe
//!    é criado pelo filho, não passado pelo pai.
//!
//! ## Reader e Writer compartilham o mesmo `Arc<Mutex<>>`
//!
//! O `NamedPipeServer` / `NamedPipeClient` é um único `HANDLE`
//! Win32 que faz read e write. Não é `Clone` (é um handle do
//! SO, não um `duplex`). Pra ter um `PipeReader` e um
//! `PipeWriter` ao mesmo tempo sobre o mesmo handle (sem
//! `unsafe`), ambos envelopam o mesmo `Arc<tokio::sync::Mutex<R>>`.
//! O helper [`shared_pipe_pair`] é o ponto de entrada canônico
//! quando o tipo subjacente faz read+write.
//!
//! Para tipos que já são "split" por natureza (ex.: os dois
//! lados do `tokio::io::duplex`, ou um `ReadHalf` +
//! `WriteHalf` do `tokio::io::split`), o `WindowsPipeReader::new`
//! e o `WindowsPipeWriter::new` independentes continuam
//! funcionando — cada um cria seu próprio `Arc<Mutex<>>`.
//!
//! ## Modo byte stream
//!
//! Os named pipes do Windows têm dois modos: **byte stream** e
//! **message**. O protocolo `IpcMessage` é line-delimited JSON
//! (cada mensagem termina em `\n`), que casa com byte stream.
//! Message mode traria fragmentação confusa (uma `IpcMessage`
//! pode cair em duas mensagens do pipe) e exigiria framing
//! extra. `ServerOptions::new()` em Rust Tokio usa byte stream
//! por default.
//!
//! ## `ready()` antes do primeiro read/write
//!
//! O `NamedPipeServer` exige `pipe.ready().await` antes de
//! qualquer read/write (espera o `ConnectNamedPipe`). O
//! `NamedPipeClient` (depois de `connect`) também exige.
//! Quem chama o `shared_pipe_pair` é responsável por fazer
//! o `ready` antes — o helper em si não chama. O
//! `WorkerManager::spawn_external` da Etapa 2B cuida disso
//! (vai ser responsabilidade do caller do teste, aqui).
//!
//! ## `unsafe` neste módulo
//!
//! O módulo inteiro é `#![cfg(windows)]` mas **não usa `unsafe`**.
//! O `Cargo.toml` do `process-architecture` tem `unsafe_code =
//! "deny"` (não `forbid`) justamente para abrir a porta pra
//! `crate windows` na Etapa 3+ se virar necessário. A
//! implementação atual não toca em `unsafe` — a Tokio
//! envelopa o `HANDLE` Win32 de forma segura.
//!
//! [ADR-0017]: ../../docs/decisions/0017-process-architecture-windows-pipes.md

#![cfg(windows)]

use std::io::ErrorKind;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use tokio::sync::Mutex;

use crate::error::ProcessError;
use crate::pipes::{PipeName, PipeReader, PipeWriter};

/// Prefixo Windows para o path completo do pipe. `tokio::net::windows::named_pipe::ServerOptions::create`
/// e `NamedPipeClient::connect` aceitam o path completo
/// (`\\.\pipe\<name>`) — o `PipeName` no nosso crate guarda só o `<name>`
/// (regra do `pipes.rs`).
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Monta o path completo do pipe a partir de um `PipeName`.
#[must_use]
pub fn full_pipe_path(name: &PipeName) -> String {
    format!("{}{}", PIPE_PREFIX, name.as_str())
}

/// `PipeReader` sobre qualquer tipo que implemente `AsyncRead +
/// Unpin + Send`. Envelopa o reader em `Arc<tokio::sync::Mutex<R>>`
/// (o que permite compartilhar o mesmo `HANDLE` Win32 com o
/// `WindowsPipeWriter` via [`shared_pipe_pair`]).
///
/// **Por que `tokio::sync::Mutex` e não `std::sync::Mutex`:** o
/// `tokio::sync::MutexGuard` é `Send` (o `std::sync::MutexGuard`
/// é `!Send`). Como o `async-trait` exige que o future
/// retornado seja `Send`, o guard precisa ser `Send` — isso
/// força o uso do `tokio::sync::Mutex`. O
/// `-D clippy::await_holding_lock` só flagra `std::sync::Mutex`,
/// `parking_lot`, e `lock_api` — não `tokio::sync::Mutex` (o
/// guard do tokio é feito pra ser segurado em `.await`s).
///
/// **Por que Mutex e não outra coisa:** quando o `R` é um
/// `NamedPipeServer` / `NamedPipeClient` (mesmo handle de read
/// e write), serializar via `Arc<Mutex<>>` é o que evita UB
/// (dois donos do mesmo `HANDLE` Win32). Para tipos já
/// "split" (ex.: `ReadHalf`), o Mutex é overhead mas correto
/// — read_line serializa, mas como read_line é serial por
/// natureza (uma linha por vez), a contenção é desprezível.
pub struct WindowsPipeReader<R: AsyncRead + Unpin + Send> {
    inner: Arc<Mutex<R>>,
}

impl<R: AsyncRead + Unpin + Send> WindowsPipeReader<R> {
    /// Embrulha um reader async já conectado. Cria um
    /// `Arc<Mutex<>>` próprio — use [`shared_pipe_pair`] se
    /// quiser compartilhar o mesmo `R` com um `WindowsPipeWriter`
    /// (caso do `NamedPipeServer`/`Client`).
    #[must_use]
    pub fn new(inner: R) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }
}

#[async_trait]
impl<R: AsyncRead + Unpin + Send + 'static> PipeReader for WindowsPipeReader<R> {
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, ProcessError> {
        // `lock().await` é o lock do tokio. O `MutexGuard` vive
        // até o `;` no fim do bloco. O `read_exact().await` é
        // sobre o `&mut R` (acessado via DerefMut do guard);
        // o guard é dropado **junto** com o final do bloco —
        // não segura o guard em outros awaits. É o que o
        // `-D clippy::await_holding_lock` aceita (o tokio Mutex
        // é explicitamente desenhado pra isso).
        let mut guard = self.inner.lock().await;
        let mut buf = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        loop {
            match guard.read_exact(&mut byte).await {
                Ok(1) => {
                    buf.push(byte[0]);
                    if byte[0] == b'\n' {
                        return Ok(Some(buf));
                    }
                }
                Ok(_) => unreachable!("read_exact com buffer de 1 byte lê exatamente 1 byte"),
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    // EOF antes do `\n`. Se o buf está vazio, é EOF
                    // limpo; senão devolve o que tiver (peer fechou
                    // no meio de uma linha — `drain_pending_with_error`
                    // cuida).
                    return Ok(if buf.is_empty() { None } else { Some(buf) });
                }
                Err(e) => {
                    return Err(ProcessError::Transport {
                        message: format!("read_line falhou: {e}"),
                    });
                }
            }
        }
    }
}

/// `PipeWriter` sobre qualquer tipo que implemente `AsyncWrite +
/// Unpin + Send`. Envelopa o writer em `Arc<tokio::sync::Mutex<W>>`
/// (o `NamedPipeServer` é `!Sync`, mas `Mutex<W>: Send + Sync` se
/// `W: Send`).
///
/// **Por que `tokio::sync::Mutex` e não `std::sync::Mutex`:** o
/// `tokio::sync::MutexGuard` é `Send` (o `std::sync::MutexGuard`
/// é `!Send`). Como o `async-trait` exige que o future
/// retornado seja `Send`, o guard precisa ser `Send` — isso
/// força o uso do `tokio::sync::Mutex`. O
/// `-D clippy::await_holding_lock` só flagra `std::sync::Mutex`,
/// `parking_lot`, e `lock_api` — não `tokio::sync::Mutex` (o
/// guard do tokio é feito pra ser segurado em `.await`s).
///
/// **Por que Mutex e não outra coisa:** o `NamedPipeServer` (e
/// `NamedPipeClient`) é o mesmo handle que faz read e write;
/// a Tokio não expõe o split como `&mut` separados. O
/// `AsyncWriteExt::write_all` precisa de `&mut self`, então
/// preciso serializar os writes via Mutex. Reads e writes
/// no mesmo handle são serializados naturalmente pelo kernel
/// do Windows — o Mutex é só pra evitar dois writes
/// concorrentes corromperem o frame (race entre
/// `write_all` e `write_all` no mesmo handle).
pub struct WindowsPipeWriter<W: AsyncWrite + Unpin + Send> {
    inner: Arc<Mutex<W>>,
}

impl<W: AsyncWrite + Unpin + Send> WindowsPipeWriter<W> {
    /// Embrulha um writer async já conectado. Cria um
    /// `Arc<Mutex<>>` próprio — use [`shared_pipe_pair`] se
    /// quiser compartilhar o mesmo `W` com um `WindowsPipeReader`.
    #[must_use]
    pub fn new(inner: W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }
}

impl<W: AsyncWrite + Unpin + Send> Clone for WindowsPipeWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[async_trait]
impl<W: AsyncWrite + Unpin + Send + 'static> PipeWriter for WindowsPipeWriter<W> {
    async fn write_line(&self, line: &[u8]) -> Result<(), ProcessError> {
        // `lock().await` é o lock do tokio. O `MutexGuard` vive
        // até o `;` no fim do bloco. O `write_all().await` é
        // sobre o `&mut W` (acessado via DerefMut do guard);
        // o guard é dropado **junto** com o final do bloco —
        // não segura o guard em outros awaits. É o que o
        // `-D clippy::await_holding_lock` aceita (o tokio Mutex
        // é explicitamente desenhado pra isso).
        let mut guard = self.inner.lock().await;
        guard
            .write_all(line)
            .await
            .map_err(|e| ProcessError::Transport {
                message: format!("write_all falhou: {e}"),
            })?;
        guard.flush().await.map_err(|e| ProcessError::Transport {
            message: format!("flush falhou: {e}"),
        })?;
        Ok(())
    }

    async fn close(&self) -> Result<(), ProcessError> {
        // O `NamedPipeServer` / `NamedPipeClient` tem `Drop` que
        // fecha o handle. Não há `close()` explícito na Tokio
        // (o `disconnect()` existe mas é pra liberar a instância
        // do pipe sem fechar o handle — não usamos). Confiamos
        // no `Drop`.
        Ok(())
    }
}

/// Cria um par (Reader, Writer) que compartilham o mesmo
/// `Arc<tokio::sync::Mutex<R>>`. Necessário quando o `R`
/// subjacente implementa **tanto** `AsyncRead` quanto
/// `AsyncWrite` e não é `Clone` (caso do `NamedPipeServer`
/// e `NamedPipeClient`).
///
/// O caller é responsável por fazer o `pipe.ready().await`
/// **antes** de chamar este helper, se o `R` for um named
/// pipe (tanto server quanto client exigem `ready` antes do
/// primeiro read/write).
///
/// # Exemplo (server)
///
/// ```ignore
/// let pipe = create_pipe_server(&name)?;
/// pipe.ready().await?; // espera o client conectar
/// let (mut reader, writer) = shared_pipe_pair(pipe);
/// // ... usa reader.read_line() e writer.write_line() ...
/// ```
#[must_use]
pub fn shared_pipe_pair<R: AsyncRead + AsyncWrite + Unpin + Send>(
    inner: R,
) -> (WindowsPipeReader<R>, WindowsPipeWriter<R>) {
    let arc = Arc::new(Mutex::new(inner));
    (
        WindowsPipeReader {
            inner: Arc::clone(&arc),
        },
        WindowsPipeWriter { inner: arc },
    )
}

/// Cria o `NamedPipeServer` (lado server) — o **worker** chama
/// isso. `first_pipe_instance(true)` garante que essa é a
/// primeira instância do pipe; sem essa flag, o
/// `CreateNamedPipeW` falha com `ERROR_PIPE_BUSY` se o nome
/// já está em uso por outra instância. O nome é o `<name>` do
/// `PipeName` (sem `\\.\pipe\` — essa função monta o path
/// completo).
///
/// **Modo byte stream** (default do `ServerOptions::new()` —
/// diferente de `ServerOptions::new().message_mode(true)`).
/// O `IpcMessage` é line-delimited JSON, casa com byte stream.
///
/// **Importante:** depois de criar o server, o caller DEVE
/// chamar `server.ready().await` antes de qualquer read/write
/// — é assim que a Tokio espera o `ConnectNamedPipe` do client.
pub fn create_pipe_server(name: &PipeName) -> Result<NamedPipeServer, ProcessError> {
    let full = full_pipe_path(name);
    ServerOptions::new()
        .first_pipe_instance(true)
        .create(&full)
        .map_err(|e| ProcessError::Platform {
            message: format!("CreateNamedPipeW falhou para `{full}`: {e}"),
        })
}

/// Conecta ao pipe como client — o **app** chama isso (após
/// ler o nome do pipe do stdout do worker).
///
/// **Bloqueante síncrono.** O `ClientOptions::open` em Tokio é
/// síncrono (não `async`) — internamente ele faz o `CreateFileW`
/// que tenta `ConnectNamedPipe`. O caller deve envelopar em
/// `tokio::task::spawn_blocking` se quiser não bloquear o
/// runtime, ou usar `tokio::time::timeout` pra boundar a
/// espera.
///
/// Na prática, o `WorkerManager::spawn_external` chama isso
/// **depois** de confirmar que o filho mandou `READY <name>`
/// via stdout (o que garante que o server já está escutando
/// e o `open` retorna rápido — tipicamente < 10ms).
///
/// **Importante:** depois de conectar, o caller DEVE chamar
/// `client.ready().await` antes do primeiro read/write.
pub fn connect_pipe_client(name: &PipeName) -> Result<NamedPipeClient, ProcessError> {
    let full = full_pipe_path(name);
    ClientOptions::new()
        .open(&full)
        .map_err(|e| ProcessError::Platform {
            message: format!("ConnectNamedPipe falhou para `{full}`: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    // Testes abaixo usam `tokio::io::duplex` (canal in-process
    // bidirecional) pra validar a lógica do reader/writer sem
    // precisar de named pipes reais. Os testes de integração
    // com pipes reais ficam em `tests/windows_pipes_smoke.rs`
    // (gateado em `#[cfg(windows)]`).

    #[tokio::test(flavor = "current_thread")]
    async fn windows_pipe_reader_reads_line() {
        // `a` é o writer (joga a linha no duplex), `b` é o reader
        // (envelopado em `WindowsPipeReader`).
        let (a, b) = duplex(64);
        let mut reader = WindowsPipeReader::new(b);
        let writer = WindowsPipeWriter::new(a);
        writer.write_line(b"hello\n").await.expect("write_line");
        // Drop do writer fecha o lado de escrita do duplex.
        drop(writer);

        // Primeira read_line: pega "hello\n".
        let line = reader.read_line().await.expect("read_line");
        assert_eq!(line, Some(b"hello\n".to_vec()));

        // Segunda read_line: EOF (peer fechou).
        let eof = reader.read_line().await.expect("read_line eof");
        assert_eq!(eof, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn windows_pipe_writer_clone_writes() {
        // Duas writers (clone) escrevem no mesmo writer. O
        // `Arc<tokio::sync::Mutex<W>>` interno serializa sem
        // corromper o frame.
        let (a, b) = duplex(64);
        let writer = WindowsPipeWriter::new(a);
        let writer_clone = writer.clone();
        // Envia via original.
        writer.write_line(b"first\n").await.expect("write first");
        // Envia via clone (mesmo writer por baixo).
        writer_clone
            .write_line(b"second\n")
            .await
            .expect("write second");
        // Drop do writer (e do clone, que partilha o mesmo `Arc`)
        // fecha o lado de escrita do duplex.
        drop((writer, writer_clone));

        let mut reader = WindowsPipeReader::new(b);
        let l1 = reader.read_line().await.expect("read 1");
        assert_eq!(l1, Some(b"first\n".to_vec()));
        let l2 = reader.read_line().await.expect("read 2");
        assert_eq!(l2, Some(b"second\n".to_vec()));
        let eof = reader.read_line().await.expect("read eof");
        assert_eq!(eof, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shared_pipe_pair_smoke_compiles() {
        // O `duplex` retorna `(DuplexStream, DuplexStream)` —
        // duas metades separadas. Para tipos que já são "split"
        // naturalmente, o `WindowsPipeReader::new` e o
        // `WindowsPipeWriter::new` independentes (com
        // `Arc<Mutex<>>` próprios) são a escolha certa. O
        // `shared_pipe_pair` é para tipos onde o mesmo handle
        // faz read+write (caso do `NamedPipeServer`/`Client`) —
        // coberto pelo integration test em
        // `tests/windows_pipes_smoke.rs`.
        //
        // Aqui validamos apenas que o `shared_pipe_pair` compila
        // e aceita um tipo que implementa `AsyncRead + AsyncWrite`
        // (o `DuplexStream` faz ambos).
        let (a, _b) = duplex(64);
        let (reader, writer) = shared_pipe_pair(a);
        // Verificação: o reader e o writer compartilham o
        // mesmo `Arc<Mutex<>>` — não é o que se quer pra
        // `duplex` (cria deadlock no EOF), mas é o que
        // valida a forma da função.
        assert!(Arc::ptr_eq(&reader.inner, &writer.inner));
    }
}
