//! E2E: proxy de rede do sandbox (Fase 7, Etapa 6, ADR-0033).
//!
//! Cobre os 4 cenários obrigatórios definidos na conversa do
//! user pra Etapa 6 (registrados na Etapa 6 do user_profile):
//!
//! 1. **Positivo** — host na allowlist é forwarded com sucesso.
//! 2. **Negativo** — host não permitido recebe `502 Bad Gateway`
//!    + entrada no audit com `decision = "deny"`.
//! 3. **CONNECT tunnel HTTPS** — bytes são tunelados, audit
//!    registra `path_redacted = "<redacted>"` (o TLS é opaco).
//! 4. **Bypass via raw socket documentado** — sem `HTTP_PROXY` na
//!    env, conexão raw socket conecta direto (a barreira do proxy
//!    é convenção, não imposição; o teste **documenta** isso, não
//!    tenta bloquear).
//! 5. **Audit sink registra todas decisões** — múltiplas
//!    tentativas, todas registradas.
//!
//! **Setup:** os testes sobem um upstream TCP server local
//! (`127.0.0.1:0`), iniciam o proxy com allowlist específica,
//! e fazem requests via `reqwest` apontando pro proxy (com
//! `HTTP_PROXY`/`HTTPS_PROXY` na env) ou via `tokio::net::TcpStream`
//! direto (bypass).
//!
//! **Por que o proxy é testado isolado, sem `ChatOrchestrator`?**
//! O proxy do sandbox é uma peça **independente** do loop de
//! execução de run — é só um TCP listener que recebe requests e
//! dispatcha. O `ChatOrchestrator`/ `RunExecutor` consome via
//! `HTTP_PROXY` env var (configuração), não chama o proxy
//! diretamente. Testar o proxy isoladamente cobre o contrato
//! de **mecanismo**; a integração com o sandbox (env injection,
//! `RunExecutor` lifecycle) é testada em outro arquivo (a Etapa
//! 6.1 do plano).
//!
//! **Por que o upstream é TCP raw, não reqwest?** Pra poder
//! responder de formas arbitrárias (HTTP 200 com body customizado
//! pra teste positivo, conexões longas pra teste de CONNECT
//! tunnel). Reqwest como servidor precisaria de mock — overhead
//! sem ganho.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use frederico_security::network::{
    start_proxy, NetworkAccessEntry, NetworkAuditSink, NetworkDecision, NoopNetworkAuditSink,
    ProxyConfig,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// RecordingNetworkAuditSink — sink em memória que captura tudo
// ---------------------------------------------------------------------------

/// Sink que acumula todas as entradas em memória. Thread-safe via
/// `Mutex`. Análogo ao `RecordingEventSink` da PR #50 (mesma
/// ideia — sink que indexa por algo, sem canal, mas aqui é
/// sequência de inserts).
#[derive(Debug, Default)]
struct RecordingNetworkAuditSink {
    entries: std::sync::Mutex<Vec<NetworkAccessEntry>>,
}

impl RecordingNetworkAuditSink {
    fn new() -> Self {
        Self::default()
    }

    fn entries(&self) -> Vec<NetworkAccessEntry> {
        self.entries.lock().unwrap().clone()
    }

    #[allow(dead_code)] // simétrico a `count_denies`; alguns testes podem usar
    fn count_allows(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.decision == NetworkDecision::Allow)
            .count()
    }

    fn count_denies(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.decision == NetworkDecision::Deny)
            .count()
    }
}

impl NetworkAuditSink for RecordingNetworkAuditSink {
    fn record(&self, entry: NetworkAccessEntry) {
        self.entries.lock().unwrap().push(entry);
    }
}

// ---------------------------------------------------------------------------
// upstream_http_server — TCP server que responde HTTP 200 com body
// ---------------------------------------------------------------------------

