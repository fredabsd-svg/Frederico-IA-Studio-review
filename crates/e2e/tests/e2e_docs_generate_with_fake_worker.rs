//! E2E — `docs.generate(docx)` com `FakeWorker` in-process.
//!
//! Caminho exercitado: **modelo → ChatOrchestrator → ToolRegistry →
//! DocsGenerateTool::execute → WorkerToolDispatcher::dispatch →
//! WorkerInvoker::invoke (via FakeWorker) → resposta do fake →
//! KitOutput → ToolResult ok → Done**.
//!
//! **Fronteira (ver `docs/architecture/testing-strategy.md` §3):**
//! este teste **para antes do Python** — o `FakeWorker` em
//! `process-architecture/src/fake.rs` devolve `{ok: true, echo:
//! <args>, env_received: ...}`, não um arquivo real. O `WordProKit::render`
//! é tolerante (linha 413 do `wordpro.rs`): se `ok == true`, aceita a
//! response com defaults (path = output_path, size_bytes = 0,
//! sections_written = 0). O `DocsGenerateTool::execute` retorna
//! `ToolResult::ok` e o `RunExecutor` emite `ToolResult` no journal.
//!
//! O que este teste prova:
//! 1. O bump atômico `documents: None → Full` (ADR-0020 §3 D3) com
//!    o catálogo de 3 tools funcionando.
//! 2. O `ToolRegistry` registra `docs.generate` quando o invoker
//!    é `Some`.
//! 3. O `WorkerToolDispatcher` chama o `WorkerInvoker` (e o
//!    `FakeWorker` responde).
//! 4. O `WordProKit::render` parseia a resposta (tolerante a
//!    shape) e devolve `Ok(KitOutput)`.
//! 5. O `RunExecutor` fecha o run como `Completed` quando o
//!    provider responde com `Done` no round 2.
//!
//! **O que este teste NÃO prova** (isso é a fronteira dos E2E —
//! ler `testing-strategy.md` §3 antes de adicionar teste novo):
//! que o `document-worker` Python gera um `.docx` válido. Isso
//! é a Etapa 5 do plano (mas com `#[ignore]`, exercitado no
//! `e2e_docs_generate_with_real_worker`).

mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_document_engine::{
    Cover, DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion,
};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use serde_json::json;

use common::{
    build_orchestrator, create_test_conversation, fake_invoker, wait_for_run_completion,
    ScriptedProvider,
};

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";

/// Spec mínimo: 1 cover + 1 heading + 1 paragraph. Cobrir o
/// caminho `Cover → Heading → Paragraph` (3 dos 20 blocos
/// cobertos pela Etapa 5 PR 2 do PDFPro). É o suficiente
/// pra `validate_semantic` passar e o `translate_spec_to_docx_payload`
/// produzir 1 seção.
fn minimal_spec() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata {
            title: Some("E2E minimal".to_string()),
            ..Default::default()
        },
        confidentiality: None,
        blocks: vec![
            DocumentBlock::Cover(Cover {
                title: "E2E minimal".to_string(),
                subtitle: Some("E2E test (fake worker)".to_string()),
                author: None,
                date: None,
            }),
            DocumentBlock::Heading {
                level: 1,
                text: "Section 1".to_string(),
                number: None,
            },
            DocumentBlock::Paragraph {
                text: "Hello from E2E.".to_string(),
                style: None,
            },
        ],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn docs_generate_with_fake_worker_closes_run_completed() {
    // 1. Invoker (FakeWorker in-process).
    let (invoker, manager) = fake_invoker().await;

    // 2. ScriptedProvider: 2 rounds.
    //    Round 1: ToolCall `docs.generate(docx)` com spec mínimo +
    //    output_path dentro do workspace da conversa.
    //    Round 2: Delta + Done.
    let spec = minimal_spec();
    let spec_json = serde_json::to_value(&spec).expect("serializa spec");
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_docs_generate_1".to_string(),
                name: "docs.generate".to_string(),
                arguments_json: json!({
                    "spec": spec_json,
                    "output_path": "minimal.docx",  // relativo ao workspace
                    "format": "docx",
                })
                .to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "Documento gerado.".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    // 3. Monta o ChatOrchestrator (com invoker — 3 tools no catálogo).
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        None,
    )
    .await;

    // 4. Cria a conversa.
    let conv = create_test_conversation(
        &h.db,
        &provider_id,
        &model_id,
        Some("e2e docs.generate (fake)"),
    )
    .await;

    // 5. Envia a mensagem.
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "gere um documento".to_string())
        .await
        .expect("send_message");

    // 6. Espera o run fechar.
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Completed,
        "run deveria completar (provider mandou Done no round 2)"
    );

    // 7. Asserções:
    // 7a. O provider foi chamado 2 vezes.
    assert_eq!(
        provider
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "esperava 2 rounds do provider"
    );

    // 7b. O ToolCall + ToolResult aparecem no journal.
    let run = frederico_storage::RunRepo::new(&h.db)
        .get(&run_id)
        .await
        .expect("get run");
    let events = h
        .orchestrator
        .get_events(run.message_id, 0)
        .await
        .expect("get_events");

    let tool_call_event = events
        .iter()
        .find(|e| e.kind == "tool_call")
        .expect("esperava tool_call no journal");
    assert_eq!(
        tool_call_event.data.get("name").and_then(|v| v.as_str()),
        Some("docs.generate")
    );

    let tool_result_event = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("esperava tool_result no journal (WordPro::render retornou Ok)");

    // 7c. O ToolResult carrega o output do `WordProKit::render`
    //     (path = output_path default, size_bytes = 0,
    //     sections_written = 0). O `FakeWorker` devolveu
    //     `{ok: true, echo: <args>, env_received: ...}` — o kit é
    //     tolerante e usa defaults quando o shape não bate.
    let data = &tool_result_event.data;
    assert_eq!(
        data.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "ToolResult.ok deveria ser true (WordPro::render retornou Ok com shape tolerado)"
    );
    let output = data.get("output").expect("esperava output no tool_result");
    let path = output.get("path").and_then(|p| p.as_str()).expect("path");
    assert!(
        path.contains("minimal.docx"),
        "esperava path contendo 'minimal.docx'; veio {path}"
    );
    assert_eq!(
        output.get("format").and_then(|f| f.as_str()),
        Some("docx"),
        "esperava format='docx'"
    );
    // 7d. O Done do round 2 fecha o run.
    let has_done = events.iter().any(|e| e.kind == "done");
    assert!(has_done, "esperava done no journal");
}
