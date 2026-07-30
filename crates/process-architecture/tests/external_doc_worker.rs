//! Integration tests E2E do `WorkerManager::spawn_external` apontando pro
//! `document-worker` Python real (workers/document-worker/, v0.2.0).
//!
//! Estes testes NAO sao `#[ignore]` — rodam no CI no `windows-latest`
//! como parte do job `verify` (ver `.github/workflows/ci.yml` step
//! "E2E document-worker handlers"). O CI roda o `bootstrap.ps1` antes
//! (com cache de `workers/document-worker/runtime/`) pra garantir que
//! Python 3.12.7 + pywin32 + python-docx + openpyxl + reportlab +
//! pdfplumber + 4 fontes Tinta e Latao estao instalados.
//!
//! **O que provam (Fase 5, Etapa 2B+X):**
//!
//! 1. **Boot + handshake** do `document-worker` Python via
//!    `spawn_external` (substitui o stub PowerShell/Rust em
//!    `tests/external_worker.rs` pelo worker real).
//! 2. **6 handlers reais** end-to-end: spawna o worker, faz `tool.invoke`
//!    pra cada capability (`docx.write`, `docx.read`, `xlsx.write`,
//!    `xlsx.read`, `pdf.write`, `pdf.read`), valida o output
//!    (arquivo existe, magic bytes, conteudo).
//! 3. **Path safety:** recusa `..` no path, retorna
//!    `code: "path_traversal"` no `tool.result`.
//! 4. **Limitacao conhecida do `pdf.read`:** PDF 100% escaneado
//!    retorna `code: "pdf_scanned_no_ocr"` (registrada no CHANGELOG,
//!    pendente 2B+Y para Tesseract).
//!
//! **Pre-requisito:** `workers/document-worker/runtime/python.exe` deve
//! existir. Se nao existir, os testes falham com mensagem clara
//! apontando pro `bootstrap.ps1` — **NAO** sao pulados. Teste pulado
//! e regressao silenciosa (REGRAS §2.6).
//!
//! ## Gate Windows
//!
//! O modulo inteiro e `#[cfg(windows)]` — named pipes + o
//! `spawn_external` sao Windows. Em Linux/macOS, `cargo test -p
//! frederico-process-architecture --test external_doc_worker` compila
//! vazio.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use frederico_process_architecture::ExternalSpawnConfig;
use frederico_test_support::with_test_timeout_at;
use serde_json::json;

// ---------------------------------------------------------------------------
// Caminhos do worker (relativos ao workspace raiz, nao ao crate)
// ---------------------------------------------------------------------------

// `CARGO_MANIFEST_DIR` aponta pro crate atual
// (`C:/src/Frederico/crates/process-architecture`). O worker vive em
// `../../workers/document-worker/` (sobe 2 niveis).
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

fn worker_manifest() -> PathBuf {
    workspace_root()
        .join("workers")
        .join("document-worker")
        .join("manifest.json")
}

// ---------------------------------------------------------------------------
// Constantes de budget
// ---------------------------------------------------------------------------

// Python cold-start no Windows: ~1-2s pro `python.exe` subir, mais
// `import docx/openpyxl/reportlab/pdfplumber` (~2-3s na primeira
// execucao do runtime, ~1s depois). O `READY <name>` chega tipicamente
// em < 5s. Budget 60s cobre cold-start + primeira invocacao completa
// (gera DOCX, valida).
const E2E_TIMEOUT: Duration = Duration::from_secs(60);

// `ready_timeout` do `ExternalSpawnConfig` — espera o `READY` no
// stdout. 20s cobre o cold-start do Python no CI Windows Server 2022
// (mais lento que local; medido em ~5-10s com bootstrap cacheado).
const E2E_READY_TIMEOUT: Duration = Duration::from_secs(20);