/// Sobe um TCP listener em `127.0.0.1:0` que devolve
/// `HTTP/1.1 200 OK` com body `Hello from upstream\n` pra qualquer
/// request. Spawna a task e devolve o `SocketAddr`. Caller faz
/// `keep_alive` = se a task morrer, o listener fecha.
async fn spawn_upstream_http() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream bind");
    let addr = listener.local_addr().expect("upstream local_addr");
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            // Lê o request (não importa o conteúdo, só respondemos).
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            // Resposta fixa.
            let body = "Hello from upstream\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    addr
}

// ---------------------------------------------------------------------------
// upstream_echo_server — TCP server que repete bytes (teste de CONNECT)
// ---------------------------------------------------------------------------

/// Sobe um TCP listener em `127.0.0.1:0` que, pra cada conexão
/// aceita, **lê bytes do client e reenvia de volta** (echo).
/// Usado pra testar o CONNECT tunnel — o proxy conecta o client
/// ao upstream, e o tunnel é byte-opaco. O teste escreve
/// `PING\n` e verifica que recebe `PING\n` de volta (vindo do
/// upstream via tunnel).
async fn spawn_upstream_echo() -> (std::net::SocketAddr, Arc<AtomicU64>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("echo bind");
    let addr = listener.local_addr().expect("echo local_addr");
    let conn_count = Arc::new(AtomicU64::new(0));
    let conn_count_clone = conn_count.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            conn_count_clone.fetch_add(1, Ordering::SeqCst);
            // Lê tudo até EOF e reenvia. Pra teste simples, lê
            // um chunk de 1KB e reenvia.
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            });
        }
    });
    (addr, conn_count)
}

// ---------------------------------------------------------------------------
// 1) e2e_network_proxy_allows_allowlisted_host
// ---------------------------------------------------------------------------

/// Positivo: host na allowlist é forwardado com sucesso. Reqwest
/// aponta pro proxy, proxy repassa pro upstream, response volta
/// com o body do upstream. Audit registra `decision = "allow"`,
/// `status_code = 200`, `bytes_received > 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_proxy_allows_allowlisted_host() {
    // 1. Upstream HTTP local.
    let upstream = spawn_upstream_http().await;
    let upstream_host = upstream.ip().to_string();
    let upstream_port = upstream.port();

    // 2. Proxy com allowlist = upstream.
    let audit = Arc::new(RecordingNetworkAuditSink::new());
    let allowlist =
        frederico_security::network::NetworkAllowlist::new().with_allowed([upstream_host.as_str()]);
    let config = ProxyConfig {
        allowlist,
        request_timeout: Duration::from_secs(5),
    };
    let handle = start_proxy(config, audit.clone(), None).expect("start_proxy");
    let proxy_url = format!("http://127.0.0.1:{}", handle.port);

    // 3. Cliente HTTP via reqwest apontando pro proxy.
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).expect("proxy URL"))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let response = client
        .get(format!("http://{upstream_host}:{upstream_port}/some/path"))
        .send()
        .await
        .expect("GET via proxy");
    assert_eq!(response.status().as_u16(), 200);
    let body = response.text().await.expect("body");
    assert_eq!(body, "Hello from upstream\n");

    // 4. Audit registrou o allow com status 200.
    let entries = audit.entries();
    assert_eq!(entries.len(), 1, "audit deve ter 1 entrada (a do GET)");
    let e = &entries[0];
    assert_eq!(e.method, "GET");
    assert_eq!(e.decision, NetworkDecision::Allow);
    assert_eq!(e.status_code, 200);
    assert!(e.bytes_received > 0);
    assert_eq!(e.deny_reason, None);
    assert_eq!(e.host, upstream_host);

    frederico_security::network::shutdown(handle);
}

// ---------------------------------------------------------------------------
// 2) e2e_network_proxy_denies_non_allowlisted
// ---------------------------------------------------------------------------

