//! E2E — portão único de transição + `RunEvent` journal (Fase 6, Etapa 2).
//!
//! Caminho exercitado: **modelo → ChatOrchestrator → RunExecutor →
//! `state_mapping::run_state_for_event(current, event)` → portão
//! `apply_transition` → `RunEventRepo::append` (journal) →
//! `RunRepo::set_state_and_heartbeat_tx` → SQLite**.
//!
//! Estes 4 testes são a **prova de caminho real** do portão. Eles
//! não usam `agent-engine::transition::apply_transition`
//! diretamente (que tem 49 testes por par cobrindo a função pura);
//! usam o **caminho completo** que o `RunExecutor` percorre em
//! produção, exatamente o que a §1.13 do `REGRAS-DO-PROJETO` exige
//! (e que o PR #26 documentou como "documentação, não invariante
//! ativo" antes da Etapa 2 da Fase 6 fechar o portão).
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (fronteira dos E2E — sem casca Tauri, sem `document-worker`).
//!
//! O que esta bateria prova (4 testes, todos consumindo
//! `build_chat_orchestrator`):
//! 1. **`run_executor_rejects_invalid_transition_through_orchestrator`**:
//!    o portão rejeita uma transição que a tabela `TRANSITIONS`
//!    não permite. A prova é direta: criar um `Run` em estado
//!    terminal (`Completed`), chamar o executor com um `Delta`,
//!    e afirmar que o `ExecutorError::InvalidTransition` é
//!    propagado pelo orchestrator até virar `RunStatus::Failed`.
//! 2. **`run_event_seq_monotonic_through_orchestrator`**: o
//!    `seq` do `RunEvent` é monotonicamente crescente por `run_id`,
//!    sem buracos, sem duplicatas. A prova: rodar um cenário
//!    feliz, ler o `run_events` por `seq ASC` e afirmar `[1, 2,
//!    3, ...]`. **Cada step do `state_mapping` é um `RunEvent`.**
//! 3. **`valid_transition_persists_in_run_event_journal`**: cada
//!    `RunEvent` carrega `from_state` e `to_state` consistentes
//!    com a tabela de transições. A prova: ler o `run_events`,
//!    e pra cada um, validar via `apply_transition(from, kind) ==
//!    to` (a função pura da `agent-engine` aceita o que o
//!    journal diz).
//! 4. **`recovery_loads_state_from_run_event_journal`**: o
//!    `recovery::recover_stale_runs` lê o **último `RunEvent`**
//!    do run (não o `run.state` legado) como fonte primária do
//!    estado. A prova: criar um run com `state = CallingModel`
//!    e `RunEvent` mais novo mostrando `state = ContinuingModel`
//!    (simulando crash no meio de uma transição), forçar
//!    `last_heartbeat_at` velho, rodar recovery, e afirmar que
//!    o `RunEvent` registrado tem `from_state = ContinuingModel`
//!    (a fonte primária).

mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_agent_engine::{Budget, RunState, RunStateParseError};
use frederico_core::{MessageId, ModelId, ProviderId, ToolId};
use frederico_execution_engine::executor::{ExecutorError, RunExecutor};
use frederico_execution_engine::recovery;
use frederico_provider_engine::types::{ChatMessage, StopReason, StreamEvent};
use frederico_storage::{ConversationRepo, MessageRepo, RunEventRepo, RunRepo};
use frederico_tool_registry::{
    static_jail_resolver, FileReadPermission, FilesReadTool, Jail, PermissionSet, Tool,
    ToolRegistry,
};
use tokio_util::sync::CancellationToken;

use common::{
    build_orchestrator, create_test_conversation, fake_invoker, wait_for_run_completion,
    ScriptedProvider,
};

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";

