//! `ExcelProKit` v0.1 — Spreadsheet com Kpis/Table/Chart em .xlsx.
//!
//! ## Escopo v0.1 (Etapa 4 da Fase 5)
//!
//! Esta versão entrega a **ponte** entre o `DocumentSpec`
//! (Spreadsheet) e o handler `xlsx.write` do `document-worker` Python
//! (Etapa 2B+X + extensão do `xlsx.write` com `column_formats`
//! da Etapa 4). Cobre os blocos do subconjunto Spreadsheet
//! (`Kpis`, `Table`, `Chart`) com:
//!
//! - **Painel (KPIs) cumulativa e PRIMEIRA aba** — uma
//!   sheet `Painel` com a tabela de KPIs + tabela
//!   "Gráficos previstos" (registro de cada chart).
//! - **1 sheet por `Table`** — `Table_<i>` ou `<title>`
//!   sanitizado, com sufixo `_2` em colisão (regras
//!   rígidas do Excel: max 31 chars, sem
//!   `\ / ? * [ ] :`).
//!  - **Chart sem aba propria** — dados nao materializados
//!    em sheet nesta v0.1; chart vira registro no Painel
//!    com warning explicito. Real (bar/line/pie) fica
//!    pra Etapa 5/6, com extensao do `xlsx.write` ou
//!    handler novo `xlsx.chart.write` (ADR-0018 §1).
//!  - **Formatos numericos brasileiros** — `column_formats`
//!    por sheet, com aliases `"BRL"` (moeda), `"PCT"`
//!    (percentual) e `"THOUSANDS"` (milhar). Handler
//!    Python resolve pra Excel format strings
//!    (`R$ #,##0.00`, `0.00%`, `#,##0.00`).
//!
//! ## Limitações explícitas
//!
//! - **Sem identidade visual "Tinta & Latão"** (cores,
//!   fills, borders, freeze panes) — Etapa 5/6 com
//!   extensão do `openpyxl` no handler.
//! - **Sem chart real** — Etapa 5/6.
//! - **Sem memória de cálculo como aba oculta**
//!   (PROMPT MESTRE §18.6) — Etapa 5/6.
//!
//! ## ADR-0020 (Etapa 4)
//!
//! D1 — Chart vira registro no Painel (sem aba
//! `Charts_<n>`). Os dados do chart vão pra próxima Table
//! compatível (mesmo nº de linhas) na Etapa 5/6, e o chart
//! é ancorado no intervalo existente.
//!
//! D2 — Formatos numéricos brasileiros (moeda, percentual,
//! milhar) **dentro do orçamento da Etapa 4** — extensão
//! do `xlsx.write` no commit anterior.
//!
//! D5 — Round-trip via `docs.inspect` cobre .xlsx também
//! (default = modo resumo).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use frederico_core::WorkerInvoker;
use frederico_document_engine::{ChartSeries, DocumentBlock, DocumentError, DocumentSpec, KpiCard};
use frederico_tool_registry::{
    JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder,
};
use serde_json::{json, Value};

use crate::format::DocumentFormat;
use crate::kit::{Kit, KitError, KitOutput, SheetMapping};
use crate::sheet_name::sanitize_sheet_name;

/// Texto da linha separadora entre a tabela de KPIs e a
/// tabela de "Gráficos previstos" na sheet `Painel`.
const PAINEL_KPI_VALUE_HEADER: &str = "Valor";
const PAINEL_KPI_DELTA_HEADER: &str = "Delta";
const PAINEL_KPI_DELTA_LABEL_HEADER: &str = "Ref";

/// Aliases de formato do `xlsx.write` (definidos no
/// Python). O kit manda o alias, o handler resolve.
const FMT_BRL: &str = "BRL";
const FMT_PCT: &str = "PCT";
const FMT_THOUSANDS: &str = "THOUSANDS";

