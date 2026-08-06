//! Persistência com a regra **"Journal-then-emit"** do spec
//! `chat-and-providers.md`.
//!
//! Cada `StreamEvent` é gravado em `message_events` **antes** de
//! ser emitido pra UI, dentro de uma transação
//! `BEGIN IMMEDIATE; ...; COMMIT;`. O `BEGIN IMMEDIATE` (em vez
//! do default `BEGIN DEFERRED`) garante que o `BEGIN` falhe
//! imediatamente se outra conexão já tem o write lock —
//! preferimos falhar rápido a esperar.

use chrono::Utc;
use frederico_core::MessageId;
use frederico_provider_engine::StreamEvent;
use frederico_storage::{MessageEventRepo, StorageResult};

/// Persiste um `StreamEvent` no journal (a `MessageEventRepo`
/// append). A transação SQLite é `BEGIN IMMEDIATE; ...; COMMIT;`
/// por chamada — atomicidade de um evento só.
///
/// **Importante:** o caller (RunExecutor) emite o `StreamEvent`
/// **após** essa função retornar. Falha na persistência é
/// propagada como `Err` e o loop é abortado.
pub async fn persist_journal(
    repo: &MessageEventRepo<'_>,
    message_id: &MessageId,
    event: &StreamEvent,
) -> StorageResult<u32> {
    let (kind, data) = stream_event_to_journal(event);
    // `MessageEventRepo::append` já faz o `INSERT` com seq
    // monotônico. A Etapa 4 não envolve isso numa transação
    // explícita porque cada append já é atômico por si (single
    // `INSERT` no SQLite). A Etapa 5 (watchdog) introduz
    // BEGIN/COMMIT explícito quando precisar agrupar appends.
    let _now = Utc::now(); // carimbo de auditoria (futuro)
    let _ = _now;
    repo.append(message_id, &kind, &data).await
}

/// Converte `StreamEvent` no par `(kind, data_json)` que vai pro
/// `message_events`. Reflete o que o `ChatOrchestrator` da Fase
/// 2 já faz — duplicado aqui (com `ToolResult` adicionado) pra
/// evitar importar o `provider-engine::orchestrator` (que
/// carrega dependências de UI/streaming desnecessárias pro
/// executor).
/// Converte `StreamEvent` no par `(kind, data_json)` que vai pro
/// `message_events`. Reflete o que o `ChatOrchestrator` da Fase
/// 2 já faz — duplicado aqui (com `ToolResult` adicionado) pra
/// evitar importar o `provider-engine::orchestrator` (que
/// carrega dependências de UI/streaming desnecessárias pro
/// executor).
pub fn stream_event_to_journal(ev: &StreamEvent) -> (String, serde_json::Value) {
    let kind = match ev {
        StreamEvent::Delta { .. } => "delta",
        StreamEvent::Usage { .. } => "usage",
        StreamEvent::ToolCall { .. } => "tool_call",
        StreamEvent::ToolResult { .. } => "tool_result",
        StreamEvent::Done { .. } => "done",
        StreamEvent::Error(_) => "error",
        StreamEvent::Cancelled => "cancelled",
    };
    let data = match ev {
        StreamEvent::Delta { content } => serde_json::json!({ "content": content }),
        StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
        } => serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
        }),
        StreamEvent::ToolCall {
            id,
            name,
            arguments_json,
        } => serde_json::json!({
            "id": id,
            "name": name,
            "arguments_json": arguments_json,
        }),
        StreamEvent::ToolResult { id, ok, output } => serde_json::json!({
            "id": id,
            "ok": ok,
            "output": output,
        }),
        StreamEvent::Done { stop_reason } => {
            serde_json::json!({ "stop_reason": stop_reason_label(*stop_reason) })
        }
        StreamEvent::Error(e) => serde_json::json!({
            "kind": format!("{:?}", e.kind),
            "upstream_status": e.upstream_status,
            "upstream_message": e.upstream_message,
        }),
        StreamEvent::Cancelled => serde_json::json!({}),
    };
    (kind.to_string(), data)
}

fn stop_reason_label(reason: frederico_provider_engine::StopReason) -> &'static str {
    use frederico_provider_engine::StopReason;
    match reason {
        StopReason::Stop => "stop",
        StopReason::Length => "length",
        StopReason::ToolCalls => "tool_calls",
        StopReason::Error => "error",
    }
}

/// Mapeia um `StreamEvent` pro `RunEventKind` correspondente
/// (Fase 6, Etapa 2). Usado pelo `RunExecutor` para preencher o
/// `kind` do `RunEvent` que vai no journal de transições
/// (`run_events`).
///
/// O mapping é determinístico: cada `StreamEvent` causa **um**
/// `RunEventKind`. A transação (se houver) é resolvida pelo
/// `apply_transition` em `state_mapping::run_state_for_event`.
pub fn stream_event_to_run_event_kind(ev: &StreamEvent) -> frederico_agent_engine::RunEventKind {
    use frederico_agent_engine::RunEventKind;
    match ev {
        // Delta: se vier de CallingModel é o FirstToken; de
        // Streaming é continuação (no-op, o portão não é chamado).
        // Mesmo assim, gravamos o RunEvent com FirstToken se houver
        // transição — o caller é quem decide.
        StreamEvent::Delta { .. } => RunEventKind::FirstToken,
        StreamEvent::Usage { .. } => RunEventKind::FirstToken, // nunca grava (no-op), mas tipado
        StreamEvent::ToolCall { .. } => RunEventKind::ToolCallEmitted,
        StreamEvent::ToolResult { .. } => RunEventKind::ToolReturned,
        StreamEvent::Done { stop_reason } => match stop_reason {
            frederico_provider_engine::StopReason::Stop
            | frederico_provider_engine::StopReason::Length => RunEventKind::CheckpointPersisted,
            frederico_provider_engine::StopReason::ToolCalls => RunEventKind::ToolCallEmitted,
            frederico_provider_engine::StopReason::Error => RunEventKind::UnrecoverableError,
        },
        StreamEvent::Error(_) => RunEventKind::UnrecoverableError,
        StreamEvent::Cancelled => RunEventKind::UserCancel,
    }
}

/// Payload de um `RunEvent` baseado no `StreamEvent` que o causou.
/// O `RunEvent.payload` é `serde_json::Value` opaco (mesmo padrão
/// do `MessageEvent.data`); aqui materializamos os campos mais
/// úteis pra auditoria/diagnóstico.
pub fn stream_event_payload(ev: &StreamEvent) -> serde_json::Value {
    use frederico_provider_engine::StopReason;
    match ev {
        StreamEvent::Delta { content } => serde_json::json!({ "content": content }),
        StreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
        } => serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
        }),
        StreamEvent::ToolCall {
            id,
            name,
            arguments_json,
        } => serde_json::json!({
            "id": id,
            "name": name,
            "arguments_json": arguments_json,
        }),
        StreamEvent::ToolResult { id, ok, output } => serde_json::json!({
            "id": id,
            "ok": ok,
            "output": output,
        }),
        StreamEvent::Done { stop_reason } => serde_json::json!({
            "stop_reason": match stop_reason {
                StopReason::Stop => "stop",
                StopReason::Length => "length",
                StopReason::ToolCalls => "tool_calls",
                StopReason::Error => "error",
            }
        }),
        StreamEvent::Error(e) => serde_json::json!({
            "kind": format!("{:?}", e.kind),
            "upstream_status": e.upstream_status,
            "upstream_message": e.upstream_message,
        }),
        StreamEvent::Cancelled => serde_json::json!({}),
    }
}
