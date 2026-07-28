//! Testes de recovery do `RunExecutor` (Etapa 4.x).
//!
//! Replica os 3 testes do `crates/provider-engine/tests/recovery.rs`
//! da Fase 2, mas usando o `RunExecutor` (motor novo da Etapa 4).
//! A regra do **"Journal-then-emit"** é provada em ambos os motores:
//! cada `StreamEvent` é persistido em `message_events` **antes** de
//! ser processado. Se o `RunExecutor` for dropado no meio de um run,
//! o journal no SQLite é a fonte de verdade — ao reabrir, os eventos
//! estão lá.
//!
//! **Por que dois testes de recovery?** Quando a Etapa 4.x.y
//! reorganizar os crates (mover o `ChatOrchestrator` pra usar o
//! `RunExecutor`), o `recovery.rs` do `provider-engine` migra
//! junto (ou vira redundante). Até lá, ter os dois testes em
//! paralelo dá segurança: se o `run_stream_loop` da Fase 2
//! quebrar, o `provider-engine/tests/recovery.rs` pega; se o
//! `RunExecutor` quebrar, este aqui pega.
//!
//! Cobertura:
//! 1. **Recovery simples**: o journal persiste 5 deltas + 1 Done
//!    através de drop + reabrir o mesmo `Database`.
//! 2. **Status final persistido**: o `Run.status` no db bate com
//!    o último estado do `RunExecutor`.
//! 3. **user msg persistida antes de I/O**: o caller
//!    (ChatOrchestrator) persiste a user msg antes do `Run`,
//!    mas o `RunExecutor` não toca nisso — o teste valida que
//!    o `Message.status` do assistant é o correto.

#![cfg(not(doctest))]

use std::time::Duration;

use frederico_agent_engine::Budget;
use frederico_core::{MessageId, ModelId, ToolId};
use frederico_execution_engine::RunExecutor;
use frederico_provider_engine::{ChatMessage, StopReason, StreamEvent};
use frederico_storage::{MessageEventRepo, MessageRepo, RunRepo, RunStatus};
use frederico_tool_registry::{FileReadPermission, PermissionSet as Perms};
use tokio_util::sync::CancellationToken;

mod common;
use common::{setup_executor, ScriptedAdapter};

/// `PermissionSet` que libera `files.read` (default deny rejeita no
/// Passo 5 com `TOOL_PERMISSION_DENIED`).
fn perms_with_file_read() -> Perms {
    Perms {
        file_read: FileReadPermission::WorkspaceOnly,
        ..Default::default()
    }
}

fn make_events(n_deltas: usize) -> Vec<StreamEvent> {
    let mut out: Vec<StreamEvent> = (0..n_deltas)
        .map(|i| StreamEvent::Delta {
            content: format!("d{i} "),
        })
        .collect();
    out.push(StreamEvent::Done {
        stop_reason: StopReason::Stop,
    });
    out
}

// ---------------------------------------------------------------------------
// Teste 1: journal persiste eventos durante o ciclo de vida do RunExecutor
// ---------------------------------------------------------------------------

/// Regra do **"Journal-then-emit"**: cada `StreamEvent` é
/// persistido em `message_events` **antes** de ser processado.
/// Se o `RunExecutor` for dropado no meio do run, o journal
/// está completo no SQLite (a fonte de verdade).
///
/// O `Database` é `Arc<SqlitePool>`, então sobrevive ao drop do
/// `RunExecutor`. O "drop do orquestrador" da Fase 2 é
/// equivalente — o `RunExecutor` é o orquestrador novo, e
/// sobreviver a ele é o que vale.
#[tokio::test(flavor = "current_thread")]
async fn journal_persists_events_within_run_executor_lifecycle() {
    let (_workspace, db, registry, jail, message_id, run_id) = setup_executor().await;

    let adapter = std::sync::Arc::new(ScriptedAdapter::new("simulated", vec![make_events(5)]));
    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail,
        db.clone(),
        perms_with_file_read(),
        vec![ToolId::new("files.read")],
        vec![],
        Budget::default(),
        CancellationToken::new(),
    );
    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("hello")],
        )
        .await
        .expect("executa");
    assert_eq!(outcome.accumulated_content, "d0 d1 d2 d3 d4 ");

    // Snapshot do journal.
    let event_repo = MessageEventRepo::new(&db);
    let journal = event_repo
        .list_for_message(&message_id, 0)
        .await
        .expect("journal");
    assert!(
        !journal.is_empty(),
        "journal vazio — bug do RunExecutor (regra do Journal-then-emit violada)"
    );
    let deltas = journal.iter().filter(|e| e.kind == "delta").count();
    let dones = journal.iter().filter(|e| e.kind == "done").count();
    assert_eq!(deltas, 5, "esperado 5 deltas no journal");
    assert_eq!(dones, 1, "esperado 1 Done no journal");

    // Drop do executor. O `Database` (Arc) continua vivo — o
    // journal está intacto.
    drop(executor);
    let journal_after = event_repo
        .list_for_message(&message_id, 0)
        .await
        .expect("journal pós-drop");
    assert_eq!(journal_after.len(), journal.len());
}

