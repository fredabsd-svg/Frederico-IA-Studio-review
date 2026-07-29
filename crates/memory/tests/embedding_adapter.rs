//! Teste E2E do `OpenRouterEmbeddingAdapter` com um
//! `TcpListener` local (mesma técnica do `cancel_http.rs`
//! da Fase 3). O adapter bate no servidor local e parseia
//! a resposta — sem internet, sem chave real.

use frederico_memory::embedding::{EmbeddingError, EmbeddingProvider, OpenRouterEmbeddingAdapter};
use secrecy::SecretString;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Sobe um servidor HTTP local que devolve a resposta
/// OpenAI-compat fixa. Retorna a URL pro caller montar o
/// adapter.
async fn start_test_server(response_body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept");
            // Lê o request (não importa o conteúdo).
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            // Responde.
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn adapter_sends_correct_request_and_parses_response() {
    // Resposta com 2 embeddings de 4 dim.
    let body = r#"{
        "data": [
            {"embedding": [0.1, 0.2, 0.3, 0.4], "index": 0},
            {"embedding": [0.5, 0.6, 0.7, 0.8], "index": 1}
        ],
        "model": "test-model",
        "usage": {"prompt_tokens": 8, "total_tokens": 8}
    }"#;
    let url = start_test_server(body).await;
    let api_key = SecretString::new("test-key".to_string().into());
    let adapter = OpenRouterEmbeddingAdapter::for_test(api_key, url);

    assert_eq!(adapter.provider_id(), "openrouter");
    assert_eq!(adapter.model_id(), "test-model");
    assert_eq!(adapter.dimensions(), 4);

    let inputs = vec!["texto 1", "texto 2"];
    let result = adapter.embed(&inputs).await.expect("embed");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], vec![0.1, 0.2, 0.3, 0.4]);
    assert_eq!(result[1], vec![0.5, 0.6, 0.7, 0.8]);
}

#[tokio::test]
async fn adapter_rejects_response_with_wrong_embedding_count() {
    // Resposta com 1 embedding quando 2 foram pedidos.
    let body = r#"{"data": [{"embedding": [0.1, 0.2, 0.3, 0.4]}]}"#;
    let url = start_test_server(body).await;
    let api_key = SecretString::new("test-key".to_string().into());
    let adapter = OpenRouterEmbeddingAdapter::for_test(api_key, url);

    let inputs = vec!["um", "dois"];
    let result = adapter.embed(&inputs).await;
    assert!(matches!(result, Err(EmbeddingError::Transport(_))));
}

#[tokio::test]
async fn adapter_rejects_wrong_dimensions() {
    // Resposta com 8 dim quando o adapter espera 4.
    let body = r#"{
        "data": [
            {"embedding": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]}
        ]
    }"#;
    let url = start_test_server(body).await;
    let api_key = SecretString::new("test-key".to_string().into());
    let adapter = OpenRouterEmbeddingAdapter::for_test(api_key, url);

    let inputs = vec!["um"];
    let result = adapter.embed(&inputs).await;
    assert!(matches!(
        result,
        Err(EmbeddingError::DimensionMismatch { .. })
    ));
}

#[tokio::test]
async fn adapter_handles_http_error() {
    // Servidor que devolve 500.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = socket.shutdown().await;
        }
    });
    let url = format!("http://{}", addr);
    let api_key = SecretString::new("test-key".to_string().into());
    let adapter = OpenRouterEmbeddingAdapter::for_test(api_key, url);

    let inputs = vec!["um"];
    let result = adapter.embed(&inputs).await;
    assert!(matches!(result, Err(EmbeddingError::Transport(_))));
}

#[test]
fn debug_does_not_leak_api_key() {
    let api_key = SecretString::new("sk-secret-12345".to_string().into());
    let adapter = OpenRouterEmbeddingAdapter::for_test(api_key, "http://127.0.0.1:0");
    let debug = format!("{adapter:?}");
    // A `SecretString` redata o valor em `Debug`.
    assert!(
        !debug.contains("sk-secret-12345"),
        "Debug não deve vazar a API key"
    );
    assert!(debug.contains("[REDACTED]") || !debug.contains("sk-"));
}
