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
/// ## Regras (v0.1)
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