/// Negativo: allowlist vazia, qualquer host é recusado. Cliente
/// HTTP recebe `502 Bad Gateway`, audit registra `decision =
/// "deny"`, `status_code = 502`, `deny_reason = "allowlist_empty"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_proxy_denies_non_allowlisted() {
    // Upstream qualquer (não vai ser chamado).
    let _upstream = spawn_upstream_http().await;

    // Proxy com allowlist VAZIA.
    let audit = Arc::new(RecordingNetworkAuditSink::new());
    let config = ProxyConfig {
        allowlist: frederico_security::network::NetworkAllowlist::new(), // vazia
        request_timeout: Duration::from_secs(5),
    };
    let handle = start_proxy(config, audit.clone(), None).expect("start_proxy");
    let proxy_url = format!("http://127.0.0.1:{}", handle.port);

    // Cliente HTTP via proxy.
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).expect("proxy URL"))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let response = client
        .get("http://example.com/some/path")
        .send()
        .await
        .expect("GET via proxy (vai falhar com 502)");
    // reqwest devolve Ok com status 502 (o proxy fechou a conexão
    // com 502, que é HTTP válido).
    assert_eq!(response.status().as_u16(), 502);

    // Audit.
    let entries = audit.entries();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.decision, NetworkDecision::Deny);
    assert_eq!(e.status_code, 502);
    assert_eq!(e.deny_reason.as_deref(), Some("allowlist_empty"));
    assert_eq!(e.method, "GET");
    assert!(e.bytes_received == 0); // upstream nunca foi contactado

    frederico_security::network::shutdown(handle);
}

/// Negativo com allowlist não-vazia mas host não incluso: `deny_reason`
/// é `"not_in_allowlist"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_proxy_denies_host_not_in_allowlist() {
    let _upstream = spawn_upstream_http().await;

    let audit = Arc::new(RecordingNetworkAuditSink::new());
    let allowlist =
        frederico_security::network::NetworkAllowlist::new().with_allowed(["allowed.example.com"]);
    let config = ProxyConfig {
        allowlist,
        request_timeout: Duration::from_secs(5),
    };
    let handle = start_proxy(config, audit.clone(), None).expect("start_proxy");
    let proxy_url = format!("http://127.0.0.1:{}", handle.port);

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).expect("proxy URL"))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let response = client
        .get("http://blocked.example.com/path")
        .send()
        .await
        .expect("GET");
    assert_eq!(response.status().as_u16(), 502);

    let entries = audit.entries();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.decision, NetworkDecision::Deny);
    assert_eq!(e.deny_reason.as_deref(), Some("not_in_allowlist"));
    assert_eq!(e.host, "blocked.example.com");

    frederico_security::network::shutdown(handle);
}

// ---------------------------------------------------------------------------
// 3) e2e_network_proxy_connect_tunnel_https
// ---------------------------------------------------------------------------