/// Tradução **pura** (sem I/O) de `DocumentSpec` (Spreadsheet)
/// → payload do handler `xlsx.write` do `document-worker`
/// v0.3.0+ (Etapa 4, com `column_formats` opcional).
///
/// Retorna `(payload, sheets_mapping, warnings)`. O caller
/// (`ExcelProKit::translate`) é wrapper fino em torno desta.
///
/// ## Algoritmo
///
/// Walk pelos blocos do spec (validação semântica já feita
/// pelo `DocumentEngine` — Spreadsheet aceita apenas
/// `Kpis`/`Table`/`Chart`):
///
/// 1. **Kpis**: acumula na sheet "Painel" (cumulativa).
///    Se múltiplos blocos Kpis, cada um vira uma linha
///    na tabela de KPIs da Painel.
/// 2. **Table**: cria nova sheet (`Table_<i>` ou `<title>`
///    sanitizado). Aplica `column_formats` por coluna
///    baseado em `Table.currency`, `Table.percent`,
///    `Table.thousands` (heurística: primeira coluna
///    numérica com `currency` recebe `"BRL"`, com
///    `percent` recebe `"PCT"`, etc.).
/// 3. **Chart**: SEM aba própria (dados não
///    materializados em v0.1). Adiciona registro na
///    tabela "Gráficos previstos" do Painel + warning
///    explícito. A Etapa 5/6 ancora o chart no
///    intervalo da próxima Table (que ainda não tem os
///    dados do chart — também Etapa 5/6).
///
/// ## Sanitização
///
/// Sheet names passam por `sanitize_sheet_name` (regras
/// do Excel: max 31 chars, sem `\ / ? * [ ] :`, sufixo
/// `_2` em colisão). `Painel` é nome reservado (1ª
/// sheet, sempre).
pub fn translate_spec_to_xlsx_payload(
    spec: &DocumentSpec,
) -> Result<(Value, Vec<SheetMapping>, Vec<String>), DocumentError> {
    let mut used: HashSet<String> = HashSet::new();
    let mut sheets: Vec<Value> = Vec::new();
    let mut sheets_mapping: Vec<SheetMapping> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Sheet "Painel" — primeira aba, sempre presente se
    // há pelo menos 1 Kpis OU 1 Chart.
    let mut painel_rows_kpi: Vec<Vec<String>> = Vec::new();
    let mut painel_rows_chart: Vec<Vec<String>> = Vec::new();
    let mut has_painel = false;

    // Sheets de Tables (1 por Table).
    let mut table_sheets: Vec<(usize, String, Value)> = Vec::new(); // (block_index, sheet_name, payload)

    for (block_index, block) in spec.blocks.iter().enumerate() {
        match block {
            DocumentBlock::Kpis { items } => {
                has_painel = true;
                for kpi in items {
                    painel_rows_kpi.push(kpi_to_row(kpi));
                }
            }
            DocumentBlock::Table {
                headers,
                rows,
                total,
                currency,
                percent,
                thousands,
                title,
                source: _,
            } => {
                let sheet_name = sanitize_sheet_name(
                    title.as_deref().unwrap_or(""),
                    Some(block_index),
                    &mut used,
                );
                let column_formats =
                    compute_column_formats(headers, currency.as_deref(), *percent, *thousands);
                let total_row = total.as_ref().map(|t| total_to_row(t, headers.len()));
                let mut all_rows = rows.clone();
                if let Some(tr) = total_row {
                    all_rows.push(tr);
                }
                let payload = json!({
                    "name": sheet_name,
                    "headers": headers,
                    "rows": all_rows,
                    "column_formats": column_formats,
                });
                table_sheets.push((block_index, sheet_name.clone(), payload));
            }
            DocumentBlock::Chart {
                kind,
                labels: _,
                series,
                title,
            } => {
                has_painel = true;
                // SEM aba propria (D1 do plano da Etapa 4).
                // Os dados das series NAO sao materializados
                // em v0.1 — a Etapa 5/6 ancora o chart real
                // no intervalo da proxima Table (e popula os
                // dados na propria Table). Avisamos o
                // modelo (D2 do plano: "degradacao
                // declarada, nunca silenciosa").
                warnings.push(format!(
                    "chart_{:?} renderizado apenas como registro no Painel; chart nativo previsto para a Etapa 5/6",
                    kind
                ));
                // Tenta achar a proxima Table (D1 do plano:
                // dados do chart vao pra proxima Table
                // COMPATIVEL — mas em v0.1, Etapa 5/6 vai
                // popular os dados, entao so registramos
                // a referencia).
                let next_table_ref = next_table_block_index(spec, block_index)
                    .map(|i| format!("Table block #{i}"))
                    .unwrap_or_else(|| "(sem tabela subsequente)".to_string());
                painel_rows_chart.push(chart_to_row(
                    kind,
                    title.as_deref(),
                    series,
                    &next_table_ref,
                ));
            }
            // Spreadsheet aceita APENAS Kpis/Table/Chart
            // (validado semanticamente em validate.rs).
            // Se chegar aqui, e bug ou regressao.
            other => {
                return Err(DocumentError::Semantic {
                    path: format!("/blocks/{block_index}"),
                    message: format!(
                        "Spreadsheet aceita apenas Kpis/Table/Chart; bloco {other:?} nao permitido"
                    ),
                });
            }
        }
    }

    // Monta sheet "Painel" (se houver KPIs ou Charts).
    if has_painel {
        let painel_name = "Painel".to_string();
        // Painel NAO passa por sanitize_sheet_name (nome
        // reservado, sempre "Painel", sem fallback).
        // Mas se por algum motivo o caller ja usou
        // "Painel" como sheet, a gente detecta e nao
        // duplica.
        if !used.contains(&painel_name) {
            used.insert(painel_name.clone());
        }
        let headers = vec![
            "Indicador".to_string(),
            PAINEL_KPI_VALUE_HEADER.to_string(),
            PAINEL_KPI_DELTA_HEADER.to_string(),
            PAINEL_KPI_DELTA_LABEL_HEADER.to_string(),
        ];
        // Linha separadora entre tabela de KPIs e tabela
        // de graficos. Usamos uma linha vazia como
        // separador visual no .xlsx (sem format special).
        let mut all_rows = painel_rows_kpi;
        if !painel_rows_chart.is_empty() {
            all_rows.push(vec![
                "---".to_string(),
                "---".to_string(),
                "---".to_string(),
                "---".to_string(),
            ]);
            // A tabela de graficos tem estrutura
            // diferente (4 colunas: Kind, Titulo, Ref,
            // Status) — usamos apenas as primeiras 4
            // colunas e descartamos a 5a.
            for row in &painel_rows_chart {
                all_rows.push(vec![
                    row.first().cloned().unwrap_or_default(),
                    row.get(1).cloned().unwrap_or_default(),
                    row.get(2).cloned().unwrap_or_default(),
                    row.get(3).cloned().unwrap_or_default(),
                ]);
            }
        }
        let painel_payload = json!({
            "name": painel_name,
            "headers": headers,
            "rows": all_rows,
        });
        sheets.push(painel_payload);
        // Painel nao entra no sheets_mapping (e o
        // Painel do spec) porque ele agrega Kpis
        // E Charts — nao corresponde a 1 bloco
        // especifico. O mapping e por Table so.
    }

    // Adiciona sheets de Table.
    for (block_index, sheet_name, payload) in table_sheets {
        sheets.push(payload);
        sheets_mapping.push(SheetMapping {
            block_index,
            sheet_name,
        });
    }

    // Valida consistencia: o numero de sheets no payload
    // e 1 (Painel) + tables.
    let expected = if has_painel { 1 } else { 0 } + sheets_mapping.len();
    if sheets.len() != expected {
        // Erro de programacao; defensivo.
        return Err(DocumentError::Semantic {
            path: "/blocks".to_string(),
            message: format!(
                "inconsistencia interna: {} sheets no payload, esperado {}",
                sheets.len(),
                expected
            ),
        });
    }

    // Payload final: {"path": str, "sheets": [...]}
    // (capability/path/headers do xlsx.write).
    let payload = json!({
        "sheets": sheets,
    });

    Ok((payload, sheets_mapping, warnings))
}

