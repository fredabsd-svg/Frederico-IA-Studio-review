//! `frederico-process-architecture` — base do manager de workers
//! sidecar (Fase 5, Etapa 2A: fundação do protocol e do fake).
//!
//! Esta entrega cobre o que está **verde** end-to-end e não depende
//! do `WorkerManager` (que está redesenhado — ver ADR-0015):
//!
//! 1. O envelope IPC [`protocol::IpcMessage`] — line-delimited JSON
//!    com `protocol_version`, `request_id`, `op`, `payload`, e
//!    `auth` opcional. Lista **fechada** de opcodes
//!    ([`protocol::IpcOp`]: `hello`, `ack`, `ping`, `pong`,
//!    `shutdown`, `error`, `tool.invoke`, `tool.result`).
//! 2. O [`protocol::WorkerManifest`] — o que o worker anuncia no
//!    `worker.hello`: ID, versão, capabilities, dependências,
//!    saúde, compatibilidade.
//! 3. O [`protocol::WorkerAuth`] — token de curta duração carregado
//!    no `app.ack` e em toda `tool.invoke` subsequente
//!    ([`process-architecture.md`](../../docs/architecture/process-architecture.md)
//!    §Invariantes).
//! 4. A trait [`pipes::Pipe`] — abstração de transporte
//!    (named pipes no Windows, in-process channel pra testes).
//!    A Etapa 2B adiciona o `WindowsPipeClient`/`Server` via
//!    `windows` crate, e refatora pra `PipeReader`/`PipeWriter`
//!    (sem `Arc<Mutex<>>` — ver ADR-0015).
//! 5. O [`env_allowlist::build_worker_env`] — constrói o env do
//!    worker a partir de uma allowlist explícita. O env do pai
//!    **não** é lido (regra do `process-architecture.md`
//!    §Invariantes).
//! 6. O [`fake::spawn_fake_worker`] — worker simulado in-process
//!    que entende os opcodes essenciais. Coberto por testes
//!    unitários do protocolo + o `fake_worker_handle_spawn_helper`
//!    integration test.
//!
//! **Fora desta entrega** (próxima sessão, com `WorkerManager`
//! redesenhado como ator — sem `Arc<Mutex<>>`):
//!
//! - `WorkerManager::invoke` / `ping` / `shutdown` (manager de
//!   workers). O design atual usou `Arc<Mutex<Box<dyn Pipe>>>`
//!   partilhado entre o `invoke` e a task de leitura, o que
//!   deadlocka em alguns cenários (testes de integração
//!   `invoke_roundtrip`, `ping_updates_health`, `shutdown_*`,
//!   `worker_*` travam > 60s). A correção é trocar mutex por
//!   modelo de ator: uma task dona do pipe; `invoke` manda a
//!   request por `mpsc` com `oneshot` correlacionado por
//!   `request_id`. A trait `Pipe` se divide em `PipeReader`/
//!   `PipeWriter` no construtor — ver ADR-0015.
//!
//! ## Mapa de Etapas (mesmo crate, sem novo `Cargo.toml`)
//!
//! - **Etapa 2B** — `WindowsPipeClient`/`Server` via `windows`
//!   crate (gateado); `spawn_external(command, args, pipe_name)`
//!   que spawna `document-worker.exe` via
//!   `tokio::process::Command` e abre o pipe real; o
//!   `bootstrap.ps1` em `workers/document-worker/` baixa Python
//!   embeddable + libs + Tesseract + fontes "Tinta & Latão".
//! - **Etapa 3** — `docs.generate` no `ToolRegistry` (consome o
//!   `WorkerManager::invoke` redesenhado).
//! - **Etapa 4** — ExcelPro + `docs.inspect` + cache de extração.
//! - **Etapa 5** — PDFPro + auditoria bloqueante.
//! - **Etapa 6** — UI + gate de CI.

#![deny(missing_docs)]

pub mod env_allowlist;
pub mod error;
pub mod fake;
pub mod pipes;
pub mod protocol;

pub use env_allowlist::{build_worker_env, build_worker_env_with_defaults, EnvEntry};
pub use error::{ProcessError, ProcessErrorKind};
pub use fake::{spawn_fake_worker, FakePipeClient, FakeWorkerConfig, FakeWorkerHandle};
pub use pipes::{Pipe, PipeName};
pub use protocol::{
    CompatibilityInfo, Dependency, IpcMessage, IpcOp, RequestId, WorkerAuth, WorkerHealth,
    WorkerHealthSnapshot, WorkerId, WorkerManifest,
};
