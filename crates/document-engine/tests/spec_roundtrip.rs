//! Testes E2E do `frederico-document-engine` (Etapa 1 da Fase 5).
//!
//! Cobre:
//! 1. Round-trip JSON → `DocumentSpec` → JSON preserva dados.
//! 2. `validate_against_schema` rejeita JSON inválido com path
//!    JSON pointer.
//! 3. `validate_semantic` rejeita cada regra (Kpis=1, Kpis=5,
//!    Steps=0, Table com linhas de tamanho errado, Spreadsheet com
//!    blocos proibidos, spec_version malformada, language com
//!    maiúsculas).
//! 4. Idempotência: o schema gerado em `build.rs` é determinístico
//!    — reescrever não muda byte-a-byte (exceto chaves de ordem que
//!    o `serde_json` preserva).
//! 5. `document_mode_prompt` lista todos os 20 blocos e tem tamanho
//!    mínimo razoável.

use frederico_document_engine::{
    validate_against_schema, validate_semantic, Cover, DocumentBlock, DocumentError, DocumentSpec,
    DocumentType, JsonValue,
};
use serde_json::json;

fn minimal_spec() -> DocumentSpec {
    DocumentSpec {
        spec_version: frederico_document_engine::SpecVersion("0.1.0".to_string()),
        doc_type: DocumentType::Report,
        style: frederico_document_engine::DocumentStyle::TintaELatao,
        language: "pt-br".to_string(),
        blocks: vec![DocumentBlock::Cover(Cover {
            title: "Relatório".to_string(),
            subtitle: None,
            author: None,
            date: None,
        })],
        metadata: frederico_document_engine::DocumentMetadata::default(),
        confidentiality: None,
    }
}

#[test]
fn roundtrip_preserves_data() {
    let original = minimal_spec();
    let json = serde_json::to_value(&original).expect("DocumentSpec serializa");
    let parsed =
        serde_json::from_value::<DocumentSpec>(json.clone()).expect("DocumentSpec desserializa");
    assert_eq!(original, parsed);
    // Repassa pela serialização e compara.
    let json2 = serde_json::to_value(&parsed).expect("segunda serialização");
    assert_eq!(json, json2);
}

#[test]
fn tagged_enum_serializes_with_type_field() {
    let spec = minimal_spec();
    let json = serde_json::to_value(&spec).unwrap();
    // O bloco Cover é `{"type": "cover", "title": ...}`.
    let block = &json["blocks"][0];
    assert_eq!(block["type"], "cover");
    assert_eq!(block["title"], "Relatório");
}

#[test]
fn validate_against_schema_accepts_valid_spec() {
    let spec = minimal_spec();
    let json = serde_json::to_value(&spec).unwrap();
    validate_against_schema(&json).expect("spec mínimo válido");
}

#[test]
fn validate_against_schema_rejects_wrong_type() {
    // `doc_type` é enum; passar inteiro deve falhar.
    let mut json = serde_json::to_value(minimal_spec()).unwrap();
    json["doc_type"] = json!(42);
    let err = validate_against_schema(&json).expect_err("doc_type errado tem que falhar");
    match err {
        DocumentError::Schema { path, .. } => {
            // O path aponta para /doc_type (ou próximo).
            assert!(path.contains("doc_type"), "path inesperado: {path}");
        }
        other => panic!("esperava Schema, recebi {other:?}"),
    }
}

#[test]
fn validate_against_schema_rejects_missing_required_field() {
    // `spec_version` é obrigatório.
    let mut json = serde_json::to_value(minimal_spec()).unwrap();
    json.as_object_mut().unwrap().remove("spec_version");
    let err = validate_against_schema(&json).expect_err("spec_version faltando tem que falhar");
    assert!(matches!(err, DocumentError::Schema { .. }));
}

#[test]
fn validate_semantic_rejects_empty_blocks() {
    let mut spec = minimal_spec();
    spec.blocks.clear();
    let err = validate_semantic(&spec).expect_err("blocks vazio tem que falhar");
    match err {
        DocumentError::Semantic { path, message } => {
            assert_eq!(path, "/blocks");
            assert!(message.contains("vazio"));
        }
        other => panic!("esperava Semantic, recebi {other:?}"),
    }
}

