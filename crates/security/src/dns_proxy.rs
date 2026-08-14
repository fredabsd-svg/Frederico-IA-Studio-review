//! Responder DNS mínimo do sandbox (Etapa 7 da Fase 7, ADR-0033 §D5).
//!
//! O [`dns_intercept`](crate::dns_intercept) aponta o resolver do
//! Windows (interface Loopback) pra `127.0.0.1:53`. Sem algo
//! escutando ali, o `netsh dns set` sozinho não faz nada além de
//! quebrar a resolução de nomes do filho (query cai no vazio). Este
//! módulo é o "algo": um servidor DNS mínimo em UDP que aplica a
//! mesma [`NetworkAllowlist`] do proxy HTTP/HTTPS ([`crate::network`])
//! **antes** de resolver o hostname — hostname matching, não IP
//! matching (D5 do ADR-0033).
//!
//! ## Fluxo
//!
//! 1. Filho chama `socket.getaddrinfo("host")`.
//! 2. Windows manda a query UDP pra `127.0.0.1:53` (aqui).
//! 3. `parse_query` decodifica a question (RFC 1035 §4.1.2).
//! 4. Se `qtype` não for `A` (IPv4) → `NXDOMAIN` (limitação IPv4-only
//!    da v1, mesmo espírito da lacuna HTTP/3/QUIC documentada em
//!    `crate::network`).
//! 5. Se o hostname não está na allowlist → `NXDOMAIN`.
//! 6. Se está, resolve via `tokio::net::lookup_host` (DNS real do
//!    host, fora do intercept — o intercept só vale pro adaptador
//!    Loopback) e responde com o primeiro IPv4 encontrado.
//!
//! Toda query gera uma entrada no mesmo [`NetworkAuditSink`] do
//! proxy HTTP (reuso do `NetworkAccessEntry` — sem schema novo),
//! com `method = "DNS"` e `port = 53`.
//!
//! ## Escopo desta v1
//!
//! - Só `QTYPE=A` (IPv4). `AAAA` e outros tipos voltam `NXDOMAIN`
//!   sem tentar resolver.
//! - Sem DNS message compression na question recebida (queries de
//!   resolvers padrão não comprimem a question — só respostas
//!   comprimem, e esta v1 não gera compression na resposta).
//! - `TTL` fixo de 60s nas respostas — curto o bastante pra não
//!   deixar o filho com cache stale se a allowlist mudar no meio
//!   de uma execução longa.
//!
//! ## Testes
//!
//! A lógica de parse/build e a decisão allow/deny são testadas
//! ponta-a-ponta com um `UdpSocket` real em porta efêmera
//! (`127.0.0.1:0`) — **sem** tocar `netsh` nem a porta 53. O wiring
//! com `netsh`/porta 53 fica em `crates/tool-registry/src/exec/mod.rs`
//! e não é exercitado por teste automatizado nesta sessão (reconfigura
//! DNS da máquina — verificação manual, ver `docs/status.md`).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

use crate::network::{
    iso8601_now, NetworkAccessEntry, NetworkAllowlist, NetworkAuditSink, NetworkDecision,
};

/// `QTYPE` IPv4 (RFC 1035 §3.2.2). Único tipo respondido com dados
/// nesta v1 — o resto vira `NXDOMAIN` (ver doc do módulo).
const QTYPE_A: u16 = 1;
/// `QCLASS` Internet (RFC 1035 §3.2.4). Única classe aceita — query
/// com outra classe é tratada como malformada (`parse_query` retorna
/// `None`, sem resposta).
const QCLASS_IN: u16 = 1;

const FLAG_QR: u16 = 0x8000;
const FLAG_RD: u16 = 0x0100;
const FLAG_RA: u16 = 0x0080;
const RCODE_NOERROR: u16 = 0;
const RCODE_NXDOMAIN: u16 = 3;

/// Config do responder DNS. Compartilha a mesma [`NetworkAllowlist`]
/// do proxy HTTP/HTTPS — um único ponto de política pro sandbox.
#[derive(Debug, Clone)]
pub struct DnsProxyConfig {
    pub allowlist: NetworkAllowlist,
}

/// Handle do responder em execução. Mesmo padrão RAII-por-fora do
/// [`crate::network::ProxyHandle`]: `Drop` **não** derruba o
/// listener — o caller chama [`shutdown`] explicitamente.
#[derive(Debug)]
pub struct DnsProxyHandle {
    /// Porta atribuída pelo OS quando `bind_addr` usa porta `0`
    /// (testes). Em produção, o caller passa `127.0.0.1:53`
    /// explicitamente e esse valor vem sempre `53`.
    pub port: u16,
    shutdown: oneshot::Sender<()>,
}