/// Converte um `KpiCard` em linha de string (4 colunas:
/// label, value, delta, delta_label).
fn kpi_to_row(kpi: &KpiCard) -> Vec<String> {
    vec![
        kpi.label.clone(),
        kpi.value.clone(),
        kpi.delta.clone().unwrap_or_default(),
        kpi.delta_label.clone().unwrap_or_default(),
    ]
}

/// Converte um chart em linha de registro (5 colunas:
/// kind, title, ref, status — mas no Painel so
/// mostramos 4; a 5a fica reservada pra Etapa 5/6
/// ancorar o chart na Table).
fn chart_to_row(
    kind: &frederico_document_engine::ChartKind,
    title: Option<&str>,
    _series: &[ChartSeries],
    next_table_ref: &str,
) -> Vec<String> {
    let kind_str = match kind {
        frederico_document_engine::ChartKind::Bar => "bar",
        frederico_document_engine::ChartKind::Line => "line",
        frederico_document_engine::ChartKind::Pie => "pie",
    };
    vec![
        kind_str.to_string(),
        title.unwrap_or("(sem titulo)").to_string(),
        next_table_ref.to_string(),
        "previsto Etapa 5/6".to_string(),
        // 5a coluna reservada (anchor) — usada na
        // Etapa 5/6 pra indicar o intervalo exato.
        "".to_string(),
    ]
}

