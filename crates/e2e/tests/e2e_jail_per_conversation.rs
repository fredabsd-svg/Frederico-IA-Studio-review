//! E2E — jail por conversa: 2 conversas com workspaces
//! diferentes. A conversa A tenta ler `secret.txt` do workspace
//! de B via path traversal `../<cid_b>/secret.txt`. O
//! `Jail::resolve` (Passo 1 do `Jail::resolve`: `Component::ParentDir`)
//! rejeita com `JailViolation` — sem fallback pra
//! `temp_dir`/`cwd` global.
//!
//! **Regressão §I3 do threat model:** path traversal via `..` é
//! bloqueado pelo jail em runtime, **não** só na validação. A
//! defesa em profundidade: o `validate_tool_call` Passo 7 chama
//! `ctx.jail.resolve` (que rejeita), e o `FilesReadTool::execute`
//! re-valida o path (defesa redundante).
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e o
//! `docs/architecture/security-threat-model.md` §I3.

mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use serde_json::json;

use common::{
    build_orchestrator, create_test_conversation, fake_invoker, wait_for_run_completion,
    ScriptedProvider,
};

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";
const SECRET_CONTENT: &str = "Top secret from conversation B";

/// Path de um arquivo dentro do workspace de uma conversa.
/// `FileSystemJailResolver` cria `<workspaces_root>/<cid>/` por
/// conversa; a gente escreve o arquivo direto no path esperado
/// (igual a `e2e_files_read`).
fn workspace_file(
    workspaces_root: &std::path::Path,
    conv_id: frederico_core::ConversationId,
    name: &str,
) -> std::path::PathBuf {
    workspaces_root
        .join(conv_id.as_uuid().to_string())
        .join(name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jail_per_conversation_blocks_path_traversal() {
    // 1. Cria o DB primeiro, depois cria as 2 conversas
    //    (A atacante, B alvo) ANTES de montar o `build_orchestrator`
    //    — pra poder construir o path traversal com o UUID real
    //    de B. O `build_orchestrator` recebe o DB e reusa.
    let db = std::sync::Arc::new(
        frederico_storage::Database::open_in_memory()
            .await
            .expect("open in-memory db"),
    );
    let provider_id_proto = ProviderId::new(PROVIDER_ID);
    let model_id_proto = ModelId::new(MODEL_ID);
    let conv_a = create_test_conversation(
        &db,
        &provider_id_proto,
        &model_id_proto,
        Some("conv A (attacker)"),
    )
    .await;
    let conv_b = create_test_conversation(
        &db,
        &provider_id_proto,
        &model_id_proto,
        Some("conv B (target)"),
    )
    .await;

    // 2. ScriptedProvider: 2 rounds. Round 1: `ToolCall` com path
    //    traversal; Round 2: Done.
    let cid_b_str = conv_b.id.as_uuid().to_string();
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_traversal_1".to_string(),
                name: "files.read".to_string(),
                arguments_json: json!({
                    "path": format!("../{cid_b_str}/secret.txt")
                })
                .to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "OK, tentativa de traversal bloqueada.".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    // 3. Monta o ChatOrchestrator (com invoker fake — o Jail é o
    //    que nos importa aqui, o worker fake só fica no caminho).
    //    **Passa o `db` já criado** pra que as conversas A e B
    //    existam no DB que o `h` vai usar.
    let (invoker, manager) = fake_invoker().await;
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        Some(db.clone()),
    )
    .await;

    // 4. Escreve `secret.txt` no workspace de B (no `h.workspace`).
    let secret_path = workspace_file(
        h.workspace.workspaces_root().as_path(),
        conv_b.id,
        "secret.txt",
    );
    std::fs::create_dir_all(secret_path.parent().unwrap()).expect("cria workspace de B");
    std::fs::write(&secret_path, SECRET_CONTENT).expect("escreve secret.txt em B");

    // 5. Envia a mensagem na conversa A (a atacante).
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv_a.id, "leia o secret.txt do workspace B".to_string())
        .await
        .expect("send_message");

    // 6. Espera o run fechar.
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Completed,
        "run deveria completar (modelo respondeu com Done no round 2)"
    );

    // 7. Asserções:
    // 7a. O ToolResult tem `ok: false` + `error_code: TOOL_JAIL_VIOLATION`.
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
    assert_eq!(data.get("ok").and_then(|v| v.as_bool()), Some(false));
    let err_code = data
        .get("output")
        .and_then(|o| o.get("error_code"))
        .and_then(|c| c.as_str())
        .expect("esperava output.error_code");
    assert_eq!(
        err_code, "TOOL_JAIL_VIOLATION",
        "esperava TOOL_JAIL_VIOLATION (path traversal); veio {err_code}"
    );

    // 7b. O `secret.txt` da conversa B **NÃO** foi lido — conteúdo
    //     não vazou pro conteúdo da mensagem assistant.
    let asst_msg = frederico_storage::MessageRepo::new(&h.db)
        .get(&run.message_id)
        .await
        .expect("get assistant message");
    assert!(
        !asst_msg.content.contains(SECRET_CONTENT),
        "vazamento! o conteúdo do secret.txt está no assistant: {:?}",
        asst_msg.content
    );
}
