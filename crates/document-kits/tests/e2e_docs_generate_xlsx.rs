//! Teste E2E da Etapa 4 do ExcelPro: `docs.generate` ponta-a-ponta.
//!
//! ## O que prova
//!
//! 1. **Infra completa** — `DocumentSpec` (Spreadsheet) →
//!    `ExcelProKit` (v0.1) → `WorkerToolDispatcher` →
//!    `WorkerHandle::invoke` → `document-worker` Python
//!    (handler `xlsx.write` v0.3.0+) com a extensão
//!    `column_formats` da Etapa 4.
//! 2. **Round-trip via `openpyxl` em subprocess** —
//!    reabre o `.xlsx` gerado via subprocess do mesmo
//!    Python com `openpyxl` e valida que tem:
//!    - a sheet `Painel` (1ª aba)
//!    - 1 sheet por Table (mapeamento `sheets: [{...}]`
//!      do output do `docs.generate` confere)
//!    - linha de TOTAL presente (heuristica: a
//!      última linha de dados da Table com `total`
//!      comeca com "Total")
//!    - formato de moeda aplicado (`cell.number_format`
//!      de uma célula da coluna de moeda é `R$ #,##0.00`)
//!    - chart vira registro no Painel (SEM aba Charts_<n>)
//!      + warning no output.
//!
//! ## Gate Windows
//!
//! O módulo é `#[cfg(windows)]`. `python.exe` precisa estar em
//! `workers/document-worker/runtime/`. Se faltar, o helper
//! `python_exe_or_panic` faz panic com mensagem clara
//! apontando pro `bootstrap.ps1`. **Não** é `#[ignore]`
//! (REGRAS §2.6).
//!
//! ## CI
//!
//! Adicionado ao `scripts/verify-external.ps1` como step
//! "E2E docs.generate xlsx" — roda no `windows-latest` do
//! GitHub Actions em todo PR (junto com os outros E2E).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use frederico_document_engine::{
    ChartKind, ChartSeries, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle,
    DocumentType, KpiCard, SpecVersion, TotalSpec,
};
use frederico_document_kits::{DocsGenerateTool, ExcelProKit, KitRegistry, WordProKit};
use frederico_process_architecture::ExternalSpawnConfig;
use frederico_test_support::with_test_timeout_at;
use frederico_tool_registry::{Tool, WorkerToolDispatcher};
use serde_json::json;

// ---------------------------------------------------------------------------
// Paths do worker
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR não tem 2 níveis acima")
        .to_path_buf()
}

fn python_exe() -> PathBuf {
    workspace_root()
        .join("workers")
        .join("document-worker")
        .join("runtime")
        .join("python.exe")
}

fn worker_script() -> PathBuf {
    workspace_root()
        .join("workers")
        .join("document-worker")
        .join("document-worker.py")
}

