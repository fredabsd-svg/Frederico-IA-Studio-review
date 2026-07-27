//! Parser de SSE usado por todos os adapters e pelos fakes
//! (transport-level).
//!
//! Server-Sent Events chegam como bytes com fronteiras de chunk
//! arbitrárias — um caractere UTF-8 pode ser partido entre dois chunks
//! (fixture `openai/utf8_split_across_chunks.jsonl`), o evento pode ter
//! comentários de keepalive intercalados (`: keepalive\\n\\n`), e o
//! `data:` pode estar fragmentado em múltiplas linhas
//! (`data: {"a":\\ndata: 1}\\n\\n`).
//!
//! O parser é a peça que o fake do ADR-0008 §3.1 exercita byte a byte:
//! o fake entrega bytes brutos, este parser consome, e a saída é a
//! mesma que o adapter real produziria.

use bytes::{Bytes, BytesMut};
use futures::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::types::StreamEvent;

/// Erro de parsing. O adapter traduz para `ProviderErrorKind::Unknown`
/// ou `Network` conforme o caso.
#[derive(Debug, thiserror::Error)]
pub enum SseParseError {
    #[error("linha SSE inválida: {0}")]
    InvalidLine(String),
    #[error("JSON malformado em evento SSE: {0}")]
    BadJson(String),
}

/// Estado interno do parser — montado como `Stream` que aceita bytes
/// crus e emite `(opaque_event_kind, data_json)` para o adapter
/// traduzir em `StreamEvent`. A tradução específica do provedor
/// (OpenAI, Anthropic) é responsabilidade do adapter, não deste parser.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: BytesMut,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Alimenta o parser com bytes crus. Devolve os eventos
    /// completos já presentes no buffer. Bytes incompletos (um
    /// evento partido) ficam no buffer interno para a próxima
    /// chamada.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<RawSseEvent, SseParseError>> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            // Procura por \n\n ou \r\n\r\n (delimitador de evento).
            let delim = find_event_boundary(&self.buf);
            let Some(end) = delim else { break };
            let event_bytes = self.buf.split_to(end);
            // Consome o(s) \n final(is).
            while self.buf.first() == Some(&b'\n') || self.buf.first() == Some(&b'\r') {
                let _ = self.buf.split_to(1);
            }
            match parse_event(&event_bytes) {
                Ok(Some(ev)) => out.push(Ok(ev)),
                Ok(None) => {} // comentário puro (keepalive)
                Err(e) => out.push(Err(e)),
            }
        }
        out
    }

    /// Verdadeiro se o buffer ainda tem bytes que não viraram evento
    /// (eventos pendentes esperando mais bytes).
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.buf.is_empty()
    }
}

/// Evento SSE cru, no formato `data: ...` que provedores OpenAI-compat
/// e Anthropic emitem. A tradução para [`StreamEvent`] é do adapter.
#[derive(Debug, Clone)]
pub struct RawSseEvent {
    /// Linhas `data:` concatenadas por `\n`. O `data: ` inicial é
    /// removido.
    pub data: String,
    /// Tipo do evento (quando presente). Provedores OpenAI-compat
    /// omitem; Anthropic emite `event: content_block_delta` etc.
    pub event_type: Option<String>,
}

/// Adapter de parser para `Stream<Item = Result<Bytes>>`. Recebe o
/// body de uma request HTTP (ou de um `tokio::io::DuplexStream` do
/// fake) e emite `RawSseEvent`. Eventos vazios (keepalive puro) são
/// pulos; eventos com JSON malformado emitem erro e continuam (a
/// request prossegue).
///
/// Se um chunk não fechar nenhum evento, o stream **não** emite nada
/// para esse chunk — o próximo chunk que fechar um evento emite
/// apenas esse evento (não acumula). Isso casa com o uso típico
/// (vários eventos por chunk; um evento por chunk na média).
pub fn sse_stream<S>(byte_stream: S) -> impl Stream<Item = Result<RawSseEvent, SseParseError>>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    let mut parser = SseParser::new();
    byte_stream.flat_map(move |chunk_res| {
        let chunk_res = chunk_res.map_err(|e| SseParseError::InvalidLine(e.to_string()));
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => return stream::iter(vec![Err(e)]),
        };
        let results = parser.feed(&chunk);
        stream::iter(results)
    })
}