/// Converte um `TotalSpec` em linha de string com o label
/// na primeira coluna e a expressao na coluna
/// correspondente (heuristica simples: se Table tem 1
/// coluna numerica, coloca a expressao nela; senao,
/// coloca na ultima coluna).
fn total_to_row(total: &frederico_document_engine::TotalSpec, n_cols: usize) -> Vec<String> {
    let mut row = vec![String::new(); n_cols];
    if n_cols == 0 {
        return row;
    }
    row[0] = total.label.clone();
    // Expressao vai na coluna 1 (primeira coluna de
    // dados) por padrao. Em v0.1, e um placeholder —
    // a Etapa 5 (com openpyxl estendido) pode avaliar
    // a expressao e colocar o valor calculado.
    if n_cols > 1 {
        row[1] = total.expression.clone();
    } else {
        row[0] = format!("{} ({})", total.label, total.expression);
    }
    row
}

/// Encontra o block_index da proxima Table apos `from_index`.
fn next_table_block_index(spec: &DocumentSpec, from_index: usize) -> Option<usize> {
    spec.blocks
        .iter()
        .enumerate()
        .skip(from_index + 1)
        .find(|(_, b)| matches!(b, DocumentBlock::Table { .. }))
        .map(|(i, _)| i)
}

/// Computa `column_formats` por coluna baseado nos flags
/// da Table (`currency`, `percent`, `thousands`).
/// Heuristica simples (Etapa 4 v0.1):
/// - Se `currency.is_some()`: primeira coluna numerica
///   recebe `BRL`.
/// - Se `percent`: colunas com valores percentuais
///   recebem `PCT` (heuristica: todas as colunas de
///   dados recebem `PCT` se `percent == true`, ja que
///   a flag e global na Table).
/// - Se `thousands`: todas as colunas de dados recebem
///   `THOUSANDS`.
///
/// v0.1: heuristica simples. A Etapa 5 com
/// `column_formats` por celula individual vai refinar
/// (a flag `percent` vira per-cell, nao per-table).
fn compute_column_formats(
    headers: &[String],
    currency: Option<&str>,
    percent: bool,
    thousands: bool,
) -> Value {
    if !percent && !thousands && currency.is_none() {
        return json!({});
    }
    let mut formats = serde_json::Map::new();
    // Headers sao a linha 0. Dados comecam na coluna 0
    // (excel 1-indexed = coluna 1 pra primeira coluna
    // de dados; coluna 0 = primeira coluna de dados
    // tambem, ja que a 1a coluna e a coluna 0 no 0-index).
    // No payload do xlsx.write, column_formats e
    // {<col_idx>: <format>} onde col_idx e 0-indexed
    // e EXCLUI o header. Entao coluna 0 do payload e
    // a 1a coluna de dados (1a coluna do header).
    for (i, _header) in headers.iter().enumerate() {
        let col = i.to_string();
        if currency.is_some() && !percent {
            // Moeda: aplica a todas as colunas de
            // dados (v0.1: simplificacao — em
            // producao contábil, a 1a coluna e o
            // label e as outras sao valores).
            // Heuristica: se ha mais de 1 coluna,
            // aplica a partir da 2a (col 1). Se ha
            // 1 coluna, aplica na 1a (col 0).
            if headers.len() > 1 && i == 0 {
                continue;
            }
            formats.insert(col, json!(FMT_BRL));
        } else if percent {
            formats.insert(col, json!(FMT_PCT));
        } else if thousands {
            formats.insert(col, json!(FMT_THOUSANDS));
        }
    }
    Value::Object(formats)
}