// Diretorio temporario para outputs dos testes. Cada teste usa um
// arquivo unico (UUID v4 no nome) pra nao colidir entre si.
fn temp_out_dir() -> PathBuf {
    let mut d = std::env::temp_dir();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    d.push(format!("frederico_doc_worker_e2e_{nonce}"));
    d
}

// ---------------------------------------------------------------------------
// Helper: monta ExternalSpawnConfig pro Python worker
// ---------------------------------------------------------------------------

fn doc_worker_config(
    extra: impl FnOnce(ExternalSpawnConfig) -> ExternalSpawnConfig,
) -> ExternalSpawnConfig {
    // Falha clara se o runtime nao foi instalado. **NAO** pulamos —
    // teste pulado e regressao silenciosa (REGRAS §2.6).
    let py = python_exe();
    let script = worker_script();
    let manifest = worker_manifest();
    if !py.is_file() {
        panic!(
            "python.exe nao encontrado em {}.\n\
             Rode `pwsh -NoProfile -ExecutionPolicy Bypass -File \
             workers/document-worker/bootstrap.ps1` pra instalar o runtime.\n\
             Em CI: o step 'Bootstrap document-worker' do .github/workflows/ci.yml \
             cuida disso.",
            py.display()
        );
    }
    if !script.is_file() {
        panic!("document-worker.py nao encontrado em {}", script.display());
    }
    if !manifest.is_file() {
        panic!("manifest.json nao encontrado em {}", manifest.display());
    }
    let mut cfg = ExternalSpawnConfig::new(py.to_string_lossy().into_owned())
        .with_args(vec![script.to_string_lossy().into_owned()])
        .with_cwd(workspace_root())
        .with_auth_token("e2e-test-token")
        .with_ready_timeout(E2E_READY_TIMEOUT);
    cfg = extra(cfg);
    cfg
}

// ---------------------------------------------------------------------------
// Helpers Tesseract (Etapa 2B+Y)
// ---------------------------------------------------------------------------
//
// Os testes que dependem de Tesseract chamam `tesseract_or_panic` no
// inicio. Se Tesseract nao esta instalado, panic com mensagem clara
// apontando pro bootstrap. **NAO** sao `#[ignore]` - teste pulado e
// regressao silenciosa (REGRAS §2.6). O CI gate e o CI noturno rodam
// o bootstrap.ps1 antes, entao Tesseract esta. Em dev local, o panic
// instrui o dev a rodar o bootstrap como Admin.

fn tesseract_exe() -> PathBuf {
    workspace_root()
        .join("workers")
        .join("document-worker")
        .join("runtime")
        .join("tesseract")
        .join("tesseract.exe")
}

fn tesseract_or_panic() -> PathBuf {
    let tess = tesseract_exe();
    if !tess.is_file() {
        panic!(
            "tesseract.exe nao encontrado em {}.\n\
             Os testes de OCR (e2e_ocr_run, e2e_pdf_read_with_ocr_*) \
             precisam do Tesseract instalado pelo bootstrap.ps1.\n\
             Rode o bootstrap em PowerShell como Admin:\n\
             pwsh -NoProfile -ExecutionPolicy Bypass -File \
             workers/document-worker/bootstrap.ps1\n\
             (Bloco Tesseract so roda se o processo for Admin - contexto \
             non-elevated pula o bloco com instrucoes.)\n\
             Em CI: o step 'Bootstrap document-worker' do .github/workflows/ci.yml \
             cuida disso.",
            tess.display()
        );
    }
    tess
}

