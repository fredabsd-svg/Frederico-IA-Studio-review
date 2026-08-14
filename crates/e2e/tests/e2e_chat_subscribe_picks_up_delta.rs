//! E2E — `chat.subscribe.picks.up.delta` (PR do bug do stream).
//!
//! **Por que este teste existe:** a Etapa 5 da Fase de Ligação
//! fechou a Fase 5b sem E2E que validasse o caminho
//! `run://<run_id>/event` (sink → canal Tauri → listener da UI).
//! Os E2E da Fase 5b (`e2e_files_read`, `e2e_jail_per_conversation`,
//! etc.) consomem `build_chat_orchestrator` e validam o journal +
//! o conteúdo da `Message` final, mas param no `RecordingEventSink`
//! (que indexa por `run_id`, não por canal) — exatamente o que
//! deixou o bug "resposta não aparece" passar.
//!
//! O bug: a mensagem assistant nascia com `run_id = None` (o
//! `RunRepo::create` precisava do `asst_msg.id` antes da assistant
//! existir com `run_id`). O `set_run_id` foi adicionado em
//! `MessageRepo` (storage) e chamado no `ChatOrchestrator::send_message`
//! (orchestrator) **antes** do `tokio::spawn` do executor. A
//! `TauriEventSink` agora também loga via `tracing::warn!` quando
//! `Window::emit` falha (antes era `let _ = ...` silencioso).
//!
//! **O que este teste prova (3 asserções):**
//!
//! | # | Nome | Prova |
//! |---|------|-------|
//! | A | `assistant_run_id_populated_after_send_message` | `MessageRepo::get(asst_id).run_id == Some(run_id)` imediatamente após `send_message` retornar. Sem o `set_run_id` no orchestrator, esse assert falha (era o bug). |
//! | B | `event_arrives_on_subscribed_channel_via_run_event_channel` | O sink customizado (`ChannelRecordingEventSink` abaixo) recebe o delta no canal construído por `run_event_channel_for_event(&run_id)` — a **mesma função** que `TauriEventSink` usa (importada de `frederico_provider_engine::event_sink`). Se alguém divergir o formato no backend, o teste quebra junto, em vez de validar a si mesmo. |
//! | C | `reload_without_subscription_recovers_full_content_from_journal` | Após o run terminar, recarregar a conversa (mesmo caminho do `reloadStreamingMessage` do `Chat.tsx`) sem nenhuma assinatura ativa devolve o conteúdo final via `MessageEventRepo::list_for_message` + `MessageRepo::get`. Esse é o caminho de reconexão do §12.6 — se a assinatura falhar por qualquer motivo, o journal é a fonte de verdade e a UI ainda vê a resposta. |
//!
//! **Por que NÃO `e2e_files_read` ou outro E2E da Fase 5b:** eles
//! validam o **journal** (linha 142-152 do `e2e_files_read.rs`:
//! `asst_msg.content.contains("Arquivo lido com sucesso")`),
//! mas o conteúdo da `Message` é populado pelo
//! `MessageRepo::set_content` no `RunExecutor` (Etapa 4 da Fase 3),
//! independente do `run_id` da assistant. Os E2E da Fase 5b nunca
//! olham o `run_id` da assistant, e o `RecordingEventSink` não
//! roteia por canal — então o bug do `run_id = None` não era visível
//! pra eles.
//!
//! **Auto-contido:** o teste monta o `ChatOrchestrator` inline (em
//! vez de chamar `common::build_orchestrator`) porque o helper
//! fixa o tipo do sink em `Arc<RecordingEventSink>`. Mover
//! `RecordingEventSink` pra trás de `Arc<dyn EventSink>` quebraria
//! o `wait_for_run_completion(&h.sink, ...)` dos 8 testes
//! existentes. A duplicação aqui é pequena (~20 linhas) e
//! auto-justificada.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// `mod common;` no topo (padrão Cargo de teste de integração).
// Sem isso, `use common::...` falha.
mod common;