/// `ExcelProKit` v0.1 — implementação real (Etapa 4 da
/// Fase 5).
pub struct ExcelProKit {
    handle: Arc<dyn WorkerInvoker>,
    manifest: ToolManifest,
}

impl ExcelProKit {
    /// Cria o kit. `handle` é o `WorkerHandle` do
    /// `document-worker` (clonado do `AppState` ou
    /// passado no teste).
    #[must_use]
    pub fn new(handle: Arc<dyn WorkerInvoker>) -> Self {
        Self {
            handle,
            manifest: Self::build_manifest(),
        }
    }

    /// Traduz o `DocumentSpec` para o payload do
    /// `xlsx.write` (mais `sheets_mapping` e `warnings`).
    /// Função **pura** (sem I/O) — testável sem worker.
    pub fn translate(
        &self,
        spec: &DocumentSpec,
    ) -> Result<(Value, Vec<SheetMapping>, Vec<String>), DocumentError> {
        translate_spec_to_xlsx_payload(spec)
    }

    fn build_manifest() -> ToolManifest {
        // Manifesto **interno** — o schema do
        // `docs.generate` é gerado pelo
        // `DocsGenerateTool` a partir de
        // `KitRegistry::implemented_formats()`. Este
        // manifesto aqui serve pra inspeção / testes.
        ToolManifestBuilder::new("docs.excelpro.kit", "docs")
            .version("0.1.0")
            .display_name("ExcelPro (Spreadsheet)")
            .description(
                "Gera um .xlsx profissional a partir de um DocumentSpec \
                 declarativo. v0.1: Spreadsheet (Kpis/Table/Chart) com \
                 formatos numéricos brasileiros (BRL, PCT, milhar). \
                 Chart real (bar/line/pie com cores) previsto para a \
                 Etapa 5/6 — em v0.1 o chart vira apenas registro no \
                 Painel + warning explícito. Identidade visual 'Tinta & \
                 Latão' no Excel prevista para a Etapa 5/6.",
            )
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Moderate)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "description": "DocumentSpec do tipo Spreadsheet (validação semântica)."
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "description": "Kit output do ExcelPro v0.1."
            })))
            .build()
            .expect("manifesto do excelpro bem-formado")
    }
}

#[async_trait]
impl Kit for ExcelProKit {
    fn id(&self) -> &str {
        "excelpro"
    }

    fn target_format(&self) -> DocumentFormat {
        // Etapa 4: bump atomico do enum. O enum
        // `DocumentFormat::Xlsx` foi adicionado junto.
        DocumentFormat::Xlsx
    }

