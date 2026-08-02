//! Validação de `DocumentSpec` em duas camadas.
//!
//! 1. **Schema** (estrutural) — JSON Schema gerado em runtime via
//!    `schemars` (lazy, cacheado em `OnceLock`) e validado pelo crate
//!    `jsonschema`. Cobre tipo dos campos, campos obrigatórios, e
//!    constraints básicos (`minLength`, `enum`, `type: object`, etc.).
//!
//!    REGRAS §1.9 — "Gerado vence manual": o schema **não é
//!    mantido à mão**. É derivado dos tipos em runtime (na primeira
//!    chamada de `validate_against_schema`), com cache thread-safe.
//!    Um `build.rs` que importasse o próprio crate causaria ciclo de
//!    dependência — daí a geração em runtime.
//!
//! 2. **Semântica** (regras de negócio) — invariantes que o JSON
//!    Schema **não** expressa: cardinalidade (`Kpis` aceita 2 a 4
//!    cartões), combinações proibidas (`Spreadsheet` não pode ter
//!    bloco `Toc`), normalização (BCP-47, lowercase no `language`).
//!
//! As duas camadas retornam o mesmo tipo de erro ([`DocumentError`])
//! com `code` diferente (`document_schema_invalid` vs
//! `document_semantic_invalid`) — o `execution-engine` da Etapa 3
//! mapeia os dois para `TOOL_ERROR` com a mensagem preservada.

use std::sync::OnceLock;

use jsonschema::JSONSchema;
use schemars::schema_for;
use serde_json::Value;

use crate::blocks::DocumentBlock;
use crate::error::DocumentError;
use crate::spec::DocumentSpec;

/// Erro de validação contra o JSON Schema, com path JSON pointer.
///
/// Struct separada do `DocumentError::Schema` para devolver **todos**
/// os erros de validação de uma vez, em vez do primeiro. Usada por
/// `validate_against_schema` quando o caller pede lista completa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    /// Path JSON pointer do ponto de falha (ex: `/blocks/3/text`).
    pub path: String,
    /// Mensagem do validador.
    pub message: String,
}

/// Cache thread-safe do JSON Schema compilado.
///
/// `schemars::schema_for!` é não-trivial (percorre todos os tipos,
/// constrói `$ref`s, etc.) — gerá-lo em cada chamada de
/// `validate_against_schema` seria desperdiçar CPU. O `OnceLock`
/// garante que o schema é gerado **uma vez por processo** e
/// reusado.
static COMPILED_SCHEMA: OnceLock<Result<JSONSchema, String>> = OnceLock::new();

/// Compila (ou devolve do cache) o JSON Schema do `DocumentSpec`.
fn compiled_schema() -> Result<&'static JSONSchema, DocumentError> {
    COMPILED_SCHEMA
        .get_or_init(|| {
            let schema_value = schema_for!(DocumentSpec);
            let schema_json = serde_json::to_value(&schema_value)
                .map_err(|e| format!("schema_for!(DocumentSpec) não serializa: {e}"))?;
            // `schemars` 0.8 gera JSON Schema 2020-12 (com `$schema` no
            // topo). O `jsonschema` 0.18 detecta o draft pelo campo
            // `$schema` quando `with_draft` não é chamado — desde que
            // a feature `draft202012` esteja habilitada (workspace
            // `Cargo.toml`). Mantemos o `with_draft` explícito como
            // documentação da intenção.
            JSONSchema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .compile(&schema_json)
                .map_err(|e| format!("schema gerado em runtime não compila: {e}"))
        })
        .as_ref()
        .map_err(|e| DocumentError::Schema {
            path: "/".to_string(),
            message: e.clone(),
        })
}

/// Valida um `serde_json::Value` contra o JSON Schema do `DocumentSpec`.
///
/// Devolve o **primeiro** erro de validação encontrado, com path JSON
/// pointer. (O `jsonschema` reporta todos os erros; pegamos o primeiro
/// pra manter a mensagem curta — o `execution-engine` da Etapa 3
/// consome a string e o modelo recebe a primeira pista.)
pub fn validate_against_schema(value: &Value) -> Result<(), DocumentError> {
    let validator = compiled_schema()?;
    if let Err(mut errors) = validator.validate(value) {
        if let Some(first) = errors.next() {
            return Err(DocumentError::Schema {
                path: first.instance_path.to_string(),
                message: first.to_string(),
            });
        }
    }
    Ok(())
}

