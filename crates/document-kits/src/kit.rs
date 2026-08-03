//! Trait `Kit` + `KitError` + `KitOutput` — o contrato entre
//! o `DocsGenerateTool` e as implementações de cada kit
//! (WordPro, ExcelPro, PdfPro).
//!
//! ## Skeleton vs implemented
//!
//! Todo `Kit` carrega um `ToolManifest` (que o `ToolRegistry`
//! usa) e tem `target_format()`. Mas nem todo `Kit` está
//! **implementado** — `is_implemented()` distingue. O
//! `ExcelProKit` e o `PdfProKit` da Etapa 3 da Fase 5 são
//! skeletons (existem pra provar a forma do trait, mas o
//! `render()` retorna `KitError::NotImplemented`). O
//! `KitRegistry::implemented()` filtra eles fora do schema
//! do `docs.generate` — REGRAS §1.9.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use frederico_core::InvokeError;
use frederico_document_engine::{DocumentError, DocumentSpec};
use frederico_tool_registry::ToolManifest;
use serde_json::Value;
use thiserror::Error;

use crate::format::DocumentFormat;

/// Erro de um `Kit::render`. `NotImplemented` é o erro
/// esperado dos **skeletons** (ExcelPro/PdfPro antes da
/// Etapa 4/5). `Worker` é o erro do `WorkerHandle::invoke`
/// quando o `document-worker` retorna `tool.result {ok:
/// false}` ou falha de transporte.
#[derive(Debug, Error)]
pub enum KitError {
    /// O kit existe mas ainda não foi implementado (skeleton).
    /// O `target_format` e a `etapa` (ex: "4", "5") vêm do
    /// caller; o caller pode traduzir em uma mensagem
    /// amigável pro modelo ("formato .xlsx estará disponível
    /// em versão futura").
    #[error("kit '{id}' (formato {format}) ainda não foi implementado (Etapa {etapa})")]
    NotImplemented {
        /// ID do kit (ex: `"excelpro"`).
        id: String,
        /// Formato alvo do kit (eco do `Kit::target_format`).
        format: DocumentFormat,
        /// Etapa do roadmap em que o kit será implementado
        /// (ex: `"4"`, `"5"`).
        etapa: &'static str,
    },