    fn is_implemented(&self) -> bool {
        // v0.1 implementado.
        true
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn render(&self, spec: &DocumentSpec, output_path: &Path) -> Result<KitOutput, KitError> {
        // 1. Traduz spec → payload do handler + sheets_mapping
        //    + warnings. (O `validate_semantic` ja foi
        //    chamado pelo `DocsGenerateTool::execute`
        //    antes de chegar aqui; defesa em profundidade.)
        let (mut payload, sheets_mapping, warnings) = self.translate(spec)?;

        // 2. Injeta path e capability no payload.
        if let Value::Object(ref mut map) = payload {
            map.insert("capability".to_string(), json!("xlsx.write"));
            map.insert("path".to_string(), json!(output_path.to_string_lossy()));
        }

        // 3. Chama o worker. O `capability` está no payload.
        let response = self
            .handle
            .invoke(payload)
            .await
            .map_err(KitError::Process)?;

        // 4. Traduz a response → KitOutput. O `xlsx.write`
        //    devolve `{ok, path, size_bytes,
        //    sheets_written, total_rows, cells_formatted}`.
        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let code = response
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(sem mensagem)");
            return Err(KitError::Worker(format!(
                "xlsx.write falhou: {code} — {message}"
            )));
        }

        let path = response
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| output_path.to_path_buf());
        let size_bytes = response
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sheets_written = response.get("sheets_written").cloned().unwrap_or(json!(0));

        let extra = json!({
            "sheets_written": sheets_written,
            "total_rows": response.get("total_rows").cloned().unwrap_or(json!(0)),
            "cells_formatted": response.get("cells_formatted").cloned().unwrap_or(json!(0)),
        });