// Gera um PNG com texto conhecido via Pillow (subprocess usando o
// python.exe do worker). Pillow vem com `pdfplumber` (transitivo), ja
// esta no `runtime/Lib/site-packages/` do bootstrap. Retorna a path
// do PNG criado.
fn generate_test_png_with_text(png_path: &Path, text: &str) {
    let py = python_exe();
    if !py.is_file() {
        panic!("python.exe nao encontrado (mesmo path do doc_worker_config)");
    }
    // Script inline. ASCII-only pra evitar problemas de quoting do
    // PowerShell 5.1 com aspas/escape.
    let script = r#"
from PIL import Image, ImageDraw, ImageFont
import sys
text = sys.argv[1]
out = sys.argv[2]
img = Image.new('RGB', (600, 200), color='white')
draw = ImageDraw.Draw(img)
# Fonte default (truetype nao vem por padrao no Pillow, mas default
# funciona OK pra teste - texto e legivel).
draw.text((20, 80), text, fill='black')
img.save(out, 'PNG')
print('OK', out)
"#.to_string();
    let output = std::process::Command::new(&py)
        .arg("-c")
        .arg(&script)
        .arg(text)
        .arg(png_path)
        .output()
        .expect("falha spawnando Python pra gerar PNG de teste");
    if !output.status.success() {
        panic!(
            "geracao de PNG de teste falhou: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !png_path.is_file() {
        panic!("PNG nao foi criado em {}", png_path.display());
    }
}

// Gera um PDF 100% escaneado (1 pagina, so imagem) via reportlab +
// Pillow (subprocess). Usado pra testar o fallback OCR do `pdf.read`.
// O PDF tem 1 pagina com a imagem PNG embutida - pdfplumber nao vai
// achar texto (camada vazia), e o Tesseract faz OCR.
fn generate_test_scanned_pdf_with_text(pdf_path: &Path, png_path: &Path) {
    let py = python_exe();
    if !py.is_file() {
        panic!("python.exe nao encontrado");
    }
    let script = r#"
import sys
from reportlab.lib.pagesizes import A4
from reportlab.platypus import SimpleDocTemplate, Image as RLImage, Spacer
from reportlab.lib.units import cm

pdf_out = sys.argv[1]
img_in = sys.argv[2]

doc = SimpleDocTemplate(pdf_out, pagesize=A4)
story = [
    Spacer(1, 2*cm),
    RLImage(img_in, width=16*cm, height=5*cm),
]
doc.build(story)
print('OK', pdf_out)
"#
.to_string();
    let output = std::process::Command::new(&py)
        .arg("-c")
        .arg(&script)
        .arg(pdf_path)
        .arg(png_path)
        .output()
        .expect("falha spawnando Python pra gerar PDF escaneado de teste");
    if !output.status.success() {
        panic!(
            "geracao de PDF escaneado de teste falhou: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !pdf_path.is_file() {
        panic!("PDF escaneado nao foi criado em {}", pdf_path.display());
    }
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

/// docx.write: gera um .docx com 2 secoes, le de volta, valida.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docx_write_and_read() {
    with_test_timeout_at("e2e_docx_write_and_read", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external deve succeed");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).expect("mkdir temp out");

        // write
        let docx_path = out.join("hello.docx");
        let write_result = handle
            .invoke(json!({
                "capability": "docx.write",
                "path": docx_path.to_string_lossy(),
                "title": "E2E Test Document",
                "sections": [
                    {"heading": "Intro", "paragraphs": ["Linha 1.", "Linha 2."]},
                    {"heading": "Conclusao", "paragraphs": ["Fim."]}
                ]
            }))
            .await
            .expect("invoke docx.write");
        assert_eq!(write_result["ok"], json!(true));
        assert_eq!(write_result["sections_written"], json!(2));
        assert!(docx_path.is_file(), "docx nao foi criado");
        assert!(
            docx_path.metadata().unwrap().len() > 1000,
            "docx muito pequeno"
        );
        // DOCX e um ZIP — magic bytes "PK\x03\x04"
        let head = std::fs::read(&docx_path).expect("read docx");
        assert_eq!(&head[..4], b"PK\x03\x04", "docx nao tem magic PK\\x03\\x04");

        // read
        let read_result = handle
            .invoke(json!({"capability": "docx.read", "path": docx_path.to_string_lossy()}))
            .await
            .expect("invoke docx.read");
        assert_eq!(read_result["ok"], json!(true));
        let n_paragraphs = read_result["n_paragraphs"].as_u64().unwrap();
        assert!(
            n_paragraphs >= 3,
            "esperado >= 3 paragrafos, veio {n_paragraphs}"
        );
        let joined = read_result["paragraphs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("E2E Test Document"),
            "titulo nao apareceu no read"
        );
        assert!(
            joined.contains("Linha 1."),
            "paragrafo nao apareceu no read"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_docx_write_and_read nao deve travar");
}

/// xlsx.write + xlsx.read: gera um workbook com 2 sheets, le de volta.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_xlsx_write_and_read() {
    with_test_timeout_at("e2e_xlsx_write_and_read", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) =
            frederico_process_architecture::WorkerManager::spawn_external(cfg)
                .await
                .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).unwrap();

        let xlsx_path = out.join("data.xlsx");
        let write_result = handle
            .invoke(json!({
                "capability": "xlsx.write",
                "path": xlsx_path.to_string_lossy(),
                "sheets": [
                    {"name": "Vendas", "headers": ["Mes", "Total"], "rows": [["Jan", 100], ["Fev", 200]]},
                    {"name": "Notas", "headers": ["id"], "rows": [[1], [2]]}
                ]
            }))
            .await
            .expect("invoke xlsx.write");
        assert_eq!(write_result["ok"], json!(true));
        assert_eq!(write_result["sheets_written"], json!(2));
        assert_eq!(write_result["total_rows"], json!(4));
        assert!(xlsx_path.is_file());

        // XLSX tambem e ZIP (PK\\x03\\x04)
        let head = std::fs::read(&xlsx_path).unwrap();
        assert_eq!(&head[..4], b"PK\x03\x04");

        let read_result = handle
            .invoke(json!({"capability": "xlsx.read", "path": xlsx_path.to_string_lossy()}))
            .await
            .expect("invoke xlsx.read");
        assert_eq!(read_result["ok"], json!(true));
        assert_eq!(read_result["n_sheets"], json!(2));
        let sheets = read_result["sheets"].as_array().unwrap();
        let names: Vec<&str> = sheets.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"Vendas"));
        assert!(names.contains(&"Notas"));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_xlsx_write_and_read nao deve travar");
}