/// 1. **`run_executor_rejects_invalid_transition_through_orchestrator`**
///
/// O portão fecha: `apply_transition` é consultado **antes** de
/// `set_state`. Prova direta: forçar o run a um estado terminal
/// (`Completed`) **antes** de chamar o `RunExecutor` e ver o
/// `ExecutorError::InvalidTransition` propagar — sem nenhum
/// `RunEvent` espúrio gravado e sem o `runs.state` regredir.
///
/// Por que este teste existe: a Etapa 4 da Fase 3 introduziu
/// `RunExecutor` "para conectar a máquina ao storage", mas o
/// ADR-0025 §Fato (auditoria de 2026-08-04) provou que o
/// caminho real (`state_mapping → RunRepo::set_state`) ignorava
/// a tabela `TRANSITIONS` por completo. A Etapa 2 fecha o
/// portão. Este teste é a prova no caminho real: ele força
/// o estado para um terminal via DB e roda o `RunExecutor`
/// direto (não via orchestrator) — o `state_mapping` consulta
/// `apply_transition` que rejeita com `FromTerminal`, e o
/// executor propaga como `ExecutorError::InvalidTransition`.
/// **Sem este teste, o portão é convenção, não portão.**
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_executor_rejects_invalid_transition_through_orchestrator() {
    // 1. Setup DB + repos via build_orchestrator (mesma composição
    //    da casca). Não usamos o `h.orchestrator` para nada — só
    //    queremos o `h.db` e o `h.provider` (ScriptedProvider).
    let (invoker, manager) = fake_invoker().await;
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        // O provider emite 1 round com Delta + Done{Stop}.
        // O portão deveria rejeitar no Delta porque o run
        // começa em `Completed` (terminal).
        vec![vec![
            StreamEvent::Delta {
                content: "x".to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            },
        ]],
    ));
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider.clone(),
        ProviderId::new(PROVIDER_ID),
        ModelId::new(MODEL_ID),
        None,
    )
    .await;
    let conv = ConversationRepo::new(&h.db)
        .create(&ProviderId::new(PROVIDER_ID), &ModelId::new(MODEL_ID), None)
        .await
        .expect("cria conversa de teste");
    let asst_msg: MessageId = MessageRepo::new(&h.db)
        .create(&conv.id, "assistant", "", None)
        .await
        .expect("cria assistant msg")
        .id;

    // 2. Cria o run **via DB direto** (sem passar pelo
    //    orchestrator). Isso dá controle total sobre o
    //    `run_id` — o orchestrator sempre cria um run novo.
    let run_repo = RunRepo::new(&h.db);
    let run = run_repo
        .create(&conv.id, &asst_msg)
        .await
        .expect("cria run de teste");

    // 3. Força o run para estado terminal `Completed` **antes** de
    //    chamar o executor. A transição `Completed → Streaming`
    //    (via `Delta`) é inválida pela tabela `TRANSITIONS` —
    //    terminais são imutáveis. O portão deve rejeitar com
    //    `FromTerminal { from: Completed }`.
    //
    //    Usamos `set_state` (legado) aqui **propositalmente**:
    //    é a única forma de levar o run a um estado terminal
    //    sem passar pelo caminho de produção (que sempre começa
    //    em `Created`). O ponto do teste é justamente mostrar
    //    que mesmo que alguém use `set_state` para deixar o
    //    run num estado ruim, o `RunExecutor` recusa.
    run_repo
        .set_state_unchecked(&run.id, RunState::Completed)
        .await
        .expect("força run em Completed");

    // 4. Constroi o `RunExecutor` **direto** (não via
    //    orchestrator). Mesma assinatura que o orchestrator
    //    usa internamente — o teste exercita o **mesmo
    //    construtor** que a casca usa, então o caminho de
    //    código é idêntico ao de produção.
    let registry = ToolRegistry::new();
    // `Jail::new` faz `canonicalize()` no root — o
    // `workspaces_root()` ainda não existe (é criado por
    // conversa). Crio o diretório vazio pra satisfazer o
    // `canonicalize`. Não é trabalho de produção — produção
    // já cria via `FileSystemJailResolver` antes da 1ª
    // mensagem (Etapa de Ligação §3).
    std::fs::create_dir_all(h.workspace.workspaces_root()).expect("cria workspaces/");
    let jail = Jail::new(&h.workspace.workspaces_root()).expect("cria jail");
    let jail_resolver = static_jail_resolver(jail);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FilesReadTool::new())];
    let permissions = PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        ..Default::default()
    };
    let mut executor = RunExecutor::new(
        provider.clone(),
        registry,
        jail_resolver,
        (*h.db).clone(),
        permissions,
        vec![ToolId::new("files.read")],
        tools,
        Budget::default(),
        CancellationToken::new(),
    );

    // 5. Roda o executor com o run em estado terminal. O
    //    `state_mapping` consulta `apply_transition` no
    //    primeiro `Delta`: o portão rejeita com
    //    `FromTerminal { from: Completed }`, e o executor
    //    (recovery de transições inválidas, Etapa 2) marca
    //    o run como `Failed` e grava um `RunEvent` de
    //    `UnrecoverableError` antes de propagar o erro.
    let result = executor
        .run(
            asst_msg,
            run.id,
            ModelId::new(MODEL_ID),
            vec![ChatMessage::user("oi")],
        )
        .await;

    // 6. Asserções: o portão rejeita e a mensagem é legível.
    //    O `from` reportado no erro é o estado **pós-recovery**
    //    (Failed) — é o estado em que o `current_state` ficou
    //    depois que o executor marcou como `Failed` antes de
    //    propagar. O `from` **pré-tentativa** (Completed, que
    //    forçamos antes do executor) está no `from_state` do
    //    `RunEvent` (assertion #8 abaixo) — a auditoria é
    //    honesta.
    //
    //    **Por que o `from` é Failed e não Completed**: o
    //    `UnrecoverableError` é uma aresta **especial** que o
    //    executor aplica bypassando o portão (justamente porque
    //    o `from` é terminal — a regra 1 do `apply_transition`
    //    rejeita terminais). O estado pós-recovery é `Failed`
    //    e o `from` no erro reflete isso. O `from` real
    //    (Completed) está no journal.
    match result {
        Err(ExecutorError::InvalidTransition {
            from,
            ref stream_event,
            ref cause,
        }) => {
            assert_eq!(
                from,
                RunState::Failed,
                "from no erro é o estado pós-recovery (Failed); \
                 o from pré-tentativa (Completed) está no from_state do RunEvent"
            );
            // A mensagem tem que ser útil pro log do produto —
            // não pode ser vazia nem "unknown".
            assert!(
                !stream_event.is_empty() && stream_event != "unknown",
                "stream_event tem que ser legível, veio: {stream_event:?}"
            );
            assert!(
                !cause.is_empty() && cause != "unknown",
                "cause tem que ser legível, veio: {cause:?}"
            );
        }
        other => panic!("esperava Err(ExecutorError::InvalidTransition), veio: {other:?}"),
    }

    // 7. Asserção: o `runs.state` final é `Failed` — o
    //    executor não ignora a rejeição do portão, ele marca
    //    o run como `Failed` (recovery determinístico). Se o
    //    `run.state` continuasse `Completed`, o portão não
    //    estaria sendo exercido no caminho de produto.
    let final_run = run_repo.get(&run.id).await.expect("get run");
    let final_state: RunState = final_run.state.parse().expect("state é um RunState válido");
    assert_eq!(
        final_state,
        RunState::Failed,
        "runs.state deveria ser Failed (recovery do portão)"
    );

    // 8. Asserção: o `RunEvent` de `UnrecoverableError` foi
    //    gravado **com o from pré-tentativa no payload**.
    //    É a auditoria honesta: o journal registra exatamente
    //    de onde o portão tentou transicionar (Completed) e
    //    o erro estruturado que ele devolveu. Sem este evento,
    //    a rejeição do portão seria silenciosa — o run
    //    ficaria como `Failed` sem nenhuma trilha de qual
    //    evento causou a falha.
    let run_event_repo = RunEventRepo::new(&h.db);
    let events = run_event_repo
        .list_for_run(&run.id)
        .await
        .expect("list run_events");
    assert_eq!(
        events.len(),
        1,
        "esperava exatamente 1 RunEvent (UnrecoverableError), veio {}: {:?}",
        events.len(),
        events
    );
    let ev = &events[0];
    assert_eq!(
        ev.kind, "unrecoverable_error",
        "kind deveria ser unrecoverable_error (recovery do portão)"
    );
    assert_eq!(
        ev.from_state.as_deref(),
        Some("completed"),
        "from_state do RunEvent deveria ser Completed (estado pré-tentativa)"
    );
    assert_eq!(
        ev.to_state.as_deref(),
        Some("failed"),
        "to_state do RunEvent deveria ser Failed (recovery determinístico)"
    );
    // O payload tem o erro estruturado do `apply_transition`
    // — prova que a causa raiz está no journal pra debug.
    let payload: serde_json::Value =
        serde_json::from_str(&ev.payload_json).expect("payload_json é JSON válido");
    let from_in_payload = payload
        .get("from")
        .and_then(|v| v.as_str())
        .expect("payload.from presente");
    assert_eq!(
        from_in_payload, "completed",
        "payload.from deveria ser completed (causa raiz)"
    );
    let transition_error = payload
        .get("transition_error")
        .and_then(|v| v.as_str())
        .expect("payload.transition_error presente");
    assert!(
        transition_error.contains("FromTerminal") || transition_error.contains("from_terminal"),
        "payload.transition_error deveria mencionar FromTerminal, veio: {transition_error:?}"
    );
}

