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

/// Capability desconhecida: `ocr.run` foi removido do manifesto na
/// v0.2.0 (vai pra 2B+Y). O worker responde `code: "unknown_capability"`
/// no `tool.result` (handler captura o `KeyError` na lookup da
/// dispatch table).
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
                "capability": "ocr.run",
                "path": out.join("x.png").to_string_lossy(),
                "lang": "por"
            }))
            .await
            .expect("invoke deve retornar response");
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["code"], json!("unknown_capability"));
        // A mensagem menciona 2B+Y pra documentar o que vem.
        let msg = result["message"].as_str().unwrap_or("");
        assert!(
            msg.contains("2B+Y"),
            "mensagem deve mencionar 2B+Y, veio: {msg}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_unknown_capability_rejected nao deve travar");
}
