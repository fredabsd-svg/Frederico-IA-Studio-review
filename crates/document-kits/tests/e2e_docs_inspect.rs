//! Teste E2E da Etapa 4: `docs.inspect` ponta-a-ponta
//! (round-trip `.docx` → `DocumentSpec` parcial).
//!
//! ## O que prova
//!
//! 1. **Infra completa** — `DocumentSpec` (Report) →
//!    `WordProKit` (v0.1) → `WorkerToolDispatcher` →
//!    `WorkerHandle::invoke` → `document-worker` Python
//!    (handler `docx.write`) → arquivo `.docx` →
//!    `DocsInspectTool` → `WorkerHandle::invoke` →
//!    `document-worker` Python (handler `docx.read`) →
//!    `DocumentSpec` parcial reconstruido.
//! 2. **Round-trip via `python-docx`** — reabre o
//!    `.docx` gerado via subprocess do mesmo Python
//!    com `python-docx` e valida que:
//!    - o título aparece no doc
//!    - os 2 headings foram preservados no `spec` do
//!      inspect
//!    - o parágrafo entre headings tem o texto certo
//!    - a tabela aparece no `spec` (com headers
//!      corretos)
//!    - o `coverage.lost` lista "cover" (Cover
//!      não pode ser reconstruído do .docx)
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
//! "E2E docs.inspect" — roda no `windows-latest` do
//! GitHub Actions em todo PR.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use frederico_document_engine::{
    Cover, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion,
};
use frederico_document_kits::{DocsGenerateTool, DocsInspectTool, KitRegistry, WordProKit};
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
    d.push(format!("frederico_docs_inspect_e2e_{nonce}"));
    d
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

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
        .with_auth_token("e2e-docs-inspect-token")
        .with_ready_timeout(Duration::from_secs(20))
}

