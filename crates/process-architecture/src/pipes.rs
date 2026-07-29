//! Interface abstrata de transporte para o `WorkerManager`.
//!
//! O transporte é **dividido em duas metades** ([`PipeReader`] e
//! [`PipeWriter`]), em vez de uma trait única. A divisão é o que
//! destrava o modelo de ator do `WorkerManager`
//! ([`crate::manager`], ADR-0015):
//!
//! - A **task do ator** fica com **as duas metades juntas** (`Box<dyn
//!   PipeReader>` + `Box<dyn PipeWriter>`) — ela é a única dona do
//!   pipe. Sem `Arc<Mutex<Box<dyn Pipe>>>`, sem `MutexGuard` segurado
//!   em `.await`.
//! - [`PipeWriter`] é `Clone` (a `FakePipeClient` envelopa um
//!   `mpsc::Sender`; a `WindowsPipeWriter` da Etapa 2B envelopa
//!   `Arc<NamedPipeServer>`, que é `Clone` no `windows` crate). O
//!   `WorkerManager::invoke` carrega um clone, sem lock.
//! - [`PipeReader`] **não** é `Clone` — leitura é serializada pela
//!   task do ator. Concorrência não vem de readers paralelos; vem
//!   de **múltiplas requests em voo** com `request_id` distinto
//!   (correlacionadas via `oneshot`).
//!
//! **Contrato:** line-delimited JSON. Cada mensagem termina em
//! `\n`. O `read_line` lê até o próximo `\n` (inclusivo); o
//! `write_line` aceita um buffer que **já inclui** o `\n` final
//! (o `IpcMessage::encode_line` produz esse formato).
//!
//! # Por que não `Pipe = PipeReader + PipeWriter` (uma trait só)
//!
//! A versão da Etapa 2A original tinha uma trait `Pipe` única, e
//! isso foi a raiz do deadlock (o `WorkerManager` envolvia
//! `Arc<Mutex<Box<dyn Pipe>>>` e o `MutexGuard` era segurado em
//! `tx.send().await` no write e em `rx.recv().await` no read).
//! Dividir em `PipeReader` + `PipeWriter` separados torna o
//! impossível (a `PipeReader` é o que é enviada à task; a
//! `PipeWriter` é o que é clonada pro `invoke`; as duas nunca se
//! atravessam).

use std::fmt;

use async_trait::async_trait;

use crate::error::ProcessError;

/// Nome do pipe — string opaca validada no construtor.
///
/// Convenção (Windows): `\\.\pipe\<name>` é o caminho real; o
/// `PipeName` guarda só o `<name>`. O `tokio::net::windows::named_pipe`
/// constrói o path completo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipeName(String);

impl PipeName {
    /// Cria um `PipeName`. Rejeita nomes vazios, com `\`, e que
    /// excedam 200 chars (limite do Windows para `\\.\pipe\<name>`).
    pub fn new(s: impl Into<String>) -> Result<Self, ProcessError> {
        let s = s.into();
        if s.is_empty() {
            return Err(ProcessError::Platform {
                message: "PipeName não pode ser vazio".to_string(),
            });
        }
        if s.len() > 200 {
            return Err(ProcessError::Platform {
                message: format!("PipeName muito longo: {} chars (máx 200)", s.len()),
            });
        }
        if s.contains('\\') || s.contains('/') {
            return Err(ProcessError::Platform {
                message: format!("PipeName não pode conter `\\` ou `/`: {s:?}"),
            });
        }
        Ok(Self(s))
    }

    /// A string por baixo.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PipeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Metade de **leitura** do transporte. Não é `Clone` — a leitura
/// é serializada pela task do ator. Concorrência vem de múltiplas
/// requests em voo, não de readers paralelos.
#[async_trait]
pub trait PipeReader: Send {
    /// Lê uma linha (terminada em `\n`). Devolve `None` em EOF
    /// (peer fechou a conexão limpa).
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, ProcessError>;
}

/// Metade de **escrita** do transporte. É `Clone` — o
/// `WorkerManager` carrega um clone, sem lock. Concorrência
/// interna fica a cargo da implementação concreta: a
/// `FakePipeWriter` envelopa um `mpsc::Sender` (que já é
/// concorrente); a `WindowsPipeWriter` da Etapa 2B envelopa
/// `Arc<NamedPipeServer>`, que é serializado pelo SO.
#[async_trait]
pub trait PipeWriter: Send + Sync {
    /// Escreve uma linha. O `line` **já deve incluir** o `\n`
    /// final.
    async fn write_line(&self, line: &[u8]) -> Result<(), ProcessError>;

    /// Fecha a conexão (idempotente).
    async fn close(&self) -> Result<(), ProcessError>;
}