fn temp_out_dir() -> PathBuf {
    let mut d = std::env::temp_dir();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    d.push(format!("frederico_docs_generate_xlsx_e2e_{nonce}"));
    d
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

const E2E_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn python_exe_or_panic() -> PathBuf {
    let py = python_exe();
    if !py.is_file() {
        panic!(
            "python.exe nao encontrado em {}.\n\
             Rode `pwsh -NoProfile -ExecutionPolicy Bypass -File \
             workers/document-worker/bootstrap.ps1` para instalar o runtime.\n\
             Em CI: o step 'document-worker bootstrap' do verify-external.ps1 cuida disso.",
            py.display()
        );
    }
    py
}

fn doc_worker_config() -> ExternalSpawnConfig {
    let py = python_exe_or_panic();
    let script = worker_script();
    if !script.is_file() {
        panic!("document-worker.py não encontrado em {}", script.display());
    }
    ExternalSpawnConfig::new(py.to_string_lossy().into_owned())
        .with_args(vec![script.to_string_lossy().into_owned()])
        .with_cwd(workspace_root())
        .with_auth_token("e2e-docs-generate-xlsx-token")
        .with_ready_timeout(Duration::from_secs(20))
}

use std::time::Duration;

/// Constrói o `DocumentSpec` DoD da Etapa 4 do ExcelPro:
/// Kpis (2) + Table com total + Table com currency +
/// Table com percent + Chart. O inspect depois valida
/// que o arquivo gerado tem a estrutura esperada.
fn spec_do_etapa_4_xlsx() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Spreadsheet,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("Planilha de Etapa 4".to_string()),
            ..Default::default()
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Kpis {
                items: vec![
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
                ],
            },
            DocumentBlock::Table {
                headers: vec!["Mes".to_string(), "Receita (R$)".to_string()],
                rows: vec![
                    vec!["Jan".to_string(), "10000".to_string()],
                    vec!["Fev".to_string(), "20000".to_string()],
                    vec!["Mar".to_string(), "30000".to_string()],
                ],
                total: Some(TotalSpec {
                    label: "Total geral".to_string(),
                    expression: "SUM".to_string(),
                }),
                currency: Some("BRL".to_string()),
                percent: false,
                thousands: false,
                title: Some("Receitas por Mes".to_string()),
                source: None,
            },
            DocumentBlock::Table {
                headers: vec!["Mes".to_string(), "Crescimento".to_string()],
                rows: vec![
                    vec!["Jan".to_string(), "0.05".to_string()],
                    vec!["Fev".to_string(), "0.10".to_string()],
                ],
                total: None,
                currency: None,
                percent: true,
                thousands: false,
                title: Some("Crescimento Mensal".to_string()),
                source: None,
            },
            DocumentBlock::Chart {
                kind: ChartKind::Bar,
                labels: vec!["Jan".to_string(), "Fev".to_string(), "Mar".to_string()],
                series: vec![ChartSeries {
                    name: "Faturamento".to_string(),
                    values: vec![
                        "10000".to_string(),
                        "20000".to_string(),
                        "30000".to_string(),
                    ],
                }],
                title: Some("Faturamento Mensal".to_string()),
            },
        ],
    }
}

