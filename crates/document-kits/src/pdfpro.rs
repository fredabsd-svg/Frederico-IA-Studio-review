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
            .version("0.1.0-skeleton")
            .display_name("PDFPro (skeleton v0.1)")
            .description(
                "PR 1 (Etapa 5, ADR-0021): bump atômico do enum `DocumentFormat::Pdf` \
                 + `is_implemented = true` no `KitRegistry`. A v0.1 do `render` \
                 (fontes Tinta & Latão embutidas, identidade visual, modo Sóbrio, \
                 20 blocos) entra no PR 2. A auditoria bloqueante do §19.6 (visual \
                 + estrutural) entra nos PRs 3 e 4. Até lá, o `render` retorna \
                 `KitError::NotImplemented { etapa: \"5.v0.1\" }` — o enum do schema \
                 do `docs.generate` já mostra `pdf` como opção, mas chamar retorna \
                 erro honesto. Sem \"plano B\" silencioso.",
            )
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Moderate)
            .disabled()
            .input_schema(JsonSchema(serde_json::json!({
                "type": "object",
                "description": "PR 1 (Etapa 5, ADR-0021): schema detalhado entra no PR 2 (render real)."
            })))
            .output_schema(JsonSchema(serde_json::json!({
                "type": "object",
                "description": "PR 1 (Etapa 5, ADR-0021): schema detalhado entra no PR 2."
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
        // PR 1 (Etapa 5, ADR-0021): bump atômico do enum
        // `DocumentFormat::Pdf`. O `KitRegistry::implemented_formats()`
        // volta de `["docx", "xlsx"]` para `["docx", "xlsx", "pdf"]`
        // no mesmo commit (REGRAS §1.9 — inventário não mente).
        // O `is_implemented() == true` reflete que o **kit existe
        // e está registrado**; a v0.1 do `render` entra no PR 2.
        // Até lá, chamar `render` retorna `NotImplemented` com
        // `etapa: "5.v0.1"` — honesto, sem mentir.
        DocumentFormat::Pdf
    }

    fn is_implemented(&self) -> bool {
        // PR 1 (Etapa 5, ADR-0021): `true`. O `PdfProKit` está
        // implementado e registrado no `KitRegistry`; o que falta é
        // a v0.1 do `render` em si, que retorna `NotImplemented`
        // com `etapa: "5.v0.1"`. O `KitRegistry::implemented()`
        // filtra por este flag antes de gerar o enum do schema.
        // Se ficasse `false`, o modelo não veria `pdf` como
        // opção de `format` no `docs.generate` — e isso seria
        // o "inventário que mente" que REGRAS §1.9 proíbe.
        true
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn render(
        &self,
        _spec: &DocumentSpec,
        _output_path: &Path,
    ) -> Result<KitOutput, KitError> {
        // PR 1 (Etapa 5, ADR-0021): `render` ainda não implementado
        // — entra no PR 2 (fontes Tinta & Latão embutidas,
        // identidade visual "Tinta & Latão" + modo Sóbrio, 20 blocos
        // cobertos, glifo-check via `fontTools`).
        //
        // A auditoria bloqueante do §19.6 (visual §19.3 + estrutural
        // §19.4) entra nos PRs 3 e 4. A `etapa: "5.v0.1"` sinaliza
        // ao caller que a v0.1 do PDFPro está em construção e que
        // ele deve tentar de novo na próxima release. O caller
        // (modelo via `docs.generate`) traduz em mensagem amigável
        // pro usuário final.
        Err(KitError::NotImplemented {
            id: self.id().to_string(),
            format: self.target_format(),
            etapa: "5.v0.1",
        })
    }
}
