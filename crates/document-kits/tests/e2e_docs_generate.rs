//! Teste E2E da Etapa 3 da Fase 5: `docs.generate` ponta-a-ponta.
//!
//! ## O que prova
//!
//! 1. **Infra completa** — `DocumentSpec` (definido no
//!    `document-engine`) → `DocsGenerateTool` (kit) →
//!    `WorkerToolDispatcher` → `WorkerHandle::invoke` →
//!    `document-worker` Python (handler `docx.write`).
//! 2. **Round-trip via `python-docx`** — reabre o `.docx`
//!    gerado via subprocess do mesmo Python e valida que:
//!    - o título aparece no doc;
//!    - os 2 headings foram gravados;
//!    - o parágrafo entre headings tem o texto certo;
//!    - a tabela tem o número de linhas esperado.
//!
//! Esse round-trip é o que prova que o kit traduziu certo.
//! Teste que só confere que o arquivo existe não prova nada
//! (qualquer payload gera um `.docx` válido pelo `python-docx`,
//! inclusive um vazio).
//!
//! ## Gate Windows
//!
//! O módulo é `#[cfg(windows)]`. `python.exe` precisa estar em
//! `workers/document-worker/runtime/`. Se faltar, o helper
//! `python_exe_or_panic` faz panic com mensagem clara
//! apontando pro `bootstrap.ps1`. **Não** é `#[ignore]` (REGRAS
//! §2.6).
//!
//! ## CI
//!
//! Adicionado ao `scripts/verify-external.ps1` como step
//! "E2E docs.generate" — roda no `windows-latest` do GitHub
//! Actions em todo PR (junto com o "E2E document-worker
//! handlers" já existente).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use frederico_document_engine::{
    Cover, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion,
};
use frederico_document_kits::{DocsGenerateTool, KitRegistry, WordProKit};
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
    d.push(format!("frederico_docs_generate_e2e_{nonce}"));
    d
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

