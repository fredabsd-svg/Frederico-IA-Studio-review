//! `frederico-tool-registry` — Tool Registry, manifesto de
//! ferramentas, validação, jail e a única ferramenta in-process
//! (`files.read`) (Fase 3, Etapa 2).
//!
//! Crate do núcleo (sem dependência de plataforma, `unsafe_code =
//! "forbid"`, verificado por `scripts/check-core-purity.ps1`).
//!
//! ## O que está aqui
//!
//! - [`ToolManifest`] e o [`ToolManifestBuilder`] — o contrato do
//!   spec `tool-registry-specification.md` §"Contrato do manifesto".
//! - [`JsonSchema`] — wrapper sobre `serde_json::Value` com
//!   validação via `jsonschema` crate.
//! - [`RiskLevel`], [`ToolCategory`], [`ProviderMode`], [`Platform`],
//!   [`Availability`], [`WorkerHealth`] — enums auxiliares.
//! - [`ToolRegistry`] — registro thread-safe; `register`/`get`/`all`
//!   e a função [`ToolRegistry::effective_tools`] (a interseção do
//!   spec §"Interseção de inventário por execução").
//! - [`Jail`] — ponto único de normalização de caminhos. Rejeita
//!   `..` (path traversal), caminhos absolutos, UNC, letra de
//!   unidade diferente, symlinks apontando pra fora.
//! - [`validate_tool_call`] + [`ValidationContext`] + [`ToolCall`] +
//!   [`ValidationOutcome`] — os 10 passos do spec §"Validação antes
//!   de execução" (Etapa 2 cobre 1, 2, 3, 4, 6, 7, 8, 9; Etapa 3
//!   fecha o 5 e Etapa 5 fecha o 10).
//! - [`ApprovalRequest`], [`ApprovalDecision`], [`ApprovalScope`] —
//!   o modelo de aprovação que a UI da Etapa 6 consome.
//! - [`Tool`] (trait) + [`ToolResult`] — a interface que as
//!   ferramentas concretas implementam.
//! - [`FilesReadTool`] (em `tools/files_read`) — a única
//!   ferramenta do catálogo inicial da Etapa 2. Lê um arquivo do
//!   workspace; jail e paginação via `max_bytes`.
//!
//! ## O que **não** está aqui (próximas etapas)
//!
//! - **`files.write`, `files.list`, `files.edit`** (Etapa 4).
//! - **`PermissionSet` e o invariante "subagente ⊆ pai"** (Etapa 3).
//! - **`ProviderToolAdapter`** (Etapa 4) — converte `ToolManifest`
//!   no formato `tools:` de cada provedor.
//! - **Workers sidecar** (Fase 5+) — `files.read` é in-process na
//!   Etapa 2; o manifesto fica igual quando ganhar o backend
//!   sandbox.
//! - **Healthcheck ativo dos workers** (Etapa 5) — `WorkerHealth`
//!   já tem o tipo, mas a Etapa 2 não consulta nada.
//! - **Integração com a máquina de estados** (Etapa 4) —
//!   `ApprovalRequired` da validação vira `RunState::WaitingUserApproval`.

// `allow(missing_docs)` na Etapa 2 — o crate tem 84+ warnings de
// `missing_docs` em builders/impl helpers. A Etapa 2 priorizou o
// escopo de ferramentas e modelo de validação; o sweep completo de
// docs é trabalho de um hardeings. Pendência registrada no
// `docs/modules/tool-registry.md` §"Decisões não óbvias". A Etapa 3
// e o primeiro hardeings da Fase 3 promovem pra `warn` e depois
// `deny`.
#![allow(missing_docs)]

pub mod approval;
pub mod audit;
pub mod error;
pub mod exec;
pub mod jail_resolver;
pub mod manifest;
pub mod permission;
pub mod permission_loader;
pub mod registry;
pub mod tools;
pub mod validate;
pub mod worker_dispatch;
pub mod workspace;

// Reexports do `jail_resolver` (Etapa 1 da Fase de Ligação,
// ADR-0022 §D2). A trait mora no toolkit para que ferramentas
// como `FilesReadTool` (que vivem no `frederico-tool-registry`)
// possam usar o tipo sem ciclo com `frederico-app`. A impl
// default (`FileSystemJailResolver`) mora no `frederico-app`.
pub use jail_resolver::{
    static_jail_resolver, JailResolver, JailResolverError, JailResolverResult, StaticJailResolver,
};

// Reexport do `ToolContext` (Etapa 1 da Fase de Ligação,
// ADR-0022 §D3). O `RunExecutor` constrói por tool_call;
// tools o recebem por referência no `execute`.
pub use tools::ToolContext;

pub use approval::{ApprovalDecision, ApprovalRequest, ApprovalScope};
pub use audit::{AuditEntry, AuditSink, NoopAuditSink};
pub use error::{ToolError, ToolErrorCode};
pub use manifest::{
    Availability, JsonSchema, Platform, ProviderMode, RiskLevel, ToolCategory, ToolManifest,
    ToolManifestBuilder, WorkerHealth,
};
pub use permission::{
    DocumentPermission, FileReadPermission, GitHubPermission, GitPermission, MemoryPermission,
    PermissionSet, RuntimePermission, TerminalMode,
};
pub use permission_loader::{PermissionLoadError, PermissionLoader};
pub use registry::ToolRegistry;
pub use tools::{FilesReadTool, Tool, ToolResult};
pub use validate::{validate_tool_call, ToolCall, ValidationContext, ValidationOutcome};
pub use worker_dispatch::{validate_against_allowlist, DispatchError, WorkerToolDispatcher};
pub use workspace::{Jail, Workspace};
