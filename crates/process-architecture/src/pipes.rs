//! Interface abstrata de transporte para o `WorkerManager`.
//!
//! O `WorkerManager` opera com `Box<dyn Pipe>` — a implementação
//! concreta (named pipes Windows, in-process channel pra testes,
//! unix socket, ...) vive em submódulos. A Etapa 2A entrega só o
//! fake in-process; a Etapa 2B adiciona o `WindowsPipeClient`/
//! `WindowsPipeServer` via `windows` crate (gateado em
//! `#[cfg(target_os = "windows")]`).
//!
//! **Contrato:** line-delimited JSON. Cada mensagem termina em
//! `\n`. O `read_line` lê até o próximo `\n` (inclusivo); o
//! `write_line` aceita um buffer que **já inclui** o `\n` final
//! (o `IpcMessage::encode_line` produz esse formato).

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

/// Conexão de transporte subjacente (named pipe, in-process, etc.).
///
/// O `WorkerManager` trata `Box<dyn Pipe>` como opaco — a única
/// coisa que importa é o contrato line-delimited JSON.
#[async_trait]
pub trait Pipe: Send {
    /// Lê uma linha (terminada em `\n`). Devolve `None` em EOF
    /// (peer fechou a conexão limpa).
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, ProcessError>;

    /// Escreve uma linha. O `line` **já deve incluir** o `\n`
    /// final.
    async fn write_line(&mut self, line: &[u8]) -> Result<(), ProcessError>;

    /// Fecha a conexão (idempotente).
    async fn close(&mut self) -> Result<(), ProcessError>;
}
