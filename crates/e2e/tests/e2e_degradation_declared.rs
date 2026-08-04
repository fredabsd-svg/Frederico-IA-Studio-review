//! E2E — degradação declarada: `invoker = None` → catálogo só tem
//! `files.read`. O provedor tenta chamar `docs.generate` mesmo
//! assim (simulando prompt injection ou manifest injection do
//! modelo). O `RunExecutor` rejeita com
//! `NotInExecutionInventory` (Passo 4 do `validate_tool_call`).
//!
//! **Por que este teste é o que mais protege contra regressão:**
//! a Etapa 2.B instituiu o **bump atômico** do
//! `documents: None → Full` (ADR-0020 §3 D3) e da allowlist (3
//! tools vs. 1 tool) — exatamente pra que o modelo nunca veja
//! um tool que não consegue invocar. Se alguém regredir e
//! registrar `docs.generate` no `ToolRegistry` sem o
//! `documents: Full` (ou sem o invoker), o `validate_tool_call`
//! Passo 4 ainda rejeita — mas por outro motivo
//! (`NotInExecutionInventory` vs. `JailViolation` vs. falha
//! do `WorkerInvoker::invoke`). Este teste documenta o caminho
//! esperado quando o invoker é `None`: o `RunExecutor` rejeita
//! **antes** de chamar o tool.
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e a
//! lição "Degradação declarada > substituição silenciosa" na
//! memória do agent.

mod common;

use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use serde_json::json;

use common::{
    build_orchestrator, create_test_conversation, wait_for_run_completion, ScriptedProvider,
};

const PROVIDER_ID: &str = "openai";
const MODEL_ID: &str = "gpt-4o-mini";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn degradation_declared_rejects_docs_generate_without_invoker() {
    // 1. ScriptedProvider: 2 rounds.
    //    Round 1: emite `ToolCall` chamando `docs.generate` (que
    //             NÃO está no `ToolRegistry` — invoker é None).
    //    Round 2: depois do `ToolResult` (rejeitado pelo Passo 4),
    //             emite Delta + Done.
    let provider = Arc::new(ScriptedProvider::new(
        PROVIDER_ID,
        MODEL_ID,
        vec![
            vec![StreamEvent::ToolCall {
                id: "call_docs_generate_attack".to_string(),
                name: "docs.generate".to_string(),
                arguments_json: json!({
                    "spec": {"metadata": {"title": "hack"}},
                    "output_path": "C:/Windows/hacked.docx",
                    "format": "docx"
                })
                .to_string(),
            }],
            vec![
                StreamEvent::Delta {
                    content: "OK, tentei gerar mas o tool não está disponível.".to_string(),
                },
                StreamEvent::Done {
                    stop_reason: StopReason::Stop,
                },
            ],
        ],
    ));
    let provider_id = ProviderId::new(PROVIDER_ID);
    let model_id = ModelId::new(MODEL_ID);

    // 2. Monta o ChatOrchestrator SEM invoker — degradação declarada.
    //    `build_default_tools(None)` retorna só `[FilesReadTool]`;
    //    `build_default_allowed_for_run(None)` retorna só
    //    `[files.read]`; `documents` permission = None.
    let h = build_orchestrator(
        None,
        None,
        provider.clone(),
        provider_id.clone(),
        model_id.clone(),
        None,
    )
    .await;

    // 3. Cria a conversa + envia a mensagem.
    let conv =
        create_test_conversation(&h.db, &provider_id, &model_id, Some("e2e degradation")).await;
    let (_user_msg, run_id) = h
        .orchestrator
        .send_message(conv.id, "gere um documento".to_string())
        .await
        .expect("send_message");

    // 4. Espera o run fechar.
    //    Comportamento esperado: o `docs.generate` não está no
    //    `ToolRegistry` (invoker é None → só `files.read`).
    //    O `executor.rs:702` faz `self.registry.get()` antes do
    //    `validate_tool_call` e retorna `Err(ExecutorError::UnknownTool)`
    //    → o `ChatOrchestrator` mapeia pra `RunStatus::Failed`
    //    (não silenciosamente deixa passar). Isso **é** a
    //    degradação declarada: o modelo não vê `docs.generate` no
    //    schema; se tentar, o sistema rejeita com erro visível.
    //    A alternativa (degradação silenciosa) seria o modelo
    //    chamar `docs.generate`, o tool executar com sucesso, e
    //    o user ver um doc falso — exatamente o que a Fase de
    //    Ligação existe pra evitar.
    let status = wait_for_run_completion(&h.sink, run_id, Duration::from_secs(10)).await;
    assert_eq!(
        status,
        frederico_storage::RunStatus::Failed,
        "esperava Failed (degradação declarada via UnknownTool); o user vê erro claro no Message"
    );

    // 5. Asserções:
    // 5a. O provider foi chamado 1 vez só (o `Err` aborta o loop
    //     do executor antes do round 2).
    assert_eq!(
        provider
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "esperava 1 round do provider (loop abortou no round 1)"
    );

    // 5b. O ToolCall foi persistido no journal (prova que o
    //     modelo tentou chamar — o `executor` aborta DEPOIS
    //     de persistir o evento).
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
        Some("docs.generate"),
        "o tool_call persistido deve ser docs.generate"
    );

    // 5c. **NÃO há `tool_result`** — o `handle_tool_call` nem
    //     chegou a ser chamado (o pre-check do `executor.rs:702`
    //     retornou `Err` antes). O `tool_result` apareceria se
    //     o tool ESTIVESSE no registry mas o `validate_tool_call`
    //     rejeitasse em algum dos 10 passos.
    let tool_result_count = events.iter().filter(|e| e.kind == "tool_result").count();
    assert_eq!(
        tool_result_count, 0,
        "esperava 0 tool_result (pre-check do executor abortou antes do handle_tool_call)"
    );

    // 5d. A `Message` do assistant tem `error` populado (o
    //     `ChatOrchestrator` faz `set_error` quando o executor
    //     falha — ver `orchestrator.rs:327-329`).
    let asst_msg = frederico_storage::MessageRepo::new(&h.db)
        .get(&run.message_id)
        .await
        .expect("get assistant message");
    let err_text = asst_msg.error.as_deref().unwrap_or("");
    assert!(
        err_text.contains("erro do executor") || err_text.contains("abortado"),
        "esperava Message.error mencionando erro/abort; veio {err_text:?}"
    );
}
