//! `frederico-document-kits` — Kits WordPro / ExcelPro /
//! PDFPro + `DocsGenerateTool` (Fase 5, Etapa 3).
//!
//! ## O que está aqui (Etapa 3 da Fase 5 — WordPro mínimo)
//!
//! - **`DocumentFormat`** — enum dos formatos disponíveis.
//!   Na Etapa 3 contém apenas `Docx`. Adicionar `Xlsx`/`Pdf`
//!   é bump atômico do enum **junto** com a
//!   implementação do kit (REGRAS §1.9 — gerado vence
//!   manual; inventário não mente).
//! - **`Kit`** trait — o contrato dos kits. Todo kit tem
//!   `id`, `target_format`, `is_implemented`, `manifest`,
//!   `render`. Skeletons (Etapa 4/5) implementam o trait
//!   com `is_implemented() = false` e `render` retornando
//!   `KitError::NotImplemented` — provam a forma do
//!   trait sem aparecer no schema do modelo.
//! - **`KitRegistry`** — registro. `implemented_formats()`
//!   é o que gera o enum `format` do schema do
//!   `docs.generate`. **Inventário nunca mente.**
//! - **`WordProKit`** — implementação real. Traduz
//!   `DocumentSpec.blocks` → payload do `docx.write`.
//!   Cobertura: Cover, Heading, Paragraph, List, Table
//!   (texto), Kpis, Callout, Quote, Steps, Chart
//!   (placeholder), Image (caption), Code, Divider,
//!   Spacer, PageBreak, Signatures, BackCover, Footer
//!   (placeholder), Toc (placeholder), KeyValue.
//! - **`ExcelProKit` / `PdfProKit`** — skeletons
//!   (`is_implemented = false`). Existem pra provar a
//!   forma do trait. **Não** registrados no schema do
//!   `docs.generate`. Etapa 4 (ExcelPro) e Etapa 5
//!   (PDFPro, com auditoria bloqueante do §19.6)
//!   substituem o `is_implemented` por `true` e o
//!   `render` por uma implementação real.
//! - **`DocsGenerateTool`** — o **único** tool que o
//!   modelo vê. Roteador: valida `output_path` contra a
//!   allowlist, re-valida o `DocumentSpec`, roteia pro
//!   kit certo, devolve o `ToolResult`.
//!
//! ## O que **não** está aqui (próximas etapas)
//!
//! - **Identidade visual "Tinta & Latão"** (Etapa 6 — UI
//!   e estilo). A v0.1 do WordPro é deliberadamente
//!   "feia" em tipografia — o handler `docx.write` é
//!   primitivo, e o kit só traduz; cores, fontes,
//!   header/footer bonitos são trabalho do Etapa 6.
//! - **Round-trip de `DocumentSpec` a partir do `.docx`**
//!   (Etapa 4 — `docs.inspect`). O `docs.generate` é
//!   one-way (spec → arquivo); ler o arquivo de volta é
//!   outro tool, que re-constrói um spec parcial.
//! - **Auditoria bloqueante do PDFPro** (§19.6) — só
//!   entra na Etapa 5, junto com o PdfPro. **Sem**
//!   interruptor. A Etapa 3 evita explicitamente
//!   entregar um `pdf.write` sem a auditoria.

#![deny(missing_docs)]

pub mod excelpro;
pub mod format;
pub mod generate;
pub mod kit;
pub mod pdfpro;
pub mod registry;
pub mod wordpro;

pub use excelpro::ExcelProKit;
pub use format::DocumentFormat;
pub use generate::DocsGenerateTool;
pub use kit::{Kit, KitError, KitOutput};
pub use pdfpro::PdfProKit;
pub use registry::KitRegistry;
pub use wordpro::WordProKit;