/// Erro fatal do `start_dns_proxy`. Erros **dentro** do loop
/// (query malformada, resolve falhou) não abortam o listener — só
/// resultam em sem-resposta ou `NXDOMAIN` + entrada no audit.
#[derive(Debug, Error)]
pub enum DnsProxyError {
    #[error("falha ao criar UdpSocket em {0}: {1}")]
    ListenerBind(SocketAddr, String),
}

/// Sobe o responder DNS em `bind_addr`. Produção usa
/// `127.0.0.1:53` (porta fixa — DNS é sempre 53 por protocolo, o
/// `netsh dns set` não permite apontar pra outra porta). Testes
/// usam `127.0.0.1:0` (porta efêmera) e leem a porta real via
/// `DnsProxyHandle::port`.
///
/// **Síncrona** (não `async fn`) pelo mesmo motivo de
/// [`crate::network::start_proxy`]: bind via `std::net::UdpSocket`,
/// `set_nonblocking`, e `tokio::net::UdpSocket::from_std`, sem
/// `.await`. Isso deixa o caller (`start_network_proxy` do
/// `tool-registry`, hoje síncrono) chamar sem precisar virar
/// `async fn` — só precisa rodar dentro de um runtime Tokio (pro
/// `tokio::spawn` funcionar), não dentro de uma task async.
pub fn start_dns_proxy(
    bind_addr: SocketAddr,
    config: DnsProxyConfig,
    audit: Arc<dyn NetworkAuditSink>,
    run_id: Option<String>,
) -> Result<DnsProxyHandle, DnsProxyError> {
    let std_socket = std::net::UdpSocket::bind(bind_addr)
        .map_err(|e| DnsProxyError::ListenerBind(bind_addr, e.to_string()))?;
    std_socket
        .set_nonblocking(true)
        .map_err(|e| DnsProxyError::ListenerBind(bind_addr, e.to_string()))?;
    let socket = UdpSocket::from_std(std_socket)
        .map_err(|e| DnsProxyError::ListenerBind(bind_addr, e.to_string()))?;
    let port = socket
        .local_addr()
        .map_err(|e| DnsProxyError::ListenerBind(bind_addr, e.to_string()))?
        .port();
    let socket = Arc::new(socket);

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        // 512 bytes: tamanho clássico máximo de mensagem DNS sobre
        // UDP sem EDNS0 (RFC 1035 §2.3.4). As queries de
        // `getaddrinfo` (o único caller real, via `netsh dns`)
        // nunca excedem isso.
        let mut buf = vec![0u8; 512];
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    tracing::info!("dns_proxy: shutdown recebido, listener fechando");
                    break;
                }
                recv = socket.recv_from(&mut buf) => {
                    match recv {
                        Ok((n, peer)) => {
                            let query = buf[..n].to_vec();
                            let socket = Arc::clone(&socket);
                            let allowlist = config.allowlist.clone();
                            let audit = Arc::clone(&audit);
                            let run_id = run_id.clone();
                            tokio::spawn(async move {
                                handle_query(socket, peer, query, allowlist, audit, run_id).await;
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "dns_proxy: recv_from falhou");
                        }
                    }
                }
            }
        }
    });

    Ok(DnsProxyHandle {
        port,
        shutdown: shutdown_tx,
    })
}

/// Sinaliza o shutdown do listener. Idempotente (chamar 2x é
/// no-op, mesmo comportamento de [`crate::network::shutdown`]).
pub fn shutdown(handle: DnsProxyHandle) {
    let _ = handle.shutdown.send(());
}