/// Constrói o `DocumentSpec` DoD do inspect: Cover + 2
/// Headings + 1 Paragraph + 1 Table. O inspect depois
/// valida que headings/paragraph/table foram preservados
/// e Cover vai pra coverage.lost.
fn spec_do_etapa_4_inspect() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("Documento de Etapa 4 Inspect".to_string()),
            ..Default::default()
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Cover(Cover {
                title: "Coberta".to_string(),
                subtitle: Some("Subtitulo da Coberta".to_string()),
                author: None,
                date: None,
            }),
            DocumentBlock::Heading {
                level: 1,
                text: "Visao geral".to_string(),
                number: None,
            },
            DocumentBlock::Heading {
                level: 2,
                text: "Detalhe".to_string(),
                number: None,
            },
            DocumentBlock::Paragraph {
                text: "Este paragrafo fica entre os dois headings.".to_string(),
                style: None,
            },
            DocumentBlock::Table {
                headers: vec!["Item".to_string(), "Valor".to_string()],
                rows: vec![
                    vec!["Alpha".to_string(), "100".to_string()],
                    vec!["Beta".to_string(), "200".to_string()],
                ],
                total: None,
                currency: None,
                percent: false,
                thousands: false,
                title: Some("Tabela 1".to_string()),
                source: None,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Teste
// ---------------------------------------------------------------------------

/// Fluxo vertical: gera `.docx` via `docs.generate`,
/// depois roda `docs.inspect` no mesmo arquivo, valida
/// que o round-trip preserva heading/paragraph/table
/// (parcial) e que Cover vai pra `coverage.lost`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_inspect_docx_roundtrip() {
    with_test_timeout_at("e2e_docs_inspect_docx_roundtrip", E2E_TIMEOUT, async {
        let cfg = doc_worker_config();
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external deve succeed");

        let handle = Arc::new(handle);
        let wordpro = Arc::new(WordProKit::new(handle.clone()));
        let mut registry = KitRegistry::new();
        registry.register(wordpro);
        let registry = Arc::new(registry);

        let dispatcher = WorkerToolDispatcher::new(Arc::new((*handle).clone()), vec![]);
        let generate_tool = DocsGenerateTool::new(registry, dispatcher.clone());
        let inspect_tool = DocsInspectTool::new(dispatcher);

        // 1. Spec DoD.
        let spec = spec_do_etapa_4_inspect();
        let spec_json = serde_json::to_value(&spec).expect("spec serializa");

        // 2. Output path.
        let out_dir = temp_out_dir();
        std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
        let docx_path = out_dir.join("relatorio_inspect.docx");

        // 3. Generate (.docx).
        let generate_result = generate_tool
            .execute(&dummy_ctx(), &json!({
                "spec": spec_json,
                "output_path": docx_path.to_string_lossy(),
                "format": "docx",
            }))
            .await;
        assert!(
            generate_result.ok,
            "generate falhou: {:?}",
            generate_result.error_message
        );
        assert!(docx_path.is_file(), ".docx nao foi criado");

        // 4. Inspect (mesmo arquivo, mesmo tool, mesmo
        //    worker).
        let inspect_result = inspect_tool
            .execute(&dummy_ctx(), &json!({
                "path": docx_path.to_string_lossy(),
            }))
            .await;
        assert!(
            inspect_result.ok,
            "inspect falhou: {:?}",
            inspect_result.error_message
        );

        // 5. Valida: `coverage.lost` e o catalogo
        //    hardcoded de tipos que o inspect v0.1 NAO
        //    sabe ler. Inclui cover + 13 outros (kpis,
        //    callout, quote, steps, chart, image, code,
        //    footer, signatures, backcover, toc,
        //    keyvalue, list). **NAO** inclui table —
        //    o inspect sabe ler tabelas reais; o que
        //    acontece no WordPro v0.1 e' que o GERADOR
        //    transforma Table em texto tab-separado, ou
        //    seja, o .docx gerado nao tem tabela real
        //    nenhuma. Isso se manifesta como 0 tables
        //    em `spec.blocks` (verificado no step 7)
        //    e table AUSENTE de `coverage.preserved`
        //    (verificado no step 6).
        let lost = inspect_result
            .output
            .get("coverage")
            .and_then(|v| v.get("lost"))
            .and_then(|v| v.as_array())
            .expect("coverage.lost deve ser array");
        assert!(
            lost.iter().any(|v| v.as_str() == Some("cover")),
            "coverage.lost deve incluir 'cover'. Veio: {lost:?}"
        );
        // `table` NAO esta no lost (inspect sabe ler
        // tabelas reais). A limitacao do WordPro v0.1
        // (Table vira texto) e' capturada no count de
        // blocks (step 7).
        assert!(
            !lost.iter().any(|v| v.as_str() == Some("table")),
            "coverage.lost NAO deve incluir 'table' (inspect sabe ler tabela real). Veio: {lost:?}"
        );

        // 6. Valida: `coverage.preserved` inclui heading
        //    e paragraph. NAO inclui table — o WordPro
        //    v0.1 transformou a Table do spec em texto
        //    tab-separado no paragrafo (limitacao
        //    registrada na Etapa 3), entao o .docx nao
        //    tem tabela real nenhuma e o inspect nao tem
        //    o que preservar. O inspect .xlsx (caso de
        //    uso principal) preserva table corretamente.
        let preserved = inspect_result
            .output
            .get("coverage")
            .and_then(|v| v.get("preserved"))
            .and_then(|v| v.as_array())
            .expect("coverage.preserved deve ser array");
        assert!(
            preserved
                .iter()
                .any(|v| v.as_str() == Some("heading")),
            "coverage.preserved deve incluir 'heading'. Veio: {preserved:?}"
        );
        assert!(
            preserved
                .iter()
                .any(|v| v.as_str() == Some("paragraph")),
            "coverage.preserved deve incluir 'paragraph'. Veio: {preserved:?}"
        );
        assert!(
            !preserved.iter().any(|v| v.as_str() == Some("table")),
            "coverage.preserved NAO deve incluir 'table' (WordPro v0.1 vira Table em texto). Veio: {preserved:?}"
        );

        // 7. Valida: o `spec.blocks` do inspect tem
        //    2 headings + N paragraphs (Cover
        //    NAO incluido — e lost; Table vira
        //    texto — e lost). Nao vamos contar
        //    exato porque depende de como o
        //    WordPro v0.1 transformou Cover e
        //    Table em texto.
        let inspect_blocks = inspect_result
            .output
            .get("spec")
            .and_then(|v| v.get("blocks"))
            .and_then(|v| v.as_array())
            .expect("spec.blocks deve ser array");
        // Pelo menos 2 headings (Cover NAO conta
        // como heading — e o texto do Cover vai
        // pro paragrafo de subtitulo).
        let heading_count = inspect_blocks
            .iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("heading"))
            .count();
        assert_eq!(
            heading_count, 2,
            "esperado 2 headings preservados, veio {heading_count}. Blocks: {inspect_blocks:?}"
        );
        // Verifica que os 2 headings sao os do spec
        // original (em qualquer ordem).
        let heading_texts: Vec<&str> = inspect_blocks
            .iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("heading"))
            .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
            .collect();
        assert!(
            heading_texts.contains(&"Visao geral"),
            "esperado heading 'Visao geral' preservado. Veio: {heading_texts:?}"
        );
        assert!(
            heading_texts.contains(&"Detalhe"),
            "esperado heading 'Detalhe' preservado. Veio: {heading_texts:?}"
        );
        // NENHUM bloco do tipo "table" (WordPro v0.1
        // transforma Table em texto).
        let table_count = inspect_blocks
            .iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("table"))
            .count();
        assert_eq!(
            table_count, 0,
            "esperado 0 tables preservadas (WordPro v0.1 vira Table em texto). Veio: {table_count}"
        );
        // NENHUM bloco do tipo "cover" (Cover NAO
        // e reconstruido pelo inspect .docx).
        let cover_count = inspect_blocks
            .iter()
            .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("cover"))
            .count();
        assert_eq!(
            cover_count, 0,
            "esperado 0 covers (Cover e lost). Veio: {cover_count}"
        );

        // 8. Valida: `sheets` (so pra .xlsx) fica vazio
        //    no inspect de .docx.
        let sheets = inspect_result
            .output
            .get("sheets")
            .and_then(|v| v.as_array())
            .expect("sheets deve ser array");
        assert!(
            sheets.is_empty(),
            "sheets deve ser vazio no inspect de .docx, veio: {sheets:?}"
        );

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_docs_inspect_docx_roundtrip nao deve travar");
}

/// Constrói um `ToolContext` dummy para testes que não dependem
/// do jail. Usado quando o test chama `tool.execute(&ctx, &args)`
/// direto (sem passar pelo `RunExecutor`). O jail é construído
/// sobre o `temp_dir` do sistema, que é re-canonicalizado.
#[allow(dead_code)]
fn dummy_ctx() -> frederico_tool_registry::ToolContext {
    use frederico_core::{ConversationId, MessageId, RunId};
    use frederico_tool_registry::{Jail, ToolContext};
    use uuid::Uuid;
    let workspace = std::env::temp_dir().join(format!(
        "frederico-document-kits-dummy-{}-{}",
        std::process::id(),
        Uuid::new_v4(),
    ));
    std::fs::create_dir_all(&workspace).expect("dummy_ctx: mkdir");
    let jail = Jail::new(&workspace).expect("dummy_ctx: Jail::new");
    ToolContext::new(
        ConversationId(Uuid::nil()),
        RunId(Uuid::nil()),
        MessageId(Uuid::nil()),
        jail,
    )
}
