//! Fakes no nível do transporte.
//!
//! Estes fakes **não implementam** o trait `ProviderAdapter`. Eles
//! produzem bytes brutos que o mesmo parser de SSE do
//! [`crate::parser`] consome. É o que o §3.1 do ADR-0008 quer
//! exercitado em todo PR.
//!
//! ## Formato de golden file (`.jsonl`)
//!
//! Cada arquivo `.jsonl` em `fixtures/` tem o formato:
//!
//! - **Linha 1**: objeto JSON com `_fixture_header` carregando
//!   `provider`, `model`, `scenario`, `recorded_at`, `recorder_version`
//!   e `source_endpoint`. O loader rejeita arquivos sem este header.
//! - **Linhas 2+**: cada linha é uma **string JSON** com o chunk
//!   bruto (bytes de SSE, com `\n` escapado como `\\n` no JSON). Cada
//!   linha vira um chunk separado no body — fronteira explícita, que
//!   é o que o ADR-0008 §3.1 quer.

use std::path::Path;

use bytes::Bytes;
use futures::stream::{self, Stream};
use thiserror::Error;

use crate::parser::{sse_stream, FixtureHeader, FixtureHeaderInner, RawSseEvent};

#[derive(Debug, Error)]
pub enum FakeError {
    #[error("falha ao abrir fixture {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("fixture sem cabeçalho _fixture_header: {0}")]
    MissingHeader(String),
    #[error("header da fixture inválido: {0}")]
    BadHeader(String),
}

/// Lê um golden file do disco e devolve o header + um stream de bytes
/// pronto para ser passado ao [`crate::parser::sse_stream`].
pub async fn load_golden_file(
    path: impl AsRef<Path>,
) -> Result<
    (
        FixtureHeaderInner,
        impl Stream<Item = Result<Bytes, std::io::Error>>,
    ),
    FakeError,
> {
    let path = path.as_ref();
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| FakeError::Open {
            path: path.display().to_string(),
            source,
        })?;
    let (header, chunks) =
        parse_golden(&content).map_err(|e| FakeError::BadHeader(e.to_string()))?;
    let body = stream::iter(chunks.into_iter().map(|c| Ok(Bytes::from(c))));
    Ok((header, body))
}

/// Lê um golden file de uma string em memória (útil para testes
/// que não querem mexer no disco).
pub fn parse_golden(content: &str) -> Result<(FixtureHeaderInner, Vec<String>), FakeError> {
    let mut lines = content.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| FakeError::MissingHeader("(arquivo vazio)".to_string()))?;
    let header: FixtureHeader = serde_json::from_str(header_line)
        .map_err(|e| FakeError::MissingHeader(format!("linha 1 não é header válido: {e}")))?;
    let chunks: Vec<String> = lines
        .map(|line| {
            // Cada linha é uma string JSON (com \\n escapado). Decodifica.
            serde_json::from_str::<String>(line)
                .map_err(|e| FakeError::BadHeader(format!("linha não é string JSON: {e}")))
        })
        .collect::<Result<_, _>>()?;
    Ok((header.header, chunks))
}

/// Concatena um header + chunks em uma string `.jsonl` válida.
/// Usado pelo `provider-recorder` (Etapa 1.5 ou Leva 2).
pub fn serialize_golden(header: &FixtureHeaderInner, chunks: &[String]) -> String {
    let mut out = String::new();
    let h = serde_json::json!({ "_fixture_header": header });
    out.push_str(&serde_json::to_string(&h).unwrap());
    out.push('\n');
    for c in chunks {
        out.push_str(&serde_json::to_string(c).unwrap());
        out.push('\n');
    }
    out
}

/// Helper: carrega golden file e devolve um stream de `RawSseEvent`
/// já passado pelo parser de SSE. Encapsula o "wire do fake até o
/// parser real".
pub async fn golden_file_events(
    path: impl AsRef<Path>,
) -> Result<
    (
        FixtureHeaderInner,
        impl Stream<Item = Result<RawSseEvent, crate::parser::SseParseError>>,
    ),
    FakeError,
> {
    let (header, body) = load_golden_file(path).await?;
    Ok((header, sse_stream(body)))
}

/// Evento de um script. Sequência de bytes + fecha. Patologias
/// (stall, truncation) são sequências específicas deste enum +
/// watchdog do orquestrador.
#[derive(Debug, Clone)]
pub enum ScriptedEvent {
    /// Escreve bytes no body.
    Write(Vec<u8>),
    /// Fecha a conexão abruptamente. Bytes já escritos continuam
    /// válidos; o consumer vê fim de stream antes do esperado.
    Close,
}

/// Script de body — sequência de eventos. Constrói um stream de bytes.
pub struct ScriptedBody {
    events: Vec<ScriptedEvent>,
}

impl ScriptedBody {
    #[must_use]
    pub fn new(events: Vec<ScriptedEvent>) -> Self {
        Self { events }
    }

