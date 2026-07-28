//! Acumulador de deltas de `tool_call` (Fase 3, Etapa 4.1).
//!
//! ## Por que existe
//!
//! O `openai_compat_translate` (parser SSE) é **puro por chunk** —
//! recebe um `RawSseEvent` e devolve zero ou um `StreamEvent`.
//! Mas a OpenAI envia o `tool_call` em **múltiplos chunks** quando
//! os argumentos são grandes: o primeiro chunk traz o `id`,
//! `name` e `arguments: ""`; os chunks seguintes trazem só
//! `function.arguments` com pedacinhos do JSON; o último
//! `Done { stop_reason: "tool_calls" }` sinaliza o fim.
//!
//! O `ToolCallDeltaAccumulator` mantém estado entre chunks: uma
//! `HashMap<u32, ToolCallPartial>` indexada pelo `index` que o
//! OpenAI atribui. Quando o `feed` recebe um chunk com
//! `finish_reason: "tool_calls"`, devolve os `ToolCall`s
//! acumulados.
//!
//! ## Onde é usado
//!
//! O `OpenAiCompatAdapter::stream` insere o accumulator entre o
//! SSE stream e o `openai_compat_translate`. Para o `RunExecutor`
//! é transparente — ele continua consumindo um `BoxStream<StreamEvent>`.
//!
//! ## Thread-safety
//!
//! O accumulator é local a um stream (criado e droppado dentro
//! do adapter). Não é `Send` — fica confinado a uma task.

use std::collections::HashMap;

use crate::types::StreamEvent;

/// Partial state de um `tool_call` em construção. Os campos são
/// `Option<String>` porque o OpenAI pode enviar `id` e `name` no
/// primeiro chunk e `arguments` em vários chunks seguintes.
#[derive(Debug, Default, Clone)]
struct ToolCallPartial {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Acumulador de deltas de `tool_call`. Estado entre chunks.
#[derive(Debug, Default)]
pub struct ToolCallDeltaAccumulator {
    /// Index → partial state. O `index` vem do `delta.tool_calls[i].index`.
    partials: HashMap<u32, ToolCallPartial>,
}

impl ToolCallDeltaAccumulator {
    /// Constrói um accumulator vazio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Alimenta o accumulator com um chunk parseado. Devolve:
    /// - `Some(StreamEvent::ToolCall { id, name, arguments_json })`
    ///   quando o chunk sinaliza fim (`finish_reason:
    ///   "tool_calls"` ou `delta.tool_calls` com `id` e `name`
    ///   completos) — nesse caso o accumulator já consumiu todos
    ///   os deltas anteriores.
    /// - `None` quando o chunk é só delta parcial (acumulou
    ///   internamente, sem emitir nada).
    /// - `Some(StreamEvent::Done { stop_reason })` se o `finish_reason`
    ///   é `tool_calls` E já emitimos o(s) `ToolCall`(s) — caso
    ///   comum onde `finish_reason: "tool_calls"` vem num chunk
    ///   que tem o último `arguments` delta e mais nada. O adapter
    ///   vai emitir o `Done` separadamente quando o `stop_reason`
    ///   é "tool_calls" e o `accumulator` ainda tem partials (e
    ///   emite os ToolCalls e DEPOIS o Done no próximo call).
    pub fn feed(&mut self, raw_value: &serde_json::Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let choice = raw_value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first());
        let Some(choice) = choice else {
            return out;
        };
        let delta = choice.get("delta");

        // 1. Acumula os deltas de tool_calls (se houver).
        let mut last_index_with_complete: Option<u32> = None;
        if let Some(tool_calls) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(|t| t.as_array())
        {
            for tc in tool_calls {
                let index = tc
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
                    .unwrap_or(0);
                let partial = self.partials.entry(index).or_default();

                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    partial.id = Some(id.to_string());
                }
                if let Some(name) = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                {
                    partial.name = Some(name.to_string());
                }
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    partial.arguments.push_str(args);
                }