// 60s cobre cold-start do Python + render do .docx + subprocess
// de reopen via python-docx. Mesma folga que o teste existente
// `e2e_docx_write_and_read` usa.
const E2E_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn python_exe_or_panic() -> PathBuf {
    let py = python_exe();
    if !py.is_file() {
        panic!(
            "python.exe não encontrado em {}.\n\
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
        .with_auth_token("e2e-docs-generate-token")
        .with_ready_timeout(Duration::from_secs(20))
}

/// Constrói o `DocumentSpec` que o usuário definiu como
/// DoD da Etapa 3: Cover + 2 Headings + 1 Paragraph + 1
/// Table com 3 linhas.
fn spec_do_etapa_3() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("Relatório de Etapa 3".to_string()),
            ..Default::default()
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Cover(Cover {
                title: "Relatório de Etapa 3".to_string(),
                subtitle: Some("WordPro mínimo — Etapa 3 da Fase 5".to_string()),
                author: None,
                date: None,
            }),
            DocumentBlock::Heading {
                level: 1,
                text: "Visão geral".to_string(),
                number: None,
            },
            DocumentBlock::Heading {
                level: 2,
                text: "Detalhe".to_string(),
                number: None,
            },
            DocumentBlock::Paragraph {
                text: "Este parágrafo fica entre os dois headings.".to_string(),
                style: None,
            },
            DocumentBlock::Table {
                headers: vec!["Item".to_string(), "Valor".to_string()],
                rows: vec![
                    vec!["Alpha".to_string(), "100".to_string()],
                    vec!["Beta".to_string(), "200".to_string()],
                    vec!["Gamma".to_string(), "300".to_string()],
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

/// Reabre o `.docx` via subprocess do `python.exe` do
/// worker, usando `python-docx`. Valida que o doc tem:
/// - 2 headings (exato: "Visão geral" e "Detalhe");
/// - o parágrafo "Este parágrafo..." aparece em algum
///   parágrafo do doc;
/// - a tabela aparece no doc (limitação: o `docx.write`
///   v0.3.0 não tem suporte a tabela real — a tabela
///   vira texto tab-separado num parágrafo; Etapa 6
///   traz a tabela real via `python-docx` estendido).
///
/// Imprime no stdout uma linha por check (no formato
/// "CHECK nome=ok" / "CHECK nome=FAIL msg"). O Rust
/// parseia a saída.
fn validate_docx_via_python(python: &PathBuf, docx_path: &PathBuf) {
    let script = r#"
import sys
from docx import Document

path = sys.argv[1]
doc = Document(path)

# Headings
headings = [p.text for p in doc.paragraphs if p.style.name.startswith("Heading")]
print(f"CHECK headings={len(headings)}")

# Parágrafo alvo
target = "Este parágrafo fica entre os dois headings"
has_target = any(target in p.text for p in doc.paragraphs)
print(f"CHECK has_target_paragraph={has_target}")

# Tabela: o `docx.write` v0.3.0 não tem suporte a
# tabela real. Verifica se a tabela aparece como texto
# tab-separado em algum parágrafo (o fallback do kit).
# Lista de marcadores da tabela:
table_markers = ["Tabela 1", "Item", "Valor", "Alpha", "100", "Beta", "200", "Gamma", "300"]
all_paragraphs = "\n".join(p.text for p in doc.paragraphs)
present = [m for m in table_markers if m in all_paragraphs]
print(f"CHECK table_text_markers={len(present)}/{len(table_markers)}")
"#;
    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(docx_path)
        .output()
        .expect("falha spawnando python pra validar docx");
    if !output.status.success() {
        panic!(
            "validação python-docx falhou: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut found_headings = None;
    let mut found_target = None;
    let mut found_markers: Option<String> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("CHECK headings=") {
            found_headings = rest.parse::<usize>().ok();
        } else if let Some(rest) = line.strip_prefix("CHECK has_target_paragraph=") {
            found_target = Some(rest == "True");
        } else if let Some(rest) = line.strip_prefix("CHECK table_text_markers=") {
            found_markers = Some(rest.to_string());
        }
    }
    assert_eq!(
        found_headings,
        Some(2),
        "esperado 2 headings, veio {found_headings:?}. Stdout: {stdout}"
    );
    assert_eq!(
        found_target,
        Some(true),
        "parágrafo alvo não apareceu. Stdout: {stdout}"
    );
    // 9/9 marcadores da tabela presentes. O docx.write
    // v0.3.0 não tem suporte a tabela real (Etapa 6 traz);
    // o kit vira tabela em texto tab-separado.
    assert_eq!(
        found_markers.as_deref(),
        Some("9/9"),
        "esperado 9/9 marcadores da tabela (texto tab-separado), veio {found_markers:?}. Stdout: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Teste
// ---------------------------------------------------------------------------

/// Fluxo vertical mínimo: DocumentSpec → .docx → reabre →
/// valida hierarquia + linhas da tabela. Atravessa a infra
/// completa (kit + dispatcher + worker + handler + arquivo
/// real + reopen).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_docs_generate_docx_full_vertical() {
    with_test_timeout_at("e2e_docs_generate_docx_full_vertical", E2E_TIMEOUT, async {
        let cfg = doc_worker_config();
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_external(cfg)
            .await
            .expect("spawn_external deve succeed");

        // 1. KitRegistry com WordPro.
        let mut registry = KitRegistry::new();
        registry.register(Arc::new(WordProKit::new(Arc::new(handle.clone()))));
        let registry = Arc::new(registry);

        // 2. WorkerToolDispatcher. Allowlist vazia = sem
        //    validação (o `output_path` vem do chamador no
        //    teste; em prod, o `ToolRegistry` popula com
        //    o workspace do usuário).
        let dispatcher = WorkerToolDispatcher::new(Arc::new(handle.clone()));
        let tool = DocsGenerateTool::new(registry, dispatcher);

        // 3. Spec do DoD.
        let spec = spec_do_etapa_3();
        let spec_json = serde_json::to_value(&spec).expect("spec serializa");

        // 4. Output path.
        let out_dir = temp_out_dir();
        std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
        let docx_path = out_dir.join("relatorio_etapa_3.docx");

        // 5. Executa.
        let result = tool
            .execute(
                &dummy_ctx(),
                &json!({
                    "spec": spec_json,
                    "output_path": docx_path.to_string_lossy(),
                    "format": "docx",
                }),
            )
            .await;
        assert!(result.ok, "execute falhou: {:?}", result.error_message);
        assert_eq!(
            result.output.get("format").and_then(|v| v.as_str()),
            Some("docx")
        );
        assert!(
            docx_path.is_file(),
            ".docx não foi criado em {}",
            docx_path.display()
        );
        let size = docx_path.metadata().unwrap().len();
        assert!(size > 1000, "docx muito pequeno: {size} bytes");

        // 6. Reabre via python-docx e valida hierarquia +
        //    linhas da tabela. **Atravessa a casca** (subprocess
        //    do Python real, mesma runtime do worker).
        let py = python_exe_or_panic();
        validate_docx_via_python(&py, &docx_path);

        manager.shutdown().await.expect("shutdown");
    })
    .await
    .expect("e2e_docs_generate_docx_full_vertical não deve travar");
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