fn find_event_boundary(buf: &[u8]) -> Option<usize> {
    // Procura por \n\n ou \r\n\r\n.
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        if i + 3 < buf.len()
            && buf[i] == b'\r'
            && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r'
            && buf[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

fn parse_event(bytes: &[u8]) -> Result<Option<RawSseEvent>, SseParseError> {
    let text = std::str::from_utf8(bytes).map_err(|e| SseParseError::InvalidLine(e.to_string()))?;
    let mut data_lines: Vec<&str> = Vec::new();
    let mut event_type: Option<String> = None;
    let mut any_meaningful_line = false;
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(':') {
            // Comentário (keepalive ou similar). Ignora.
            let _ = rest;
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            any_meaningful_line = true;
            data_lines.push(rest.trim_start());
        } else if let Some(rest) = line.strip_prefix("event:") {
            any_meaningful_line = true;
            event_type = Some(rest.trim().to_string());
        } else {
            // Campo desconhecido (id:, retry:, etc.). Ignora por enquanto.
            let _ = line;
        }
    }
    if !any_meaningful_line {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    if data.is_empty() {
        return Ok(None);
    }
    Ok(Some(RawSseEvent { data, event_type }))
}

/// Helper para o adapter OpenAI-compat: dado um `RawSseEvent` no
/// formato OpenAI, devolve o `StreamEvent` correspondente. Retorna
/// `None` se for o `[DONE]` sentinela (OpenAI-compat usa
/// `data: [DONE]` para fechar).
pub fn openai_compat_translate(
    raw: RawSseEvent,
) -> Result<Option<StreamEvent>, SseParseError> {
    if raw.data.trim() == "[DONE]" {
        return Ok(Some(StreamEvent::Done {
            stop_reason: crate::types::StopReason::Stop,
        }));
    }
    let v: serde_json::Value = serde_json::from_str(&raw.data)
        .map_err(|e| SseParseError::BadJson(format!("{e}: {}", raw.data)))?;
    // OpenAI-compat: choices[0].delta.content, choices[0].finish_reason
    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());
    let Some(choice) = choice else {
        // Sem `choices` — pode ser um evento de erro ou usage puro.
        if let Some(usage) = v.get("usage") {
            let prompt = usage
                .get("prompt_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32;
            let completion = usage
                .get("completion_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32;
            return Ok(Some(StreamEvent::Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
            }));
        }
        return Ok(None);
    };
    let delta = choice.get("delta");
    if let Some(content) = delta.and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
        if !content.is_empty() {
            return Ok(Some(StreamEvent::Delta {
                content: content.to_string(),
            }));
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
        let stop_reason = match reason {
            "stop" => crate::types::StopReason::Stop,
            "length" => crate::types::StopReason::Length,
            "tool_calls" => crate::types::StopReason::ToolCalls,
            "content_filter" => crate::types::StopReason::Error,
            _ => crate::types::StopReason::Stop,
        };
        return Ok(Some(StreamEvent::Done { stop_reason }));
    }
    if let Some(usage) = v.get("usage") {
        let prompt = usage
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as u32;
        let completion = usage
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as u32;
        return Ok(Some(StreamEvent::Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
        }));
    }
    Ok(None)
}

/// Header de fixture — ver ADR-0008 §3.3. O loader rejeita fixtures
/// sem este header.
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureHeader {
    #[serde(rename = "_fixture_header")]
    pub header: FixtureHeaderInner,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FixtureHeaderInner {
    pub provider: String,
    pub model: String,
    pub scenario: String,
    pub recorded_at: String,
    pub recorder_version: String,
    pub source_endpoint: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn parser_handles_complete_event() {
        let mut p = SseParser::new();
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"oi\"}}]}\n\n";
        let events = p.feed(chunk);
        assert_eq!(events.len(), 1);
        let ev = events.into_iter().next().unwrap().unwrap();
        assert_eq!(ev.data, "{\"choices\":[{\"delta\":{\"content\":\"oi\"}}]}");
    }

    #[test]
    fn parser_handles_chunked_event_across_calls() {
        let mut p = SseParser::new();
        let chunk1 = b"data: {\"choices\":[{\"del";
        let chunk2 = b"ta\":{\"content\":\"oi\"}}]}\n\n";
        assert!(p.feed(chunk1).is_empty());
        let events = p.feed(chunk2);
        assert_eq!(events.len(), 1);
        assert!(!p.has_pending());
    }

    #[test]
    fn parser_handles_keepalive_comment() {
        let mut p = SseParser::new();
        // :keepalive é comentário puro, filtrado pelo parser.
        // O segundo evento é o data de verdade.
        let chunk = b": keepalive\n\ndata: {\"a\":1}\n\n";
        let events = p.feed(chunk);
        // Só o evento de data sobrevive — o keepalive some.
        assert_eq!(events.len(), 1);
        let ev = events.into_iter().next().unwrap().unwrap();
        assert_eq!(ev.data, "{\"a\":1}");
    }

    #[test]
    fn parser_handles_utf8_split_across_chunks() {
        // Conteúdo "c" + 0xC3 (1º byte de ç, incompleto) no chunk1.
        // 0xA7 0xC3 0xA3 0x6F (resto de ç + ã + o) no chunk2.
        // Resultado esperado: "c" + "ção" = "cção" — o parser tem que
        // reagrupar os bytes sem corromper o caractere UTF-8.
        let mut p = SseParser::new();
        let chunk1: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"c\xC3";
        let chunk2: &[u8] = b"\xA7\xC3\xA3\x6F\"}}]}\n\n";
        assert!(p.feed(chunk1).is_empty());
        let events = p.feed(chunk2);
        assert_eq!(events.len(), 1);
        let ev = events.into_iter().next().unwrap().unwrap();
        let translated = openai_compat_translate(ev).unwrap().unwrap();
        match translated {
            StreamEvent::Delta { content } => {
                assert_eq!(content, "cção");
            }
            _ => panic!("esperava Delta"),
        }
    }

    #[test]
    fn parser_handles_multi_line_data() {
        // data: em múltiplas linhas é concatenado por \n.
        let mut p = SseParser::new();
        let chunk = b"data: {\"a\":\ndata: 1}\n\n";
        let events = p.feed(chunk);
        assert_eq!(events.len(), 1);
        let ev = events.into_iter().next().unwrap().unwrap();
        assert_eq!(ev.data, "{\"a\":\n1}");
    }

    #[test]
    fn openai_translate_delta() {
        let raw = RawSseEvent {
            data: r#"{"choices":[{"delta":{"content":"olá"}}]}"#.to_string(),
            event_type: None,
        };
        let ev = openai_compat_translate(raw).unwrap().unwrap();
        match ev {
            StreamEvent::Delta { content } => assert_eq!(content, "olá"),
            _ => panic!("esperava Delta"),
        }
    }

    #[test]
    fn openai_translate_done_sentinel() {
        let raw = RawSseEvent {
            data: "[DONE]".to_string(),
            event_type: None,
        };
        let ev = openai_compat_translate(raw).unwrap().unwrap();
        assert!(matches!(
            ev,
            StreamEvent::Done {
                stop_reason: crate::types::StopReason::Stop
            }
        ));
    }

    #[test]
    fn openai_translate_finish_reason_length() {
        let raw = RawSseEvent {
            data: r#"{"choices":[{"finish_reason":"length"}]}"#.to_string(),
            event_type: None,
        };
        let ev = openai_compat_translate(raw).unwrap().unwrap();
        assert!(matches!(
            ev,
            StreamEvent::Done {
                stop_reason: crate::types::StopReason::Length
            }
        ));
    }

    #[test]
    fn openai_translate_tool_calls_flag() {
        let raw = RawSseEvent {
            data: r#"{"choices":[{"finish_reason":"tool_calls"}]}"#.to_string(),
            event_type: None,
        };
        let ev = openai_compat_translate(raw).unwrap().unwrap();
        assert!(matches!(
            ev,
            StreamEvent::Done {
                stop_reason: crate::types::StopReason::ToolCalls
            }
        ));
    }

    #[test]
    fn openai_translate_usage_event() {
        let raw = RawSseEvent {
            data: r#"{"usage":{"prompt_tokens":12,"completion_tokens":34}}"#.to_string(),
            event_type: None,
        };
        let ev = openai_compat_translate(raw).unwrap().unwrap();
        match ev {
            StreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                assert_eq!(prompt_tokens, 12);
                assert_eq!(completion_tokens, 34);
            }
            _ => panic!("esperava Usage"),
        }
    }

    #[test]
    fn sse_stream_emits_events_as_chunks_arrive() {
        use futures::stream;

        let bytes = vec![
            Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n")),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let mut s = Box::pin(sse_stream(stream::iter(bytes)));
        // Primeiro evento: Delta.
        let r1 = futures::executor::block_on(s.next()).unwrap().unwrap();
        let raw1 = r1;
        let translated1 = openai_compat_translate(raw1).unwrap().unwrap();
        assert!(matches!(translated1, StreamEvent::Delta { .. }));
        // Segundo: Done.
        let r2 = futures::executor::block_on(s.next()).unwrap().unwrap();
        let translated2 = openai_compat_translate(r2).unwrap().unwrap();
        assert!(matches!(translated2, StreamEvent::Done { .. }));
    }
}
