//! `PdfProKit` — skeleton da Etapa 5.
//!
//! Existe pra provar a forma do trait `Kit` e ter um
//! `ToolManifest` consistente. **NÃO** está implementado
//! (`is_implemented() == false`) — o `KitRegistry::implemented()`
//! filtra ele fora.
//!
//! Quando a Etapa 5 entrar:
//! 1. Adicionar `DocumentFormat::Pdf` ao `format.rs`.
//! 2. Trocar `is_implemented` para `true`.
//! 3. Implementar `render` com a tradução
//!    `DocumentSpec` → `pdf.write` payload.
//! 4. **Auditoria bloqueante do §19.6** é parte do
//!    `render` — sem ela, não é PDFPro. Sem a auditoria,
//!    é "um `pdf.write` sem conferências", que é o
//!    precedente ruim que a Etapa 3 evita explicitamente.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use frederico_document_engine::DocumentSpec;
use frederico_process_architecture::WorkerHandle;
use frederico_tool_registry::{
    JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder,
};

use crate::format::DocumentFormat;
use crate::kit::{Kit, KitError, KitOutput};

/// Skeleton do PDFPro. **Não** implementado na Etapa 3.
pub struct PdfProKit {
    #[allow(dead_code)]
    handle: Arc<WorkerHandle>,
    manifest: ToolManifest,
}

impl PdfProKit {
    /// Cria o skeleton. `handle` é o `WorkerHandle` do
    /// `document-worker` (não usado enquanto
    /// `is_implemented() == false`; mantido pra simetria
    /// com `WordProKit::new` e pra evitar mudança de
    /// assinatura quando Etapa 5 chegar).
    #[must_use]
    pub fn new(handle: Arc<WorkerHandle>) -> Self {
        Self {
            handle,
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new("docs.pdfpro.skeleton", "docs")
            .version("0.0.0")
            .display_name("PDFPro (skeleton)")
            .description(
                "Skeleton do kit PDFPro. Será implementado na Etapa 5 da Fase 5, COM \
                 auditoria bloqueante do PROMPT MESTRE §19.6 (sem interruptor). \
                 Até lá, não aparece no schema do `docs.generate`.",
            )
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Moderate)
            .disabled()
            .input_schema(JsonSchema(serde_json::json!({
                "type": "object",
                "description": "Skeleton — schema não exposto ao modelo."
            })))
            .output_schema(JsonSchema(serde_json::json!({
                "type": "object",
                "description": "Skeleton — schema não exposto ao modelo."
            })))
            .build()
            .expect("manifesto skeleton bem-formado")
    }
}

#[async_trait]
impl Kit for PdfProKit {
    fn id(&self) -> &str {
        "pdfpro"
    }

    fn target_format(&self) -> DocumentFormat {
        // Etapa 5: trocar para `DocumentFormat::Pdf` (junto
        // com a variante no enum). Sem a auditoria
        // bloqueante, não é PDFPro — não entregar.
        DocumentFormat::Docx
    }

    fn is_implemented(&self) -> bool {
        false
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn render(
        &self,
        _spec: &DocumentSpec,
        _output_path: &Path,
    ) -> Result<KitOutput, KitError> {
        Err(KitError::NotImplemented {
            id: self.id().to_string(),
            format: self.target_format(),
            etapa: "5",
        })
    }
}