/// Workaround: o teste acima não pode reusar o run depois do
/// `send_message` (o orchestrator sempre cria um novo). Pra
/// testar o portão, criamos um cenário controlado via DB +
/// `apply_transition` direto. **Este teste é a "linha de
/// defesa" do portão** — se `state_mapping` algum dia esquecer
/// de consultar `apply_transition` (regressão), o teste do
/// `valid_transition_persists` (abaixo) cobre o caso feliz, e
/// este aqui é o "se o caminho for burlado, o portão pega" via
/// unit test. A versão E2E no orchestrator é integrada nos
/// outros 3 testes (que exercitam o caminho real com eventos
/// válidos).
#[test]
fn portao_rejects_transition_from_terminal_state() {
    // Estado `Completed` é terminal. Qualquer evento (incluindo
    // `UserCancel`, que é global) é rejeitado com
    // `FromTerminal { from: Completed }`.
    let r = frederico_agent_engine::apply_transition(
        RunState::Completed,
        frederico_agent_engine::RunEventKind::UserCancel,
    );
    assert!(
        matches!(
            r,
            Err(frederico_agent_engine::TransitionError::FromTerminal {
                from: RunState::Completed
            })
        ),
        "esperava FromTerminal, veio {r:?}"
    );
}

/// 2. **`run_event_seq_monotonic_through_orchestrator`**
///
/// O `seq` do `RunEvent` é monotonicamente crescente por `run_id`,
/// sem buracos, sem duplicatas. Prova: rodar um cenário feliz
/// (1 round com Delta + Done), ler o `run_events` por `seq ASC`
/// e afirmar `[1, 2, ...]` sem buracos. **Cada step do
/// `state_mapping` é um `RunEvent`** — então o número de
/// `RunEvent`s é o número de transições reais.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_event_seq_monotonic_through_orchestrator() {
    let (invoker, manager) = fake_invoker().await;
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![vec![
            StreamEvent::Delta {
                content: "x".to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            },
        ]],
    ));
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider,
        ProviderId::new(PROVIDER_ID),
        ModelId::new(MODEL_ID),
        None,
    )
    .await;
    let conv = create_test_conversation(
        &h.db,
        &ProviderId::new(PROVIDER_ID),
        &ModelId::new(MODEL_ID),
        None,
    )
    .await;
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "oi".to_string())
        .await
        .expect("send_message ok");
    let _ = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(2)).await;

    // Lê os RunEvents em ordem de seq.
    let run_event_repo = RunEventRepo::new(&h.db);
    let events = run_event_repo.list_for_run(&run_id).await.expect("list");
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert!(!seqs.is_empty(), "esperava >= 1 RunEvent, veio 0");

    // Cada `seq` é único e contíguo (1, 2, 3, ...).
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seqs.len(),
        "RunEvent.seq tem duplicatas: {seqs:?}"
    );
    for (i, s) in sorted.iter().enumerate() {
        assert_eq!(
            *s,
            (i + 1) as u64,
            "RunEvent.seq fora de ordem ou com buraco: esperado {}, veio {s} (todos: {seqs:?})",
            i + 1
        );
    }
}