/// Valida as regras semânticas do `DocumentSpec` (além do JSON
/// Schema). Recebe o spec já desserializado — a Etapa 3 usa
/// `serde_json::from_value` antes desta chamada.
///
/// ## Regras (v0.2)
///
/// 1. `blocks` não pode ser vazio.
/// 2. `spec_version` deve ser `MAJOR.MINOR.PATCH` parseável.
/// 3. `Kpis` aceita 2 a 4 cartões.
/// 4. `Steps` aceita 1 a N passos (`N` razoável, sem teto duro por
///    enquanto — limite da engine, não do spec).
/// 5. `Table.headers` e cada `Table.rows[i]` devem ter o mesmo
///    número de colunas.
/// 6. `Spreadsheet` (`doc_type`) aceita apenas blocos `Kpis`, `Table`
///    e `Chart` — `Cover`, `Toc`, `Heading`, `Paragraph`, `List`, etc.
///    são rejeitados. Justificativa: o `DocumentSpec` é por
///    **documento**, não por planilha; um workbook completo
///    multi-aba vira uma **lista** de specs (Etapa 4 decide o
///    formato de batch).
/// 7. `language` deve estar em minúsculas (BCP-47 recomenda).
/// 8. `style == Sobrio` rejeita `metadata.watermark.is_some()` (Etapa
///    5, ADR-0021 §D-PDF2). Modo Sóbrio é para registráveis (ata,
///    contrato, alteração contratual); tarja visual atravessando
///    instrumento da Junta Comercial é erro. O validador rejeita a
///    combinação em vez de obedecer silenciosamente.
pub fn validate_semantic(spec: &DocumentSpec) -> Result<(), DocumentError> {
    // (1) blocks não vazio
    if spec.blocks.is_empty() {
        return Err(DocumentError::Semantic {
            path: "/blocks".to_string(),
            message: "blocks não pode ser vazio".to_string(),
        });
    }

    // (2) spec_version no formato MAJOR.MINOR.PATCH
    let parts: Vec<&str> = spec.spec_version.0.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(DocumentError::Semantic {
            path: "/spec_version".to_string(),
            message: format!(
                "spec_version {:?} não está no formato MAJOR.MINOR.PATCH",
                spec.spec_version.0
            ),
        });
    }

    // (7) language em minúsculas
    if spec.language != spec.language.to_lowercase() {
        return Err(DocumentError::Semantic {
            path: "/language".to_string(),
            message: format!(
                "language {:?} deve estar em minúsculas (BCP-47)",
                spec.language
            ),
        });
    }

    // (8) Sobrio + marca d'água rejeitados (Etapa 5, ADR-0021
    // §D-PDF2). O validador do spec rejeita a combinação, em vez
    // de obedecer silenciosamente. Modo Sóbrio é para registráveis
    // (ata, contrato, alteração contratual); tarja visual
    // atravessando instrumento da Junta Comercial é erro.
    if spec.style == crate::spec::DocumentStyle::Sobrio && spec.metadata.watermark.is_some() {
        return Err(DocumentError::Semantic {
            path: "/metadata/watermark".to_string(),
            message: "marca d'água visual (watermark) não pode ser usada com DocumentStyle::Sobrio; modo Sóbrio é para registráveis (Junta Comercial) e tarja visual é inadequada".to_string(),
        });
    }

    // (3, 4, 5, 6) per-block
    for (i, block) in spec.blocks.iter().enumerate() {
        let path = |suffix: &str| format!("/blocks/{i}{suffix}");

        match block {
            DocumentBlock::Kpis { items } => {
                let n = items.len();
                if !(2..=4).contains(&n) {
                    return Err(DocumentError::Semantic {
                        path: path("/items"),
                        message: format!("Kpis aceita 2 a 4 cartões; recebido {n}"),
                    });
                }
            }
            DocumentBlock::Steps { items } => {
                if items.is_empty() {
                    return Err(DocumentError::Semantic {
                        path: path("/items"),
                        message: "Steps precisa de pelo menos 1 passo".to_string(),
                    });
                }
            }
            DocumentBlock::Table { headers, rows, .. } => {
                let ncols = headers.len();
                for (r, row) in rows.iter().enumerate() {
                    if row.len() != ncols {
                        return Err(DocumentError::Semantic {
                            path: path(&format!("/rows/{r}")),
                            message: format!(
                                "linha {r} tem {} colunas; cabeçalho tem {ncols}",
                                row.len()
                            ),
                        });
                    }
                }
                if headers.is_empty() {
                    return Err(DocumentError::Semantic {
                        path: path("/headers"),
                        message: "Table precisa de pelo menos 1 coluna no cabeçalho".to_string(),
                    });
                }
            }
            _ => {}
        }

        // (6) Spreadsheet aceita apenas Kpis, Table, Chart
        if spec.doc_type == crate::spec::DocumentType::Spreadsheet {
            let allowed = matches!(
                block,
                DocumentBlock::Kpis { .. }
                    | DocumentBlock::Table { .. }
                    | DocumentBlock::Chart { .. }
            );
            if !allowed {
                return Err(DocumentError::Semantic {
                    path: path(""),
                    message: format!(
                        "doc_type=spreadsheet aceita apenas blocos Kpis/Table/Chart; recebido {block_kind:?}",
                        block_kind = block_kind(block)
                    ),
                });
            }
        }
    }

    Ok(())
}

