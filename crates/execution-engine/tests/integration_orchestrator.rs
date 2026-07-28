//! Tests de integração do `ChatOrchestrator` (Etapa 4.x.y).
//!
//! Estes tests verificam o `ChatOrchestrator` que **agora vive no
//! `execution-engine::orchestrator`** (movido do `provider-engine`
//! na Etapa 4.x.y). O `ChatOrchestrator` delega o loop de stream
//! pro `RunExecutor` — a "regra Journal-then-emit" continua
//! válida (o `RunExecutor` persiste ANTES de processar).

#![cfg(not(doctest))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use frederico_core::{ModelId, ProviderId, RunId};
use frederico_execution_engine::orchestrator::{ChatOrchestrator, ChatOrchestratorError};
use frederico_model_catalog::Catalog;
use frederico_provider_engine::event_sink::RecordingEventSink;
use frederico_provider_engine::fake::trait_level::FakeProviderAdapter;
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_security::fake::FakeClock;
use frederico_security::Clock;
use frederico_storage::{ConversationRepo, Database, MessageEventRepo, MessageRepo, RunRepo};
use frederico_tool_registry::{Jail, ToolRegistry};

// Counter process-wide pra evitar colisão de tempdir entre tests
// paralelos (o `chrono::Utc::now().timestamp_nanos_opt` não garantia
// unicidade em máquinas rápidas e o `Database::open` do SQLite em
// paralelo no mesmo path travava o lock). Mesmo padrão do
// `tests/common/mod.rs` (Etapa 4.x).
static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique = format!(
        "frederico-exec-orch-test-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        n,
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn make_orchestrator() -> (Arc<ChatOrchestrator>, PathBuf, Arc<RecordingEventSink>) {
    let dir = tempdir();
    let db = Database::open(&dir.join("orch.db")).await.unwrap();
    let db = Arc::new(db);
    let clock: Arc<dyn Clock> = FakeClock::new();
    let catalog = Arc::new(Catalog::load().clone());
    let providers = Arc::new({
        let mut m = ProviderMap::new();
        m.insert(Arc::new(FakeProviderAdapter::new("simulated")));
        m
    });
    let runs = RunRegistry::new();
    let sink = Arc::new(RecordingEventSink::new());
    let sink_dyn: Arc<dyn frederico_provider_engine::event_sink::EventSink> = sink.clone();
    // Tooling vazio — os tests de `ChatOrchestrator` da Fase 2
    // não exercitam tools concretas. Os tests com `files.read`
    // estão no `tests/recovery.rs` do execution-engine.
    let tool_registry = ToolRegistry::new();
    let jail = Jail::new(std::env::temp_dir().as_path()).unwrap();
    let orch = ChatOrchestrator::new(
        providers,
        runs,
        sink_dyn,
        db,
        clock,
        catalog,
        tool_registry,
        jail,
        vec![],
        vec![],
    );
    (Arc::new(orch), dir, sink)
}

async fn wait_for_run_completion(orch: &Arc<ChatOrchestrator>, run_id: RunId) {
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Ok(r) = RunRepo::new(&orch.db).get(&run_id).await {
            if r.status != "running" && r.status != "created" {
                return;
            }
        }
    }
}

#[tokio::test]
async fn send_message_persists_user_first() {
    let (orch, _dir, _sink) = make_orchestrator().await;
    let conv_repo = ConversationRepo::new(&orch.db);
    let conv = conv_repo
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let (user_msg, _run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
    assert_eq!(user_msg.role, "user");
    assert_eq!(user_msg.content, "oi");
}

#[tokio::test]
async fn send_message_persists_journal_and_finalizes() {
    let (orch, _dir, _sink) = make_orchestrator().await;
    let conv_repo = ConversationRepo::new(&orch.db);
    let conv = conv_repo
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
    wait_for_run_completion(&orch, run_id).await;
    let run = RunRepo::new(&orch.db).get(&run_id).await.unwrap();
    assert!(
        run.status == "completed",
        "status inesperado: {}",
        run.status
    );
    let msgs = MessageRepo::new(&orch.db)
        .list_for_conversation(&conv.id)
        .await
        .unwrap();
    let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
    let events = MessageEventRepo::new(&orch.db)
        .list_for_message(&asst.id, 0)
        .await
        .unwrap();
    assert!(events.iter().any(|e| e.kind == "delta"));
    assert!(events.iter().any(|e| e.kind == "done"));
}

#[tokio::test]
async fn get_events_with_since_seq_skips_old() {
    let (orch, _dir, _sink) = make_orchestrator().await;
    let conv_repo = ConversationRepo::new(&orch.db);
    let conv = conv_repo
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
    wait_for_run_completion(&orch, run_id).await;
    let msgs = MessageRepo::new(&orch.db)
        .list_for_conversation(&conv.id)
        .await
        .unwrap();
    let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
    let all = orch.get_events(asst.id, 0).await.unwrap();
    assert!(!all.is_empty());
    let from_1 = orch.get_events(asst.id, 1).await.unwrap();
    assert_eq!(from_1.len(), all.len() - 1);
}

#[tokio::test]
async fn cancel_run_marks_requested() {
    let (orch, _dir, _sink) = make_orchestrator().await;
    let conv_repo = ConversationRepo::new(&orch.db);
    let conv = conv_repo
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let (_user, run_id) = orch.send_message(conv.id, "oi".to_string()).await.unwrap();
    let _ = orch.cancel_run(run_id).await;
}

#[tokio::test]
async fn unknown_provider_returns_error() {
    let (orch, _dir, _sink) = make_orchestrator().await;
    let conv_repo = ConversationRepo::new(&orch.db);
    let conv = conv_repo
        .create(
            &ProviderId::new("not_registered"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .unwrap();
    let r = orch.send_message(conv.id, "oi".to_string()).await;
    assert!(matches!(r, Err(ChatOrchestratorError::ProviderNotFound(_))));
}