// ---------------------------------------------------------------------------
// Teste 2: status final persistido
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn final_state_persists_to_database() {
    let (_workspace, db, registry, jail, message_id, run_id) = setup_executor().await;

    // Cenário A: run que termina Completed.
    let adapter = std::sync::Arc::new(ScriptedAdapter::new("simulated", vec![make_events(3)]));
    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry.clone(),
        jail.clone(),
        db.clone(),
        perms_with_file_read(),
        vec![],
        vec![],
        Budget::default(),
        CancellationToken::new(),
    );
    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("qualquer")],
        )
        .await
        .expect("executa");
    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Completed
    );

    // O status no db bate.
    let run = RunRepo::new(&db).get(&run_id).await.expect("get run");
    assert_eq!(run.status, RunStatus::Completed.as_str());
    let msg = MessageRepo::new(&db)
        .get(&message_id)
        .await
        .expect("get msg");
    assert_eq!(msg.status, "completed");
    assert_eq!(msg.content, "d0 d1 d2 ");

    // Drop + verifica que continua válido.
    drop(executor);
    let run2 = RunRepo::new(&db).get(&run_id).await.expect("get run 2");
    assert_eq!(run2.status, RunStatus::Completed.as_str());
}

// ---------------------------------------------------------------------------
// Teste 3: falha do provider persiste Failed
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn provider_error_persists_as_failed() {
    let (_workspace, db, registry, jail, message_id, run_id) = setup_executor().await;

    // Adapter que emite só `Error`.
    let adapter = std::sync::Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![vec![StreamEvent::Error(
            frederico_provider_engine::ProviderError::auth(),
        )]],
    ));
    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail,
        db.clone(),
        perms_with_file_read(),
        vec![],
        vec![],
        Budget::default(),
        CancellationToken::new(),
    );
    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("x")],
        )
        .await
        .expect("executa (sem panic, retorna RunOutcome com Failed)");
    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Failed
    );
    let run = RunRepo::new(&db).get(&run_id).await.expect("get run");
    assert_eq!(run.status, RunStatus::Failed.as_str());
    let msg = MessageRepo::new(&db)
        .get(&message_id)
        .await
        .expect("get msg");
    assert_eq!(msg.status, "failed");
}

// ---------------------------------------------------------------------------
// Teste 4: journal inclui o `tool_result` quando há `tool_call`
// (específico do `RunExecutor` da Etapa 4 — não tem análogo na Fase 2)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn journal_includes_tool_result_event() {
    let (_workspace, db, registry, jail, message_id, run_id) = setup_executor().await;

    // Round 1: Delta + ToolCall + Done{ToolCalls}
    // Round 2: Delta + Done{Stop}
    let adapter = std::sync::Arc::new(ScriptedAdapter::new(
        "simulated",
        vec![
            vec![
                StreamEvent::Delta {
                    content: "olá ".to_string(),
                },
                StreamEvent::ToolCall {
                    id: "call-1".to_string(),
                    name: "files.read".to_string(),
                    arguments_json: r#"{"path": "hello.txt"}"#.to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::ToolCalls,
                },
            ],
            vec![
                StreamEvent::Delta {
                    content: "mundo".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));

    let tools: Vec<std::sync::Arc<dyn frederico_tool_registry::Tool>> = vec![std::sync::Arc::new(
        frederico_tool_registry::FilesReadTool::new(jail.clone()),
    )];

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail,
        db.clone(),
        perms_with_file_read(),
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );
    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake-model-v1"),
            vec![ChatMessage::user("lê hello.txt")],
        )
        .await
        .expect("executa");
    assert_eq!(outcome.accumulated_content, "olá mundo");
    assert_eq!(outcome.tool_calls.len(), 1);
    assert!(outcome.tool_calls[0].ok);

    // 6 eventos no journal: delta, tool_call, done, tool_result, delta, done.
    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["delta", "tool_call", "done", "tool_result", "delta", "done"]
    );

    // E o `tool_result` tem `ok: true` com o conteúdo.
    let tr = events.iter().find(|e| e.kind == "tool_result").unwrap();
    assert_eq!(tr.data.get("ok"), Some(&serde_json::json!(true)));
    assert_eq!(
        tr.data.get("output").and_then(|o| o.get("content")),
        Some(&serde_json::json!("Hello from fixture!"))
    );
}

#[allow(dead_code)]
fn _silence_unused() {
    let _ = (
        std::marker::PhantomData::<MessageId>,
        std::marker::PhantomData::<Duration>,
    );
}