#[test]
fn validate_semantic_rejects_kpis_with_one_card() {
    let mut spec = minimal_spec();
    spec.blocks.push(DocumentBlock::Kpis {
        items: vec![frederico_document_engine::KpiCard {
            label: "X".to_string(),
            value: "1".to_string(),
            delta: None,
            delta_label: None,
        }],
    });
    let err = validate_semantic(&spec).expect_err("1 KPI tem que falhar");
    assert!(matches!(err, DocumentError::Semantic { .. }));
}

#[test]
fn validate_semantic_rejects_kpis_with_five_cards() {
    let mut spec = minimal_spec();
    let items = (0..5)
        .map(|i| frederico_document_engine::KpiCard {
            label: format!("K{i}"),
            value: "0".to_string(),
            delta: None,
            delta_label: None,
        })
        .collect();
    spec.blocks.push(DocumentBlock::Kpis { items });
    let err = validate_semantic(&spec).expect_err("5 KPIs tem que falhar");
    assert!(matches!(err, DocumentError::Semantic { .. }));
}

#[test]
fn validate_semantic_accepts_kpis_with_two_three_or_four_cards() {
    for n in [2, 3, 4] {
        let mut spec = minimal_spec();
        let items = (0..n)
            .map(|i| frederico_document_engine::KpiCard {
                label: format!("K{i}"),
                value: "0".to_string(),
                delta: None,
                delta_label: None,
            })
            .collect();
        spec.blocks.push(DocumentBlock::Kpis { items });
        validate_semantic(&spec).unwrap_or_else(|e| panic!("{n} KPIs válidos falharam: {e}"));
    }
}

#[test]
fn validate_semantic_rejects_empty_steps() {
    let mut spec = minimal_spec();
    spec.blocks.push(DocumentBlock::Steps { items: vec![] });
    let err = validate_semantic(&spec).expect_err("Steps vazio tem que falhar");
    assert!(matches!(err, DocumentError::Semantic { .. }));
}

#[test]
fn validate_semantic_rejects_table_with_mismatched_columns() {
    let mut spec = minimal_spec();
    spec.blocks.push(DocumentBlock::Table {
        headers: vec!["A".to_string(), "B".to_string()],
        rows: vec![vec!["1".to_string()]], // linha com 1 coluna
        total: None,
        currency: None,
        percent: false,
        thousands: false,
        title: None,
        source: None,
    });
    let err = validate_semantic(&spec).expect_err("linha com nº errado de colunas tem que falhar");
    match err {
        DocumentError::Semantic { path, .. } => {
            assert!(path.contains("/rows/0"), "path inesperado: {path}");
        }
        other => panic!("esperava Semantic, recebi {other:?}"),
    }
}

#[test]
fn validate_semantic_rejects_table_without_headers() {
    let mut spec = minimal_spec();
    spec.blocks.push(DocumentBlock::Table {
        headers: vec![],
        rows: vec![],
        total: None,
        currency: None,
        percent: false,
        thousands: false,
        title: None,
        source: None,
    });
    let err = validate_semantic(&spec).expect_err("Table sem headers tem que falhar");
    assert!(matches!(err, DocumentError::Semantic { .. }));
}

#[test]
fn validate_semantic_rejects_spreadsheet_with_cover() {
    let mut spec = minimal_spec();
    spec.doc_type = DocumentType::Spreadsheet;
    // O bloco Cover (do minimal_spec) deve ser rejeitado em Spreadsheet.
    let err = validate_semantic(&spec).expect_err("Cover em Spreadsheet tem que falhar");
    match err {
        DocumentError::Semantic { path, message } => {
            assert!(path.contains("/blocks/0"), "path inesperado: {path}");
            assert!(message.contains("spreadsheet"), "mensagem: {message}");
        }
        other => panic!("esperava Semantic, recebi {other:?}"),
    }
}