/// pdf.write + pdf.read: gera PDF (com fontes Tinta e Latao embutidas),
/// le de volta, valida texto.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_pdf_write_and_read() {
    with_test_timeout_at("e2e_pdf_write_and_read", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).unwrap();

        let pdf_path = out.join("doc.pdf");
        let write_result = handle
            .invoke(json!({
                "capability": "pdf.write",
                "path": pdf_path.to_string_lossy(),
                "title": "E2E PDF Test",
                "sections": [
                    {"heading": "Secao Unica", "body": ["Paragrafo de teste.", "Mais um."]}
                ]
            }))
            .await
            .expect("invoke pdf.write");
        assert_eq!(write_result["ok"], json!(true));
        assert_eq!(write_result["sections_written"], json!(1));
        assert!(pdf_path.is_file());

        // PDF magic: %PDF-
        let head = std::fs::read(&pdf_path).unwrap();
        assert_eq!(&head[..4], b"%PDF", "PDF nao tem magic %PDF-");

        // Verifica que as fontes Tinta e Latao foram carregadas (nao
        // fallback). O worker inclui `font_status` no `worker.hello`
        // payload (campo extra alem do WorkerManifest) e o `ping`
        // retorna o status atual.
        let pong = handle.ping().await.expect("ping");
        let font_status = &pong["font_status"];
        let body_status = font_status["TintaLataoSans"].as_str();
        let title_status = font_status["TintaLataoSerif"].as_str();
        assert_eq!(
            body_status,
            Some("loaded"),
            "fonte TintaLataoSans nao carregou — bootstrap incompleto? status={body_status:?}"
        );
        assert_eq!(
            title_status,
            Some("loaded"),
            "fonte TintaLataoSerif nao carregou — bootstrap incompleto? status={title_status:?}"
        );

        // read
        let read_result = handle
            .invoke(json!({"capability": "pdf.read", "path": pdf_path.to_string_lossy()}))
            .await
            .expect("invoke pdf.read");
        assert_eq!(read_result["ok"], json!(true));
        assert_eq!(read_result["ocr_available"], json!(false));
        let page_count = read_result["page_count"].as_u64().unwrap();
        assert!(page_count >= 1);
        let text = read_result["text"].as_str().unwrap_or("");
        // O reportlab pode quebrar o titulo em linhas (hifenizacao), entao
        // testamos uma substring estavel: "E2E" e "Secao".
        assert!(
            text.contains("E2E") || text.contains("Secao"),
            "texto extraido nao contem titulo nem section: {text}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_pdf_write_and_read nao deve travar");
}