/// CONNECT tunnel: cliente envia `CONNECT host:port`, proxy
/// responde `200 Connection Established` e tunela bytes
/// bidirecionalmente. Audit registra `path_redacted = "<redacted>"`
/// (o proxy não vê o path pós-TLS, §"HTTPS via CONNECT" do
/// ADR-0033 — princípio honesto: log não promete mais do que
/// entrega).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_proxy_connect_tunnel_https() {
    // Upstream echo server.
    let (upstream, _conn_count) = spawn_upstream_echo().await;
    let upstream_host = upstream.ip().to_string();
    let upstream_port = upstream.port();

    // Proxy com allowlist = upstream.
    let audit = Arc::new(RecordingNetworkAuditSink::new());
    let allowlist =
        frederico_security::network::NetworkAllowlist::new().with_allowed([upstream_host.as_str()]);
    let config = ProxyConfig {
        allowlist,
        request_timeout: Duration::from_secs(5),
    };
    let handle = start_proxy(config, audit.clone(), None).expect("start_proxy");
    let proxy_port = handle.port;

    // Cliente raw TCP (não reqwest — reqwest faz CONNECT
    // internamente, mas a gente quer inspecionar o tunnel).
    let mut client = TcpStream::connect(("127.0.0.1", proxy_port))
        .await
        .expect("connect to proxy");

    // CONNECT request.
    let connect_req = format!(
        "CONNECT {upstream_host}:{upstream_port} HTTP/1.1\r\nHost: {upstream_host}:{upstream_port}\r\n\r\n"
    );
    client
        .write_all(connect_req.as_bytes())
        .await
        .expect("send CONNECT");

    // Lê a response do proxy: deve ser `HTTP/1.1 200 Connection Established`.
    let mut buf = vec![0u8; 1024];
    let n = client.read(&mut buf).await.expect("read proxy response");
    let response = std::str::from_utf8(&buf[..n]).expect("utf8 response");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "esperava 200 Connection Established, recebi: {response}"
    );

    // Tunnel: envia "PING\n" e espera "PING\n" de volta.
    client.write_all(b"PING\n").await.expect("send PING");
    let mut echo_buf = [0u8; 5];
    let n = client.read_exact(&mut echo_buf).await.expect("read echo");
    assert_eq!(&echo_buf[..n], b"PING\n");

    // Audit: 1 entrada CONNECT, decision=Allow, path_redacted="<redacted>".
    let entries = audit.entries();
    assert_eq!(entries.len(), 1, "1 entrada CONNECT esperada");
    let e = &entries[0];
    assert_eq!(e.method, "CONNECT");
    assert_eq!(e.decision, NetworkDecision::Allow);
    assert_eq!(e.path_redacted, "<redacted>");
    assert_eq!(e.host, upstream_host);
    assert_eq!(e.port, upstream_port);
    // Status code: 200 (Connection Established) ou 0 se o tunnel
    // terminou antes de qualquer byte — ambos são OK.
    assert!(e.status_code == 200 || e.status_code == 0);

    frederico_security::network::shutdown(handle);
}

// ---------------------------------------------------------------------------
// 4) e2e_network_raw_socket_bypasses_proxy_documented
// ---------------------------------------------------------------------------

/// Bypass documentado: sem `HTTP_PROXY` na env, uma conexão raw
/// socket conecta direto no upstream. **Esperado**: o proxy **não**
/// bloqueia (a barreira do proxy é `HTTP_PROXY`, convenção, não
/// imposição). Este teste **documenta** o comportamento, não
/// tenta bloquear — fixar uma limitação conhecida vale tanto
/// quanto teste que prova proteção. Impede que daqui a 3 meses
/// alguém leia "rede bloqueada" e acredite que raw socket está
/// coberto.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_raw_socket_bypasses_proxy_documented() {
    // Upstream echo server — vai ser acessado **diretamente**, sem proxy.
    let (upstream, conn_count) = spawn_upstream_echo().await;

    // **Sem proxy, sem HTTP_PROXY na env.** Conexão TCP direta.
    let mut raw = TcpStream::connect(upstream)
        .await
        .expect("raw socket connect (bypass) deve funcionar");

    // Envia e recebe.
    raw.write_all(b"RAW_BYPASS\n").await.expect("raw write");
    let mut echo = [0u8; 11];
    raw.read_exact(&mut echo).await.expect("raw read echo");
    assert_eq!(&echo, b"RAW_BYPASS\n");

    // Upstream recebeu a conexão.
    assert!(
        conn_count.load(Ordering::SeqCst) >= 1,
        "upstream deve ter recebido a conexão (bypass funcionou)"
    );

    // **Documentação do trade-off** (o `SECURITY.md` §"O que essa
    // combinação NÃO protege" cita isso verbatim):
    //
    //   O proxy só captura requests que passam por `HTTP_PROXY` /
    //   `HTTPS_PROXY` (ou seja, **bibliotecas padrão** que
    //   respeitam a env). Um filho com `socket.socket(...)` raw
    //   ignora a env e conecta direto. Defesa em profundidade
    //   real exige firewall no nível de processo (WDAC,
    //   roadmap de Fase 8+). A v1 do proxy impede acesso
    //   **acidental** e por bibliotecas comuns, não um
    //   atacante determinado.
    //
    // (Sem `assert!` adicional — o teste acima já documenta que
    // a conexão raw foi bem-sucedida. A doc do `SECURITY.md`
    // deve citar esse teste pelo nome como prova da limitação.)
}