/// 3. **`valid_transition_persists_in_run_event_journal`**
///
/// Cada `RunEvent` carrega `from_state`/`to_state`/`kind`
/// consistentes com a tabela `TRANSITIONS`. Prova: ler o
/// `run_events` e, pra cada um, validar via `apply_transition`
/// (a função pura da `agent-engine`) que a transição é
/// aceita. **Este é o teste que distingue o portão de uma
/// gravação cega** — sem ele, o `RunEventRepo` poderia gravar
/// qualquer `from`/`to`/`kind` e o journal mentiria sobre o
/// estado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_transition_persists_in_run_event_journal() {
    use frederico_agent_engine::apply_transition;

    let (invoker, manager) = fake_invoker().await;
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![vec![
            StreamEvent::Delta {
                content: "x".to_string(),
            },
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            },
        ]],
    ));
    let h = build_orchestrator(
        Some(invoker),
        Some(manager),
        provider,
        ProviderId::new(PROVIDER_ID),
        ModelId::new(MODEL_ID),
        None,
    )
    .await;
    let conv = create_test_conversation(
        &h.db,
        &ProviderId::new(PROVIDER_ID),
        &ModelId::new(MODEL_ID),
        None,
    )
    .await;
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "oi".to_string())
        .await
        .expect("send_message ok");
    let _ = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(2)).await;

    let run_event_repo = RunEventRepo::new(&h.db);
    let events = run_event_repo.list_for_run(&run_id).await.expect("list");
    assert!(!events.is_empty(), "esperava >= 1 RunEvent");

    for ev in &events {
        // Cada RunEvent tem `from_state`/`to_state`/`kind`. A
        // paridade com a tabela `TRANSITIONS` (ou a tabela
        // `GLOBAL_TRANSITIONS`) é o que garante que o portão
        // é a fonte primária do estado, não a vontade do
        // executor.
        let from: RunState = ev
            .from_state
            .as_deref()
            .unwrap_or("created") // fallback tolerante (run_events pode ter from=NULL em runs antigos)
            .parse()
            .map_err(|e: RunStateParseError| {
                format!(
                    "from_state inválido '{}': {e}",
                    ev.from_state.as_deref().unwrap_or("?")
                )
            })
            .expect("from parse");
        let to: RunState = ev
            .to_state
            .as_deref()
            .unwrap_or("created")
            .parse()
            .map_err(|e: RunStateParseError| {
                format!(
                    "to_state inválido '{}': {e}",
                    ev.to_state.as_deref().unwrap_or("?")
                )
            })
            .expect("to parse");
        let kind: frederico_agent_engine::RunEventKind = ev.kind.parse().expect("kind parse");

        // Aplica a transição. Se o portão aceitou (gravou no
        // journal), a tabela tem que aceitar também.
        let result = apply_transition(from, kind);
        let new_state = result.as_ref().unwrap_or_else(|e| {
            panic!(
                "RunEvent gravou transição inválida: from={from} kind={kind} \
                 to={to} (apply_transition falhou: {e:?})"
            )
        });
        assert_eq!(
            *new_state, to,
            "RunEvent.to_state ({to}) não bate com apply_transition \
             (from={from} kind={kind})"
        );
    }
}

