//! `KitRegistry` — o registro dos kits disponíveis.
//!
//! Fonte da verdade do inventário documental (REGRAS §1.9).
//! O `DocsGenerateTool::build_manifest` consulta
//! `implemented_formats()` para gerar o enum `format` do
//! schema — adicionar `DocumentFormat::Xlsx` exige um
//! `ExcelProKit` com `is_implemented() == true` registrado.
//!
//! **Skeletons ficam na registry** (provam a forma do trait,
//! geram manifests internos, podem ser inspecionados em
//! testes), mas **não aparecem no schema do modelo**.
//! Separação: o `register` aceita qualquer `Kit`; a leitura
//! filtra por `is_implemented`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::format::DocumentFormat;
use crate::kit::Kit;

/// Registro de kits. Thread-safe via interior mutability
/// (`Arc<RwLock<>>` no caller — o `KitRegistry` em si é
/// `Send + Sync` por construção: o `HashMap` é interno mas
/// só é mutado em construção; depois de "fechado" o caller
/// congela em `Arc` e compartilha).
///
/// `Clone` é derivado: clone barato do `HashMap<Arc<dyn Kit>>`
/// (cada kit é `Arc`-ed).
///
/// `Debug` **não** é derivado: `dyn Kit` não implementa
/// `Debug` (mesma situação do `WorkerToolDispatcher`). Pra
/// inspecionar a registry em testes, use `len()` / `all()`.
#[derive(Default, Clone)]
pub struct KitRegistry {
    kits: HashMap<String, Arc<dyn Kit>>,
}

impl KitRegistry {
    /// Cria registry vazio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adiciona um kit. Substitui se já existir (mesmo `id`).
    /// Aceita tanto `is_implemented() == true` quanto
    /// `false` (skeleton) — o filtro é só no schema.
    pub fn register(&mut self, kit: Arc<dyn Kit>) {
        let id = kit.id().to_string();
        self.kits.insert(id, kit);
    }

