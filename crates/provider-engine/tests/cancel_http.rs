//! Teste E2E do cancelamento real do HTTP no `OpenAiCompatAdapter`.
//!
//! ## Por que este teste existe
//!
//! A Etapa 4.x do `RunExecutor` monitora o `CancellationToken` em 3
//! pontos (entre rounds, entre eventos, e no `StreamEvent::Cancelled`).
//! Mas o `RunExecutor` é uma camada acima do adapter — ele só
//! interrompe o que ele mesmo está fazendo. O que estava em aberto
//! era a Etapa 5.x: confirmar que o **`OpenAiCompatAdapter` também
//! interrompe a request `reqwest` em andamento** quando o token
//! dispara.
//!
//! O `OpenAiCompatAdapter::stream` já tem um `tokio::select!` com
//! `cancel.cancelled()` entre `byte_stream.next()`. Quando o cancel
//! dispara, o `break` sai do loop e o `byte_stream` (wrapper do
//! `reqwest::Response`) é dropado, o que **deveria** abortar a
//! request. Este teste prova que isso acontece de fato subindo um
//! TCP server local que detecta quando o `reqwest` fecha a conexão.
//!
//! Se este teste passar, a stack de cancelamento é **full-stack**:
//! usuário clica Parar → `cancel_run` dispara o token → executor
//! observa no `select!` → adapter observa no `select!` → drop do
//! `byte_stream` → `reqwest` fecha a conexão TCP → server detecta
//! EOF → request abortada no upstream.
//!
//! ## Estrutura do teste
//!
//! 1. Sobe um `TcpListener` em porta aleatória.
//! 2. Spawna uma task que aceita a conexão, escreve uma resposta
//!    HTTP/1.1 válida com `Content-Type: text/event-stream`, manda
//!    1 chunk SSE válido (1 `data: ...\n\n` parseável como Delta),
//!    e fica esperando mais dados. Quando a conexão é fechada
//!    pelo cliente (read retorna 0), marca um `Arc<AtomicBool>`
//!    como `true`.
//! 3. Cria um `OpenAiCompatAdapter` apontando pro server local
//!    (`http://127.0.0.1:<porta>`), sem auth.
//! 4. Cria um `ChatRequest` com `cancel` token, chama
//!    `adapter.stream(req)` e consome o `BoxStream`.
//! 5. Lê 1 `Delta` (o chunk que o server mandou).
//! 6. Cancela o token.
//! 7. Verifica que o server detectou o fechamento da conexão.

#![cfg(not(doctest))]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use frederico_core::{ModelId, ProviderId};
use frederico_provider_engine::openai_compat::OpenAiCompatAdapter;
use frederico_provider_engine::types::{ChatMessage, ChatRequest, StopReason, StreamEvent};
use frederico_provider_engine::ProviderAdapter;
use frederico_security::fake::FakeCredentialStore;
use futures::stream::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Um chunk SSE válido pra OpenAI-compat. O executor parseia isso
/// como `Delta { content: "olá" }`.
const SSE_DELTA: &str = "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"fake\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"olá\"},\"finish_reason\":null}]}\n\n";

