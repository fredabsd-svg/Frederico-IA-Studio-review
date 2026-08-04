//! E2E — `docs.generate(docx)` com o `DocumentWorkerLauncher` real
//! (Python + `document-worker.py`).
//!
//! **Diferente de `e2e_docs_generate_with_fake_worker`:** aqui
//! o `WorkerInvoker` é o `DocumentWorkerLauncher` real (Etapa 2.A,
//! ADR-0023), que spawna o `python.exe` em
//! `workers/document-worker/runtime/` e fala via named pipes.
//! O `document-worker.py` (Python) gera o `.docx` de verdade via
//! `python-docx`. O teste reabre o `.docx` e valida a hierarquia.
//!
//! **Fronteira (ver `docs/architecture/testing-strategy.md` §3):**
//! este é **o único teste que atravessa o caminho completo do
//! produto** — modelo → casca → WorkerInvoker real → Python →
//! arquivo no disco. Os outros 4 E2E param antes do Python.
//!
//! **Status:** `#[ignore]` por default. Ativado por
//! `scripts/verify-external.ps1` no CI depois do
//! `bootstrap.ps1` (que instala o `runtime/` com Python +
//! dependências). Roda em todo PR.
//!
//! **Pré-requisito local:** o developer precisa rodar
//! `pwsh workers/document-worker/bootstrap.ps1` uma vez
//! antes de `cargo test -p frederico-e2e -- --include-ignored`.
//! Sem runtime, este teste faz `panic!` com mensagem clara
//! apontando pro bootstrap (mesmo padrão do
//! `crates/process-architecture/tests/external_doc_worker.rs`).

#![cfg(windows)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use frederico_app::launcher::{DocumentWorkerLauncher, LauncherConfig};
use frederico_app::runtime::{RuntimeLocation, RuntimeSource};
use frederico_core::{ModelId, ProviderId};
use frederico_document_engine::{
    Cover, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion,
};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use serde_json::json;

use common::{
    build_orchestrator, create_test_conversation, wait_for_run_completion, ScriptedProvider,
};

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";

/// Path do `python.exe` no runtime do `document-worker`. Mesmo
/// padrão do `external_doc_worker.rs`.
fn python_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR não tem 2 níveis acima")
        .join("workers")
        .join("document-worker")
        .join("runtime")
        .join("python.exe")
}

fn worker_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR não tem 2 níveis acima")
        .join("workers")
        .join("document-worker")
        .join("document-worker.py")
}

fn worker_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR não tem 2 níveis acima")
        .join("workers")
        .join("document-worker")
}

fn minimal_spec() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("E2E real worker".to_string()),
            ..Default::default()
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Cover(Cover {
                title: "E2E real worker".to_string(),
                subtitle: Some("document-worker Python".to_string()),
                author: None,
                date: None,
            }),
            DocumentBlock::Heading {
                level: 1,
                text: "Section 1".to_string(),
                number: None,
            },
            DocumentBlock::Paragraph {
                text: "Hello from real document-worker.".to_string(),
                style: None,
            },
        ],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requer document-worker runtime (rode workers/document-worker/bootstrap.ps1 primeiro; CI ativa via verify-external.ps1)"]
