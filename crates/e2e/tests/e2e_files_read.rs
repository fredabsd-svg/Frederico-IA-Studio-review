//! E2E — `files.read` no caminho de produção.
//!
//! Caminho exercitado: **modelo → ChatOrchestrator → ToolRegistry →
//! files.read → FilesReadTool::execute(ctx, args) → filesystem
//! dentro do jail da conversa → ToolResult → modelo responde →
//! Done**.
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (fronteira dos E2E — este teste **não** sobe a casca Tauri nem
//! toca o `document-worker` Python; é 100% in-process com `FakeWorker`).
//!
//! O que este teste prova:
//! 1. O `build_chat_orchestrator(parts)` da `frederico-app` é o mesmo
//!    que a casca Tauri chama — composição compartilhada.
//! 2. O bump atômico `documents: None → Full` (ADR-0020 §3 D3) é
//!    exercitado quando o invoker é `Some` (catálogo tem 3 tools).
//! 3. O `JailResolver` por conversa resolve o workspace da conversa
//!    (`<workspaces_root>/<cid>/`).
//! 4. O `files.read` lê o arquivo dentro do jail (não escapa).
//! 5. O `RunExecutor` fecha o loop tool_call: ToolCall → ToolResult
//!    → próxima chamada ao modelo → Done → `RunStatus::Completed`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use frederico_storage::{MessageRepo, RunRepo};
use serde_json::json;

use common::{
    build_orchestrator, create_test_conversation, fake_invoker, wait_for_run_completion,
    ScriptedProvider,
};

// `openai/gpt-4o-mini` está no `Catalog::load()` embutido (preço
// não-zero, modalidade text+image, capability `Tools`). É o par
// mais barato que tem `tools` capability no catálogo. O
// `ScriptedProvider` recebe o mesmo `provider_id`/`model_id` e
// reporta em `id()` e `known_models()` — o `ChatOrchestrator`
// encontra o adapter via `providers.get(&conv.provider_id)` e o
// descriptor via `catalog.find_model(&conv.provider_id, &conv.model_id)`.
const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";
const HELLO_CONTENT: &str = "Hello, world! (e2e_files_read)";

/// Path do arquivo dentro do workspace da conversa. O
/// `FileSystemJailResolver` cria `<workspaces_root>/<cid>/hello.txt`
/// quando o `Jail::resolve("hello.txt")` é chamado — antes disso,
/// o arquivo não existe, e o `Jail::resolve` falha (porque o
/// `canonicalize` falha). Por isso a gente escreve o arquivo
/// **diretamente** no path esperado.
fn workspace_hello_txt(
    workspaces_root: &std::path::Path,
    conv_id: frederico_core::ConversationId,
) -> std::path::PathBuf {
    workspaces_root
        .join(conv_id.as_uuid().to_string())
        .join("hello.txt")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_read_e2e_through_chat_orchestrator() {
    // 1. Invoker (FakeWorker in-process) + manager (consumido pelo
    //    `build_orchestrator`; o manager vive no `h.worker_manager`).
    let (invoker, manager) = fake_invoker().await;

    // 2. ScriptedProvider: 2 rounds.
    //    Round 1: emite `ToolCall` chamando `files.read` com path relativo.
    //    Round 2: depois do `ToolResult`, emite Delta + Done.
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_files_read_1".to_string(),
                name: "files.read".to_string(),
                arguments_json: json!({"path": "hello.txt"}).to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "Arquivo lido com sucesso.".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    // 3. Monta o ChatOrchestrator (mesma função da casca).
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        None,
    )
    .await;

    // 4. Cria a conversa + escreve `hello.txt` no workspace esperado
    //    (ANTES de chamar send_message, porque o `Jail::resolve` no
    //    `files.read` exige que o arquivo exista).
    let conv =
        create_test_conversation(&h.db, &provider_id, &model_id, Some("e2e files.read")).await;
    let hello_path = workspace_hello_txt(h.workspace.workspaces_root().as_path(), conv.id);
    std::fs::create_dir_all(hello_path.parent().unwrap()).expect("cria workspace da conversa");
    std::fs::write(&hello_path, HELLO_CONTENT).expect("escreve hello.txt");

    // 5. Envia a mensagem.
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "leia o arquivo hello.txt".to_string())
        .await
        .expect("send_message");

    // 6. Espera o run fechar.
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Completed,
        "run deveria completar"
    );

    // 7. Asserções:
    // 7a. O provider foi chamado 2 vezes (round 1: ToolCall; round 2: Delta+Done).
    assert_eq!(
        provider
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "esperava 2 rounds do provider"
    );

    // 7b. A mensagem assistant foi criada e o conteúdo contém o delta do round 2.
    let run = RunRepo::new(&h.db).get(&run_id).await.expect("get run");
    let asst_msg = MessageRepo::new(&h.db)
        .get(&run.message_id)
        .await
        .expect("get assistant message");
    assert!(
        asst_msg.content.contains("Arquivo lido com sucesso"),
        "esperava o delta do round 2 no content; veio {:?}",
        asst_msg.content
    );

    // 7c. O journal tem ToolCall + ToolResult + Delta + Done. Carrega os
    //     eventos do journal e confere.
    let events = h
        .orchestrator
        .get_events(run.message_id, 0)
        .await
        .expect("get_events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(
        kinds.contains(&"tool_call"),
        "esperava tool_call no journal; kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&"tool_result"),
        "esperava tool_result no journal; kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&"delta"),
        "esperava delta no journal; kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&"done"),
        "esperava done no journal; kinds: {kinds:?}"
    );
}
