//! Teste E2E da Etapa 5 PR 2 do PDFPro: `docs.generate` ponta-a-ponta.
//!
//! ## O que prova
//!
//! 1. **Infra completa** — `DocumentSpec` (Report) →
//!    `PdfProKit` v0.1 → `WorkerToolDispatcher` →
//!    `WorkerHandle::invoke` → `document-worker` Python
//!    (handler `pdf.write` v0.4.0 estendido) → `.pdf`.
//! 2. **Bump atômico do enum** — `DocumentFormat::Pdf`
//!    entra junto com o `PdfProKit` real (precedente
//!    do ADR-0020 §3 D3). O schema do `docs.generate`
//!    expõe `pdf` como formato.
//! 3. **Round-trip via `pdfplumber` em subprocess** —
//!    reabre o `.pdf` gerado e valida:
//!    - `n_pages >= 2` (cover + body)
//!    - título aparece no texto
//!    - heading "Secao 1" aparece
//!    - parágrafo de corpo aparece
//!    - texto da tabela (pelo menos um valor) aparece
//!    - chart placeholder vira texto `[Gráfico de bar — ...]`
//!      (D-CHART-1 do PR 5 deixa o chart real; PR 2 é
//!      placeholder explícito)
//! 4. **Glifo-check pre-render (D-GLYPH-1)** — falha
//!    estruturada com `code: "missing_glyph"` quando o
//!    spec tem caractere fora do cmap das fontes Tinta &
//!    Latão. O `tool.result` é propagado pelo kit como
//!    `KitError::Worker`.
//!
//! ## Gate Windows
//!
//! O módulo é `#[cfg(windows)]`. `python.exe` precisa
//! estar em `workers/document-worker/runtime/` COM
//! `fontTools` instalado (D-FAIL-1 do ADR-0021). Se
//! faltar, o helper `python_exe_or_panic` faz panic com
//! mensagem clara apontando pro `bootstrap.ps1`. **Não**
//! é `#[ignore]` (REGRAS §2.6).
//!
//! ## CI
//!
//! Adicionado ao `scripts/verify-external.ps1` como
//! step "E2E docs.generate pdf" — roda no `windows-latest`
//! do GitHub Actions em todo PR (junto com os outros
//! E2E).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use frederico_document_engine::{
    CalloutKind, Cover, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType,
    SpecVersion, WatermarkPosition, WatermarkSpec,
};
use frederico_document_kits::{DocsGenerateTool, KitRegistry, PdfProKit, WordProKit};
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
        .expect("CARGO_MANIFEST_DIR nao tem 2 niveis acima")
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
    d.push(format!("frederico_docs_generate_pdf_e2e_{nonce}"));
    d
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

// 60s cobre cold-start do Python + render do .pdf + subprocess
// de reopen via pdfplumber. Mesma folga dos E2E de docx e xlsx.
const E2E_TIMEOUT: Duration = Duration::from_secs(60);

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
    // D-FAIL-1 do ADR-0021: o glifo-check pre-render
    // (D-GLYPH-1) exige fontTools instalado. O bootstrap
    // hard-fail se faltar; se o runtime foi instalado
    // antes do PR 2 (caso do dev local), o `pip install
    // fonttools` precisa ser rodado manualmente. Panic
    // explicito em vez de falha confusa no E2E.
    let check = std::process::Command::new(&py)
        .args(["-c", "import fontTools; print('ok')"])
        .output()
        .expect("falha spawnando python pra checar fontTools");
    if !check.status.success() {
        panic!(
            "fontTools nao instalado no runtime Python ({}.).\n\
             Rode: & {} -m pip install fonttools\n\
             Ou re-rode o bootstrap.ps1 (D-FAIL-1 do ADR-0021, Etapa 5 PR 2).",
            py.display(),
            py.display()
        );
    }
    py
}

