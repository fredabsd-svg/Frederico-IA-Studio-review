//! `frederico-process-architecture` — manager de workers sidecar
//! + IPC via named pipes + handshake (Fase 5, Etapa 2A).
//!
//! Esta entrega fecha a Etapa 2A com o `WorkerManager` redesenhado
//! como **ator** (sem `Arc<Mutex<Box<dyn Pipe>>>`) — ver ADR-0015.
//! O design suporta invocações concorrentes (cada `invoke` carrega
//! seu próprio `oneshot`, correlacionado por `request_id`).
//!
//! ## O que está **no repositório** e verde
//!
//! 1. **Envelope IPC** [`protocol::IpcMessage`] — line-delimited
//!    JSON com `protocol_version`, `request_id`, `op`, `payload`, e
//!    `auth` opcional. 8 `IpcOp` estáveis em snake_case
//!    (`worker.hello`, `app.ack`, `app.ping`, `worker.pong`,
//!    `app.shutdown`, `worker.error`, `tool.invoke`, `tool.result`).
//! 2. **Manifesto** [`protocol::WorkerManifest`] — ID, versão,
//!    capabilities, dependências, saúde, compatibilidade. O
//!    `worker.hello` de boot carrega isto.
//! 3. **Auth** [`protocol::WorkerAuth`] — token de curta duração
//!    carregado no `app.ack` e em toda `tool.invoke` subsequente.
//! 4. **Transporte** [`pipes::PipeReader`] + [`pipes::PipeWriter`]
//!    — duas metades separadas (sem trait única). É o que destrava
//!    o modelo de ator (o `Box<dyn PipeReader>` é movido pra task
//!    do ator; o `Box<dyn PipeWriter>` é `Clone` e fica no
//!    `WorkerHandle`).
//! 5. **Env allowlist** [`env_allowlist::build_worker_env`] —
//!    constrói o env do worker a partir de allowlist explícita. O
//!    env do pai **não** é lido (regra do `process-architecture.md`
//!    §Invariantes).
//! 6. **Fake worker** [`fake::spawn_fake_worker`] — worker
//!    simulado in-process que implementa `PipeReader + PipeWriter`
//!    sobre `mpsc::channel` e entende os opcodes essenciais.
//!    Envia `worker.hello` no **boot** (antes do loop), modelando
//!    o que o worker real faz ao subir.
//! 7. **Manager** [`manager::WorkerManager`] + [`manager::WorkerHandle`]
//!    — modelo de ator (ADR-0015). Suporta invocações
//!    concorrentes sem lock no caminho do `invoke`.
//!
//! ## Handshake
//!
//! O `WorkerManager::spawn_in_process` faz handshake **síncrono**
//! antes de spawnar o ator:
//!
//! 1. O fake server envia `worker.hello` com o manifesto no boot.
//! 2. O manager lê o `hello`, gera um `WorkerAuth` (UUID v4), e
//!    responde com `app.ack` carregando o token.
//! 3. Daí em diante, toda `tool.invoke` carrega o token; o fake
//!    valida contra o auth que recebeu.
//!
//! ## Mapa de Etapas
//!
//! - **Etapa 2A (esta entrega)** — protocolo + manager com
//!   `FakeWorker` in-process, modelo de ator.
//! - **Etapa 2B** — `WindowsPipeReader`/`Writer` via `windows`
//!   crate (gateado em `#[cfg(windows)]`); `spawn_external` via
//!   `tokio::process::Command`; bootstrap do `document-worker.exe`
//!   Python (Tesseract, fontes "Tinta & Latão").
//! - **Etapa 3** — `docs.generate` no `ToolRegistry` (consome o
//!   `WorkerHandle::invoke`).
//! - **Etapa 4** — ExcelPro + `docs.inspect` + cache de extração.
//! - **Etapa 5** — PDFPro + auditoria bloqueante.
//! - **Etapa 6** — UI + gate de CI.

#![deny(missing_docs)]

pub mod env_allowlist;
pub mod error;
#[cfg(windows)]
pub mod external;
pub mod fake;
pub mod manager;
pub mod pipes;
pub mod protocol;
pub mod worker_invoker_impl;

#[cfg(windows)]
pub mod windows_pipes;

pub use env_allowlist::{build_worker_env, build_worker_env_with_defaults, EnvEntry};
pub use error::{ProcessError, ProcessErrorKind};
#[cfg(windows)]
pub use external::ExternalSpawnConfig;
pub use fake::{
    spawn_fake_worker, unique_pipe_name, FakePipeReader, FakePipeWriter, FakeWorkerConfig,
    FakeWorkerHandle,
};
pub use manager::{WorkerHandle, WorkerManager, WorkerSpawnConfig};
pub use pipes::{PipeName, PipeReader, PipeWriter};
pub use protocol::{
    CompatibilityInfo, Dependency, IpcMessage, IpcOp, RequestId, WorkerAuth, WorkerHealth,
    WorkerHealthSnapshot, WorkerId, WorkerManifest,
};