/// 4. **`recovery_loads_state_from_run_event_journal`**
///
/// O `recovery::recover_stale_runs` lê o **último `RunEvent`**
/// do run (não o `run.state` legado) como fonte primária do
/// estado. Prova: criar um run com `state = CallingModel`
/// (legado) e `RunEvent` mais novo mostrando `state =
/// ContinuingModel` (simulando crash no meio de uma transição),
/// forçar `last_heartbeat_at` velho, rodar recovery, e
/// afirmar que o `RunEvent` registrado tem `from_state =
/// ContinuingModel` (a fonte primária).
///
/// Por que este teste existe: antes da Etapa 2, o recovery
/// inferia o estado do `run.state` (que pode estar stale
/// se o app crashou entre dois `set_state`). A Etapa 2
/// introduz o `RunEvent` como journal autoritativo — o
/// recovery agora lê o último `RunEvent` e usa o `to_state`
/// dele como o estado verdadeiro. Sem essa primazia, o
/// recovery pode "voltar" o estado (rollback) ou "pular"
/// uma transição (lost update).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_loads_state_from_run_event_journal() {
    use chrono::Utc;
    use frederico_agent_engine::RunEventKind;
    use frederico_storage::{ConversationRepo, MessageRepo};

    let db = frederico_storage::Database::open_in_memory()
        .await
        .expect("open in-memory db");

    // 1. Cria um run em estado `CallingModel` (legado). O
    //    `last_heartbeat_at` é "agora" — run saudável.
    let conv = ConversationRepo::new(&db)
        .create(&ProviderId::new(PROVIDER_ID), &ModelId::new(MODEL_ID), None)
        .await
        .expect("cria conversa");
    let asst = MessageRepo::new(&db)
        .create(&conv.id, "assistant", "", None)
        .await
        .expect("cria message");
    let run_repo = RunRepo::new(&db);
    let run = run_repo.create(&conv.id, &asst.id).await.expect("cria run");
    run_repo
        .set_state_unchecked(&run.id, RunState::CallingModel)
        .await
        .expect("set_state CallingModel");

    // 2. Grava um `RunEvent` mais novo mostrando `to_state =
    //    ContinuingModel` — simula "app crashou depois de
    //    processar a tool_result, antes de atualizar `runs.state`".
    //    O `to_state` é a fonte primária do estado real.
    let run_event_repo = RunEventRepo::new(&db);
    run_event_repo
        .append(
            &run.id,
            RunEventKind::ResultValid,
            Some(RunState::WaitingToolCall),
            Some(RunState::ContinuingModel),
            serde_json::json!({}),
        )
        .await
        .expect("grava RunEvent");

    // 3. Força `last_heartbeat_at` pra 1 hora atrás — o run é
    // considerado stale pelo recovery.
    let one_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    run_repo
        .force_heartbeat_at_for_test(&run.id, &one_hour_ago)
        .await
        .expect("force heartbeat");

    // 4. Roda o recovery com threshold de 60s.
    let marked = recovery::recover_stale_runs(&run_repo, &run_event_repo, Duration::from_secs(60))
        .await
        .expect("recovery ok");
    assert_eq!(marked, 1, "esperava 1 run marcado, veio {marked}");

    // 5. O `RunEvent` do recovery tem `from_state = ContinuingModel`
    //    (a fonte primária do estado, lida do `RunEvent.to_state`),
    //    **não** `CallingModel` (o `run.state` legado). Prova
    //    que o recovery consulta o `RunEvent` e não o `run.state`.
    let recovery_event = run_event_repo
        .latest_for_run(&run.id)
        .await
        .expect("latest")
        .expect("recovery RunEvent existe");
    assert_eq!(recovery_event.kind, "app_crash_recovery");
    assert_eq!(
        recovery_event.from_state.as_deref(),
        Some("continuing_model"),
        "from_state do recovery RunEvent é a fonte primária \
         (RunEvent anterior) — esperada 'continuing_model' \
         (a transição ResultValid do journal), veio {:?}",
        recovery_event.from_state
    );

    // Sanity: o run agora está interrupted.
    let r = run_repo.get(&run.id).await.expect("get");
    assert_eq!(r.state, "interrupted");
}