/// Reabre o `.xlsx` via subprocess do `python.exe` do
/// worker, usando `openpyxl`. Valida que tem:
/// - 1ª sheet = "Painel"
/// - pelo menos 1 sheet por Table do spec
/// - 1 das sheets tem linha de TOTAL (ultima linha
///   comeca com "Total")
/// - 1 das sheets tem formato de moeda
///   (`cell.number_format` igual a "R$ #,##0.00")
///
/// Imprime CHECK linhas no formato "CHECK nome=valor"
/// pro Rust parsear.
fn validate_xlsx_via_python(python: &PathBuf, xlsx_path: &PathBuf) {
    let script = r#"
import sys
from openpyxl import load_workbook

path = sys.argv[1]
wb = load_workbook(path, data_only=True)

# 1) Primeira sheet = "Painel"
first_sheet = wb.worksheets[0].title
print(f"CHECK first_sheet={first_sheet}")

# 2) Numero de sheets
n_sheets = len(wb.worksheets)
print(f"CHECK n_sheets={n_sheets}")

# 3) Para cada sheet, conferir: tem header na linha 1?
#    e a sheet name bate com o esperado?
sheet_names = [ws.title for ws in wb.worksheets]
print(f"CHECK sheet_names={','.join(sheet_names)}")

# 4) Alguma sheet tem linha de TOTAL?
# Heuristica: a ultima linha de dados (row 2+ ate ws.max_row)
# comeca com "Total" (case insensitive) na primeira coluna.
has_total = False
total_sheet = ""
for ws in wb.worksheets:
    if ws.max_row >= 2:
        # Pega a primeira celula da ultima linha de dados.
        last_row = list(ws.iter_rows(min_row=ws.max_row, max_row=ws.max_row, values_only=True))[0]
        if last_row and last_row[0] and "total" in str(last_row[0]).lower():
            has_total = True
            total_sheet = ws.title
            break
print(f"CHECK has_total={has_total}")
print(f"CHECK total_sheet={total_sheet}")

# 5) Alguma celula tem formato de moeda BRL?
# `cell.number_format == "R$ #,##0.00"` (exato, como o
# XLSX_FORMAT_ALIASES produz).
has_brl = False
for ws in wb.worksheets:
    for row in ws.iter_rows():
        for cell in row:
            if cell.value is not None and cell.number_format == "R$ #,##0.00":
                has_brl = True
                break
        if has_brl:
            break
    if has_brl:
        break
print(f"CHECK has_brl_format={has_brl}")

# 6) Alguma celula tem formato de percentual 0.00%?
has_pct = False
for ws in wb.worksheets:
    for row in ws.iter_rows():
        for cell in row:
            if cell.value is not None and cell.number_format == "0.00%":
                has_pct = True
                break
        if has_pct:
            break
    if has_pct:
        break
print(f"CHECK has_pct_format={has_pct}")

# 7) NUNCA deve existir sheet comeca com "Charts_"
# (D1 do plano: chart SEM aba Charts_<n>).
charts_n = sum(1 for ws in wb.worksheets if ws.title.startswith("Charts_"))
print(f"CHECK charts_sheet_count={charts_n}")
"#;
    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(xlsx_path)
        .output()
        .expect("falha spawnando python pra validar xlsx");
    if !output.status.success() {
        panic!(
            "validacao openpyxl falhou: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut first_sheet = String::new();
    let mut n_sheets: Option<usize> = None;
    let mut sheet_names = String::new();
    let mut has_total: Option<bool> = None;
    let mut total_sheet = String::new();
    let mut has_brl: Option<bool> = None;
    let mut has_pct: Option<bool> = None;
    let mut charts_n: Option<usize> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("CHECK first_sheet=") {
            first_sheet = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("CHECK n_sheets=") {
            n_sheets = rest.parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("CHECK sheet_names=") {
            sheet_names = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("CHECK has_total=") {
            // Python imprime `True`/`False` (capitalizado);
            // Rust `bool::from_str` so aceita `true`/`false`.
            // Normaliza antes do parse.
            has_total = match rest.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("CHECK total_sheet=") {
            total_sheet = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("CHECK has_brl_format=") {
            has_brl = match rest.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("CHECK has_pct_format=") {
            has_pct = match rest.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("CHECK charts_sheet_count=") {
            charts_n = rest.parse::<usize>().ok();
        }
    }
    assert_eq!(
        first_sheet, "Painel",
        "esperado 1a sheet = 'Painel', veio '{first_sheet}'. Stdout: {stdout}"
    );
    assert_eq!(
        n_sheets,
        Some(3),
        "esperado 3 sheets (Painel + 2 Tables), veio {n_sheets:?}. Stdout: {stdout}"
    );
    assert_eq!(
        has_total,
        Some(true),
        "esperado has_total=true (linha de TOTAL presente), veio {has_total:?}. Stdout: {stdout}"
    );
    assert!(
        !total_sheet.is_empty(),
        "total_sheet nao pode ser vazio. Stdout: {stdout}"
    );
    assert_eq!(
        has_brl,
        Some(true),
        "esperado has_brl_format=true (currency aplicado), veio {has_brl:?}. Stdout: {stdout}"
    );
    assert_eq!(
        has_pct,
        Some(true),
        "esperado has_pct_format=true (percent aplicado), veio {has_pct:?}. Stdout: {stdout}"
    );
    assert_eq!(
        charts_n,
        Some(0),
        "esperado 0 sheets Charts_*, veio {charts_n:?}. Stdout: {stdout}"
    );
    // Sanity: as sheets esperadas existem (Painel + 2
    // sanitizadas dos titles das Tables).
    let has_receitas =
        sheet_names.contains("Receitas por Mes") || sheet_names.contains("ReceitasporMes");
    assert!(
        has_receitas,
        "sheet_names deve conter 'Receitas por Mes' ou versao sem espaco. Veio: {sheet_names}"
    );
}

// ---------------------------------------------------------------------------
// Teste
// ---------------------------------------------------------------------------

/// Fluxo vertical minimo: DocumentSpec Spreadsheet (Kpis
/// + Table com total + Table com percent + Chart) →
/// `docs.generate` (formato xlsx) → kit → dispatcher →
/// worker → .xlsx → reopen via `openpyxl` em subprocess →
/// validacao da estrutura (Painel 1a aba, 1 sheet por
/// Table, linha de TOTAL presente, formato de moeda
/// aplicado, formato percentual aplicado, NENHUMA sheet
/// Charts_<n>).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_generate_xlsx_full_vertical() {
    with_test_timeout_at("e2e_docs_generate_xlsx_full_vertical", E2E_TIMEOUT, async {
        let cfg = doc_worker_config();
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external deve succeed");

        // 1. KitRegistry com WordPro e ExcelPro
        //    registrados.
        let handle = Arc::new(handle);
        let wordpro = Arc::new(WordProKit::new(handle.clone()));
        let excelpro = Arc::new(ExcelProKit::new(handle.clone()));
        let mut registry = KitRegistry::new();
        registry.register(wordpro);
        registry.register(excelpro);
        let registry = Arc::new(registry);

        // 2. WorkerToolDispatcher. Allowlist vazia =
        //    sem validacao (o `output_path` vem do
        //    chamador no teste; em prod, o ToolRegistry
        //    popula com o workspace do usuario).
        let dispatcher = WorkerToolDispatcher::new((*handle).clone(), vec![]);
        let tool = DocsGenerateTool::new(registry, dispatcher);

        // 3. Spec DoD.
        let spec = spec_do_etapa_4_xlsx();
        let spec_json = serde_json::to_value(&spec).expect("spec serializa");

        // 4. Output path.
        let out_dir = temp_out_dir();
        std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
        let xlsx_path = out_dir.join("planilha_etapa_4.xlsx");

        // 5. Executa.
        let result = tool
            .execute(&json!({
                "spec": spec_json,
                "output_path": xlsx_path.to_string_lossy(),
                "format": "xlsx",
            }))
            .await;
        assert!(result.ok, "execute falhou: {:?}", result.error_message);
        assert_eq!(
            result.output.get("format").and_then(|v| v.as_str()),
            Some("xlsx")
        );
        assert!(
            xlsx_path.is_file(),
            ".xlsx nao foi criado em {}",
            xlsx_path.display()
        );
        let size = xlsx_path.metadata().unwrap().len();
        assert!(size > 1000, "xlsx muito pequeno: {size} bytes");

        // 6. O `sheets: [{block_index, sheet_name}]` no
        //    output deve listar 2 Tables (Chart SEM
        //    aba, Kpis vao pro Painel).
        let sheets = result
            .output
            .get("sheets")
            .and_then(|v| v.as_array())
            .expect("output deve ter sheets array");
        assert_eq!(
            sheets.len(),
            2,
            "esperado 2 sheets no mapping (Tables), veio {}. Sheets: {:?}",
            sheets.len(),
            sheets
        );
        // 1a Table (block_index 1) tem title
        // "Receitas por Mes" (sanitizado: espaco removido
        // pelo Excel? nao, " " nao e forbidden.
        // Vira "Receitas por Mes" mas com 14 chars < 31).
        // Verifico que tem o sheet_name esperado.
        let sheet_names: Vec<String> = sheets
            .iter()
            .filter_map(|s| {
                s.get("sheet_name")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .collect();
        assert!(
            sheet_names.iter().any(|n| n.contains("Receitas")),
            "esperado sheet com 'Receitas' no nome, veio: {sheet_names:?}"
        );
        assert!(
            sheet_names.iter().any(|n| n.contains("Crescimento")),
            "esperado sheet com 'Crescimento' no nome, veio: {sheet_names:?}"
        );

        // 7. Warnings: chart deve gerar warning (D2 do
        //    plano: degradacao declarada).
        let warnings = result
            .output
            .get("warnings")
            .and_then(|v| v.as_array())
            .expect("output deve ter warnings array");
        assert!(
            !warnings.is_empty(),
            "esperado ao menos 1 warning (chart), veio vazio"
        );
        let has_chart_warning = warnings
            .iter()
            .any(|w| w.as_str().map(|s| s.contains("chart")).unwrap_or(false));
        assert!(
            has_chart_warning,
            "esperado warning sobre chart, veio: {warnings:?}"
        );

        // 8. Reabre via `openpyxl` em subprocess e
        //    valida estrutura (D5 do plano: definicao
        //    de pronto do E2E do ExcelPro).
        let py = python_exe_or_panic();
        validate_xlsx_via_python(&py, &xlsx_path);

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_docs_generate_xlsx_full_vertical nao deve travar");
}