/// Lida com uma query recebida: parse, decide (qtype + allowlist),
/// resolve se permitido, responde, audita.
async fn handle_query(
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    query: Vec<u8>,
    allowlist: NetworkAllowlist,
    audit: Arc<dyn NetworkAuditSink>,
    run_id: Option<String>,
) {
    let parsed = match parse_query(&query) {
        Some(p) => p,
        None => {
            // Query malformada — sem resposta (mesmo comportamento
            // de um resolver real diante de garbage: silêncio, não
            // erro, pra não virar oráculo de reflexão).
            tracing::debug!(peer = %peer, "dns_proxy: query malformada, ignorando");
            return;
        }
    };

    if parsed.qtype != QTYPE_A {
        respond(&socket, peer, &parsed, None).await;
        audit_record(
            &audit,
            &run_id,
            &parsed.name,
            NetworkDecision::Deny,
            Some("qtype_not_supported"),
        );
        return;
    }

    if !allowlist.contains(&parsed.name) {
        respond(&socket, peer, &parsed, None).await;
        let reason = if allowlist.is_empty() {
            "allowlist_empty"
        } else {
            "not_in_allowlist"
        };
        audit_record(
            &audit,
            &run_id,
            &parsed.name,
            NetworkDecision::Deny,
            Some(reason),
        );
        return;
    }

    // Resolve via o DNS real do host — o intercept só desvia o
    // adaptador Loopback; a resolução daqui em diante usa o
    // resolver normal do sistema (D5 do ADR-0033: hostname
    // matching acontece antes de resolver, não depois).
    let resolved = tokio::net::lookup_host((parsed.name.as_str(), 0))
        .await
        .ok()
        .and_then(|addrs| {
            addrs.into_iter().find_map(|addr| match addr {
                SocketAddr::V4(v4) => Some(*v4.ip()),
                SocketAddr::V6(_) => None,
            })
        });

    match resolved {
        Some(ipv4) => {
            respond(&socket, peer, &parsed, Some(ipv4)).await;
            audit_record(&audit, &run_id, &parsed.name, NetworkDecision::Allow, None);
        }
        None => {
            respond(&socket, peer, &parsed, None).await;
            audit_record(
                &audit,
                &run_id,
                &parsed.name,
                NetworkDecision::Deny,
                Some("resolve_failed"),
            );
        }
    }
}

async fn respond(
    socket: &UdpSocket,
    peer: SocketAddr,
    parsed: &ParsedQuery,
    answer: Option<Ipv4Addr>,
) {
    let response = build_response(parsed, answer);
    if let Err(e) = socket.send_to(&response, peer).await {
        tracing::debug!(peer = %peer, error = %e, "dns_proxy: send_to falhou");
    }
}

fn audit_record(
    audit: &Arc<dyn NetworkAuditSink>,
    run_id: &Option<String>,
    host: &str,
    decision: NetworkDecision,
    deny_reason: Option<&str>,
) {
    audit.record(NetworkAccessEntry {
        run_id: run_id.clone(),
        host: host.to_string(),
        port: 53,
        method: "DNS".to_string(),
        // Sem path em DNS — reusa o mesmo schema do audit HTTP com
        // um sentinel, em vez de criar tabela nova.
        path_redacted: "<n/a>".to_string(),
        // `status_code` não tem equivalente em DNS (sem HTTP
        // status); `0` é o mesmo sentinel que o proxy HTTP usa
        // quando a conexão nunca chega no upstream.
        status_code: 0,
        bytes_sent: 0,
        bytes_received: 0,
        decision,
        deny_reason: deny_reason.map(str::to_string),
        timestamp: iso8601_now(),
    });
}

/// Query DNS decodificada — só o necessário pro dispatch (nome,
/// tipo) + os bytes crus da question (reusados pra echo na
/// resposta, RFC 1035 §4.1.2 exige a question original de volta).
struct ParsedQuery {
    id: u16,
    name: String,
    qtype: u16,
    /// Bytes crus da question section (nome codificado + QTYPE +
    /// QCLASS), exatamente como vieram na query.
    question_bytes: Vec<u8>,
}

/// Decodifica o header + a primeira question de uma query DNS
/// (RFC 1035 §4.1.1-4.1.2). Só lê **uma** question (`QDCOUNT`
/// maior que 1 é ignorado além da primeira — resolvers reais
/// mandam sempre `QDCOUNT=1`). Retorna `None` pra qualquer coisa
/// malformada ou fora do escopo (`QCLASS != IN`).
fn parse_query(buf: &[u8]) -> Option<ParsedQuery> {
    // Header tem 12 bytes fixos (RFC 1035 §4.1.1).
    if buf.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount == 0 {
        return None;
    }

    let (name, pos) = parse_name(buf, 12)?;
    if pos + 4 > buf.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
    let qclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
    if qclass != QCLASS_IN {
        return None;
    }
    let question_end = pos + 4;
    let question_bytes = buf[12..question_end].to_vec();

    Some(ParsedQuery {
        id,
        name,
        qtype,
        question_bytes,
    })
}