                // Se id e name estão presentes E o argumento parece
                // completo (termina com `}`), marca como "pronto pra
                // emitir". A heurística do `}` é frágil mas o caso
                // comum: o OpenAI envia o último chunk com o `}`
                // final. Se não terminar com `}`, ainda é parcial.
                if partial.id.is_some()
                    && partial.name.is_some()
                    && partial.arguments.trim_end().ends_with('}')
                {
                    last_index_with_complete = Some(index);
                }
            }
        }

        // 2. Emite os `ToolCall`s completos (se houver).
        if let Some(index) = last_index_with_complete {
            if let Some(partial) = self.partials.remove(&index) {
                out.push(StreamEvent::ToolCall {
                    id: partial.id.unwrap_or_default(),
                    name: partial.name.unwrap_or_default(),
                    arguments_json: partial.arguments,
                });
            }
        }

        // 3. `finish_reason: "tool_calls"` sinaliza fim de TODOS os
        //    tool_calls pendentes (mesmo se o último delta não
        //    terminou com `}`). Emitimos qualquer partial restante.
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            if reason == "tool_calls" {
                // Drena todos os partials em ordem de index.
                let mut indices: Vec<u32> = self.partials.keys().copied().collect();
                indices.sort_unstable();
                for idx in indices {
                    if let Some(partial) = self.partials.remove(&idx) {
                        if partial.id.is_some() && partial.name.is_some() {
                            out.push(StreamEvent::ToolCall {
                                id: partial.id.unwrap_or_default(),
                                name: partial.name.unwrap_or_default(),
                                arguments_json: partial.arguments,
                            });
                        }
                    }
                }
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_chunk_with_complete_tool_call_emits_immediately() {
        let mut acc = ToolCallDeltaAccumulator::new();
        let v = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "files.read",
                            "arguments": "{\"path\":\"hello.txt\"}"
                        }
                    }]
                }
            }]
        });
        let events = acc.feed(&v);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolCall {
                id,
                name,
                arguments_json,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "files.read");
                assert_eq!(arguments_json, "{\"path\":\"hello.txt\"}");
            }
            _ => panic!("esperava ToolCall, veio {:?}", events[0]),
        }
    }

    #[test]
    fn multiple_chunks_accumulate_arguments() {
        let mut acc = ToolCallDeltaAccumulator::new();
        // Chunk 1: id + name + arguments: "" (parcial — não emite).
        let v1 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "files.read",
                            "arguments": ""
                        }
                    }]
                }
            }]
        });
        let events1 = acc.feed(&v1);
        assert!(events1.is_empty(), "primeiro chunk não deve emitir");

        // Chunk 2: arguments parcial (sem `}` final — não emite).
        let v2 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "{\"path\":"
                        }
                    }]
                }
            }]
        });
        let events2 = acc.feed(&v2);
        assert!(events2.is_empty(), "chunk intermediário não deve emitir");

        // Chunk 3: arguments final (com `}` — emite).
        let v3 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "\"/tmp/hello.txt\"}"
                        }
                    }]
                }
            }]
        });
        let events3 = acc.feed(&v3);
        assert_eq!(events3.len(), 1);
        match &events3[0] {
            StreamEvent::ToolCall {
                id,
                name,
                arguments_json,
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "files.read");
                assert_eq!(arguments_json, "{\"path\":\"/tmp/hello.txt\"}");
            }
            _ => panic!("esperava ToolCall, veio {:?}", events3[0]),
        }
    }

    #[test]
    fn finish_reason_tool_calls_drains_remaining_partials() {
        // O OpenAI pode enviar o `finish_reason: tool_calls` antes
        // do último delta de arguments (em alguns provedores). O
        // accumulator drena os partials quando vê o finish_reason.
        let mut acc = ToolCallDeltaAccumulator::new();
        // Chunk 1: id + name + arguments parcial.
        let v1 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "files.read",
                            "arguments": "{\"path\":"
                        }
                    }]
                }
            }]
        });
        acc.feed(&v1);
        // Chunk 2: arguments final (sem `}`) + finish_reason.
        let v2 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "\"/tmp/hello.txt\""
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let events = acc.feed(&v2);
        assert_eq!(events.len(), 1, "esperava 1 ToolCall, veio {events:?}");
        match &events[0] {
            StreamEvent::ToolCall { arguments_json, .. } => {
                // O JSON fica sem o `}` final (não veio), mas o
                // accumulator emite assim mesmo (best effort — o
                // `validate_tool_call` vai rejeitar com
                // `TOOL_SCHEMA_INVALID` se o JSON não parsear).
                assert!(arguments_json.contains("/tmp/hello.txt"));
            }
            _ => panic!("esperava ToolCall, veio {:?}", events[0]),
        }
    }
}