/// Sobe o mock server e devolve o `SocketAddr` + a flag de
/// "conexão fechada detectada".
async fn start_mock_server() -> (SocketAddr, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    let closed_clone = closed.clone();

    tokio::spawn(async move {
        // Aceita UMA conexão (o teste só faz uma request).
        let (mut stream, _peer) = listener.accept().await.unwrap();
        // Lê o request HTTP/1.1 inteiro (até \r\n\r\n).
        let mut buf = [0u8; 4096];
        let mut total = Vec::new();
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    total.extend_from_slice(&buf[..n]);
                    if total.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // Resposta HTTP/1.1 chunked + 1 chunk SSE.
        let response_head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n";
        let chunk = format!("{:x}\r\n{}\r\n", SSE_DELTA.len(), SSE_DELTA);
        let _ = stream.write_all(response_head.as_bytes()).await;
        let _ = stream.write_all(chunk.as_bytes()).await;
        let _ = stream.flush().await;
        // Fica esperando mais dados. Quando o client fecha, read
        // retorna 0 (EOF) — marca a flag.
        let mut rest = [0u8; 1024];
        loop {
            match stream.read(&mut rest).await {
                Ok(0) => {
                    closed_clone.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    (addr, closed)
}

fn make_adapter(base_url: String) -> OpenAiCompatAdapter {
    // `FakeCredentialStore::new()` já devolve `Arc<FakeCredentialStore>`;
    // a coerção pra `Arc<dyn CredentialStore>` é automática. O adapter
    // exige a credencial cadastrada (mesmo sem usar no header) —
    // cadastramos uma dummy.
    let credentials: Arc<dyn frederico_security::CredentialStore> =
        FakeCredentialStore::with_credentials([(ProviderId::new("mock"), "dummy-key")]);
    OpenAiCompatAdapter::without_auth(ProviderId::new("mock"), base_url, credentials)
}

fn make_request(cancel: CancellationToken) -> ChatRequest {
    let mut req = ChatRequest::new(
        ProviderId::new("mock"),
        ModelId::new("fake"),
        vec![ChatMessage::user("oi")],
    );
    req.cancel = cancel;
    req.event_timeout = Duration::from_secs(5);
    req
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_drops_reqwest_connection() {
    let (addr, closed_flag) = start_mock_server().await;
    let base_url = format!("http://{addr}/v1");

    let adapter = make_adapter(base_url);
    let cancel = CancellationToken::new();
    let req = make_request(cancel.clone());

    // O `OpenAiCompatAdapter::stream` é síncrono por causa da trait,
    // mas usa `tokio::spawn` internamente. Devolve um `BoxStream`.
    let mut stream = adapter.stream(req).unwrap();

    // Lê 1 Delta (o chunk que o server mandou).
    let first = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout esperando 1º evento")
        .expect("stream retornou None antes do 1º evento");
    assert!(
        matches!(first, StreamEvent::Delta { .. }),
        "esperava Delta, veio {first:?}"
    );

    // Cancela o token. O `tokio::select!` no adapter deve sair do
    // loop, dropando o `byte_stream` (= body da response), o que
    // fecha a conexão TCP.
    cancel.cancel();

    // Dá tempo do server detectar o EOF (read retorna 0).
    let detected = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if closed_flag.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or(false);

    assert!(
        detected,
        "cancel cancelou o token, mas o reqwest NÃO fechou a conexão TCP — \
         a stack de cancelamento NÃO é full-stack. O adapter precisa \
         dropar o `byte_stream` (ou chamar `response.close()`) antes \
         do `break` do `tokio::select!`."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_completes_normally_when_not_cancelled() {
    // Sanity: sem cancel, o stream termina normalmente. Esse teste
    // existe pra garantir que o teste acima não está passando por
    // um motivo diferente (ex.: o stream fechando sozinho por outra
    // razão).
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let closed_flag = Arc::new(AtomicBool::new(false));
    let closed_clone = closed_flag.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let mut total = Vec::new();
        while !total.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => total.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        // Envia 1 Delta + 1 Done (com finish_reason: stop).
        let done = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"fake\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        let combined = format!("{}{}", SSE_DELTA, done);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            combined.len(),
            combined
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
        // Espera o client fechar.
        let mut rest = [0u8; 1024];
        loop {
            match stream.read(&mut rest).await {
                Ok(0) => {
                    closed_clone.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    let adapter = make_adapter(format!("http://{addr}/v1"));
    let cancel = CancellationToken::new();
    let req = make_request(cancel);
    let mut stream = adapter.stream(req).unwrap();

    // Lê até Done.
    let mut got_done = false;
    let mut got_delta = false;
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
    {
        match ev {
            StreamEvent::Delta { .. } => got_delta = true,
            StreamEvent::Done {
                stop_reason: StopReason::Stop,
            } => {
                got_done = true;
                break;
            }
            _ => {}
        }
    }
    assert!(got_delta, "esperava Delta no caminho normal");
    assert!(got_done, "esperava Done no caminho normal");

    // O client vai naturalmente fechar a conexão quando o stream
    // for dropado após Done — a flag também deve ficar true.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if closed_flag.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}
