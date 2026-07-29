//! Erros do `frederico-process-architecture`.
//!
//! O `ProcessError` é o tipo único de erro que o `WorkerManager` e
//! o `PipeClient`/`PipeServer` devolvem. Cada variante carrega
//! contexto suficiente pra o caller distinguir entre erro de
//! protocolo (manifesto inválido, opcode desconhecido), erro de
//! transporte (pipe quebrado, timeout), e erro de plataforma
//! (worker não subiu, executável faltando).

use thiserror::Error;

/// Categoria do erro. Consumida pelo `execution-engine` da Etapa 3
/// pra mapear o erro pra `TOOL_ERROR` com a mensagem preservada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ProcessErrorKind {
    /// Erro de protocolo — JSON malformado, opcode desconhecido,
    /// manifesto inválido, request_id duplicado.
    Protocol,
    /// Erro de transporte — pipe quebrado, conexão recusada, EOF
    /// inesperado, write falhou.
    Transport,
    /// Worker demorou mais que o `timeout_ms` declarado.
    Timeout,
    /// Worker foi morto pelo watchdog (passou do budget).
    Cancelled,
    /// Worker não está saudável (health != Ok).
    Unhealthy,
    /// Plataforma — executável faltando, sem permissão, OS
    /// incompatível.
    Platform,
}

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum ProcessError {
    /// Erro de protocolo IPC.
    #[error("erro de protocolo: {message}")]
    Protocol {
        /// Mensagem curta do que falhou.
        message: String,
    },

    /// Erro de transporte (pipe / conexão).
    #[error("erro de transporte: {message}")]
    Transport {
        /// Mensagem curta do que falhou.
        message: String,
    },

    /// Timeout — o worker não respondeu dentro do `timeout_ms`.
    #[error("timeout: worker {worker_id} não respondeu em {timeout_ms}ms")]
    Timeout {
        /// ID do worker.
        worker_id: String,
        /// Tempo limite configurado.
        timeout_ms: u32,
    },

    /// Worker cancelado pelo watchdog.
    #[error("worker {worker_id} cancelado: {reason}")]
    Cancelled {
        /// ID do worker.
        worker_id: String,
        /// Motivo do cancelamento.
        reason: String,
    },

    /// Worker reportou `health != Ok` no último healthcheck.
    #[error("worker {worker_id} não está saudável: {message}")]
    Unhealthy {
        /// ID do worker.
        worker_id: String,
        /// Mensagem do healthcheck.
        message: String,
    },

    /// Erro de plataforma.
    #[error("erro de plataforma: {message}")]
    Platform {
        /// Mensagem curta do que falhou.
        message: String,
    },
}

impl ProcessError {
    /// Categoria do erro.
    #[must_use]
    pub const fn kind(&self) -> ProcessErrorKind {
        match self {
            Self::Protocol { .. } => ProcessErrorKind::Protocol,
            Self::Transport { .. } => ProcessErrorKind::Transport,
            Self::Timeout { .. } => ProcessErrorKind::Timeout,
            Self::Cancelled { .. } => ProcessErrorKind::Cancelled,
            Self::Unhealthy { .. } => ProcessErrorKind::Unhealthy,
            Self::Platform { .. } => ProcessErrorKind::Platform,
        }
    }

    /// Código estável (snake_case) — consumido pelo `execution-engine`
    /// pra mapear pro envelope `TOOL_ERROR`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Protocol { .. } => "process_protocol_error",
            Self::Transport { .. } => "process_transport_error",
            Self::Timeout { .. } => "process_timeout",
            Self::Cancelled { .. } => "process_cancelled",
            Self::Unhealthy { .. } => "process_unhealthy",
            Self::Platform { .. } => "process_platform_error",
        }
    }
}
