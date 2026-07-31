//! `ExcelProKit` — skeleton da Etapa 4.
//!
//! Existe pra provar a forma do trait `Kit` e ter um
//! `ToolManifest` consistente. **NÃO** está implementado
//! (`is_implemented() == false`) — o `KitRegistry::implemented()`
//! filtra ele fora, então o `DocumentFormat::Xlsx` não
//! aparece no schema do `docs.generate` e o modelo não pode
//! pedir `.xlsx`.
//!
//! Quando a Etapa 4 entrar:
//! 1. Adicionar `DocumentFormat::Xlsx` ao `format.rs`.
//! 2. Trocar `is_implemented` para `true`.
//! 3. Implementar `render` com a tradução
//!    `DocumentSpec` (Spreadsheet) → `xlsx.write` payload.
//! 4. O schema do `docs.generate` cresce sozinho com
//!    `"xlsx"` no enum.

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

/// Skeleton do ExcelPro. **Não** implementado na Etapa 3.
pub struct ExcelProKit {
    #[allow(dead_code)]
    handle: Arc<WorkerHandle>,
    manifest: ToolManifest,
}

impl ExcelProKit {
    /// Cria o skeleton. `handle` é o `WorkerHandle` do
    /// `document-worker` (não usado enquanto
    /// `is_implemented() == false`; mantido pra simetria
    /// com `WordProKit::new` e pra evitar mudança de
    /// assinatura quando Etapa 4 chegar).
    #[must_use]
    pub fn new(handle: Arc<WorkerHandle>) -> Self {
        Self {
            handle,
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        // Manifesto **interno** — usado só em testes e
        // inspeção. O schema do `docs.generate` é gerado
        // pelo `DocsGenerateTool` a partir de
        // `KitRegistry::implemented_formats()`.
        ToolManifestBuilder::new("docs.excelpro.skeleton", "docs")
            .version("0.0.0")
            .display_name("ExcelPro (skeleton)")
            .description(
                "Skeleton do kit ExcelPro. Será implementado na Etapa 4 da Fase 5. \
                 Até lá, não aparece no schema do `docs.generate` — o modelo não pode \
                 pedir .xlsx.",
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
impl Kit for ExcelProKit {
    fn id(&self) -> &str {
        "excelpro"
    }

    fn target_format(&self) -> DocumentFormat {
        // Etapa 4: trocar para `DocumentFormat::Xlsx`
        // (adição atômica junto com a variante no enum).
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
            etapa: "4",
        })
    }
}
