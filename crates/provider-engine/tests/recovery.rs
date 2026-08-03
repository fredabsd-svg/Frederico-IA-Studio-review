//! Testes de recovery do `ChatOrchestrator`.
//!
//! Validam a regra do **"Journal de eventos"**: cada `StreamEvent` é
//! persistido em `message_events` **antes** de ser emitido via
//! `EventSink`. Se o processo for morto no meio de um run, o journal
//! no SQLite é a fonte de verdade — ao reabrir, o orquestrador
//! novo consegue ler os eventos persistidos via
//! `MessageEventRepo::list_for_message`.
//!
//! Estes tests não falam com Tauri nem com rede. Usam
//! `FakeProviderAdapter` (trait-level, gera eventos direto) +
//! `RecordingEventSink` (captura o que foi emitido) + DB em tempdir.
//!
//! Cobertura:
//! 1. **Recovery simples**: orquestrador A inicia um run, persiste N
//!    eventos, é dropado. Orquestrador B reabriu o mesmo db e
//!    consegue ler os eventos via `MessageEventRepo`.
//! 2. **Cancel persistido**: cancel mid-run grava status final no
//!    db; orquestrador B vê status consistente pós-recovery.
//! 3. **user msg persistida antes de I/O**: a mensagem do user
//!    está no db ANTES de qualquer evento de stream.

#![cfg(not(doctest))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use frederico_core::{MessageId, ModelId, ProviderId};
use frederico_execution_engine::orchestrator::ChatOrchestrator;
use frederico_model_catalog::Catalog;
use frederico_provider_engine::event_sink::RecordingEventSink;
use frederico_provider_engine::fake::trait_level::FakeProviderAdapter;
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_provider_engine::types::StreamEvent;
use frederico_security::fake::FakeClock;
use frederico_security::Clock;
use frederico_storage::{
    ConversationRepo, Database, MessageEventRepo, MessageRepo, RunRepo, RunStatus,
};
use frederico_tool_registry::{Jail, ToolRegistry};

/// Contador atômico: o relógio sozinho não garante unicidade. No Windows
/// a granularidade de `timestamp_nanos` é grosseira, e os testes deste
/// arquivo rodam em paralelo no mesmo processo — dois deles podiam cair no
/// mesmo valor, compartilhar o mesmo `test.db` e disparar duas migrações
/// concorrentes ("UNIQUE constraint failed: _sqlx_migrations.version").
/// Mesmo padrão já aplicado no `execution-engine`.
static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique = format!(
        "frederico-recovery-test-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        n,
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).expect("cria tempdir");
    dir
}

fn build_orchestrator(
    db: Arc<Database>,
    events: Vec<StreamEvent>,
    sink: Arc<RecordingEventSink>,
) -> Arc<ChatOrchestrator> {
    let mut map = ProviderMap::new();
    let adapter = FakeProviderAdapter::new("simulated").with_events(events);
    map.insert(Arc::new(adapter));
    let providers = Arc::new(map);
    // `RunRegistry::new()` já retorna `Arc<Self>` (igual
    // `FakeCredentialStore::new()`). Ver `src/run_registry.rs:23`.
    let runs: Arc<RunRegistry> = RunRegistry::new();
    let catalog = Arc::new(Catalog::load().clone());
    let clock_fc: Arc<FakeClock> = FakeClock::new();
    let clock: Arc<dyn Clock> = clock_fc;
    Arc::new(ChatOrchestrator::new(
        providers,
        runs,
        sink,
        db,
        clock,
        catalog,
        ToolRegistry::new(),
        frederico_tool_registry::static_jail_resolver(
            Jail::new(std::env::temp_dir().as_path()).unwrap(),
        ),
        vec![],
        vec![],
        frederico_tool_registry::PermissionSet::default(),
        None, // memory_extractor (Etapa 5 da Fase 4)
    ))
}

fn make_events(n_deltas: usize) -> Vec<StreamEvent> {
    let mut out: Vec<StreamEvent> = (0..n_deltas)
        .map(|i| StreamEvent::Delta {
            content: format!("d{i} "),
        })
        .collect();
    out.push(StreamEvent::Done {
        stop_reason: frederico_provider_engine::types::StopReason::Stop,
    });
    out
}

