//! `frederico-document-engine` — Document Engine (Fase 5, Etapa 1).
//!
//! A Etapa 1 entrega a **fundação do Document Engine**:
//!
//! 1. O `DocumentSpec` — o contrato declarativo que o modelo emite
//!    para os kits Word/Excel/PDF (`PROMPT MESTRE` §16.4). Vinte
//!    blocos na v0.1, derivados do stub em
//!    `docs/architecture/document-engine-architecture.md`.
//! 2. Validação via JSON Schema — gerado em `build.rs` a partir dos
//!    tipos Rust via `schemars`, validado em runtime pelo crate
//!    `jsonschema` (mesma versão que o `tool-registry` usa). Regras
//!    semânticas que o JSON Schema não expressa (ex: `Kpis` aceita
//!    2 a 4 cartões) ficam em [`validate::validate_semantic`].
//! 3. O `PromptTemplate` — a função pura que gera o prompt do modo
//!    documental a partir do **catálogo de blocos** (REGRAS §1.9 —
//!    "Gerado vence manual": o prompt é derivado do schema, não
//!    mantido à mão).
//!
//! O crate é **puro**: `unsafe_code = "forbid"`, sem `tauri` /
//! `windows` / `winapi`, sem dependência de plataforma (verificado
//! por `scripts/check-core-purity.ps1` — ADR-0003). Etapas seguintes
//! (Etapa 2: `process-architecture` + `document-worker`;
//! Etapa 3: `docs.generate` no `ToolRegistry`) adicionam I/O e
//! IPC — mas o núcleo do spec fica puro e testável em isolamento.
//!
//! ## Mapa de Etapas (mesmo crate, sem novo `Cargo.toml`)
//!
//! - **Etapa 2** — `process-architecture` (manager de workers,
//!   named pipes, handshake) + `document-worker` (Python embeddable
//!   com `python-docx`/`openpyxl`/`reportlab`/`PyMuPDF` + Tesseract
//!   + fontes "Tinta & Latão" — `ADR-0004`). O motor desta Etapa 1
//!     envia `DocumentSpec` JSON pelo pipe; o worker renderiza.
//! - **Etapa 3** — `docs.generate` no `ToolRegistry` + WordPro
//!   mínimo (fluxo vertical mínimo: spec → `.docx` no disco +
//!   round-trip com `python-docx`).
//! - **Etapa 4** — ExcelPro + `docs.inspect` + cache de extração
//!   (Fluxo vertical 2 do `PROMPT MESTRE` §33: planilha → revisão
//!   multimodelo).
//! - **Etapa 5** — PDFPro + auditoria bloqueante (`PROMPT MESTRE`
//!   §19.6, sem interruptor) + modo Sóbrio. ADR novo: `reportlab`
//!   vs `PyMuPDF` (decisão adiada da Fase 0).
//! - **Etapa 6** — UI do modo documental + identidade visual nos
//!   3 kits + gate de CI da Fase 5.

#![deny(missing_docs)]

pub mod blocks;
pub mod error;
pub mod prompt;
pub mod spec;
pub mod validate;

pub use blocks::{
    CalloutKind, ChartKind, ChartSeries, CodeBlock, ConfidentialityLevel, ConfidentialityMark,
    ContactInfo, Cover, DocumentBlock, ImageBlock, KpiCard, ListItem, Quote, SignaturePair, Step,
    TotalSpec,
};
pub use error::DocumentError;
pub use spec::{
    DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion, WatermarkPosition,
    WatermarkSpec,
};
pub use validate::{validate_against_schema, validate_semantic, SchemaError};

/// Re-exporta `serde_json::Value` por conveniência — `docs.generate`
/// recebe `Value` no payload do IPC e devolve `Value` no resultado.
pub use serde_json::Value as JsonValue;