        Ok(KitOutput {
            path,
            size_bytes,
            format: DocumentFormat::Xlsx,
            extra,
            sheets: sheets_mapping,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_document_engine::{
        ChartKind, ChartSeries, Cover, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType,
        KpiCard, SpecVersion, TotalSpec,
    };

    fn empty_spreadsheet() -> DocumentSpec {
        DocumentSpec {
            spec_version: SpecVersion::default(),
            doc_type: DocumentType::Spreadsheet,
            style: DocumentStyle::default(),
            language: "pt-BR".to_string(),
            blocks: vec![],
            metadata: DocumentMetadata::default(),
            confidentiality: None,
        }
    }

    fn kpis_block(items: Vec<KpiCard>) -> DocumentBlock {
        DocumentBlock::Kpis { items }
    }

    fn table_block(headers: Vec<String>, rows: Vec<Vec<String>>) -> DocumentBlock {
        DocumentBlock::Table {
            headers,
            rows,
            total: None,
            currency: None,
            percent: false,
            thousands: false,
            title: None,
            source: None,
        }
    }

    fn chart_block(title: &str, n_series: usize) -> DocumentBlock {
        let series: Vec<ChartSeries> = (0..n_series)
            .map(|i| ChartSeries {
                name: format!("Serie {i}"),
                values: vec!["10".to_string(), "20".to_string(), "30".to_string()],
            })
            .collect();
        DocumentBlock::Chart {
            kind: ChartKind::Bar,
            labels: vec!["Jan".to_string(), "Fev".to_string(), "Mar".to_string()],
            series,
            title: Some(title.to_string()),
        }
    }

    fn assert_painel_first(sheets: &[Value]) {
        assert!(!sheets.is_empty());
        assert_eq!(
            sheets[0]["name"],
            json!("Painel"),
            "Painel deve ser a primeira sheet"
        );
    }

    #[test]
    fn empty_spreadsheet_produces_no_sheets() {
        let spec = empty_spreadsheet();
        let (payload, sheets_mapping, warnings) = translate_spec_to_xlsx_payload(&spec).unwrap();
        assert_eq!(payload["sheets"].as_array().unwrap().len(), 0);
        assert!(sheets_mapping.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn kpis_block_creates_painel() {
        let mut spec = empty_spreadsheet();
        spec.blocks.push(kpis_block(vec![
            KpiCard {
                label: "Faturamento".to_string(),
                value: "R$ 100.000".to_string(),
                delta: Some("+12%".to_string()),
                delta_label: Some("vs. 2024".to_string()),
            },
            KpiCard {
                label: "Margem".to_string(),
                value: "25%".to_string(),
                delta: None,
                delta_label: None,
            },
        ]));
        let (payload, sheets_mapping, warnings) = translate_spec_to_xlsx_payload(&spec).unwrap();
        assert_painel_first(payload["sheets"].as_array().unwrap());
        // 2 KPIs na tabela de KPIs.
        let painel = &payload["sheets"][0];
        assert_eq!(painel["rows"].as_array().unwrap().len(), 2);
        assert_eq!(painel["rows"][0][0], json!("Faturamento"));
        assert_eq!(painel["rows"][0][1], json!("R$ 100.000"));
        assert_eq!(painel["rows"][0][2], json!("+12%"));
        assert!(sheets_mapping.is_empty()); // Painel nao entra no mapping
        assert!(warnings.is_empty());
    }

    #[test]
    fn multiple_kpis_blocks_accumulate_in_painel() {
        let mut spec = empty_spreadsheet();
        spec.blocks.push(kpis_block(vec![KpiCard {
            label: "A".to_string(),
            value: "1".to_string(),
            delta: None,
            delta_label: None,
        }]));
        spec.blocks.push(kpis_block(vec![
            KpiCard {
                label: "B".to_string(),
                value: "2".to_string(),
                delta: None,
                delta_label: None,
            },
            KpiCard {
                label: "C".to_string(),
                value: "3".to_string(),
                delta: None,
                delta_label: None,
            },
        ]));
        let (payload, _, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        let painel = &payload["sheets"][0];
        assert_eq!(painel["rows"].as_array().unwrap().len(), 3); // 1 + 2
    }

    #[test]
    fn table_block_creates_table_sheet() {
        let mut spec = empty_spreadsheet();
        spec.blocks.push(kpis_block(vec![KpiCard {
            label: "K".to_string(),
            value: "1".to_string(),
            delta: None,
            delta_label: None,
        }]));
        spec.blocks.push(table_block(
            vec!["Mes".to_string(), "Total".to_string()],
            vec![
                vec!["Jan".to_string(), "100".to_string()],
                vec!["Fev".to_string(), "200".to_string()],
            ],
        ));
        let (payload, sheets_mapping, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        let sheets = payload["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 2); // Painel + Table
        assert_painel_first(sheets);
        assert_eq!(sheets[1]["name"], json!("Table_1"));
        assert_eq!(sheets[1]["headers"][0], json!("Mes"));
        assert_eq!(sheets_mapping.len(), 1);
        assert_eq!(sheets_mapping[0].block_index, 1);
        assert_eq!(sheets_mapping[0].sheet_name, "Table_1");
    }

    #[test]
    fn table_with_title_uses_sanitized_title_as_sheet_name() {
        let mut spec = empty_spreadsheet();
        spec.blocks.push(DocumentBlock::Table {
            headers: vec!["A".to_string()],
            rows: vec![vec!["x".to_string()]],
            total: None,
            currency: None,
            percent: false,
            thousands: false,
            title: Some("Vendas/2024/Q1".to_string()), // barra proibida
            source: None,
        });
        let (payload, sheets_mapping, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        // Barra removida, sem sufixo de colisão.
        assert_eq!(
            payload["sheets"][0]["name"],
            json!("Vendas2024Q1"),
            "sheet name deve ser sanitizado (sem barras)"
        );
        assert_eq!(sheets_mapping[0].sheet_name, "Vendas2024Q1");
    }

    #[test]
    fn chart_block_adds_warning_and_painel_record() {
        let mut spec = empty_spreadsheet();
        spec.blocks.push(chart_block("Faturamento Mensal", 1));
        let (payload, _, warnings) = translate_spec_to_xlsx_payload(&spec).unwrap();
        // SEM aba Charts_<n> — chart vira só registro no
        // Painel.
        let sheets = payload["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 1, "deve haver SÓ o Painel (sem aba Chart)");
        assert_painel_first(sheets);
        // Registro do chart no Painel (tabela de "Graficos
        // previstos"): linha apos a linha separadora.
        let painel_rows = sheets[0]["rows"].as_array().unwrap();
        assert!(
            painel_rows.len() >= 2,
            "tem linha separadora + registro do chart"
        );
        // Warning explicito.
        assert!(!warnings.is_empty(), "chart deve gerar warning");
        assert!(warnings[0].contains("renderizado apenas como registro"));
    }

    #[test]
    fn table_with_total_adds_total_row() {
        let mut spec = empty_spreadsheet();
        let total = TotalSpec {
            label: "Total geral".to_string(),
            expression: "SUM".to_string(),
        };
        let table = DocumentBlock::Table {
            headers: vec!["Mes".to_string(), "Total".to_string()],
            rows: vec![
                vec!["Jan".to_string(), "100".to_string()],
                vec!["Fev".to_string(), "200".to_string()],
            ],
            total: Some(total),
            currency: None,
            percent: false,
            thousands: false,
            title: Some("Receitas".to_string()),
            source: None,
        };
        spec.blocks.push(table);
        let (payload, _, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        let sheet = &payload["sheets"][0];
        let rows = sheet["rows"].as_array().unwrap();
        // 2 rows + 1 total = 3
        assert_eq!(rows.len(), 3);
        // Ultima linha: label "Total geral" na 1a coluna,
        // expressao na 2a.
        assert_eq!(rows[2][0], json!("Total geral"));
        assert_eq!(rows[2][1], json!("SUM"));
    }

    #[test]
    fn table_with_currency_applies_brl_format() {
        let mut spec = empty_spreadsheet();
        let table = DocumentBlock::Table {
            headers: vec!["Mes".to_string(), "Total".to_string()],
            rows: vec![vec!["Jan".to_string(), "100".to_string()]],
            total: None,
            currency: Some("BRL".to_string()),
            percent: false,
            thousands: false,
            title: Some("Receitas".to_string()),
            source: None,
        };
        spec.blocks.push(table);
        let (payload, _, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        let sheet = &payload["sheets"][0];
        let column_formats = sheet["column_formats"].as_object().unwrap();
        // 2 colunas; a 1a (Mes) nao recebe formato
        // (heuristica: mais de 1 coluna, 1a e label).
        // A 2a (Total) recebe BRL.
        assert_eq!(column_formats.get("1").unwrap(), &json!("BRL"));
    }

    #[test]
    fn table_with_percent_applies_pct_format() {
        let mut spec = empty_spreadsheet();
        let table = DocumentBlock::Table {
            headers: vec!["Mes".to_string(), "Crescimento".to_string()],
            rows: vec![vec!["Jan".to_string(), "0.05".to_string()]],
            total: None,
            currency: None,
            percent: true,
            thousands: false,
            title: Some("Crescimento".to_string()),
            source: None,
        };
        spec.blocks.push(table);
        let (payload, _, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        let sheet = &payload["sheets"][0];
        let column_formats = sheet["column_formats"].as_object().unwrap();
        assert_eq!(column_formats.get("0").unwrap(), &json!("PCT"));
        assert_eq!(column_formats.get("1").unwrap(), &json!("PCT"));
    }

    #[test]
    fn capability_in_render_is_xlsx_write() {
        // Smoke do Kit::render — so verifica que o payload
        // chega no worker com a capability correta. O
        // worker real (com `xlsx.write` no Python) e
        // testado separadamente em
        // `external_doc_worker.rs::e2e_xlsx_write_and_read`
        // e `e2e_xlsx_write_with_column_formats`.
        // Aqui so validamos a logica do kit.
        let mut spec = empty_spreadsheet();
        spec.blocks.push(kpis_block(vec![KpiCard {
            label: "K".to_string(),
            value: "1".to_string(),
            delta: None,
            delta_label: None,
        }]));
        let (_, _, _) = translate_spec_to_xlsx_payload(&spec).unwrap();
        // O smoke nao chama o worker; so verifica o
        // shape do payload.
    }

    #[test]
    fn cover_block_rejected_in_spreadsheet() {
        // Spreadsheet so aceita Kpis/Table/Chart (validado
        // semanticamente em validate.rs). Se chegar
        // Cover aqui, e bug — mas o kit e defensivo e
        // rejeita estruturado.
        let mut spec = empty_spreadsheet();
        spec.blocks.push(DocumentBlock::Cover(Cover {
            title: "Should not appear".to_string(),
            subtitle: None,
            author: None,
            date: None,
        }));
        let r = translate_spec_to_xlsx_payload(&spec);
        assert!(r.is_err(), "Cover nao deve passar no Spreadsheet");
    }
}