async fn docs_generate_with_real_worker_produces_valid_docx() {
    // 1. Verifica runtime. Panic claro apontando pro bootstrap.
    let py = python_exe();
    let script = worker_script();
    if !py.is_file() {
        panic!(
            "document-worker runtime não instalado em {}\n\
             Rode: pwsh -NoProfile -ExecutionPolicy Bypass -File {}/workers/document-worker/bootstrap.ps1",
            py.display(),
            worker_root().parent().unwrap().display(),
        );
    }
    if !script.is_file() {
        panic!("document-worker.py não encontrado em {}", script.display());
    }

    // 2. Cria o `DocumentWorkerLauncher` (lazy — só spawna no
    //    primeiro invoke).
    let location = RuntimeLocation {
        python_exe: py,
        script,
        root: worker_root(),
        source: RuntimeSource::DevRepo,
    };
    let launcher = DocumentWorkerLauncher::new(location, LauncherConfig::default());
    let invoker: Arc<dyn frederico_core::WorkerInvoker> = Arc::new(launcher);

    // 3. ScriptedProvider: 2 rounds.
    let spec = minimal_spec();
    let spec_json = serde_json::to_value(&spec).expect("serializa spec");
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_docs_generate_real_1".to_string(),
                name: "docs.generate".to_string(),
                arguments_json: json!({
                    "spec": spec_json,
                    "output_path": "real_minimal.docx",
                    "format": "docx",
                })
                .to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "Documento real gerado.".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    // 4. Monta o ChatOrchestrator com o launcher real. O
    //    `worker_manager` é `None` — o `DocumentWorkerLauncher`
    //    é o owner do ciclo de vida do worker (tem `Drop`
    //    síncrono, ADR-0023 §D3), não precisa do `WorkerManager`
    //    do fake.
    let h = build_orchestrator(
        Some(invoker),
        None,
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        None,
    )
    .await;

    // 5. Cria a conversa + envia.
    let conv = create_test_conversation(
        &h.db,
        &provider_id,
        &model_id,
        Some("e2e docs.generate (real)"),
    )
    .await;
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "gere um documento real".to_string())
        .await
        .expect("send_message");

    // 6. Espera (Python cold-start é mais lento — budget generoso).
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(60)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Completed,
        "run deveria completar (document-worker Python gerou o docx)"
    );

    // 7. Asserções:
    // 7a. ToolCall + ToolResult aparecem no journal.
    let run = frederico_storage::RunRepo::new(&h.db)
        .get(&run_id)
        .await
        .expect("get run");
    let events = h
        .orchestrator
        .get_events(run.message_id, 0)
        .await
        .expect("get_events");

    let tool_result_event = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("esperava tool_result no journal");
    let data = &tool_result_event.data;

    // **DIAGNÓSTICO** (Etapa 5 PR #24 — primeiro CI vermelho):
    // a asserção original `assert_eq!(data.get("ok"), Some(true))`
    // colapsava a informação útil — o worker Python devolve
    // `{ok: false, code, message, ...}` quando falha, mas o panic
    // só dizia `Some(false) != Some(true)`. Aqui imprimimos o JSON
    // inteiro pra ver o motivo real (path não existe? spec
    // rejeitado? capability não implementada?) na primeira execução
    // que falhar. Próximo passo: substituir por uma asserção
    // focada depois que soubermos a causa.
    if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        panic!(
            "tool_result.ok != true. JSON completo do tool_result:\n\
             \n\
             {}\n\
             \n\
             Campos típicos em caso de falha:\n\
             - code:    categoria do erro (path_not_found, capability_unknown, ...)\n\
             - message: mensagem legível do Python\n\
             - path:    path que o worker tentou usar\n",
            serde_json::to_string_pretty(&tool_result_event)
                .unwrap_or_else(|e| { format!("<falha ao serializar tool_result_event: {e}>") }),
        );
    }

    let output = data.get("output").expect("output no tool_result");
    let path_str = output
        .get("path")
        .and_then(|p| p.as_str())
        .expect("path no output");

    // 7b. O arquivo `.docx` existe no disco. **Aqui a
    //     fronteira dos E2E é atravessada** — o documento
    //     foi gerado pelo Python de verdade, não pelo fake.
    //
    // **Fase de Ligação Etapa 5.X (patch-allowed-paths):**
    // este teste é o ÚNICO que sustenta o cenário 1 do
    // `path_safety` (o cenário "allow relativo" perdeu a
    // asserção `is_file()` porque `FakeWorker` não toca no
    // FS — o teste aqui é a única prova de que o arquivo
    // real é criado dentro do jail da conversa, não no
    // CWD do Python). Asserção extra: o `path` retornado
    // pelo worker bate com o canônico que o
    // `Jail::resolve_allowing_nonexistent` produz (path
    // com `\\?\` no Windows, sem verbatim no Linux). Se
    // um dia o worker devolver um path fora do jail ou
    // o `kit.render` receber um path diferente do canônico
    // (e.g. alguém reescrever o `output_path` antes do
    // `kit.render`), este assert quebra.
    let docx_path = std::path::Path::new(path_str);
    let abs = if !docx_path.is_absolute() {
        // **Comentário aspiracional consertado (Fase de
        // Ligação Etapa 5.X):** a Etapa 5 escreveu isto
        // como "O `output_path` veio relativo ao workspace
        // da conversa; junta com o `<workspaces_root>/<cid>/`
        // pra ter o path absoluto". Antes do bump atômico
        // (commit 2), o path **não** era relativo ao
        // workspace — vinha do literal `output_path_str`
        // (`real_minimal.docx`) sem resolução, e o Python
        // escrevia no CWD (`workers/document-worker/`).
        // Agora a barreira primária
        // (`Jail::resolve_allowing_nonexistent` em
        // `document-kits/src/generate.rs`) produz o canônico
        // dentro do jail, e o `kit.render` recebe esse
        // canônico — o path devolvido pelo worker **é** o
        // canônico do jail, e juntar com o
        // `<workspaces_root>/<cid>/` reproduz o mesmo
        // arquivo. O assert abaixo prova isso de fato
        // (não aspiracional).
        h.workspace
            .workspaces_root()
            .join(conv.id.as_uuid().to_string())
            .join(path_str)
    } else {
        docx_path.to_path_buf()
    };
    assert!(abs.is_file(), ".docx não existe em {abs:?}");

    // **Asserção extra do Etapa 5.X:** o canônico retornado
    // pelo `Jail::resolve_allowing_nonexistent` (que tem
    // prefixo verbatim `\\?\` no Windows) é exatamente o
    // mesmo path que o `kit.render` abriu. Confirma que a
    // barreira primária produz o path que o worker Python
    // efetivamente usa, sem desvio intermediário. Roda o
    // canonicalize do path **esperado** e compara com o
    // `path_str` que o worker devolveu (canônico também).
    let expected_canonical = h
        .workspace
        .workspaces_root()
        .join(conv.id.as_uuid().to_string())
        .join("real_minimal.docx")
        .canonicalize()
        .expect("canonicalize do path esperado do jail");
    let actual_canonical = docx_path
        .canonicalize()
        .expect("canonicalize do path devolvido pelo worker");
    assert_eq!(
        actual_canonical, expected_canonical,
        "path devolvido pelo worker diverge do canonico esperado do jail.\n\
         Esperado: {expected_canonical:?}\n\
         Atual:    {actual_canonical:?}\n\
         Possiveis causas: (a) worker abriu um path diferente do que o\n\
         Jail::resolve_allowing_nonexistent produziu, (b) o kit.render\n\
         recebeu um path que nao e' o canonico, (c) o worker escreveu\n\
         em CWD em vez do path canônico (regressão do Etapa 5)."
    );

    // 7c. O `sections_written` é > 0 (significa que o
    //     `docx.write` realmente escreveu seções, não retornou
    //     um docx vazio).
    let sections_written = output
        .get("sections_written")
        .and_then(|v| v.as_u64())
        .expect("sections_written no output");
    assert!(
        sections_written > 0,
        "sections_written deveria ser > 0; veio {sections_written}"
    );
}