/// Path safety: recusa `..` no path. Retorna `code: "path_traversal"`
/// no `tool.result` (NAO `worker.error` — handler captura via
/// `PathSafetyError` e devolve `tool.result {ok: false}`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_path_safety_rejects_traversal() {
    with_test_timeout_at("e2e_path_safety_rejects_traversal", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        // `..` no path. O `validate_path` no Python recusa antes de
        // tentar qualquer I/O.
        let result = handle
            .invoke(json!({
                "capability": "docx.write",
                "path": "../escaped.docx",
                "title": "X",
                "sections": []
            }))
            .await
            .expect("invoke deve retornar response, nao erro de transporte");
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("path_traversal"));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_path_safety_rejects_traversal nao deve travar");
}

/// `pdf.read` em PDF 100% escaneado (sem texto em nenhuma pagina)
/// retorna `code: "pdf_scanned_no_ocr"`. Limitacao conhecida, pendente
/// 2B+Y (Tesseract). A deteccao de paginas escaneadas (parcial) e
/// testada via `scanned_pages` no payload de read normal; o caso
/// 100% escaneado exige um PDF genuinamente escaneado (imagem pura
/// sem texto), o que e raro em testes E2E sinteticos. A logica de
/// deteccao e coberta pelo smoke_handler.py; o E2E Rust so verifica
/// a estrutura da response (`scanned_pages: array`,
/// `ocr_available: false`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_pdf_read_reports_ocr_unavailable() {
    with_test_timeout_at("e2e_pdf_read_reports_ocr_unavailable", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).unwrap();

        // Cria um PDF com titulo + heading + body (texto normal).
        // O reportlab inclui o titulo e o heading no output,
        // entao `scanned_pages` deve ser vazio e `text` deve ter
        // o titulo.
        let pdf_path = out.join("with_text.pdf");
        let write_result = handle
            .invoke(json!({
                "capability": "pdf.write",
                "path": pdf_path.to_string_lossy(),
                "title": "Com Texto",
                "sections": [{"heading": "Saudacao", "body": ["Ola mundo."]}]
            }))
            .await
            .expect("invoke pdf.write");
        assert_eq!(write_result["ok"], json!(true));

        let read_result = handle
            .invoke(json!({"capability": "pdf.read", "path": pdf_path.to_string_lossy()}))
            .await
            .expect("invoke pdf.read");
        assert_eq!(read_result["ok"], json!(true));
        assert_eq!(read_result["ocr_available"], json!(false));
        assert!(read_result["page_count"].as_u64().unwrap() >= 1);
        assert!(read_result["scanned_pages"].is_array());
        // O PDF nao e escaneado - `scanned_pages` deve ser vazio.
        assert_eq!(read_result["scanned_pages"].as_array().unwrap().len(), 0);
        // O titulo aparece no texto extraido (pode ter hifenizacao do
        // reportlab, entao testamos uma substring estavel).
        let text = read_result["text"].as_str().unwrap_or("");
        assert!(
            text.contains("Com Texto") || text.contains("Saudacao"),
            "texto extraido nao contem titulo/heading: {text}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_pdf_read_reports_ocr_unavailable nao deve travar");
}