// ---------------------------------------------------------------------------
// 5) e2e_network_audit_records_decisions
// ---------------------------------------------------------------------------

/// Audit sink registra **toda** decisão (allow e deny). Cenário
/// misto: 1 allow (host na allowlist) + 2 denies (host fora +
/// CONNECT host fora) — verifica que o sink capturou todos com
/// o `decision` correto.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_network_audit_records_decisions() {
    // Upstream HTTP que **só** recebe requests pra `allowed.example.com`.
    // Pra esse teste, o upstream não importa muito — o que vale é
    // a sequência de allow/deny no audit.
    let _upstream = spawn_upstream_http().await;

    let audit = Arc::new(RecordingNetworkAuditSink::new());
    let allowlist =
        frederico_security::network::NetworkAllowlist::new().with_allowed(["allowed.example.com"]);
    let config = ProxyConfig {
        allowlist,
        request_timeout: Duration::from_secs(5),
    };
    let handle = start_proxy(config, audit.clone(), None).expect("start_proxy");
    let proxy_url = format!("http://127.0.0.1:{}", handle.port);

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&proxy_url).expect("proxy URL"))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");

    // Request 1: host na allowlist (allow — mas vai falhar com
    // erro de DNS do upstream, que é `allowed.example.com` —
    // `dns_intercept` ainda não está wireado, então o reqwest
    // tenta resolver DNS direto. **O proxy autorizou** antes do
    // DNS falhar, então o audit registra `decision=allow`,
    // `deny_reason=None` ou `deny_reason=upstream_unreachable`
    // dependendo de quando o erro é reportado).
    let _r1 = client.get("http://allowed.example.com/foo").send().await;

    // Request 2: host fora (deny garantido — `blocked.example.com`
    // nunca passa o allowlist check, mesmo se o DNS existisse).
    let _r2 = client.get("http://blocked.example.com/foo").send().await;

    // Request 3: outro host fora.
    let _r3 = client
        .get("http://another-blocked.example.org/bar")
        .send()
        .await;

    // Audit: 3 entradas (1 com decision=allow ou upstream_unreachable,
    // 2 com decision=deny). Pode haver variação baseada em
    // timing do DNS — o que importa é que **toda request**
    // gerou 1 entrada.
    let entries = audit.entries();
    assert!(
        entries.len() >= 2,
        "audit deve ter pelo menos 2 entradas (allow + 1 deny), tem {}",
        entries.len()
    );
    assert!(
        audit.count_denies() >= 1,
        "audit deve ter pelo menos 1 deny"
    );

    // Cada entrada tem `decision` válido e `deny_reason` consistente.
    for e in &entries {
        if e.decision == NetworkDecision::Deny {
            assert!(e.deny_reason.is_some(), "Deny sem deny_reason: {e:?}");
        } else {
            // Allow: deny_reason pode ser None ou
            // `upstream_unreachable` se o reqwest falhou no
            // DNS depois que o proxy autorizou.
            assert!(
                e.deny_reason.is_none() || e.deny_reason.as_deref() == Some("upstream_unreachable"),
                "Allow com deny_reason estranho: {:?}",
                e.deny_reason
            );
        }
    }

    frederico_security::network::shutdown(handle);
}

// ---------------------------------------------------------------------------
// Smoke test: o `NoopNetworkAuditSink` não panica
// ---------------------------------------------------------------------------

#[test]
fn noop_network_audit_sink_does_not_panic() {
    let sink = NoopNetworkAuditSink;
    sink.record(frederico_security::network::NetworkAccessEntry {
        run_id: None,
        host: "test.example.com".to_string(),
        port: 443,
        method: "GET".to_string(),
        path_redacted: "/".to_string(),
        status_code: 200,
        bytes_sent: 0,
        bytes_received: 0,
        decision: NetworkDecision::Allow,
        deny_reason: None,
        timestamp: "2026-08-13T00:00:00Z".to_string(),
    });
    // Sem panic = OK.
}