/// Decodifica um nome DNS length-prefixed a partir de `pos`
/// (labels terminados por um byte `0x00`). Retorna `(nome com
/// pontos, posição logo após o terminador)`. **Não** suporta
/// message compression (ponteiros `0xC0`) — queries de resolvers
/// padrão não comprimem a question, só respostas comprimem.
fn parse_name(buf: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    loop {
        let len = *buf.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 != 0 {
            // Compression pointer — fora de escopo (ver doc acima).
            return None;
        }
        pos += 1;
        let label_bytes = buf.get(pos..pos + len)?;
        labels.push(std::str::from_utf8(label_bytes).ok()?.to_string());
        pos += len;
    }
    Some((labels.join("."), pos))
}

/// Monta a resposta DNS (RFC 1035 §4.1). Sempre ecoa a question
/// original (`id` + `question_bytes`). `answer = Some(ip)` produz
/// `NOERROR` com um registro `A`; `answer = None` produz
/// `NXDOMAIN` sem registros.
fn build_response(parsed: &ParsedQuery, answer: Option<Ipv4Addr>) -> Vec<u8> {
    let (rcode, ancount) = match answer {
        Some(_) => (RCODE_NOERROR, 1u16),
        None => (RCODE_NXDOMAIN, 0u16),
    };
    let flags = FLAG_QR | FLAG_RD | FLAG_RA | rcode;

    let mut out = Vec::with_capacity(parsed.question_bytes.len() + 12 + 16);
    out.extend_from_slice(&parsed.id.to_be_bytes());
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&ancount.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    // Question (echo — RFC 1035 §4.1.2, obrigatório na resposta).
    out.extend_from_slice(&parsed.question_bytes);

    if let Some(ip) = answer {
        // Answer record: `NAME` via ponteiro de compression pra
        // offset 12 (onde a question começa — economiza reenviar o
        // nome, RFC 1035 §4.1.4).
        out.extend_from_slice(&[0xC0, 0x0C]);
        out.extend_from_slice(&QTYPE_A.to_be_bytes());
        out.extend_from_slice(&QCLASS_IN.to_be_bytes());
        out.extend_from_slice(&60u32.to_be_bytes()); // TTL
        out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        out.extend_from_slice(&ip.octets());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Codifica uma query DNS mínima pra `name`/`qtype`/`qclass` —
    /// usado tanto pelos testes de `parse_query` quanto pelos
    /// testes de integração via UDP real.
    fn encode_query(id: u16, name: &str, qtype: u16, qclass: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&FLAG_RD.to_be_bytes()); // query: só RD
        out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        out.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
        out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
        out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0); // terminador
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&qclass.to_be_bytes());
        out
    }

    fn decode_response_header(buf: &[u8]) -> (u16, u16, u16, u16) {
        let flags = u16::from_be_bytes([buf[2], buf[3]]);
        let ancount = u16::from_be_bytes([buf[6], buf[7]]);
        let rcode = flags & 0x000F;
        let id = u16::from_be_bytes([buf[0], buf[1]]);
        (id, flags, ancount, rcode)
    }

    #[test]
    fn parse_query_extracts_id_name_and_qtype() {
        let raw = encode_query(0x1234, "example.com", QTYPE_A, QCLASS_IN);
        let parsed = parse_query(&raw).expect("query válida");
        assert_eq!(parsed.id, 0x1234);
        assert_eq!(parsed.name, "example.com");
        assert_eq!(parsed.qtype, QTYPE_A);
    }

    #[test]
    fn parse_query_rejects_qdcount_zero() {
        let mut raw = encode_query(1, "example.com", QTYPE_A, QCLASS_IN);
        raw[4] = 0;
        raw[5] = 0; // QDCOUNT = 0
        assert!(parse_query(&raw).is_none());
    }

    #[test]
    fn parse_query_rejects_qclass_not_in() {
        let raw = encode_query(1, "example.com", QTYPE_A, 3); // QCLASS=CH
        assert!(parse_query(&raw).is_none());
    }

    #[test]
    fn parse_query_rejects_truncated_buffer() {
        let raw = encode_query(1, "example.com", QTYPE_A, QCLASS_IN);
        assert!(parse_query(&raw[..8]).is_none());
    }

    #[test]
    fn parse_query_rejects_compression_pointer_in_question() {
        // Byte 12 com os 2 bits altos setados (0xC0) é um ponteiro
        // de compression — não suportado na question de entrada.
        let mut raw = encode_query(1, "example.com", QTYPE_A, QCLASS_IN);
        raw[12] = 0xC0;
        assert!(parse_query(&raw).is_none());
    }

    #[test]
    fn build_response_nxdomain_has_rcode_3_and_no_answers() {
        let query = encode_query(0xABCD, "attacker.example", QTYPE_A, QCLASS_IN);
        let parsed = parse_query(&query).unwrap();
        let response = build_response(&parsed, None);
        let (id, _flags, ancount, rcode) = decode_response_header(&response);
        assert_eq!(id, 0xABCD);
        assert_eq!(ancount, 0);
        assert_eq!(rcode, RCODE_NXDOMAIN);
    }

    #[test]
    fn build_response_answer_has_rcode_0_and_encodes_ip() {
        let query = encode_query(0x0042, "pypi.org", QTYPE_A, QCLASS_IN);
        let parsed = parse_query(&query).unwrap();
        let ip = Ipv4Addr::new(151, 101, 0, 223);
        let response = build_response(&parsed, Some(ip));
        let (id, _flags, ancount, rcode) = decode_response_header(&response);
        assert_eq!(id, 0x0042);
        assert_eq!(ancount, 1);
        assert_eq!(rcode, RCODE_NOERROR);
        // Últimos 4 bytes da resposta são o RDATA (o IP).
        let rdata = &response[response.len() - 4..];
        assert_eq!(rdata, ip.octets());
    }

    /// Sobe o proxy em porta efêmera, manda uma query real via
    /// `UdpSocket`, e retorna a resposta crua — helper comum aos
    /// testes de integração abaixo.
    async fn send_query_and_recv(allowlist: NetworkAllowlist, name: &str, qtype: u16) -> Vec<u8> {
        let config = DnsProxyConfig { allowlist };
        let handle = start_dns_proxy(
            "127.0.0.1:0".parse().unwrap(),
            config,
            Arc::new(crate::network::NoopNetworkAuditSink),
            None,
        )
        .expect("bind em porta efêmera não deveria falhar");

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let query = encode_query(0x9999, name, qtype, QCLASS_IN);
        client
            .send_to(&query, ("127.0.0.1", handle.port))
            .await
            .unwrap();

        let mut buf = vec![0u8; 512];
        let n = tokio::time::timeout(Duration::from_secs(5), client.recv(&mut buf))
            .await
            .expect("resposta do dns_proxy não deveria dar timeout")
            .unwrap();
        buf.truncate(n);

        shutdown(handle);
        buf
    }

    #[tokio::test]
    async fn dns_proxy_denies_host_not_in_allowlist_with_nxdomain() {
        let response =
            send_query_and_recv(NetworkAllowlist::new(), "attacker.example", QTYPE_A).await;
        let (_id, _flags, ancount, rcode) = decode_response_header(&response);
        assert_eq!(ancount, 0);
        assert_eq!(rcode, RCODE_NXDOMAIN);
    }

    #[tokio::test]
    async fn dns_proxy_denies_qtype_other_than_a_with_nxdomain() {
        const QTYPE_AAAA: u16 = 28;
        let allowlist = NetworkAllowlist::new().with_allowed(["example.com"]);
        let response = send_query_and_recv(allowlist, "example.com", QTYPE_AAAA).await;
        let (_id, _flags, ancount, rcode) = decode_response_header(&response);
        assert_eq!(ancount, 0);
        assert_eq!(rcode, RCODE_NXDOMAIN);
    }

    /// Usa um host real (`example.com`, sempre resolvível — mesmo
    /// padrão de `e2e_network_proxy_wired_into_exec_python.rs` pra
    /// evitar falso-positivo de domínio fake que já resolveria
    /// NXDOMAIN por conta própria, mascarando o teste).
    #[tokio::test]
    async fn dns_proxy_allows_host_in_allowlist_and_resolves_a_record() {
        let allowlist = NetworkAllowlist::new().with_allowed(["example.com"]);
        let response = send_query_and_recv(allowlist, "example.com", QTYPE_A).await;
        let (_id, _flags, ancount, rcode) = decode_response_header(&response);
        assert_eq!(rcode, RCODE_NOERROR);
        assert_eq!(ancount, 1);
        // RDATA são os últimos 4 bytes — um IPv4 válido não-zero.
        let rdata = &response[response.len() - 4..];
        assert_ne!(rdata, [0, 0, 0, 0]);
    }
}