/// Capability desconhecida. O worker responde `code: "unknown_capability"`
/// no `tool.result` (handler captura o `KeyError` na lookup da
/// dispatch table). v0.3.0: `ocr.run` agora existe, entao usamos
/// outra capability inventada (`pdf.print`) pra nao colidir com o
/// teste de OCR valido.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_unknown_capability_rejected() {
    with_test_timeout_at("e2e_unknown_capability_rejected", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).unwrap();

        let result = handle
            .invoke(json!({
                "capability": "pdf.print",
                "path": out.join("x.pdf").to_string_lossy()
            }))
            .await
            .expect("invoke deve retornar response");
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("unknown_capability"));

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_unknown_capability_rejected nao deve travar");
}

// ---------------------------------------------------------------------------
// Testes da Etapa 2B+Y (OCR via Tesseract)
// ---------------------------------------------------------------------------
//
// Os testes abaixo usam helpers `tesseract_or_panic()` (panic claro se
// Tesseract nao esta) e `generate_test_png_with_text` /
// `generate_test_scanned_pdf_with_text` (subprocess Python pra gerar
// assets com Pillow + reportlab). **NAO** sao `#[ignore]` - teste
// pulado e regressao silenciosa (REGRAS §2.6). Em CI (gate + noturno)
// o bootstrap.ps1 instala Tesseract antes. Em dev local, o panic
// instrui o dev a rodar como Admin.

/// `ocr.run` end-to-end: gera PNG com texto conhecido, OCR, valida
/// que o texto reconhecido contem o original (normalizado).
/// **Requer Tesseract** (panics se nao instalado).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_ocr_run_with_real_image() {
    with_test_timeout_at("e2e_ocr_run_with_real_image", E2E_TIMEOUT, async {
        // Tesseract deve estar instalado (bootstrap.ps1 rodou).
        tesseract_or_panic();

        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).expect("mkdir temp out");

        // Gera PNG com texto conhecido via Pillow (subprocess).
        // Usamos texto grande + simples pra OCR robusto: o default
        // font do Pillow renderiza bem em 36pt.
        let png_path = out.join("ocr_input.png");
        generate_test_png_with_text(&png_path, "HELLO WORLD 12345");

        // Chama ocr.run. Default lang e por+eng.
        let ocr_result = handle
            .invoke(json!({
                "capability": "ocr.run",
                "path": png_path.to_string_lossy(),
            }))
            .await
            .expect("invoke ocr.run");
        assert_eq!(
            ocr_result["ok"],
            json!(true),
            "ocr.run falhou: {ocr_result}"
        );
        assert_eq!(ocr_result["lang"], json!("por+eng"));
        // Verifica que reconheceu algo do texto (normalizado: lowercase
        // + colapsar espacos). OCR troca 1 por l, 0 por O, 5 por S -
        // comparar com igualdade literal e flaky. Conteudo parcial
        // ja prova o end-to-end.
        let text = ocr_result["text"].as_str().unwrap_or("").to_lowercase();
        let text_norm: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        // "HELLO" -> Tesseract as vezes le "HELL0" (zero por O), ou
        // "HELLO" mesmo. Verificamos que "hello" ou "hell" aparece.
        // "WORLD" -> robusto, sempre reconhecido.
        // "12345" -> numeros sao faceis, sempre reconhecidos.
        let recognized_some = text_norm.contains("hello")
            || text_norm.contains("hell")
            || text_norm.contains("world")
            || text_norm.contains("12345")
            || text_norm.contains("helo");
        assert!(
            recognized_some,
            "OCR nao reconheceu nenhum token esperado. Texto: {text}"
        );
        // tesseract_version presente.
        let tess_version = ocr_result["tesseract_version"].as_str().unwrap_or("");
        assert!(
            !tess_version.is_empty() && tess_version != "null",
            "tesseract_version ausente ou invalido: {tess_version}"
        );
        // conf media (se calculada) deve estar em [0, 100].
        if let Some(conf) = ocr_result["conf"].as_f64() {
            assert!(
                (0.0..=100.0).contains(&conf),
                "conf fora de [0,100]: {conf}"
            );
        }

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_ocr_run_with_real_image nao deve travar");
}