    /// Atalho: script com só Writes e um Close no fim.
    pub fn from_bytes(chunks: &[&[u8]]) -> Self {
        let mut events: Vec<ScriptedEvent> = chunks
            .iter()
            .map(|c| ScriptedEvent::Write(c.to_vec()))
            .collect();
        events.push(ScriptedEvent::Close);
        Self::new(events)
    }

    /// Devolve o body como um `Stream<Item = Result<Bytes>>`.
    pub fn into_stream(self) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
        stream::iter(self.events.into_iter().map(|ev| match ev {
            ScriptedEvent::Write(b) => Ok(Bytes::from(b)),
            ScriptedEvent::Close => Ok(Bytes::new()), // EOF
        }))
    }
}

/// Helper: dado um script, devolve um stream de `RawSseEvent` passado
/// pelo parser real.
pub fn scripted_events(
    body: ScriptedBody,
) -> impl Stream<Item = Result<RawSseEvent, crate::parser::SseParseError>> {
    sse_stream(body.into_stream())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::openai_compat_translate;
    use crate::types::StreamEvent;
    use futures::StreamExt;

    const SHORT_RESPONSE: &str = r#"{"_fixture_header":{"provider":"openai","model":"gpt-4o","scenario":"short_response","recorded_at":"2026-07-27T00:00:00Z","recorder_version":"0.2.0","source_endpoint":"https://api.openai.com/v1/chat/completions"}}
"data: {\"choices\":[{\"delta\":{\"content\":\"ol\"}}]}\n\n"
"data: {\"choices\":[{\"delta\":{\"content\":\"á\"}}]}\n\n"
"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n"
"data: [DONE]\n\n"
"#;

    #[test]
    fn parse_golden_extracts_header_and_chunks() {
        let (header, chunks) = parse_golden(SHORT_RESPONSE).unwrap();
        assert_eq!(header.provider, "openai");
        assert_eq!(header.model, "gpt-4o");
        assert_eq!(header.scenario, "short_response");
        assert_eq!(chunks.len(), 4);
        // O primeiro chunk tem o "ol" + nova linha dupla.
        assert!(chunks[0].starts_with("data: "));
        assert!(chunks[0].ends_with("\n\n"));
    }

    #[test]
    fn serialize_golden_roundtrips() {
        let (header, chunks) = parse_golden(SHORT_RESPONSE).unwrap();
        let serialized = serialize_golden(&header, &chunks);
        let (header2, chunks2) = parse_golden(&serialized).unwrap();
        assert_eq!(header.provider, header2.provider);
        assert_eq!(chunks, chunks2);
    }

    #[test]
    fn golden_file_without_header_is_rejected() {
        let bad = "data: foo\n\n";
        let r = parse_golden(bad);
        assert!(matches!(r, Err(FakeError::MissingHeader(_))));
    }

    #[test]
    fn golden_file_with_invalid_header_is_rejected() {
        let bad = "not json\ndata: foo\n\n";
        let r = parse_golden(bad);
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn scripted_body_passes_through_real_parser() {
        // Os bytes são reais, passam pelo parser real, viram StreamEvent.
        // Note: este é o teste que prova §3.1 do ADR-0008 — o fake
        // produz bytes, o parser real é exercitado.
        let chunks: &[&[u8]] = &[
            b"data: {\"choices\":[{\"delta\":{\"content\":\"oi\"}}]}\n\n",
            b"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
            b"data: [DONE]\n\n",
        ];
        let body = ScriptedBody::from_bytes(chunks);
        let mut s = Box::pin(scripted_events(body));

        let raw1 = s.next().await.unwrap().unwrap();
        let ev1 = openai_compat_translate(raw1).unwrap().unwrap();
        match ev1 {
            StreamEvent::Delta { content } => assert_eq!(content, "oi"),
            _ => panic!("esperava Delta"),
        }

        let raw2 = s.next().await.unwrap().unwrap();
        let ev2 = openai_compat_translate(raw2).unwrap().unwrap();
        assert!(matches!(ev2, StreamEvent::Done { .. }));

        let raw3 = s.next().await.unwrap().unwrap();
        let ev3 = openai_compat_translate(raw3).unwrap().unwrap();
        assert!(matches!(ev3, StreamEvent::Done { .. }));
    }

    #[test]
    fn serialize_golden_escapes_newlines_in_chunks() {
        // Garante que chunks com \n viram \\n no JSON.
        let header = FixtureHeaderInner {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            scenario: "test".to_string(),
            recorded_at: "2026-07-27T00:00:00Z".to_string(),
            recorder_version: "0.2.0".to_string(),
            source_endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        };
        let chunks = vec!["data: a\n\n".to_string()];
        let s = serialize_golden(&header, &chunks);
        // O \n literal aparece como \n no JSON (escaped), não como quebra de linha.
        assert!(s.contains(r#"\"#));
        // Linhas: header + chunk = 2 linhas.
        assert_eq!(s.lines().count(), 2);
    }
}