/// Nome do tipo de bloco, para mensagem de erro amigável.
fn block_kind(block: &DocumentBlock) -> &'static str {
    match block {
        DocumentBlock::Cover(_) => "cover",
        DocumentBlock::Toc => "toc",
        DocumentBlock::Heading { .. } => "heading",
        DocumentBlock::Paragraph { .. } => "paragraph",
        DocumentBlock::List { .. } => "list",
        DocumentBlock::Table { .. } => "table",
        DocumentBlock::KeyValue { .. } => "key_value",
        DocumentBlock::Kpis { .. } => "kpis",
        DocumentBlock::Callout { .. } => "callout",
        DocumentBlock::Quote(_) => "quote",
        DocumentBlock::Steps { .. } => "steps",
        DocumentBlock::Chart { .. } => "chart",
        DocumentBlock::Image(_) => "image",
        DocumentBlock::Code(_) => "code",
        DocumentBlock::Divider => "divider",
        DocumentBlock::Spacer { .. } => "spacer",
        DocumentBlock::PageBreak => "page_break",
        DocumentBlock::Footer { .. } => "footer",
        DocumentBlock::Signatures { .. } => "signatures",
        DocumentBlock::BackCover { .. } => "back_cover",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{CalloutKind, Cover, KpiCard};
    use crate::spec::{
        DocumentMetadata, DocumentStyle, DocumentType, SpecVersion, WatermarkPosition,
        WatermarkSpec,
    };

    /// Helper: spec mínimo válido, com `style` e watermark
    /// customizáveis. Blocks não vazios (regra 1) e
    /// `spec_version` MAJOR.MINOR.PATCH (regra 2) garantidos.
    fn spec_with(style: DocumentStyle, watermark: Option<WatermarkSpec>) -> DocumentSpec {
        DocumentSpec {
            spec_version: SpecVersion("0.3.0".to_string()),
            doc_type: DocumentType::Report,
            style,
            language: "pt-br".to_string(),
            blocks: vec![DocumentBlock::Cover(Cover {
                title: "Teste".to_string(),
                subtitle: None,
                author: None,
                date: None,
            })],
            metadata: DocumentMetadata {
                title: None,
                author: None,
                organization: None,
                keywords: None,
                description: None,
                watermark,
                pdfa: None,
            },
            confidentiality: None,
        }
    }

    /// Regra 8: `style == Sobrio` rejeita `watermark.is_some()`.
    /// Etapa 5 (ADR-0021 §D-PDF2): modo Sóbrio é para
    /// registráveis; tarja visual atravessando instrumento da
    /// Junta é erro. O validador rejeita em vez de obedecer
    /// silenciosamente.
    #[test]
    fn sobrio_rejects_watermark() {
        let spec = spec_with(
            DocumentStyle::Sobrio,
            Some(WatermarkSpec {
                text: "CONFIDENCIAL".to_string(),
                position: WatermarkPosition::Diagonal,
                opacity: None,
                font_size: None,
            }),
        );
        let err = validate_semantic(&spec).unwrap_err();
        match err {
            DocumentError::Semantic { path, message } => {
                assert_eq!(path, "/metadata/watermark");
                assert!(
                    message.contains("Sobrio"),
                    "mensagem deve mencionar Sóbrio: {message}"
                );
            }
            other => panic!("esperava Semantic, recebi {other:?}"),
        }
    }

    /// Tinta & Latão + watermark é aceito (regra 8 não
    /// dispara). Caso de uso: relatório interno com tarja
    /// CONFIDENCIAL diagonal.
    #[test]
    fn tinta_e_latao_accepts_watermark() {
        let spec = spec_with(
            DocumentStyle::TintaELatao,
            Some(WatermarkSpec {
                text: "USO INTERNO".to_string(),
                position: WatermarkPosition::Center,
                opacity: Some(0.10),
                font_size: Some(72.0),
            }),
        );
        assert!(validate_semantic(&spec).is_ok());
    }

    /// Default (sem watermark, qualquer estilo) é aceito. O
    /// default de `metadata.watermark` é `None` — o caso
    /// comum, sem opt-in.
    #[test]
    fn no_watermark_is_accepted_for_every_style() {
        for style in [DocumentStyle::TintaELatao, DocumentStyle::Sobrio] {
            let spec = spec_with(style, None);
            assert!(
                validate_semantic(&spec).is_ok(),
                "style {style:?} sem watermark deve aceitar"
            );
        }
    }

    /// Bump de `SpecVersion` 0.2.0 → 0.3.0 (PR 3 da Etapa 5
    /// adiciona `DocumentMetadata.pdfa`) continua validando
    /// o formato MAJOR.MINOR.PATCH (regra 2). Defesa contra
    /// alguém voltar o default sem perceber.
    #[test]
    fn spec_version_0_3_0_passes_format_check() {
        let spec = spec_with(DocumentStyle::TintaELatao, None);
        // O spec construído pelo helper já tem "0.3.0".
        assert_eq!(spec.spec_version.0, "0.3.0");
        assert!(validate_semantic(&spec).is_ok());
    }

    /// Smoke: `Cover` é um bloco válido (não dispara regra 6
    /// `Spreadsheet`). Garante que a regra 8 foi adicionada
    /// sem quebrar regras pré-existentes.
    #[test]
    fn cover_block_accepted_in_report_style() {
        let spec = spec_with(DocumentStyle::TintaELatao, None);
        assert!(validate_semantic(&spec).is_ok());
    }

    /// Smoke: `Kpis` com 3 cartões é aceito (regra 3, 2 ≤ n ≤
    /// 4). Cobertura mínima pra garantir que o helper não
    /// distorceu o teste.
    #[test]
    fn kpis_three_cards_accepted() {
        let mut spec = spec_with(DocumentStyle::TintaELatao, None);
        spec.blocks = vec![DocumentBlock::Kpis {
            items: vec![
                KpiCard {
                    label: "Receita".to_string(),
                    value: "R$ 100k".to_string(),
                    delta: None,
                    delta_label: None,
                },
                KpiCard {
                    label: "Margem".to_string(),
                    value: "30%".to_string(),
                    delta: None,
                    delta_label: None,
                },
                KpiCard {
                    label: "Clientes".to_string(),
                    value: "150".to_string(),
                    delta: None,
                    delta_label: None,
                },
            ],
        }];
        assert!(validate_semantic(&spec).is_ok());
    }

    /// Smoke de não-regressão: `Callout` continua aceito em
    /// `style = TintaELatao` sem watermark.
    #[test]
    fn callout_block_accepted_in_tinta_e_latao() {
        let mut spec = spec_with(DocumentStyle::TintaELatao, None);
        spec.blocks = vec![DocumentBlock::Callout {
            kind: CalloutKind::Info,
            text: "Tudo ok".to_string(),
        }];
        assert!(validate_semantic(&spec).is_ok());
    }
}