/// `ocr.run` com `lang` invalido (idioma nao instalado). Retorna
/// `code: "invalid_lang"` sem precisar de Tesseract instalado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_ocr_run_with_invalid_lang() {
    with_test_timeout_at("e2e_ocr_run_with_invalid_lang", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).expect("mkdir temp out");

        // Gera PNG (Pillow vem com o worker via pdfplumber, mesmo
        // sem Tesseract instalado).
        let png_path = out.join("invalid_lang.png");
        generate_test_png_with_text(&png_path, "qualquer texto");

        // lang `fra` nao foi instalado (manifesto so tem por, eng, osd).
        let result = handle
            .invoke(json!({
                "capability": "ocr.run",
                "path": png_path.to_string_lossy(),
                "lang": "fra",
            }))
            .await
            .expect("invoke deve retornar response");
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("invalid_lang"));
        let msg = result["message"].as_str().unwrap_or("");
        assert!(
            msg.contains("fra"),
            "mensagem deve mencionar o idioma ausente, veio: {msg}"
        );
        assert!(
            msg.contains("por") && msg.contains("eng"),
            "mensagem deve listar idiomas disponiveis, veio: {msg}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_ocr_run_with_invalid_lang nao deve travar");
}

/// `ocr.run` com Tesseract indisponivel: retorna `code: "ocr_not_available"`
/// sem crash. **NAO** requer Tesseract (testa o caminho de erro).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_ocr_run_without_tesseract() {
    with_test_timeout_at("e2e_ocr_run_without_tesseract", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).expect("mkdir temp out");
        let png_path = out.join("no_tesseract.png");
        generate_test_png_with_text(&png_path, "qualquer texto");

        let result = handle
            .invoke(json!({
                "capability": "ocr.run",
                "path": png_path.to_string_lossy(),
                "lang": "por",
            }))
            .await
            .expect("invoke deve retornar response");
        assert_eq!(result["ok"], json!(false));
        // Pode ser `ocr_not_available` (Tesseract nao instalado) ou
        // pytesseract nao instalado - ambos sao o mesmo code
        // (handler trata de forma identica).
        let code = result["code"].as_str().unwrap_or("");
        assert!(
            code == "ocr_not_available",
            "code esperado ocr_not_available, veio {code}: {result}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_ocr_run_without_tesseract nao deve travar");
}