    /// O `DocumentSpec` é inválido (schema ou regras
    /// semânticas). Vem direto do `document-engine`.
    #[error(transparent)]
    InvalidSpec(#[from] DocumentError),

    /// O `WorkerHandle::invoke` falhou (transporte, timeout,
    /// protocolo) ou o worker devolveu `tool.result {ok:
    /// false}`.
    #[error("worker falhou: {0}")]
    Worker(String),

    /// Erro bruto do `ProcessError` (re-exportado para que o
    /// caller do kit não precise importar `process-architecture`
    /// direto).
    #[error(transparent)]
    Process(#[from] InvokeError),

    /// O `output_path` viola a allowlist do `ToolManifest`
    /// (a mesma checagem que o `WorkerToolDispatcher` faz —
    /// defesa em profundidade).
    #[error("output_path fora da allowlist: {0}")]
    PathNotAllowed(String),

    /// A auditoria bloqueante do §19.6 (PROMPT MESTRE) falhou.
    /// Etapa 5 PR 3 (D-PDF5 do ADR-0021) introduz o
    /// `pdf.audit` no document-worker e mapeia qualquer falha
    /// da auditoria estrutural pra este erro. **§19.6 nao tem
    /// interruptor**: o kit NAO entrega o artefato quando a
    /// auditoria falha. O `code` (ex: `"pdf_audit_structural_failed"`)
    /// e a `message` vem do handler; `failed` e a lista de
    /// checks que nao passaram.
    #[error("auditoria bloqueante falhou: {code} - {message}")]
    AuditFailed {
        /// Codigo estruturado do handler (ex: `"pdf_audit_structural_failed"`).
        code: String,
        /// Mensagem legivel com o motivo da falha.
        message: String,
        /// Checks que falharam (lista de `{check, expected, got}`).
        failed: serde_json::Value,
    },
}

/// Mapeamento de um bloco do `DocumentSpec` pra uma sheet
/// do workbook gerado (Etapa 4 da Fase 5, ExcelPro).
///
/// Devolvido no `KitOutput::sheets` pra que o `docs.inspect`
/// (e o modelo, via `output` do `docs.generate`) saiba em
/// que sheet cada bloco caiu — sem isso, o round-trip
/// precisa adivinhar em que aba caiu cada Table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetMapping {
    /// Índice do bloco no `DocumentSpec.blocks` (0-based).
    pub block_index: usize,
    /// Nome da sheet criada (sanitizado — sem caracteres
    /// proibidos pelo Excel, max 31 chars, único).
    pub sheet_name: String,
}

/// Saída de um `Kit::render`. Traduzida em `ToolResult.output`
/// pelo `DocsGenerateTool::execute`.
#[derive(Debug, Clone)]
pub struct KitOutput {
    /// Path final do arquivo gerado (canonicalizado pelo
    /// worker).
    pub path: PathBuf,
    /// Tamanho em bytes.
    pub size_bytes: u64,
    /// Formato (eco do `target_format` do kit — o caller
    /// usa pra montar o `output_schema`).
    pub format: DocumentFormat,
    /// Metadados extras do worker (ex: `sections_written` do
    /// `docx.write`). Merged no `ToolResult.output`.
    pub extra: Value,
    /// Mapeamento `bloco → sheet` (Etapa 4, ExcelPro). Vazio
    /// pra WordPro/PdfPro (que não produzem sheets no
    /// sentido de workbook).
    pub sheets: Vec<SheetMapping>,
    /// Avisos do kit (ex: chart reduzido a dados tabulares
    /// porque o `xlsx.write` v0.3.0 não renderiza chart real).
    /// **Sempre declaram degradações** — o modelo precisa
    /// poder dizer a verdade ao usuário (Etapa 4 deltas).
    /// Default: vazio. O `DocsGenerateTool` propaga no
    /// `output.warnings` do ToolResult.
    pub warnings: Vec<String>,
}

impl KitOutput {
    /// Helper de conveniência: cria um `KitOutput` com os
    /// campos minimos (sem sheets, sem warnings) — usado
    /// por kits que não produzem sheets (WordPro v0.1) ou
    /// em testes.
    #[must_use]
    pub fn simple(path: PathBuf, size_bytes: u64, format: DocumentFormat, extra: Value) -> Self {
        Self {
            path,
            size_bytes,
            format,
            extra,
            sheets: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Trait de um kit.
///
/// `async_trait` para consistência com `Tool::execute` (também
/// async desde a Etapa 3 da Fase 5). O `render` é o ponto
/// onde o kit faz a tradução `DocumentSpec` → payload do
/// handler e chama o `WorkerHandle` (injetado pelo
/// `DocsGenerateTool` ou passado via context).
///
/// `Send + Sync` para que o `KitRegistry` possa ser envolto
/// em `Arc` e compartilhado entre threads (Tauri roda em
/// multi-thread).
#[async_trait]
pub trait Kit: Send + Sync {
    /// ID do kit (ex: `"wordpro"`, `"excelpro"`, `"pdfpro"`).
    /// **Diferente** do `ToolManifest::id` (que é o id do
    /// tool exposto ao modelo, ex: `"docs.generate"`).
    #[must_use]
    fn id(&self) -> &str;

    /// Formato que este kit produz. Usado pelo `KitRegistry`
    /// pra rotear o `docs.generate` pro kit certo quando o
    /// chamador pede `format: "docx"`.
    #[must_use]
    fn target_format(&self) -> DocumentFormat;

    /// `true` se o `render` está implementado de verdade;
    /// `false` se é skeleton (Etapa 4/5). O `KitRegistry`
    /// filtra por `implemented()` antes de gerar o schema do
    /// `docs.generate`.
    #[must_use]
    fn is_implemented(&self) -> bool;

    /// `ToolManifest` do kit — usado pelo `DocsGenerateTool`
    /// pra construir o schema (a parte do schema que **não**
    /// é gerada automaticamente: `display_name`, `description`,
    /// `risk_level`).
    #[must_use]
    fn manifest(&self) -> &ToolManifest;

    /// Renderiza o `spec` no `output_path`. A allowlist de
    /// paths é responsabilidade do `DocsGenerateTool` (que
    /// passa o `WorkerHandle` validado); o `render` confia
    /// que `output_path` já foi validado.
    async fn render(&self, spec: &DocumentSpec, output_path: &Path) -> Result<KitOutput, KitError>;
}
