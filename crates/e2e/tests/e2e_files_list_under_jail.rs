//! E2E — `files.list` no caminho de produção.
//!
//! Caminho exercitado: **modelo → ChatOrchestrator → ToolRegistry →
//! files.list → FilesListTool::execute(ctx, args) → filesystem
//! dentro do jail da conversa → ToolResult → modelo responde →
//! Done**.
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (fronteira dos E2E — este teste **não** sobe a casca Tauri nem
//! toca o `document-worker` Python; é 100% in-process).
//!
//! **Diferenças vs. `e2e_files_read.rs` (Etapa 1 da Fase de Ligação):**
//!
//! 1. `files.list` é uma **ferramenta de Phase 7 Etapa 5** (Piece 1
//!    do PR #45), com `risk_level: Safe` e **sem
//!    `requires_user_approval`** — o Passo 9 do `validate_tool_call`
//!    deixa passar sem exigir `ApprovalDecision`.
//! 2. O tool é registrado tanto no `build_default_tools(None)`
//!    (sem runtime) quanto no `build_default_tools(Some(invoker))`
//!    (com runtime) — `in-process` independe de `WorkerInvoker`.
//!
//! **O que este teste prova:**
//!
//! 1. O `build_chat_orchestrator(parts)` é o mesmo que a casca
//!    Tauri chama — composição compartilhada.
//! 2. O `JailResolver` por conversa resolve o workspace da
//!    conversa (`<workspaces_root>/<cid>/`).
//! 3. O `files.list` lista arquivos dentro do jail (sem path
//!    traversal).
//! 4. O `RunExecutor` fecha o loop tool_call: ToolCall →
//!    ToolResult → próxima chamada ao modelo → Done →
//!    `RunStatus::Completed`.

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

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";

/// Workspace da conversa (mesma fórmula do `e2e_files_read.rs`).
/// O `FileSystemJailResolver` cria
/// `<workspaces_root>/<cid>/` por conversa.
fn workspace_dir(
    workspaces_root: &std::path::Path,
    conv_id: frederico_core::ConversationId,
) -> std::path::PathBuf {
    workspaces_root.join(conv_id.as_uuid().to_string())
}

/// Setup do workspace da conversa: cria 3 arquivos + 1 subdiretório
/// + 1 arquivo no subdiretório, pra ter mistura de raiz/subdir.
fn setup_workspace(workspace_dir: &std::path::Path) {
    std::fs::create_dir_all(workspace_dir).expect("cria workspace da conversa");
    std::fs::write(workspace_dir.join("alpha.txt"), "alpha").expect("escreve alpha.txt");
    std::fs::write(workspace_dir.join("beta.txt"), "beta").expect("escreve beta.txt");
    std::fs::write(workspace_dir.join("gamma.md"), "gamma").expect("escreve gamma.md");
    std::fs::create_dir(workspace_dir.join("sub")).expect("cria sub/");
    std::fs::write(workspace_dir.join("sub").join("delta.txt"), "delta").expect("delta.txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_list_e2e_through_chat_orchestrator_lists_root() {
    // 1. Invoker (FakeWorker in-process) + manager (consumido pelo
    //    `build_orchestrator`; o manager vive no `h.worker_manager`).
    let (invoker, manager) = fake_invoker().await;

    // 2. ScriptedProvider: 2 rounds.
    //    Round 1: emite `ToolCall` chamando `files.list` com path
    //             default (".").
    //    Round 2: depois do `ToolResult`, emite Delta + Done.
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_files_list_1".to_string(),
                name: "files.list".to_string(),
                arguments_json: json!({}).to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "Lista lida com sucesso.".to_string(),
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

    // 4. Cria a conversa + escreve o workspace (5 arquivos:
    //    3 na raiz, 1 no subdir).
    let conv =
        create_test_conversation(&h.db, &provider_id, &model_id, Some("e2e files.list")).await;
    let workspace = workspace_dir(h.workspace.workspaces_root().as_path(), conv.id);
    setup_workspace(&workspace);

    // 5. Envia a mensagem.
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "liste os arquivos do workspace".to_string())
        .await
        .expect("send_message");

    // 6. Espera o run fechar.
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Completed,
        "run deveria completar (files.list é Safe, sem approval)"
    );

    // 7. Asserções:
    // 7a. O provider foi chamado 2 vezes (round 1: ToolCall;
    //     round 2: Delta+Done).
    assert_eq!(
        provider
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "esperava 2 rounds do provider"
    );

    // 7b. O journal tem ToolCall + ToolResult + Delta + Done. Carrega
    //     os eventos do journal e confere.
    let run = RunRepo::new(&h.db).get(&run_id).await.expect("get run");
    let asst_msg = MessageRepo::new(&h.db)
        .get(&run.message_id)
        .await
        .expect("get assistant message");
    assert!(
        asst_msg.content.contains("Lista lida com sucesso"),
        "esperava o delta do round 2 no content; veio {:?}",
        asst_msg.content
    );
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

    // 7c. O `ToolResult` retornou `ok: true` e o output contém os
    //     4 itens da raiz (alpha, beta, gamma, sub). O subdiretório
    //     `sub/` aparece como uma entry com `is_dir: true`.
    let tool_result_event = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("evento tool_result");
    let payload = tool_result_event
        .data
        .get("output")
        .expect("output em tool_result.data");
    let entries = payload
        .get("entries")
        .and_then(|e| e.as_array())
        .expect("entries array");
    let entry_count = payload.get("entry_count").and_then(|c| c.as_u64());
    let truncated = payload.get("truncated").and_then(|t| t.as_bool());

    assert_eq!(entry_count, Some(4), "esperava 4 entries na raiz");
    assert_eq!(truncated, Some(false), "esperava truncated=false");

    // Confere que os 3 arquivos + 1 subdiretório estão presentes.
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .collect();
    assert!(names.contains(&"alpha.txt"), "faltou alpha.txt: {names:?}");
    assert!(names.contains(&"beta.txt"), "faltou beta.txt: {names:?}");
    assert!(names.contains(&"gamma.md"), "faltou gamma.md: {names:?}");
    assert!(names.contains(&"sub"), "faltou sub/: {names:?}");

    // O subdiretório tem `is_dir: true`.
    let sub_entry = entries
        .iter()
        .find(|e| e.get("name").and_then(|n| n.as_str()) == Some("sub"))
        .expect("entry 'sub'");
    assert_eq!(
        sub_entry.get("is_dir").and_then(|d| d.as_bool()),
        Some(true),
        "sub/ deveria ter is_dir=true"
    );

    // Os arquivos têm `is_dir: false` e `size` > 0.
    for fname in ["alpha.txt", "beta.txt", "gamma.md"] {
        let e = entries
            .iter()
            .find(|x| x.get("name").and_then(|n| n.as_str()) == Some(fname))
            .unwrap_or_else(|| panic!("faltou {fname}"));
        assert_eq!(e.get("is_dir").and_then(|d| d.as_bool()), Some(false));
        assert!(
            e.get("size").and_then(|s| s.as_u64()).unwrap_or(0) > 0,
            "{fname} deveria ter size > 0"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_list_e2e_through_chat_orchestrator_lists_subdir() {
    // Verifica que `files.list` aceita path relativo a subdiretório
    // dentro do jail — caminho feliz do Passo 7 do
    // `validate_tool_call`.

    let (invoker, manager) = fake_invoker().await;

    // Pede `path: "sub"` — lista o subdiretório (1 arquivo:
    // delta.txt).
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_files_list_sub".to_string(),
                name: "files.list".to_string(),
                arguments_json: json!({"path": "sub"}).to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "sub listado".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        None,
    )
    .await;

    let conv =
        create_test_conversation(&h.db, &provider_id, &model_id, Some("e2e files.list sub")).await;
    let workspace = workspace_dir(h.workspace.workspaces_root().as_path(), conv.id);
    setup_workspace(&workspace);

    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "liste o conteudo de sub/".to_string())
        .await
        .expect("send_message");

    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(status, frederico_storage::RunStatus::Completed);

    let run = RunRepo::new(&h.db).get(&run_id).await.expect("get run");
    let events = h
        .orchestrator
        .get_events(run.message_id, 0)
        .await
        .expect("get_events");
    let tool_result_event = events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("evento tool_result");
    let payload = tool_result_event
        .data
        .get("output")
        .expect("output em tool_result.data");
    let entries = payload
        .get("entries")
        .and_then(|e| e.as_array())
        .expect("entries array");
    let entry_count = payload.get("entry_count").and_then(|c| c.as_u64());
    assert_eq!(entry_count, Some(1), "esperava 1 entry em sub/");
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .collect();
    assert_eq!(names, vec!["delta.txt"]);
}