/// Espera o sink ver um evento de status específico. Polling
/// pequeno — o `FakeProviderAdapter` emite tudo no mesmo tick.
/// Polling de 20ms; 5s de janela total (250 iteracoes). Era 2s
/// (100 iteracoes) mas flakeou no CI Windows Server 2022 em
/// 2026-07-31 (provavelmente overhead maior do runner do que
/// dev local - 3-5x em Windows). 5s e folgado o suficiente
/// pra nao mascarar regressao real mas tolerar slowness de
/// CI compartilhado.
async fn wait_for_status(sink: &RecordingEventSink, target: RunStatus) {
    for _ in 0..250 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let events = sink.events.lock().unwrap();
        if events
            .iter()
            .any(|(_, ev)| matches!(ev, frederico_provider_engine::event_sink::RecordedEvent::Status(s) if *s == target))
        {
            return;
        }
    }
    panic!("sink não viu status {target:?} em 5s");
}

#[tokio::test(flavor = "current_thread")]
async fn journal_persists_events_across_orchestrator_drop() {
    let dir = tempdir();
    let db_path = dir.join("test.db");

    // ---- Sessão A ----
    let db_a = Arc::new(Database::open(&db_path).await.expect("abre db A"));
    let conv = ConversationRepo::new(&db_a)
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            Some("recovery-1"),
        )
        .await
        .expect("cria conversa");

    let sink_a = Arc::new(RecordingEventSink::new());
    let orch_a = build_orchestrator(db_a.clone(), make_events(5), sink_a.clone());
    let (user_msg, run_id) = orch_a
        .send_message(conv.id, "hello".to_string())
        .await
        .expect("send_message A");
    let _user_message_id: MessageId = user_msg.id;

    // O `Run.message_id` aponta pra assistant message (cujo
    // journal é o do stream). É o que vamos ler.
    let run_repo_a = RunRepo::new(&db_a);
    let run_a = run_repo_a.get(&run_id).await.expect("run A");
    let assistant_message_id: MessageId = run_a.message_id;

    // Espera o run completar.
    wait_for_status(&sink_a, RunStatus::Completed).await;

    // Snapshot do journal ANTES do drop.
    let event_repo_a = MessageEventRepo::new(&db_a);
    let journal_a = event_repo_a
        .list_for_message(&assistant_message_id, 0)
        .await
        .expect("journal A");
    assert!(
        !journal_a.is_empty(),
        "journal A vazio — bug no orquestrador (regra do Journal-then-emit violada)"
    );

    // ---- Dropa A, abre B ----
    drop(orch_a);
    let db_b = Arc::new(Database::open(&db_path).await.expect("abre db B"));
    let _orch_b = build_orchestrator(db_b.clone(), vec![], Arc::new(RecordingEventSink::new()));

    // ---- Lê journal pós-recovery ----
    let event_repo_b = MessageEventRepo::new(&db_b);
    let journal_b = event_repo_b
        .list_for_message(&assistant_message_id, 0)
        .await
        .expect("journal B");

    // O journal B deve ter o mesmo conteúdo do A.
    assert_eq!(
        journal_a.len(),
        journal_b.len(),
        "tamanho do journal mudou após recovery: A={} B={}",
        journal_a.len(),
        journal_b.len()
    );

    let deltas_a = journal_a.iter().filter(|e| e.kind == "delta").count();
    let deltas_b = journal_b.iter().filter(|e| e.kind == "delta").count();
    assert_eq!(deltas_a, 5, "esperado 5 deltas no journal A");
    assert_eq!(deltas_b, 5, "esperado 5 deltas no journal B após recovery");

    let dones = journal_b.iter().filter(|e| e.kind == "done").count();
    assert!(dones >= 1, "journal B sem Done: {journal_b:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_idempotent_and_status_persists() {
    // Valida que:
    // 1. `cancel_run` pode ser chamado a qualquer momento (idempotente).
    // 2. O status final da run, seja `Completed` ou `Cancelled`, é
    //    persistido no db e visível após recovery.

    let dir = tempdir();
    let db_path = dir.join("test.db");

    let db_a = Arc::new(Database::open(&db_path).await.expect("abre db A"));
    let conv = ConversationRepo::new(&db_a)
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            None,
        )
        .await
        .expect("cria conversa");

    let sink_a = Arc::new(RecordingEventSink::new());
    let orch_a = build_orchestrator(db_a.clone(), make_events(3), sink_a.clone());
    let (_user_msg, run_id) = orch_a
        .send_message(conv.id, "qualquer".to_string())
        .await
        .expect("send_message");

    // Chama cancel_run **após** o run provavelmente já ter terminado
    // (FakeProviderAdapter emite tudo no mesmo tick). Deve ser no-op
    // e não quebrar.
    let cancel_result = orch_a.cancel_run(run_id).await;
    assert!(
        cancel_result.is_ok(),
        "cancel_run deve ser idempotente mesmo se o run já terminou"
    );

    // Espera o sink ver o status final (Completed ou Cancelled).
    let final_status = {
        let mut found = None;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let events = sink_a.events.lock().unwrap();
            // Procura o ÚLTIMO status emitido.
            for (_, ev) in events.iter().rev() {
                if let frederico_provider_engine::event_sink::RecordedEvent::Status(s) = ev {
                    found = Some(*s);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        found.expect("sink não viu nenhum status em 2s")
    };

    // Dropa A, abre B.
    drop(orch_a);
    let db_b = Arc::new(Database::open(&db_path).await.expect("abre db B"));

    // O status persistido no db tem que bater com o status do sink.
    let run_repo = RunRepo::new(&db_b);
    let run_b = run_repo.get(&run_id).await.expect("run B");
    let status_str_b = &run_b.status;
    let expected_str = match final_status {
        RunStatus::Completed => "completed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Failed => "failed",
        RunStatus::Timeout => "timeout",
        RunStatus::Running => "running",
        RunStatus::Created => "created",
    };
    assert_eq!(
        status_str_b, expected_str,
        "status do db B ({status_str_b}) não bate com o do sink ({expected_str})"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn user_message_persisted_before_run_starts() {
    // Regra de teste: a mensagem do user é persistida ANTES de
    // qualquer I/O de rede. Após recovery, ela tem que estar no db
    // mesmo se o orquestrador A for dropado antes do run completar.
    let dir = tempdir();
    let db_path = dir.join("test.db");

    let db_a = Arc::new(Database::open(&db_path).await.expect("abre db A"));
    let conv = ConversationRepo::new(&db_a)
        .create(
            &ProviderId::new("simulated"),
            &ModelId::new("fake-model-v1"),
            Some("user-first"),
        )
        .await
        .expect("cria conversa");

    // Script que demora (sem Done).
    let events_slow: Vec<StreamEvent> = vec![StreamEvent::Delta {
        content: "devagar".to_string(),
    }];
    let sink_a = Arc::new(RecordingEventSink::new());
    let orch_a = build_orchestrator(db_a.clone(), events_slow, sink_a.clone());
    let (user_msg, _run_id) = orch_a
        .send_message(conv.id, "minha mensagem importante".to_string())
        .await
        .expect("send_message");
    let user_id: MessageId = user_msg.id;

    // Imediatamente após `send_message` retornar (que acontece
    // ANTES do run completar), a user msg já deve estar no db.
    // Verificamos no db A direto.
    let msg_repo = MessageRepo::new(&db_a);
    let msgs_a = msg_repo
        .list_for_conversation(&conv.id)
        .await
        .expect("lista A");
    let found_a = msgs_a.iter().find(|m| m.id == user_id);
    assert!(
        found_a.is_some(),
        "user msg não está no db A imediatamente após send_message — viola a regra do Journal-then-emit"
    );
    let user_a = found_a.unwrap();
    assert_eq!(user_a.role, "user");
    assert_eq!(user_a.content, "minha mensagem importante");

    // Drop A e abre B.
    drop(orch_a);
    let db_b = Arc::new(Database::open(&db_path).await.expect("abre db B"));
    let msg_repo_b = MessageRepo::new(&db_b);
    let msgs_b = msg_repo_b
        .list_for_conversation(&conv.id)
        .await
        .expect("lista B");
    let found_b = msgs_b.iter().find(|m| m.id == user_id);
    assert!(found_b.is_some(), "user msg sumiu após recovery");
    assert_eq!(found_b.unwrap().content, "minha mensagem importante");
}