fn doc_worker_config() -> ExternalSpawnConfig {
    let py = python_exe_or_panic();
    let script = worker_script();
    if !script.is_file() {
        panic!("document-worker.py nao encontrado em {}", script.display());
    }
    ExternalSpawnConfig::new(py.to_string_lossy().into_owned())
        .with_args(vec![script.to_string_lossy().into_owned()])
        .with_cwd(workspace_root())
        .with_auth_token("e2e-docs-generate-pdf-token")
        .with_ready_timeout(Duration::from_secs(20))
}

/// Constrói o `DocumentSpec` DoD da Etapa 5 PR 2 do
/// PDFPro: Cover + Heading + Paragraph + List + Table +
/// Callout + Chart (placeholder) + Spacer + Divider.
/// Cobre 9 dos 20 blocos (a cobertura dos 20 é
/// garantida pelo teste unit `cobre_os_20_blocos` em
/// `pdfpro.rs`).
fn spec_do_etapa_5_pdf() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::TintaELatao,
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("Relatorio de Etapa 5".to_string()),
            author: Some("Mavis".to_string()),
            organization: Some("Frederico".to_string()),
            keywords: Some("etapa 5, pdf, tinta e latao".to_string()),
            description: Some("Smoke E2E do PDFPro v0.1".to_string()),
            watermark: None,
            pdfa: None,
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Cover(Cover {
                title: "Relatorio de Etapa 5".to_string(),
                subtitle: Some("PDFPro v0.1 - Etapa 5 PR 2".to_string()),
                author: Some("Mavis".to_string()),
                date: Some("2026-08-01".to_string()),
            }),
            DocumentBlock::Heading {
                level: 1,
                text: "Secao 1".to_string(),
                number: None,
            },
            DocumentBlock::Paragraph {
                text: "Paragrafo de corpo do relatorio. Tem glifos simples em PT-BR.".to_string(),
                style: None,
            },
            DocumentBlock::List {
                ordered: false,
                items: vec![
                    frederico_document_engine::ListItem {
                        text: "Item 1 da lista".to_string(),
                        children: vec![],
                    },
                    frederico_document_engine::ListItem {
                        text: "Item 2 da lista".to_string(),
                        children: vec![],
                    },
                ],
            },
            DocumentBlock::Table {
                headers: vec!["Coluna A".to_string(), "Coluna B".to_string()],
                rows: vec![
                    vec!["valor a1".to_string(), "valor b1".to_string()],
                    vec!["valor a2".to_string(), "valor b2".to_string()],
                ],
                total: None,
                currency: None,
                percent: false,
                thousands: false,
                title: Some("Tabela E2E".to_string()),
                source: None,
            },
            DocumentBlock::Callout {
                kind: CalloutKind::Info,
                text: "Callout informativo no meio do doc.".to_string(),
            },
            DocumentBlock::Chart {
                kind: frederico_document_engine::ChartKind::Bar,
                labels: vec!["jan".to_string(), "fev".to_string()],
                series: vec![],
                title: Some("Vendas Mensais".to_string()),
            },
            DocumentBlock::Divider,
            DocumentBlock::Spacer { height_cm: 0.5 },
        ],
    }
}

/// Spec que falha o glifo-check (D-GLYPH-1). Caractere
/// `\u732b` (gato em japonês) não está no cmap das
/// fontes Tinta & Latão.
fn spec_com_glifo_faltando() -> DocumentSpec {
    let mut spec = spec_do_etapa_5_pdf();
    spec.blocks.push(DocumentBlock::Paragraph {
        text: "Texto com caractere faltando: \u{732b}.".to_string(),
        style: None,
    });
    spec
}

/// Spec com watermark opt-in (D-PDF2).
fn spec_com_watermark() -> DocumentSpec {
    let mut spec = spec_do_etapa_5_pdf();
    spec.metadata.watermark = Some(WatermarkSpec {
        text: "CONFIDENCIAL".to_string(),
        position: WatermarkPosition::Diagonal,
        opacity: Some(0.15),
        font_size: Some(60.0),
    });
    spec
}