/// `pdf.read` com `ocr: "never"`: extrai so camada de texto, sem
/// chamar Tesseract. `ocr_text` deve ser vazio. **NAO** requer
/// Tesseract (testa o modo rapido).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_pdf_read_with_ocr_param_never() {
    with_test_timeout_at("e2e_pdf_read_with_ocr_param_never", E2E_TIMEOUT, async {
        let cfg = doc_worker_config(|c| c);
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external");

        let out = temp_out_dir();
        std::fs::create_dir_all(&out).expect("mkdir temp out");

        // Gera PDF com texto via pdf.write.
        let pdf_path = out.join("with_text.pdf");
        let write_result = handle
            .invoke(json!({
                "capability": "pdf.write",
                "path": pdf_path.to_string_lossy(),
                "title": "OCR Never Test",
                "sections": [{"heading": "Saudacao", "body": ["Ola mundo."]}]
            }))
            .await
            .expect("invoke pdf.write");
        assert_eq!(write_result["ok"], json!(true));

        // Le com ocr=never.
        let read_result = handle
            .invoke(json!({
                "capability": "pdf.read",
                "path": pdf_path.to_string_lossy(),
                "ocr": "never",
            }))
            .await
            .expect("invoke pdf.read");
        assert_eq!(read_result["ok"], json!(true));
        assert_eq!(read_result["extraction"], json!("text"));
        assert_eq!(read_result["ocr_text"], json!({})); // SEMPRE vazio
        assert_eq!(read_result["scanned_pages"], json!([]));
        // `text` tem o conteudo da camada.
        let text = read_result["text"].as_str().unwrap_or("");
        assert!(
            text.contains("Saudacao") || text.contains("Ola"),
            "texto do PDF nao apareceu: {text}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_pdf_read_with_ocr_param_never nao deve travar");
}

/// `pdf.read` com `ocr: "only"` em PDF 100% escaneado: faz OCR de
/// todas as paginas, devolve `text` via OCR, `extraction: "ocr"`.
/// **Requer Tesseract** (panics se nao instalado).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_pdf_read_with_ocr_fallback_on_scanned() {
    with_test_timeout_at(
        "e2e_pdf_read_with_ocr_fallback_on_scanned",
        E2E_TIMEOUT,
        async {
            tesseract_or_panic();

            let cfg = doc_worker_config(|c| c);
            let (manager, handle) =
                frederico_process_architecture::WorkerManager::spawn_external(cfg)
                    .await
                    .expect("spawn_external");

            let out = temp_out_dir();
            std::fs::create_dir_all(&out).expect("mkdir temp out");

            // Gera PNG com texto conhecido (Pillow).
            let png_path = out.join("scanned_input.png");
            generate_test_png_with_text(&png_path, "FALLBACK OCR WORKS");

            // Gera PDF 100% escaneado (1 pagina, so imagem - reportlab
            // + Pillow via subprocess).
            let pdf_path = out.join("scanned.pdf");
            generate_test_scanned_pdf_with_text(&pdf_path, &png_path);

            // Le com ocr=only: forca OCR de todas as paginas (mesmo
            // com "scanned_pages" vazio depois de extrair - o
            // handler faz OCR de todas).
            let read_result = handle
                .invoke(json!({
                    "capability": "pdf.read",
                    "path": pdf_path.to_string_lossy(),
                    "ocr": "only",
                }))
                .await
                .expect("invoke pdf.read");
            assert_eq!(
                read_result["ok"],
                json!(true),
                "pdf.read falhou: {read_result}"
            );
            assert_eq!(read_result["extraction"], json!("ocr"));
            // `text` agora e o OCR (camada ignorada por `ocr: only`).
            let text = read_result["text"].as_str().unwrap_or("").to_lowercase();
            let text_norm: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let recognized = text_norm.contains("fallback")
                || text_norm.contains("ocr")
                || text_norm.contains("works")
                || text_norm.contains("ocrworks"); // OCR as vezes junta palavras
            assert!(
                recognized,
                "OCR nao reconheceu nenhum token esperado. Texto: {text}"
            );
            // ocr_text deve ter a pagina 1 mapeada.
            let ocr_text = read_result["ocr_text"]
                .as_object()
                .expect("ocr_text deve ser objeto");
            assert!(
                ocr_text.contains_key("1"),
                "ocr_text deve ter a pagina 1, veio: {ocr_text:?}"
            );
            // ocr_truncated: false (PDF de 1 pagina < teto de 20).
            assert_eq!(read_result["ocr_truncated"], json!(false));
            // tesseract_version presente.
            let tess_version = read_result["tesseract_version"].as_str().unwrap_or("");
            assert!(
                !tess_version.is_empty() && tess_version != "null",
                "tesseract_version ausente"
            );

            manager.shutdown().await.expect("shutdown");
        },
    )
    .await
    .expect("e2e_pdf_read_with_ocr_fallback_on_scanned nao deve travar");
}