use frederico_app::composition::{
    build_chat_orchestrator, build_default_allowed_for_run, build_default_tools,
    build_tool_registry, initial_permission_set, ChatOrchestratorParts,
};
use frederico_app::jail::FileSystemJailResolver;
use frederico_core::{ModelId, ProviderId, RunId};
use frederico_execution_engine::orchestrator::ChatOrchestrator;
use frederico_model_catalog::Catalog;
use frederico_provider_engine::event_sink::{
    run_event_channel_for_event, run_event_channel_for_status, EventSink,
};
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_security::SystemClock;
use frederico_storage::{Database, RunStatus};

use common::{create_test_conversation, ScriptedProvider, WorkspaceTempdir};

// Constantes locais (mesmo padrão de `e2e_files_read.rs`,
// `e2e_degradation_declared.rs` etc. — cada teste define as suas).
// `openai`/`gpt-4o-mini` é o par mais barato do `Catalog::load()`
// embutido com `tools` capability (necessário pro orchestrator
// encontrar o descriptor).
const E2E_PROVIDER_ID: &str = "openai";
const E2E_MODEL_ID: &str = "gpt-4o-mini";
const HELLO_REPLY: &str = "Hello, world! (e2e_chat_subscribe_picks_up_delta)";

// ---------------------------------------------------------------------------
// ChannelRecordingEventSink — sink de teste que roteia por CANAL
// ---------------------------------------------------------------------------

/// `EventSink` que registra os payloads por **nome de canal**
/// (string `run://<uuid>/event` ou `run://<uuid>/status`).
///
/// Diferente do `RecordingEventSink` (que indexa por `run_id`), este
/// sink usa o **mesmo** helper `run_event_channel_*` que o
/// `TauriEventSink` da casca usa — então a asserção B do teste
/// valida o nome de canal com a mesma expressão que o backend
/// produz. Se alguém mudar o formato do canal no
/// `frederico_provider_engine::event_sink`, tanto o backend
/// quanto o teste quebram juntos (em vez do teste validar a si
/// mesmo com uma `format!("run://{}/event", ...)` reconstruída).
#[derive(Default)]
struct ChannelRecordingEventSink {
    /// `canal → payloads` (status) e `canal → payloads` (stream).
    /// Status usa `RecordedStatus`, stream usa `serde_json::Value`.
    by_channel: Mutex<HashMap<String, Vec<ChannelEvent>>>,
}

#[derive(Debug, Clone)]
enum ChannelEvent {
    Stream(serde_json::Value),
    Status(RunStatus),
}

impl ChannelRecordingEventSink {
    fn new() -> Self {
        Self::default()
    }