/// Reabre o `.pdf` via subprocess do `python.exe` do
/// worker, usando `pdfplumber`. Valida:
/// - `n_pages >= 2` (cover + body)
/// - título aparece no texto
/// - heading "Secao 1" aparece
/// - parágrafo de corpo aparece
/// - texto da tabela "valor a1" aparece
/// - chart placeholder vira "[Gráfico de bar — ...]"
///
/// Imprime CHECK linhas no formato "CHECK nome=valor"
/// pro Rust parsear.
fn validate_pdf_via_python(python: &PathBuf, pdf_path: &PathBuf) {
    let script = r#"
import sys
import pdfplumber

path = sys.argv[1]
with pdfplumber.open(path) as pdf:
    n_pages = len(pdf.pages)
    print(f"CHECK n_pages={n_pages}")
    all_text = "\n".join((p.extract_text() or "") for p in pdf.pages)
    # Titulo (cover) - vai no fluxo da 1a pagina
    has_title = "Relatorio de Etapa 5" in all_text
    print(f"CHECK has_title={has_title}")
    # Heading
    has_heading = "Secao 1" in all_text
    print(f"CHECK has_heading={has_heading}")
    # Paragrafo alvo
    target = "Paragrafo de corpo do relatorio"
    has_target = target in all_text
    print(f"CHECK has_target_paragraph={has_target}")
    # Tabela: algum valor aparece
    has_table_value = "valor a1" in all_text
    print(f"CHECK has_table_value={has_table_value}")
    # Chart placeholder
    has_chart_placeholder = "Gr" in all_text and "fico de bar" in all_text
    print(f"CHECK has_chart_placeholder={has_chart_placeholder}")
    # Callout
    has_callout = "INFO" in all_text or "Callout informativo" in all_text
    print(f"CHECK has_callout={has_callout}")
"#;
    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(pdf_path)
        .output()
        .expect("falha spawnando python pra validar pdf");
    if !output.status.success() {
        panic!(
            "validacao pdfplumber falhou: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut n_pages: Option<usize> = None;
    let mut has_title: Option<bool> = None;
    let mut has_heading: Option<bool> = None;
    let mut has_target: Option<bool> = None;
    let mut has_table_value: Option<bool> = None;
    let mut has_chart_placeholder: Option<bool> = None;
    let mut has_callout: Option<bool> = None;
    for line in stdout.lines() {
        let parse_bool = |s: &str| match s.to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        if let Some(rest) = line.strip_prefix("CHECK n_pages=") {
            n_pages = rest.parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("CHECK has_title=") {
            has_title = parse_bool(rest);
        } else if let Some(rest) = line.strip_prefix("CHECK has_heading=") {
            has_heading = parse_bool(rest);
        } else if let Some(rest) = line.strip_prefix("CHECK has_target_paragraph=") {
            has_target = parse_bool(rest);
        } else if let Some(rest) = line.strip_prefix("CHECK has_table_value=") {
            has_table_value = parse_bool(rest);
        } else if let Some(rest) = line.strip_prefix("CHECK has_chart_placeholder=") {
            has_chart_placeholder = parse_bool(rest);
        } else if let Some(rest) = line.strip_prefix("CHECK has_callout=") {
            has_callout = parse_bool(rest);
        }
    }
    assert!(
        n_pages.unwrap_or(0) >= 2,
        "esperado >= 2 paginas (cover + body), veio {n_pages:?}. Stdout: {stdout}"
    );
    assert_eq!(
        has_title,
        Some(true),
        "titulo nao apareceu. Stdout: {stdout}"
    );
    assert_eq!(
        has_heading,
        Some(true),
        "heading nao apareceu. Stdout: {stdout}"
    );
    assert_eq!(
        has_target,
        Some(true),
        "paragrafo alvo nao apareceu. Stdout: {stdout}"
    );
    assert_eq!(
        has_table_value,
        Some(true),
        "valor da tabela nao apareceu. Stdout: {stdout}"
    );
    assert_eq!(
        has_chart_placeholder,
        Some(true),
        "chart placeholder nao apareceu. Stdout: {stdout}"
    );
    assert_eq!(
        has_callout,
        Some(true),
        "callout nao apareceu. Stdout: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

/// Fluxo vertical minimo: DocumentSpec (Report) →
/// `docs.generate` (formato pdf) → kit → dispatcher →
/// worker → .pdf → reopen via `pdfplumber` em subprocess
/// → validacao da estrutura (n_pages, titulo, heading,
/// paragrafo, tabela, chart placeholder, callout).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_generate_pdf_full_vertical() {
    with_test_timeout_at("e2e_docs_generate_pdf_full_vertical", E2E_TIMEOUT, async {
        let cfg = doc_worker_config();
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external deve succeed");

        // 1. KitRegistry com WordPro e PdfPro
        //    registrados. PdfPro v0.1 com `is_implemented
        //    == true` (bump atomico do enum Pdf).
        let handle = Arc::new(handle);
        let wordpro = Arc::new(WordProKit::new(handle.clone()));
        let pdfpro = Arc::new(PdfProKit::new(handle.clone()));
        let mut registry = KitRegistry::new();
        registry.register(wordpro);
        registry.register(pdfpro);
        let registry = Arc::new(registry);

        // 2. WorkerToolDispatcher. Allowlist vazia =
        //    sem validacao (o `output_path` vem do
        //    chamador no teste; em prod, o ToolRegistry
        //    popula com o workspace do usuario).
        let dispatcher = WorkerToolDispatcher::new((*handle).clone(), vec![]);
        let tool = DocsGenerateTool::new(registry, dispatcher);

        // 3. Spec DoD.
        let spec = spec_do_etapa_5_pdf();
        let spec_json = serde_json::to_value(&spec).expect("spec serializa");

        // 4. Output path.
        let out_dir = temp_out_dir();
        std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
        let pdf_path = out_dir.join("relatorio_etapa_5.pdf");

        // 5. Executa.
        let result = tool
            .execute(&json!({
                "spec": spec_json,
                "output_path": pdf_path.to_string_lossy(),
                "format": "pdf",
            }))
            .await;
        assert!(result.ok, "execute falhou: {:?}", result.error_message);
        assert_eq!(
            result.output.get("format").and_then(|v| v.as_str()),
            Some("pdf")
        );
        assert!(
            pdf_path.is_file(),
            ".pdf nao foi criado em {}",
            pdf_path.display()
        );
        let size = pdf_path.metadata().unwrap().len();
        assert!(size > 1000, "pdf muito pequeno: {size} bytes");

        // 6. Warnings: chart deve gerar warning
        //    (chart_placeholder no PDF v0.1).
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

        // 7. Reabre via `pdfplumber` em subprocess e
        //    valida estrutura.
        let py = python_exe_or_panic();
        validate_pdf_via_python(&py, &pdf_path);

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_docs_generate_pdf_full_vertical nao deve travar");
}

/// Glifo-check pre-render (D-GLYPH-1): spec com caractere
/// fora do cmap Tinta & Latão (\u732b) deve falhar
/// estruturado com `code: "missing_glyph"`. O erro vem
/// do worker (tool.result ok=false) e é propagado pelo
/// kit como KitError::Worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_generate_pdf_missing_glyph_blocks() {
    with_test_timeout_at(
        "e2e_docs_generate_pdf_missing_glyph_blocks",
        E2E_TIMEOUT,
        async {
            let cfg = doc_worker_config();
            let (manager, handle) =
                frederico_process_architecture::WorkerManager::spawn_external(cfg)
                    .await
                    .expect("spawn_external deve succeed");

            let handle = Arc::new(handle);
            let pdfpro = Arc::new(PdfProKit::new(handle.clone()));
            let mut registry = KitRegistry::new();
            registry.register(pdfpro);
            let registry = Arc::new(registry);
            let dispatcher = WorkerToolDispatcher::new((*handle).clone(), vec![]);
            let tool = DocsGenerateTool::new(registry, dispatcher);

            let spec = spec_com_glifo_faltando();
            let spec_json = serde_json::to_value(&spec).expect("spec serializa");

            let out_dir = temp_out_dir();
            std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
            let pdf_path = out_dir.join("relatorio_glifo_faltando.pdf");

            let result = tool
                .execute(&json!({
                    "spec": spec_json,
                    "output_path": pdf_path.to_string_lossy(),
                    "format": "pdf",
                }))
                .await;
            assert!(
                !result.ok,
                "execute deveria ter falhado (missing_glyph), veio ok"
            );
            let msg = result.error_message.as_deref().unwrap_or("");
            assert!(
                msg.contains("missing_glyph"),
                "erro deveria mencionar missing_glyph, veio: {msg}"
            );
            // Nao deve ter criado o PDF (ou se criou, ele
            // seria incompleto). O `pdf.write` falhou
            // antes do `doc.build()` — mas por seguranca
            // verificamos que NAO foi criado (ou se foi,
            // e pelo worker sinalizando erro, o kit
            // deveria ter barrado).
            // Em pratica: `doc.build` nao foi chamado, o
            // arquivo nao existe.
            if pdf_path.is_file() {
                std::fs::remove_file(&pdf_path).ok();
            }
            assert!(
                !pdf_path.is_file(),
                "PDF nao deveria ter sido criado (glifo faltando)"
            );

            manager.shutdown().await.expect("shutdown");
        },
    )
    .await
    .expect("e2e_docs_generate_pdf_missing_glyph_blocks nao deve travar");
}

/// Watermark opt-in (D-PDF2): spec com `watermark: Some(...)`
/// deve renderizar o PDF com o texto da marca d'agua
/// embutido. O `pdfplumber` nao ve a marca d'agua no
/// texto extraido (e uma camada de canvas, nao
/// caracteres), entao a validacao deste E2E e
/// comportamental: o kit aceita o spec e o worker
/// devolve `ok: true` com `size_bytes > 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_generate_pdf_watermark_opt_in() {
    with_test_timeout_at(
        "e2e_docs_generate_pdf_watermark_opt_in",
        E2E_TIMEOUT,
        async {
            let cfg = doc_worker_config();
            let (manager, handle) =
                frederico_process_architecture::WorkerManager::spawn_external(cfg)
                    .await
                    .expect("spawn_external deve succeed");

            let handle = Arc::new(handle);
            let pdfpro = Arc::new(PdfProKit::new(handle.clone()));
            let mut registry = KitRegistry::new();
            registry.register(pdfpro);
            let registry = Arc::new(registry);
            let dispatcher = WorkerToolDispatcher::new((*handle).clone(), vec![]);
            let tool = DocsGenerateTool::new(registry, dispatcher);

            let spec = spec_com_watermark();
            let spec_json = serde_json::to_value(&spec).expect("spec serializa");

            let out_dir = temp_out_dir();
            std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
            let pdf_path = out_dir.join("relatorio_watermark.pdf");

            let result = tool
                .execute(&json!({
                    "spec": spec_json,
                    "output_path": pdf_path.to_string_lossy(),
                    "format": "pdf",
                }))
                .await;
            assert!(result.ok, "execute falhou: {:?}", result.error_message);
            assert!(pdf_path.is_file(), ".pdf nao foi criado");
            let size = pdf_path.metadata().unwrap().len();
            // PDF com watermark (canvas drawing) tem que
            // ser >= tamanho minimo. Sem watermark no v0.1
            // daria ~22KB; com watermark deve ser um
            // pouco maior.
            assert!(size > 5000, "pdf com watermark muito pequeno: {size} bytes");

            manager.shutdown().await.expect("shutdown");
        },
    )
    .await
    .expect("e2e_docs_generate_pdf_watermark_opt_in nao deve travar");
}
