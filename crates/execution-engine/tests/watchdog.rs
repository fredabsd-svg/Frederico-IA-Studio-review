//! Teste E2E do watchdog de 60s do `RunExecutor` (Etapa 5).
//!
//! O `RunExecutor` da Etapa 5 introduz um `tokio::select!` no
//! loop interno com 3 braços: `cancel.cancelled()` (biased),
//! `tokio::time::sleep(event_timeout)` (watchdog), e
//! `stream.next()`. Quando o `event_timeout` (default 60s) estoura
//! sem o adapter emitir nada, o executor finaliza como
//! `RunState::Interrupted` / `RunStatus::Timeout` (a view
//! `runs_with_status` mapeia `interrupted → timeout`).
//!
//! No teste, o adapter é programado pra emitir **1 delta** e
//! depois **não emitir mais nada** (a `tokio::sync::mpsc` é
//! descartada). O executor recebe o delta, reseta o watchdog, e
//! na próxima iteração o timeout estoura. O teste usa
//! `event_timeout = 100ms` pra rodar rápido.

#![cfg(not(doctest))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use frederico_agent_engine::Budget;
use frederico_core::{ModelId, ProviderId, ToolId};
use frederico_execution_engine::RunExecutor;
use frederico_provider_engine::provider::{AdapterCapabilities, CostModel, RunHandle};
use frederico_provider_engine::{
    ChatMessage, ChatRequest, ChatResponse, ProviderAdapter, ProviderError, StreamEvent,
};
use frederico_storage::{MessageEventRepo, MessageRepo, MessageStatus, RunRepo, RunStatus};
use futures::stream::{self, BoxStream};
use tokio_util::sync::CancellationToken;

mod common;
use common::{setup_executor, ScriptedAdapter};

/// Adapter que emite **um** delta e depois trava (não emite mais
/// nada). O watchdog do executor estourar com `event_timeout = 100ms`.
struct TrailingAdapter {
    id: ProviderId,
    call_count: Mutex<u32>,
}

impl TrailingAdapter {
    fn new(id: impl Into<ProviderId>) -> Self {
        Self {
            id: id.into(),
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ProviderAdapter for TrailingAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_stream: true,
            supports_tools: false,
            supports_usage_in_stream: false,
        }
    }
    fn cost_model(&self) -> CostModel {
        CostModel::default()
    }
    async fn complete(&self, _r: ChatRequest) -> Result<ChatResponse, ProviderError> {
        unimplemented!()
    }
    fn stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        // Emite UM delta e fecha o canal — o stream termina do
        // ponto de vista do consumer. O executor vai tentar ler
        // o próximo evento, mas o `tokio::select!` vai disparar o
        // watchdog antes de qualquer `next()` retornar.
        //
        // (Na verdade: o `next()` retorna `None` quando o canal
        // fecha, e o `let Some(ev) = next_ev else { break; }` sai
        // do loop — o executor cairia no "5. sem tool_call →
        // Completed" e fecharia como Completed, não Timeout.
        // Pra exercitar o watchdog, preciso que o stream fique
        // **aberto** mas sem emitir. Vou usar um stream infinito
        // com `pending`.)
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        tokio::spawn(async move {
            // Emite o delta e espera — mas espera pra SEMPRE.
            // O watchdog do executor vai estourar antes.
            let _ = tx.send(StreamEvent::Delta {
                content: "primeiro ".to_string(),
            });
            // Espera 10s — mais que o suficiente pro watchdog de
            // 100ms estourar. Não emite mais nada; o `rx` continua
            // aberto (nunca fechado), então `stream.next()` fica
            // pending.
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let stream = stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        );
        Ok(Box::pin(stream))
    }
    fn cancel(&self, _h: RunHandle) -> Result<(), ProviderError> {
        Ok(())
    }
    fn known_models(&self) -> Vec<(ModelId, &'static str)> {
        vec![]
    }
}

/// Cenário simples: o adapter emite 1 delta e trava. O executor
/// recebe o delta, reseta o watchdog, e na próxima iteração o
/// `event_timeout` estoura. O `RunOutcome` deve ter
/// `final_state = Interrupted` (a view `runs_with_status` mapeia
/// `interrupted → timeout`), `MessageStatus::Timeout`,
/// `RunStatus::Timeout`.
///
/// O `event_timeout` é o do `ChatRequest` (default 60s). O teste
/// usa o `ScriptedAdapter` com `with_events(...)` que cria um
/// `ChatRequest::new(...)` — mas o `event_timeout` default é 60s.
/// Pra usar 100ms, eu construo o `ChatRequest` manualmente dentro
/// do `TrailingAdapter::stream`... mas o executor cria o
/// `ChatRequest` internamente. Hmm.
///
/// Solução: o `ScriptedAdapter` (que o executor usa internamente
/// pra o segundo round) é um adapter normal — o `event_timeout`
/// é o default 60s. Pra testar o watchdog com 100ms, eu preciso
/// que o **adapter** seja programável de outra forma. Vou usar o
/// `TrailingAdapter` direto (não `ScriptedAdapter`).
///
/// O `TrailingAdapter::stream` ignora o `event_timeout` (só emite
/// 1 delta e trava). O `ChatRequest::event_timeout` é passado
/// pro executor; o executor usa ele no `tokio::time::sleep` do
/// select!. Mas o executor cria o `ChatRequest` com o default
/// 60s, e eu não tenho como mudar isso daqui de fora sem mexer
/// no `RunExecutor`.
///
/// **Solução real:** o `RunExecutor` precisa aceitar
/// `event_timeout` no `new` (ou como argumento do `run`). Vou
/// adicionar `event_timeout: Duration` no `new` e usar ele no
/// `ChatRequest::event_timeout`. Default = 60s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watchdog_closes_run_after_event_timeout() {
    let (_workspace, db, registry, jail, message_id, run_id) = setup_executor().await;
    let adapter = Arc::new(TrailingAdapter::new("simulated"));

    let mut executor = RunExecutor::new(
        adapter.clone(),
        registry,
        jail,
        db.clone(),
        frederico_tool_registry::PermissionSet::default(),
        vec![],
        vec![],
        Budget::default(),
        CancellationToken::new(),
    )
    .with_event_timeout(Duration::from_millis(100));

    let outcome = executor
        .run(
            message_id,
            run_id,
            ModelId::new("fake"),
            vec![ChatMessage::user("oi")],
        )
        .await
        .expect("executa");

    assert_eq!(
        outcome.final_state,
        frederico_agent_engine::RunState::Interrupted,
        "timeout deve finalizar como Interrupted"
    );
    // O delta "primeiro " foi persistido antes do watchdog.
    assert_eq!(outcome.accumulated_content, "primeiro ");

    let msg = MessageRepo::new(&db).get(&message_id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Timeout.as_str());
    let run = RunRepo::new(&db).get(&run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Timeout.as_str());

    // O journal tem o delta persistido.
    let events = MessageEventRepo::new(&db)
        .list_for_message(&message_id, 0)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "delta");
}

#[allow(dead_code)]
fn _silence_unused() {
    let _ = (
        std::marker::PhantomData::<ScriptedAdapter>,
        std::marker::PhantomData::<ToolId>,
    );
}