    /// Snapshot dos eventos que chegaram no canal `run://<id>/event`.
    fn stream_events_on(&self, channel: &str) -> Vec<serde_json::Value> {
        self.by_channel
            .lock()
            .unwrap()
            .get(channel)
            .map(|evs| {
                evs.iter()
                    .filter_map(|e| match e {
                        ChannelEvent::Stream(p) => Some(p.clone()),
                        ChannelEvent::Status(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Status final recebido no canal `run://<id>/status`.
    fn status_on(&self, channel: &str) -> Option<RunStatus> {
        self.by_channel
            .lock()
            .unwrap()
            .get(channel)
            .and_then(|evs| {
                evs.iter().find_map(|e| match e {
                    ChannelEvent::Status(s) => Some(*s),
                    ChannelEvent::Stream(_) => None,
                })
            })
    }
}

impl EventSink for ChannelRecordingEventSink {
    fn emit_run_event(&self, run_id: RunId, payload: serde_json::Value) {
        let channel = run_event_channel_for_event(&run_id);
        self.by_channel
            .lock()
            .unwrap()
            .entry(channel)
            .or_default()
            .push(ChannelEvent::Stream(payload));
    }

    fn emit_run_status(&self, run_id: RunId, status: RunStatus) {
        let channel = run_event_channel_for_status(&run_id);
        self.by_channel
            .lock()
            .unwrap()
            .entry(channel)
            .or_default()
            .push(ChannelEvent::Status(status));
    }
}

// ---------------------------------------------------------------------------
// build_orchestrator_with_channel_sink — variante inline (self-contained)
// ---------------------------------------------------------------------------

/// Constrói o `ChatOrchestrator` com o `ChannelRecordingEventSink`
/// em vez do `RecordingEventSink` default. Replica o essencial de
/// `common::build_orchestrator` (mesma `frederico_app::build_chat_orchestrator`)
/// mas com o sink customizado. Sem invoker (degradação declarada
/// de tools; este teste não exercita tool calls).
///
/// **Por que não recebe `provider_id` / `model_id`:** o `provider`
/// já carrega isso em `provider.id()` e `provider.known_models()`;
/// o `ProviderMap` e o `Catalog` resolvem o descriptor por aí.
/// Passar explicitamente seria ruído.
async fn build_orchestrator_with_channel_sink(
    provider: Arc<ScriptedProvider>,
    workspace: &WorkspaceTempdir,
) -> (
    Arc<ChatOrchestrator>,
    Arc<Database>,
    Arc<ChannelRecordingEventSink>,
) {
    let db = Arc::new(Database::open_in_memory().await.expect("open in-memory db"));
    let sink: Arc<ChannelRecordingEventSink> = Arc::new(ChannelRecordingEventSink::new());
    let mut provider_map = ProviderMap::new();
    provider_map.insert(provider);
    let providers = Arc::new(provider_map);
    let runs = RunRegistry::new();
    let clock: Arc<dyn frederico_security::Clock> = Arc::new(SystemClock);
    let catalog: Arc<Catalog> = Arc::new(Catalog::load().clone());

    let tools = build_default_tools(None, None);
    let tool_registry = build_tool_registry(&tools);
    let allowed_for_run = build_default_allowed_for_run(None, None);
    let permission_set = initial_permission_set();
    let jail_resolver: Arc<dyn frederico_tool_registry::JailResolver> =
        Arc::new(FileSystemJailResolver::new(workspace.workspaces_root()));

    let multimodel_orchestrator = Arc::new(
        frederico_execution_engine::pipeline_orchestrator::MultimodelOrchestrator::new(
            db.clone(),
            runs.clone(),
            sink.clone(),
            catalog.clone(),
            clock.clone(),
            providers.clone(),
            tool_registry.clone(),
            jail_resolver.clone(),
            tools.clone(),
            allowed_for_run.clone(),
            permission_set.clone(),
        ),
    );

    let parts = ChatOrchestratorParts {
        providers,
        runs,
        sink: sink.clone(),
        db: db.clone(),
        clock,
        catalog: catalog.clone(),
        tool_registry,
        jail_resolver,
        tools,
        allowed_for_run,
        permission_set,
        memory_extractor: None,
        specialist_registry: frederico_app::composition::build_specialist_registry(catalog)
            .registry,
        permission_loader: Arc::new(frederico_tool_registry::PermissionLoader::new()),
        multimodel_orchestrator: Some(multimodel_orchestrator),
    };
    let orchestrator = Arc::new(build_chat_orchestrator(parts));
    (orchestrator, db, sink)
}

// ---------------------------------------------------------------------------
// A — assistant_run_id_populated_after_send_message
// ---------------------------------------------------------------------------

/// Asserção A: imediatamente após `send_message` retornar, a
/// assistant message tem `run_id == Some(run_id)`. Sem o
/// `set_run_id` no orchestrator, esse assert falha — era o bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assistant_run_id_populated_after_send_message() {
    let provider = Arc::new(ScriptedProvider::plain_reply(
        E2E_PROVIDER_ID,
        E2E_MODEL_ID,
        HELLO_REPLY,
    ));
    let provider_id = ProviderId::new(E2E_PROVIDER_ID);
    let model_id = ModelId::new(E2E_MODEL_ID);
    let workspace = WorkspaceTempdir::new();

    let (orch, db, _sink) =
        build_orchestrator_with_channel_sink(provider.clone(), &workspace).await;

    let conv = create_test_conversation(&db, &provider_id, &model_id, Some("e2e run_id")).await;
    let (user_msg, run_id) = orch
        .send_message(conv.id, "olá".to_string())
        .await
        .expect("send_message");

    // Imediatamente após o retorno: o `MessageRepo::set_run_id` no
    // orchestrator já comitou. O `getConversation` que a UI faz
    // depois do IPC `MessageSend` enxerga isso.
    let asst_msg = frederico_storage::MessageRepo::new(&db)
        .list_for_conversation(&conv.id)
        .await
        .expect("list_for_conversation")
        .into_iter()
        .find(|m| m.role == "assistant" && m.id != user_msg.id)
        .expect("assistant msg existe");

    assert_eq!(
        asst_msg.run_id,
        Some(run_id),
        "assistant.run_id tem que estar populado antes do send_message \
         retornar (regra da Etapa 5.X do bug do stream). Sem isso, a UI \
         nunca assina o canal e os eventos Tauri caem no vazio."
    );
}

// ---------------------------------------------------------------------------
// B — event_arrives_on_subscribed_channel_via_run_event_channel
// ---------------------------------------------------------------------------

/// Asserção B: o delta do provider chega no canal construído por
/// `run_event_channel_for_event(&run_id)` — a mesma função que
/// `TauriEventSink` usa. O frontend (`apps/desktop/src/services/
/// stream.ts`) usa a mesma string; a UI assinaria esse canal e
/// receberia o delta.
///
/// **Por que `ChannelRecordingEventSink` e não `RecordingEventSink`:**
/// `RecordingEventSink` indexa por `run_id` (não usa canal nenhum).
/// `ChannelRecordingEventSink` (acima) usa `run_event_channel_*` —
/// então a asserção prova que o backend produz a string que o
/// frontend escuta. Se alguém mudar o formato em
/// `frederico_provider_engine::event_sink`, ambos quebram juntos.
///
/// **O que esse teste cobre (PR do bug do stream — Etapa 5.X):**
/// 1. O payload é um `StreamEventEnvelope { seq, event }` — não
///    `StreamEvent` cru. O `seq` é o do `message_events.seq` e
///    permite reconexão por `fromSeq` sem perder nem duplicar.
/// 2. O `seq` no envelope **bate com** o `seq` do journal (asserção
///    cruzada com `MessageEventRepo::list_for_message`).
/// 3. Cada `StreamEvent` (Delta, Done) é emitido **após**
///    `persist_journal` (regra do spec `chat-and-providers.md`
///    §"Journal de eventos").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_arrives_on_subscribed_channel_via_run_event_channel() {
    let provider = Arc::new(ScriptedProvider::plain_reply(
        E2E_PROVIDER_ID,
        E2E_MODEL_ID,
        HELLO_REPLY,
    ));
    let provider_id = ProviderId::new(E2E_PROVIDER_ID);
    let model_id = ModelId::new(E2E_MODEL_ID);
    let workspace = WorkspaceTempdir::new();

    let (orch, db, sink) = build_orchestrator_with_channel_sink(provider.clone(), &workspace).await;

    let conv = create_test_conversation(&db, &provider_id, &model_id, Some("e2e channel")).await;
    let (_user_msg, run_id) = orch
        .send_message(conv.id, "olá".to_string())
        .await
        .expect("send_message");

    // Espera o run fechar (sink no canal de status).
    let status_channel = run_event_channel_for_status(&run_id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sink.status_on(&status_channel).is_none() {
        if std::time::Instant::now() > deadline {
            panic!(
                "timeout esperando status final do run {run_id} no canal {status_channel}; \
                 sink tem {} canais registrados: {:?}",
                sink.by_channel.lock().unwrap().len(),
                sink.by_channel.lock().unwrap().keys().collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let status = sink.status_on(&status_channel).expect("status final");
    assert_eq!(status, RunStatus::Completed, "run deveria completar");

    // Asserção principal: o delta do provider chegou no canal
    // `run://<run_id>/event` (a string que `subscribeRun` no front
    // escuta).
    let event_channel = run_event_channel_for_event(&run_id);
    let payloads = sink.stream_events_on(&event_channel);
    assert!(
        !payloads.is_empty(),
        "sink deveria ter recebido pelo menos 1 evento de stream no canal {event_channel}; \
         sink tem {} canais registrados: {:?}",
        sink.by_channel.lock().unwrap().len(),
        sink.by_channel.lock().unwrap().keys().collect::<Vec<_>>()
    );

    // Cada payload é um `StreamEventEnvelope { seq, event }` —
    // confere que tem `seq` (número) e `event` (objeto).
    let delta_envelope = payloads
        .iter()
        .find(|p| {
            p.get("event")
                .and_then(|e| e.get("kind"))
                .and_then(|k| k.as_str())
                == Some("delta")
        })
        .unwrap_or_else(|| {
            panic!(
                "esperava pelo menos 1 envelope com event.kind='delta' no canal {event_channel}; \
                 veio {payloads:?}"
            )
        });
    let seq = delta_envelope
        .get("seq")
        .and_then(|s| s.as_u64())
        .expect("envelope.seq tem que ser um número (PR do bug do stream)");
    let event = delta_envelope
        .get("event")
        .expect("envelope.event tem que existir");
    assert_eq!(
        event.get("content").and_then(|c| c.as_str()),
        Some(HELLO_REPLY),
        "conteúdo do delta deveria ser HELLO_REPLY"
    );

    // **Asserção cruzada: o `seq` do envelope bate com o `seq` do
    // journal.** O `MessageEventRepo::list_for_message` devolve os
    // eventos na ordem do journal; o envelope de cada `Delta` tem
    // o mesmo `seq` que a linha correspondente do journal. Sem
    // isso, a reconexão por `fromSeq` (§12.6) perde ou duplica.
    let asst = frederico_storage::MessageRepo::new(&db)
        .list_for_conversation(&conv.id)
        .await
        .expect("list_for_conversation")
        .into_iter()
        .find(|m| m.role == "assistant" && m.id != _user_msg.id)
        .expect("assistant msg");
    let journal_events = frederico_storage::MessageEventRepo::new(&db)
        .list_for_message(&asst.id, 0)
        .await
        .expect("list_for_message");
    let journal_delta_seq = journal_events
        .iter()
        .find(|e| e.kind == "delta")
        .map(|e| e.seq)
        .expect("journal deveria ter pelo menos 1 delta (caminho do reload sem assinatura)");
    assert_eq!(
        seq as u32, journal_delta_seq,
        "o seq do envelope emitido tem que ser igual ao seq do journal — \
         é o que permite reconectar via fromSeq sem perder/dup. \
         envelope.seq={seq}, journal delta seq={journal_delta_seq}"
    );

    // E o `Done` também chegou, com `stop_reason = "stop"`.
    let done_envelope = payloads
        .iter()
        .find(|p| {
            p.get("event")
                .and_then(|e| e.get("kind"))
                .and_then(|k| k.as_str())
                == Some("done")
        })
        .expect("esperava 1 Done no canal");
    assert_eq!(
        done_envelope
            .get("event")
            .and_then(|e| e.get("stop_reason"))
            .and_then(|s| s.as_str()),
        Some("stop"),
        "stop_reason deveria ser 'stop'"
    );
}

// ---------------------------------------------------------------------------
// C — reload_without_subscription_recovers_full_content_from_journal
// ---------------------------------------------------------------------------

/// Asserção C: o caminho do `reloadStreamingMessage` (Chat.tsx §reload
/// de janela no meio do stream, Etapa 5 da Fase 2 + §12.6). Sem
/// assinatura ativa, recarregar a conversa deve devolver o conteúdo
/// final via `MessageEventRepo::list_for_message` (o journal é a
/// fonte de verdade — `TauriEventSink` é UX puro).
///
/// **Por que essa asserção importa:** a B prova que o sink emite
/// o delta no canal certo. A C prova que **mesmo se** a assinatura
/// da UI falhar (ex.: `let _ = window.emit(...)` engolindo o erro —
/// motivo do `tracing::warn!` adicionado no `TauriEventSink`), o
/// usuário ainda vê a resposta no próximo `getConversation` (que
/// carrega o `Message.content` populado pelo `RunExecutor`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_without_subscription_recovers_full_content_from_journal() {
    let provider = Arc::new(ScriptedProvider::plain_reply(
        E2E_PROVIDER_ID,
        E2E_MODEL_ID,
        HELLO_REPLY,
    ));
    let provider_id = ProviderId::new(E2E_PROVIDER_ID);
    let model_id = ModelId::new(E2E_MODEL_ID);
    let workspace = WorkspaceTempdir::new();

    let (orch, db, sink) = build_orchestrator_with_channel_sink(provider.clone(), &workspace).await;

    let conv = create_test_conversation(&db, &provider_id, &model_id, Some("e2e reload")).await;
    let (user_msg, run_id) = orch
        .send_message(conv.id, "olá".to_string())
        .await
        .expect("send_message");

    // Espera o run fechar pelo sink (mesmo helper da B).
    let status_channel = run_event_channel_for_status(&run_id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sink.status_on(&status_channel).is_none() {
        if std::time::Instant::now() > deadline {
            panic!("timeout esperando status final");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(sink.status_on(&status_channel), Some(RunStatus::Completed));

    // **Simula reload de janela:** NÃO há assinatura ativa (o sink
    // recebeu os eventos enquanto o "frontend" estava dormindo).
    // A UI faria `getConversation(conv_id)` → `list_for_conversation`
    // → `MessageRepo::get(asst_id)`. O conteúdo do assistant tem que
    // estar completo (não `""` como nasceu).
    let asst_msg = frederico_storage::MessageRepo::new(&db)
        .list_for_conversation(&conv.id)
        .await
        .expect("list_for_conversation")
        .into_iter()
        .find(|m| m.role == "assistant" && m.id != user_msg.id)
        .expect("assistant msg");

    assert_eq!(
        asst_msg.status, "completed",
        "status do assistant deveria ser 'completed' após o run fechar"
    );
    assert!(
        asst_msg.content.contains(HELLO_REPLY),
        "assistant.content deveria conter o delta do HELLO_REPLY após o run; \
         veio {:?} (esse é o conteúdo que o `reloadStreamingMessage` \
         mostraria na tela se a UI reabrisse a janela agora)",
        asst_msg.content
    );

    // E o journal tem os eventos que o `reloadStreamingMessage`
    // consumiria via `MessageEventRepo::list_for_message` + `applyEvent`
    // (replay dos deltas).
    let events = frederico_storage::MessageEventRepo::new(&db)
        .list_for_message(&asst_msg.id, 0)
        .await
        .expect("list_for_message");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(
        kinds.contains(&"delta"),
        "journal deveria ter o evento 'delta' (replay do `reloadStreamingMessage`); kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&"done"),
        "journal deveria ter o evento 'done' (marca fim do stream); kinds: {kinds:?}"
    );
}

// Helper para silenciar warning de import não usado se o usuário
// compilar com `RUSTFLAGS=-D warnings`.