    /// Busca por id. Devolve `Arc<dyn Kit>` (clone barato)
    /// ou `None` se não existir. Devolve **todos** os
    /// kits (implementados e skeletons).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn Kit>> {
        self.kits.get(id).cloned()
    }

    /// Todos os kits (implementados + skeletons), em ordem
    /// estável (por `id`).
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn Kit>> {
        let mut v: Vec<_> = self.kits.values().cloned().collect();
        v.sort_by(|a, b| a.id().cmp(b.id()));
        v
    }

    /// Apenas os kits **implementados** (Etapa 3 da Fase 5:
    /// só o `WordProKit`). Em ordem estável.
    #[must_use]
    pub fn implemented(&self) -> Vec<Arc<dyn Kit>> {
        let mut v: Vec<_> = self
            .kits
            .values()
            .filter(|k| k.is_implemented())
            .cloned()
            .collect();
        v.sort_by(|a, b| a.id().cmp(b.id()));
        v
    }

    /// Formatos implementados (sem duplicatas). É o que vai
    /// no enum `format` do schema do `docs.generate`.
    #[must_use]
    pub fn implemented_formats(&self) -> Vec<DocumentFormat> {
        let mut v: Vec<_> = self
            .implemented()
            .iter()
            .map(|k| k.target_format())
            .collect();
        v.sort_by_key(|f| f.as_str()); // ordem estável
        v.dedup();
        v
    }

    /// Encontra o kit implementado que produz o formato
    /// pedido. Devolve `None` se o formato não está
    /// implementado (o chamador trata como erro).
    #[must_use]
    pub fn find_for_format(&self, format: DocumentFormat) -> Option<Arc<dyn Kit>> {
        self.implemented()
            .into_iter()
            .find(|k| k.target_format() == format)
    }

    /// Quantos kits no total (implementados + skeletons).
    #[must_use]
    pub fn len(&self) -> usize {
        self.kits.len()
    }

    /// `true` se a registry está vazia (nenhum kit
    /// registrado — nem implementado, nem skeleton).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{KitError, KitOutput};
    use async_trait::async_trait;
    use frederico_document_engine::DocumentSpec;
    use frederico_tool_registry::{RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
    use std::path::{Path, PathBuf};

    /// Kit fake que implementa tudo, pra teste.
    struct FakeKit {
        id: String,
        format: DocumentFormat,
        implemented: bool,
    }

    #[async_trait]
    impl Kit for FakeKit {
        fn id(&self) -> &str {
            &self.id
        }
        fn target_format(&self) -> DocumentFormat {
            self.format
        }
        fn is_implemented(&self) -> bool {
            self.implemented
        }
        fn manifest(&self) -> &ToolManifest {
            // Não usado nos testes.
            static M: std::sync::OnceLock<ToolManifest> = std::sync::OnceLock::new();
            M.get_or_init(|| {
                ToolManifestBuilder::new("docs.generate", "docs")
                    .display_name("Geração de documentos")
                    .description("Stub para testes.")
                    .category(ToolCategory::Docs)
                    .risk_level(RiskLevel::Safe)
                    .build()
                    .unwrap()
            })
        }
        async fn render(
            &self,
            _spec: &DocumentSpec,
            _output_path: &Path,
        ) -> Result<KitOutput, KitError> {
            if !self.implemented {
                return Err(KitError::NotImplemented {
                    id: self.id.clone(),
                    format: self.format,
                    etapa: "test",
                });
            }
            Ok(KitOutput {
                path: PathBuf::from("/tmp/out"),
                size_bytes: 0,
                format: self.format,
                extra: serde_json::json!({}),
                sheets: Vec::new(),
                warnings: Vec::new(),
            })
        }
    }

    #[test]
    fn empty_registry() {
        let r = KitRegistry::new();
        assert!(r.is_empty());
        assert!(r.all().is_empty());
        assert!(r.implemented().is_empty());
        assert!(r.implemented_formats().is_empty());
    }

    #[test]
    fn register_and_get() {
        let mut r = KitRegistry::new();
        let kit = Arc::new(FakeKit {
            id: "wordpro".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        });
        r.register(kit);
        assert_eq!(r.len(), 1);
        assert!(r.get("wordpro").is_some());
        assert!(r.get("excelpro").is_none());
    }

    #[test]
    fn implemented_filters_skeletons() {
        let mut r = KitRegistry::new();
        r.register(Arc::new(FakeKit {
            id: "wordpro".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        }));
        r.register(Arc::new(FakeKit {
            id: "skeleton_x".into(),
            format: DocumentFormat::Docx, // mesmo formato, mas skeleton
            implemented: false,
        }));
        let all = r.all();
        let impls = r.implemented();
        assert_eq!(all.len(), 2);
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].id(), "wordpro");
    }

    #[test]
    fn implemented_formats_no_duplicates() {
        let mut r = KitRegistry::new();
        // 2 kits com mesmo formato (improvável, mas defensivo).
        r.register(Arc::new(FakeKit {
            id: "a".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        }));
        r.register(Arc::new(FakeKit {
            id: "b".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        }));
        let formats = r.implemented_formats();
        assert_eq!(formats, vec![DocumentFormat::Docx]);
    }

    #[test]
    fn find_for_format_returns_none_for_skeleton() {
        let mut r = KitRegistry::new();
        r.register(Arc::new(FakeKit {
            id: "skeleton".into(),
            format: DocumentFormat::Docx,
            implemented: false,
        }));
        assert!(r.find_for_format(DocumentFormat::Docx).is_none());
    }

    #[test]
    fn all_returns_sorted_by_id() {
        let mut r = KitRegistry::new();
        r.register(Arc::new(FakeKit {
            id: "z".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        }));
        r.register(Arc::new(FakeKit {
            id: "a".into(),
            format: DocumentFormat::Docx,
            implemented: true,
        }));
        // `let all = ...` — sem isso, o Vec temporário é
        // dropped no fim do statement e os `&str` retornados
        // por `k.id()` dangling.
        let all = r.all();
        let ids: Vec<&str> = all.iter().map(|k| k.id()).collect();
        assert_eq!(ids, vec!["a", "z"]);
    }
}