#[test]
fn validate_semantic_accepts_spreadsheet_with_table_kpis_chart() {
    let mut spec = minimal_spec();
    spec.doc_type = DocumentType::Spreadsheet;
    spec.blocks.clear();
    spec.blocks.push(DocumentBlock::Table {
        headers: vec!["A".to_string()],
        rows: vec![vec!["1".to_string()]],
        total: None,
        currency: None,
        percent: false,
        thousands: false,
        title: None,
        source: None,
    });
    spec.blocks.push(DocumentBlock::Kpis {
        items: (0..2)
            .map(|i| frederico_document_engine::KpiCard {
                label: format!("K{i}"),
                value: "0".to_string(),
                delta: None,
                delta_label: None,
            })
            .collect(),
    });
    spec.blocks.push(DocumentBlock::Chart {
        kind: frederico_document_engine::ChartKind::Bar,
        labels: vec!["X".to_string()],
        series: vec![frederico_document_engine::ChartSeries {
            name: "S1".to_string(),
            values: vec!["1".to_string()],
        }],
        title: None,
    });
    validate_semantic(&spec).expect("Spreadsheet com Table+Kpis+Chart tem que passar");
}

#[test]
fn validate_semantic_rejects_bad_spec_version() {
    let mut spec = minimal_spec();
    spec.spec_version = frederico_document_engine::SpecVersion("v1".to_string());
    let err = validate_semantic(&spec).expect_err("spec_version malformado tem que falhar");
    assert!(matches!(err, DocumentError::Semantic { .. }));
}

#[test]
fn validate_semantic_rejects_uppercase_language() {
    let mut spec = minimal_spec();
    spec.language = "PT-BR".to_string();
    let err = validate_semantic(&spec).expect_err("language maiúscula tem que falhar");
    match err {
        DocumentError::Semantic { path, message } => {
            assert_eq!(path, "/language");
            assert!(message.contains("minúsculas"));
        }
        other => panic!("esperava Semantic, recebi {other:?}"),
    }
}

#[test]
fn schema_generation_is_idempotent() {
    // Gera o schema duas vezes (via duas chamadas a `schema_for!` em
    // sequência, via reimport dos tipos) e compara. Se o `build.rs`
    // rodar com `cargo:rerun-if-changed=src/blocks.rs` e
    // `cargo:rerun-if-changed=src/spec.rs`, e o teste passar, o
    // schema gerado é determinístico.
    use schemars::schema_for;
    let s1 = serde_json::to_string(&schema_for!(DocumentSpec)).unwrap();
    let s2 = serde_json::to_string(&schema_for!(DocumentSpec)).unwrap();
    assert_eq!(s1, s2, "schema_for!(DocumentSpec) não é determinístico");
}

#[test]
fn prompt_lists_every_block_kind_in_catalog() {
    use frederico_document_engine::prompt::document_mode_prompt;
    let p = document_mode_prompt();
    // Lista deve cobrir os 20 blocos.
    for name in [
        "cover",
        "toc",
        "heading",
        "paragraph",
        "list",
        "table",
        "key_value",
        "kpis",
        "callout",
        "quote",
        "steps",
        "chart",
        "image",
        "code",
        "divider",
        "spacer",
        "page_break",
        "footer",
        "signatures",
        "back_cover",
    ] {
        assert!(
            p.contains(&format!("`{name}`")),
            "bloco {name} ausente do prompt"
        );
    }
}

#[test]
fn prompt_mentions_all_semantic_rules() {
    use frederico_document_engine::prompt::document_mode_prompt;
    let p = document_mode_prompt();
    // As 7 regras têm que aparecer referenciadas no prompt.
    assert!(p.contains("vazio"), "regra 1 (blocks não vazio) ausente");
    assert!(p.contains("0.1.0"), "regra 2 (spec_version) ausente");
    assert!(p.contains("2 a 4"), "regra 3 (Kpis 2-4) ausente");
    assert!(p.contains("1 ou mais"), "regra 4 (Steps ≥ 1) ausente");
    assert!(
        p.contains("mesmo número"),
        "regra 5 (Table colunas) ausente"
    );
    assert!(p.contains("spreadsheet"), "regra 6 (Spreadsheet) ausente");
    assert!(p.contains("minúsculas"), "regra 7 (language) ausente");
}

#[test]
fn json_value_re_export_works() {
    // Smoke test: o re-export de `serde_json::Value` está funcional.
    let v: JsonValue = json!({"a": 1});
    assert_eq!(v["a"], 1);
}
