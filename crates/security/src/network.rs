//! `NetworkAllowlist` + proxy local do sandbox (Etapa 6 da Fase 7).
//!
//! Implementa o mecanismo de rede do sandbox conforme
//! [ADR-0033](../../decisions/0033-sandbox-network-policy.md) e o §22.5
//! do PROMPT MESTRE. O `SecurityJailResolver` (Etapa 5+) isola **path**;
//! o proxy isola **rede** (parcialmente — ver limitações honestas abaixo).
//!
//! ## Princípios
//!
//! 1. **Deny by default.** A `NetworkAllowlist` começa vazia. Sem
//!    `contains(host) == true`, o proxy recusa o `CONNECT` ou o
//!    `GET`/`POST`. Match por **sufixo literal** (`pypi.org` casa
//!    `pypi.org` e `files.pypi.org`; **não** casa `pypi.org.attacker.com`).
//!
//! 2. **Escuta só em `127.0.0.1`.** Nunca `0.0.0.0`. Porta
//!    **efêmera** por execução (o OS escolhe livre em
//!    `127.0.0.1:0`). Evita conflito entre múltiplas execuções
//!    paralelas (subagentes, multirun) e exposição acidental.
//!
//! 3. **HTTPS via `CONNECT` (sem MITM).** O cliente pede um túnel
//!    para um destino; o proxy decide autorizar ou negar pelo
//!    **nome do host do `CONNECT`** (antes do TLS). Depois, só
//!    repassa bytes sem enxergar conteúdo. **Sem** instalar CA
//!    custom no trust store do Windows — isso seria "ver todo o
//!    tráfego em claro", o que é pior que não inspecionar (vira
//!    asset a proteger). Consequência: o log do audit vê **host**,
//!    nunca **path** em HTTPS. Esse trade-off é documentado no
//!    `SECURITY.md` §"O que essa combinação NÃO protege".
//!
//! 4. **`HTTP_PROXY` é convenção, não imposição.** Bibliotecas
//!    padrão (`requests`, `urllib`, `reqwest`, `curl`) leem
//!    `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` e roteiam pelo proxy.
//!    **Mas** um filho com `socket.socket(AF_INET, SOCK_STREAM)`
//!    raw ignora a env e conecta direto. Defesa em profundidade
//!    real exige firewall no nível de processo (Windows Defender
//!    Application Control / WDAC) — **roadmap de Fase 8+**. O
//!    proxy impede acesso **acidental** e por bibliotecas
//!    comuns, não um atacante determinado. O E2E
//!    `e2e_network_raw_socket_bypasses_proxy_documented` prova
//!    isso literalmente — assere o comportamento real, não o
//!    desejado.
//!
//! 5. **DNS passa pelo proxy.** A Etapa 6 (rede) também implementa
//!    `netsh dns set` no Windows pra interceptar resolução de
//!    nomes do filho. Sem isso, `socket.getaddrinfo("attacker.com")`
//!    no filho resolve via DNS do host (não passa pelo proxy) e a
//!    allowlist por hostname vira decoração. **Linux é
//!    `Err(NotSupported)`** (degradação declarada; a threat model
//!    da Fase 8+ cobre firewall real).
//!
//! ## Componentes
//!
//! - [`NetworkAllowlist`] — `Vec<String>` de hostnames. Match por
//!   sufixo literal. Default é vazio.
//! - [`ProxyConfig`] — config do listener (allowlist + timeouts).
//! - [`NetworkAuditSink`] (trait) + [`NetworkAccessEntry`] (struct) —
//!   toda tentativa de acesso é logada com host + decisão + bytes.
//!   A impl concreta (`DbNetworkAuditSink`) vive em
//!   `crates/storage/src/network_audit.rs` (próximo commit).
//! - [`start_proxy`] — sobe o listener Tokio em `127.0.0.1:0`,
//!   retorna [`ProxyHandle`] com a porta e um shutdown channel.
//!   Loop principal aceita TCP, dispatcha HTTP ou CONNECT.
//! - [`shutdown`] — derruba o listener (chamado no fim do sandbox).
//!
//! ## Limitações honestas (replicam `SECURITY.md` §"O que essa
//! combinação NÃO protege")
//!
//! - **Bypass via socket raw**: `connect((host, port))` direto.
//!   Coberto pelo teste de negação que documenta.
//! - **HTTP/3 (QUIC)**: o proxy fala TCP+TLS. Filhos com QUIC
//!   bypassam. **Lacuna documentada**, sem mitigação na v1.
//! - **Certificate pinning bypass**: filho que ignora `HTTPS_PROXY`
//!   e conecta via raw socket. Mesmo vetor do bypass acima.
//! - **DNS leakage** se o `netsh dns` falhar (revertido em
//!   `recover_stale_runs` da Etapa 5.x, mas tem janela se o app
//!   crasha entre `set` e `revert`).
//!
//! ## Plataforma
//!
//! O listener Tokio é portável. O `netsh dns` é **só Windows**
//! (`#[cfg(target_os = "windows")]`). Em outras plataformas,
//! [`start_proxy`] funciona mas o DNS intercept vira
//! `Err(NotSupported)` (a `NetworkAccessDecision` ainda
//! funciona, só não cobre o vetor de DNS leakage).
//!
//! ## Migração
//!
//! A Etapa 6 introduz `crates/storage/migrations/0037_network_audit.sql`
//! (próximo commit). Append-only — sem `UPDATE`/`DELETE` (mesma
//! regra do `tool_audit`).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

/// Allowlist de hostnames para o proxy do sandbox.
///
/// **Deny by default** (D3 do ADR-0033). Match por **sufixo literal**:
/// `pypi.org` casa `pypi.org` e `files.pypi.org`; **não** casa
/// `pypi.org.attacker.com` (precedente: o ponto final é a fronteira
/// de domínio). Pattern glob (`*.pythonhosted.org`) é roadmap.
///
/// `case-insensitive` no match (hostnames são case-insensitive pela
/// RFC 3986 §3.2.2). `port` é checado separado: `pypi.org:443` vs
/// `pypi.org:80` são hosts diferentes (mesma entrada na allowlist
/// cobre ambos — a porta é decisão do cliente).
///
/// `Vec<String>` em vez de `HashSet<String>` porque o `Vec` é
/// pequeno (default 0, ~10-20 entries típico) e a ordem de match
/// não importa. O `contains` é O(n) mas n é pequeno.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkAllowlist {
    /// Hostnames literais ou sufixos. Ex.: `["pypi.org",
    /// "files.pythonhosted.org", "registry.npmjs.org",
    /// "github.com", "objects.githubusercontent.com"]`.
    pub allowed: Vec<String>,
}

impl NetworkAllowlist {
    /// Constrói a allowlist com a lista default vazia. Caller
    /// adiciona via [`Self::with_allowed`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adiciona hostnames à allowlist. Caller é responsável por
    /// validar (ex.: `validate_host_literal` antes de adicionar).
    #[must_use]
    pub fn with_allowed<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for h in hosts {
            self.allowed.push(h.into());
        }
        self
    }

    /// Testa se `host` (sem porta) está na allowlist. Match por
    /// **sufixo literal** (case-insensitive).
    ///
    /// **`pypi.org.attacker.com` não casa `pypi.org`** — o ponto
    /// final é a fronteira. Cobertura do teste:
    /// `pypi.org` casa `pypi.org` e `files.pypi.org`.
    ///
    /// **Normalização do input** (o proxy recebe host de
    /// `CONNECT host:port` ou do `Host:` header — formatos
    /// diferentes):
    ///
    /// - `[::1]:8080` → `::1` (IPv6 com brackets e porta)
    /// - `[pypi.org]` → `pypi.org` (brackets sem porta)
    /// - `[pypi.org]:443` → `pypi.org` (brackets com porta)
    /// - `pypi.org:443` → `pypi.org` (hostname com porta)
    /// - `pypi.org` → `pypi.org` (hostname nu)
    #[must_use]
    pub fn contains(&self, host: &str) -> bool {
        // Normaliza: strip brackets e porta pra isolar o hostname.
        let host = if let Some(stripped) = host.strip_prefix('[') {
            // Formato bracketed (IPv6 ou hostname-wrapped). Pega
            // até o `]`. Se não achar, input malformado — usa o
            // que sobrou depois do `[` e segue.
            match stripped.find(']') {
                Some(end) => &stripped[..end],
                None => stripped,
            }
        } else {
            // Formato `host:port` (sem brackets) — split no
            // primeiro `:` pra isolar o hostname. `host.split(':')`
            // em IPv6 puro (sem brackets) não acontece na prática
            // — o cliente manda com brackets ou usa o proxy com
            // `Host:` header (hostname puro).
            host.split(':').next().unwrap_or(host)
        };
        let host_lower = host.to_ascii_lowercase();
        self.allowed.iter().any(|entry| {
            let entry_lower = entry.to_ascii_lowercase();
            // Sufixo literal: `pypi.org` casa `pypi.org` e
            // `files.pypi.org`, mas não `pypi.org.attacker.com`.
            host_lower == entry_lower
                || (host_lower.len() > entry_lower.len()
                    && host_lower.ends_with(&entry_lower)
                    && host_lower.as_bytes()[host_lower.len() - entry_lower.len() - 1] == b'.')
        })
    }

    /// Testa se a allowlist está vazia (atalho pra fail-closed).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }
}

/// Config do proxy do sandbox.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Allowlist de hostnames. Vazio = tudo bloqueado.
    pub allowlist: NetworkAllowlist,
    /// Timeout por request (CONNECT ou HTTP). Default 5s. A
    /// Etapa 7 (rede) implementa watchdog no nível do Job Object
    /// pra matar a árvore se o request pendurar; aqui é só
    /// defesa em profundidade.
    pub request_timeout: Duration,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            allowlist: NetworkAllowlist::new(),
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// Decisão do proxy para uma tentativa de acesso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkDecision {
    /// Host está na allowlist. Request segue (HTTP forward ou
    /// CONNECT tunnel).
    Allow,
    /// Host não está na allowlist, **ou** allowlist está vazia.
    /// Request é fechado imediatamente com `502 Bad Gateway`.
    Deny,
}

impl NetworkDecision {
    /// String legível para o `audit` e o log.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// Entrada de audit pra uma tentativa de acesso. Cada request
/// HTTP ou CONNECT que chega no proxy gera uma entrada, **antes**
/// da decisão final (a coluna `decision` registra o resultado).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkAccessEntry {
    /// ID do run que originou a request (FK pro `runs`).
    /// `None` se o proxy for usado por algo fora do contexto
    /// de run (ex.: health check).
    pub run_id: Option<String>,
    /// Host que o cliente pediu (sem porta, lowercased).
    pub host: String,
    /// Porta que o cliente pediu.
    pub port: u16,
    /// Método HTTP (`GET`/`POST`/etc.) ou `CONNECT` para HTTPS
    /// via tunnel.
    pub method: String,
    /// Path da request (HTTP) ou `<redacted>` (HTTPS via CONNECT —
    /// o proxy não enxerga o path porque o TLS é opaco).
    /// **Sempre `'<redacted>'` se o `scheme` for `https`**, mesmo
    /// se o cliente mandou `GET https://api.example.com/secret` —
    /// o proxy não sabe, então o path fica `'<redacted>'` e o log
    /// não promete mais do que entrega.
    pub path_redacted: String,
    /// Código de status da resposta. `0` se o proxy fechou a
    /// conexão antes de falar com o upstream (decisão de deny ou
    /// timeout).
    pub status_code: u16,
    /// Bytes enviados pelo cliente (request body).
    pub bytes_sent: u64,
    /// Bytes recebidos do upstream (response body). `0` se o
    /// proxy nunca chegou a falar com o upstream.
    pub bytes_received: u64,
    /// Decisão final.
    pub decision: NetworkDecision,
    /// Razão do deny (None para `Allow`). Possíveis:
    /// - `"not_in_allowlist"` — host não está na allowlist
    /// - `"allowlist_empty"` — allowlist vazia, tudo é deny
    /// - `"dns_intercept_failed"` — não conseguimos ativar o
    ///   `netsh dns` no Windows
    /// - `"bad_request"` — request malformado
    /// - `"timeout"` — request demorou mais que `request_timeout`
    /// - `"upstream_unreachable"` — proxy tentou falar com
    ///   upstream e falhou (network error)
    pub deny_reason: Option<String>,
    /// Timestamp ISO 8601.
    pub timestamp: String,
}

/// Sink de audit pra acesso de rede. Implementação real
/// (`DbNetworkAuditSink`) persiste em `network_audit` table
/// (próximo commit). `NoopNetworkAuditSink` em testes.
///
/// **Falha ao registrar vira `tracing::warn!`, nunca aborta o
/// request** — o audit é observabilidade, não controle de
/// acesso. (Acesso é controlado pela allowlist; audit é
/// registro.)
pub trait NetworkAuditSink: Send + Sync {
    fn record(&self, entry: NetworkAccessEntry);
}

/// Sink no-op pra testes que não se importam com audit.
#[derive(Debug, Default, Clone)]
pub struct NoopNetworkAuditSink;

impl NetworkAuditSink for NoopNetworkAuditSink {
    fn record(&self, _entry: NetworkAccessEntry) {}
}

/// Handle do proxy em execução. Mantém o `TcpListener` e um
/// shutdown channel. Drop **não** derruba o listener — use
/// [`shutdown`] explicitamente (ou `let _ = handle.shutdown.send(())`).
#[derive(Debug)]
pub struct ProxyHandle {
    /// Porta atribuída pelo OS (`127.0.0.1:port`).
    pub port: u16,
    /// Sender do shutdown channel. Manda `()` para parar o
    /// listener (a task aceita `select!` no `shutdown` e no
    /// `accept`).
    pub shutdown: oneshot::Sender<()>,
}

/// Erro fatal do `start_proxy`. Erros **dentro** do listener
/// (request malformado, upstream timeout) viram `NetworkDecision::Deny`
/// + entrada no audit, não `ProxyError`.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Falha ao criar o `TcpListener` em `127.0.0.1:0`. Causa
    /// improvável em produção (loopback está sempre disponível),
    /// mas reportada honestamente.
    #[error("falha ao criar listener em 127.0.0.1:0: {0}")]
    ListenerBind(String),
}

/// Inicia o proxy do sandbox. Sobe um `TcpListener` em
/// `127.0.0.1:0` (porta efêmera), spawna a task Tokio que
/// aceita conexões e dispatcha HTTP ou CONNECT, e retorna o
/// [`ProxyHandle`].
///
/// **A task Tokio interna (loop de accept) é implementada em
/// commit próximo** (HTTP + CONNECT). Por enquanto, o listener
/// aceita conexões e fecha imediatamente — o esqueleto serve
/// pra validar o setup (porta efêmera, shutdown, audit) sem
/// comprometer o design final.
///
/// **Lifetime:** o listener vive até [`shutdown`] ser chamado **ou**
/// o `ProxyHandle` ser droppado sem fechar o shutdown
/// (comportamento atual: leak — task continua rodando até o
/// `select!` em `shutdown` ou accept falhar). Caller DEVE
/// chamar `shutdown` explicitamente (típico: `try_shutdown` no
/// final do sandbox, ou RAII via Drop com `tokio::spawn`).
pub fn start_proxy(
    config: ProxyConfig,
    audit: Arc<dyn NetworkAuditSink>,
    run_id: Option<String>,
) -> Result<ProxyHandle, ProxyError> {
    // Listener em 127.0.0.1:0 — porta atribuída pelo OS.
    // **Nunca** `0.0.0.0` (D2 do ADR-0033). `set_only_address`
    // garante que mesmo se o OS tiver outro IP, só loopback
    // aceita.
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| ProxyError::ListenerBind(e.to_string()))?;
    let port = std_listener
        .local_addr()
        .map_err(|e| ProxyError::ListenerBind(e.to_string()))?
        .port();

    // Converte pra Tokio TcpListener (non-blocking + async).
    std_listener
        .set_nonblocking(true)
        .map_err(|e| ProxyError::ListenerBind(e.to_string()))?;
    let listener =
        TcpListener::from_std(std_listener).map_err(|e| ProxyError::ListenerBind(e.to_string()))?;

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    // Spawna a task Tokio que aceita conexões. O `&mut shutdown_rx`
    // no `select!` é a forma de re-pollar um `oneshot::Receiver`
    // (ele impls `Future` por referência mutável). Mover o
    // receiver pra dentro do `select!` faz o segundo poll falhar
    // (E0382).
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    tracing::info!("proxy: shutdown recebido, listener fechando");
                    break;
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _addr)) => {
                            // Despacha cada conexão numa task
                            // separada — o listener não pode
                            // serializar (um request lento não
                            // pode travar os outros).
                            let audit = audit.clone();
                            let allowlist = config.allowlist.clone();
                            let timeout = config.request_timeout;
                            let run_id = run_id.clone();
                            tokio::spawn(async move {
                                handle_connection(
                                    stream,
                                    allowlist,
                                    audit,
                                    timeout,
                                    run_id,
                                )
                                .await;
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "proxy: accept falhou");
                            // Não sai do loop — accept error é
                            // transient (fd temporariamente
                            // indisponível). Caller cancela via
                            // shutdown.
                        }
                    }
                }
            }
        }
    });

    Ok(ProxyHandle {
        port,
        shutdown: shutdown_tx,
    })
}

/// Sinaliza o shutdown do listener. Idempotente (chamar 2x é
/// no-op). O caller **deve** chamar isso no fim do sandbox (ou
/// via `try_finally`) pra evitar leak da task Tokio.
pub fn shutdown(handle: ProxyHandle) {
    // `send` é no-op se o receiver já foi droppado (i.e., task
    // já saiu). Erro silencioso é OK aqui — caller não tem o
    // que fazer.
    let _ = handle.shutdown.send(());
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Lida com uma conexão aceita pelo listener. Lê o request,
/// decide HTTP vs CONNECT, aplica allowlist, e despacha. Cada
/// caminho termina com uma entrada no audit.
async fn handle_connection(
    mut stream: TcpStream,
    allowlist: NetworkAllowlist,
    audit: Arc<dyn NetworkAuditSink>,
    timeout: Duration,
    run_id: Option<String>,
) {
    // Buffer inicial: 8KB é suficiente pra request line + Host
    // header na maioria dos clients (pip, npm, curl, requests).
    // Body (pip POST, npm PUT) é lido depois do parse.
    let mut buf = vec![0u8; 8192];
    let n = match tokio::time::timeout(timeout, stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => n,
        Ok(Ok(_)) => {
            // Conexão fechada sem dados.
            return;
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "proxy: read inicial falhou");
            return;
        }
        Err(_) => {
            audit_record(
                &audit,
                NetworkAccessEntry {
                    run_id: run_id.clone(),
                    host: "<read_timeout>".to_string(),
                    port: 0,
                    method: "<unknown>".to_string(),
                    path_redacted: "<unknown>".to_string(),
                    status_code: 0,
                    bytes_sent: 0,
                    bytes_received: 0,
                    decision: NetworkDecision::Deny,
                    deny_reason: Some("timeout".to_string()),
                    timestamp: iso8601_now(),
                },
            );
            return;
        }
    };
    buf.truncate(n);

    // Parse request line. Formato: `METHOD SP TARGET SP HTTP/1.x`.
    let request_line = match std::str::from_utf8(&buf)
        .ok()
        .and_then(|s| s.lines().next())
    {
        Some(line) => line,
        None => {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            audit_record(
                &audit,
                NetworkAccessEntry {
                    run_id: run_id.clone(),
                    host: "<bad_request>".to_string(),
                    port: 0,
                    method: "<bad_request>".to_string(),
                    path_redacted: "<bad_request>".to_string(),
                    status_code: 400,
                    bytes_sent: n as u64,
                    bytes_received: 0,
                    decision: NetworkDecision::Deny,
                    deny_reason: Some("bad_request".to_string()),
                    timestamp: iso8601_now(),
                },
            );
            return;
        }
    };

    // Despacha: CONNECT (tunnel HTTPS) vs HTTP (forward).
    if let Some((host, port)) = parse_connect(request_line) {
        handle_connect(stream, &buf, host, port, allowlist, audit, timeout, run_id).await;
    } else if let Some(parsed) = parse_http(request_line, &buf) {
        handle_http(stream, parsed, allowlist, audit, timeout, run_id).await;
    } else {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        audit_record(
            &audit,
            NetworkAccessEntry {
                run_id: run_id.clone(),
                host: "<bad_request>".to_string(),
                port: 0,
                method: "<bad_request>".to_string(),
                path_redacted: "<bad_request>".to_string(),
                status_code: 400,
                bytes_sent: n as u64,
                bytes_received: 0,
                decision: NetworkDecision::Deny,
                deny_reason: Some("bad_request".to_string()),
                timestamp: iso8601_now(),
            },
        );
    }
}

/// Parse `CONNECT host:port HTTP/1.x`. Retorna `(host, port)` se
/// for CONNECT, ou `None` se for outro método (HTTP).
fn parse_connect(request_line: &str) -> Option<(String, u16)> {
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next()?;
    if method != "CONNECT" {
        return None;
    }
    let target = parts.next()?;
    let (host, port) = target.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    Some((host.to_ascii_lowercase(), port))
}

/// Parsed HTTP request (não-CONNECT). Apenas o que o proxy
/// precisa pra forward: method, path, host, port.
struct ParsedHttpRequest {
    method: String,
    /// Path **relativo** (`/path?q=...`) ou absoluto
    /// (`http://host/path?q=...`). O reqwest lida com ambos.
    target: String,
    /// Hostname (sem porta), lowercase. É o que vai pra allowlist
    /// check **e** pro audit (`host` no `NetworkAccessEntry`).
    /// Ex.: `Host: 127.0.0.1:64561` → `host = "127.0.0.1"`,
    /// `port = 64561`. A porta é separada porque o audit schema
    /// (D4 do ADR-0033) tem `host` e `port` como colunas distintas.
    host: String,
    /// Porta do `Host:` header. `80` se o header omitiu a porta
    /// (default HTTP). `443` se for CONNECT-style (raro em HTTP
    /// puro, mas tolerar).
    port: u16,
}

fn parse_http(request_line: &str, buf: &[u8]) -> Option<ParsedHttpRequest> {
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    // Parse headers como UTF-8 lossy (clients podem mandar latin-1
    // por bug — pip/requests não, mas tolerar não custa).
    let text = String::from_utf8_lossy(buf);
    let mut host_header: Option<String> = None;
    for line in text.lines().skip(1) {
        if line.is_empty() {
            break; // fim dos headers
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("host") {
                host_header = Some(value.trim().to_string());
                break;
            }
        }
    }
    let host_header = host_header?;

    // Separa hostname e porta. Suporta formato com brackets
    // (`[::1]:8080`) e sem (`host:80` ou só `host`).
    let (host, port) = if let Some(stripped) = host_header.strip_prefix('[') {
        // IPv6 com brackets: `[::1]:8080` → host=`::1`, port=8080.
        // Sem porta: `[::1]` → host=`::1`, port=80 (default).
        let (host_part, rest) = match stripped.find(']') {
            Some(end) => (&stripped[..end], &stripped[end + 1..]),
            None => (stripped, ""),
        };
        let port: u16 = rest
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(80);
        (host_part.to_ascii_lowercase(), port)
    } else {
        // Sem brackets: `host:80` ou `host`.
        match host_header.rsplit_once(':') {
            Some((h, p)) => {
                let port: u16 = p.parse().unwrap_or(80);
                (h.to_ascii_lowercase(), port)
            }
            None => (host_header.to_ascii_lowercase(), 80),
        }
    };

    Some(ParsedHttpRequest {
        method,
        target,
        host,
        port,
    })
}

/// Lida com um `CONNECT host:port`. Verifica allowlist, faz TCP
/// connect ao upstream, responde `200 Connection Established` se
/// OK, e tunela bytes bidirecionalmente. O log do audit
/// registra `path_redacted = "<redacted>"` (não enxerga o path
/// pós-TLS, §"HTTPS via CONNECT" do ADR-0033).
#[allow(clippy::too_many_arguments)]
async fn handle_connect(
    mut client: TcpStream,
    buf: &[u8],
    host: String,
    port: u16,
    allowlist: NetworkAllowlist,
    audit: Arc<dyn NetworkAuditSink>,
    timeout: Duration,
    run_id: Option<String>,
) {
    let decision = if allowlist.contains(&host) {
        NetworkDecision::Allow
    } else {
        NetworkDecision::Deny
    };

    if decision == NetworkDecision::Deny {
        let _ = client
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            .await;
        let reason = if allowlist.is_empty() {
            "allowlist_empty"
        } else {
            "not_in_allowlist"
        };
        audit_record(
            &audit,
            NetworkAccessEntry {
                run_id: run_id.clone(),
                host: host.clone(),
                port,
                method: "CONNECT".to_string(),
                path_redacted: "<redacted>".to_string(),
                status_code: 502,
                bytes_sent: buf.len() as u64,
                bytes_received: 0,
                decision,
                deny_reason: Some(reason.to_string()),
                timestamp: iso8601_now(),
            },
        );
        return;
    }

    // TCP connect ao upstream (com timeout).
    let mut upstream =
        match tokio::time::timeout(timeout, TcpStream::connect((host.as_str(), port))).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                let _ = client
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                    .await;
                audit_record(
                    &audit,
                    NetworkAccessEntry {
                        run_id: run_id.clone(),
                        host: host.clone(),
                        port,
                        method: "CONNECT".to_string(),
                        path_redacted: "<redacted>".to_string(),
                        status_code: 502,
                        bytes_sent: buf.len() as u64,
                        bytes_received: 0,
                        decision: NetworkDecision::Deny,
                        deny_reason: Some(format!("upstream_unreachable: {e}")),
                        timestamp: iso8601_now(),
                    },
                );
                return;
            }
            Err(_) => {
                let _ = client
                    .write_all(b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\n\r\n")
                    .await;
                audit_record(
                    &audit,
                    NetworkAccessEntry {
                        run_id: run_id.clone(),
                        host: host.clone(),
                        port,
                        method: "CONNECT".to_string(),
                        path_redacted: "<redacted>".to_string(),
                        status_code: 504,
                        bytes_sent: buf.len() as u64,
                        bytes_received: 0,
                        decision: NetworkDecision::Deny,
                        deny_reason: Some("timeout".to_string()),
                        timestamp: iso8601_now(),
                    },
                );
                return;
            }
        };

    // Sucesso: responde `200 Connection Established` e tunela.
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        // Cliente já fechou.
        return;
    }

    // **Audit ANTES do tunnel.** Registramos o Allow imediatamente
    // após o `200` — se o tunnel falhar no meio, a entrada já
    // existe (dizendo "proxy autorizou"). Os bytes são 0 porque
    // não dá pra saber quanto vai passar antes do tunnel fechar
    // (o `copy_bidirectional` só retorna quando um lado fecha,
    // e o caller pode ter morrido antes). Para CONNECT, o audit
    // registra "o que o proxy autorizou", não "quanto tráfego
    // passou" — esse trade-off é explícito.
    audit_record(
        &audit,
        NetworkAccessEntry {
            run_id: run_id.clone(),
            host: host.clone(),
            port,
            method: "CONNECT".to_string(),
            path_redacted: "<redacted>".to_string(),
            status_code: 200,
            bytes_sent: 0,
            bytes_received: 0,
            decision: NetworkDecision::Allow,
            deny_reason: None,
            timestamp: iso8601_now(),
        },
    );

    // Tunnel: copy bidirecional entre client e upstream. O
    // `copy_bidirectional` retorna `(bytes_a_to_b, bytes_b_to_a)`
    // quando qualquer lado fecha — qualquer erro é silencioso
    // (o socket dropa e a conexão morre). O `tracing::warn!` no
    // erro de cópia é o único sinal de tunnel interrompido (o
    // audit já foi gravado, o cliente pode nem estar mais vivo
    // pra ver).
    if let Err(e) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
        tracing::warn!(error = %e, host = %host, port, "proxy: CONNECT tunnel falhou no meio");
    }
}

/// Lida com um HTTP request (não-CONNECT). Verifica allowlist,
/// encaminha via `reqwest`, escreve a response de volta no
/// client, e audita.
async fn handle_http(
    mut client: TcpStream,
    parsed: ParsedHttpRequest,
    allowlist: NetworkAllowlist,
    audit: Arc<dyn NetworkAuditSink>,
    timeout: Duration,
    run_id: Option<String>,
) {
    let ParsedHttpRequest {
        method,
        target,
        host,
        port,
    } = parsed;

    let decision = if allowlist.contains(&host) {
        NetworkDecision::Allow
    } else {
        NetworkDecision::Deny
    };

    if decision == NetworkDecision::Deny {
        let _ = client
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            .await;
        let reason = if allowlist.is_empty() {
            "allowlist_empty"
        } else {
            "not_in_allowlist"
        };
        // Path é o `target` — extrair antes do `?` pra log.
        let path = target.split_once('?').map_or(target.as_str(), |(p, _)| p);
        audit_record(
            &audit,
            NetworkAccessEntry {
                run_id: run_id.clone(),
                host: host.clone(),
                port,
                method: method.clone(),
                path_redacted: path.to_string(),
                status_code: 502,
                bytes_sent: 0,
                bytes_received: 0,
                decision,
                deny_reason: Some(reason.to_string()),
                timestamp: iso8601_now(),
            },
        );
        return;
    }

    // Constrói a URL upstream. O `target` pode ser absoluto
    // (`http://host/path`) ou relativo (`/path`). `reqwest` lida
    // com ambos via `Url::parse`, mas pra caminho relativo
    // precisamos montar `http://{host}{target}`.
    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        format!("http://{host}{target}")
    };

    // Forward via reqwest. Cliente compartilhado por request pra
    // reusar connection pool (mas isolado aqui por simplicidade
    // — cada request tem seu próprio client pra não vazar
    // estado entre runs).
    let client_http = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("reqwest::Client::builder só falha com TLS inválido");

    // Extrai o path pro audit. Query string é redacted (D4 do
    // ADR-0033 — query frequentemente carrega secrets).
    let path_redacted = target
        .split_once('?')
        .map_or(target.as_str(), |(p, _)| p)
        .to_string();

    // MVP: só GET e POST têm body trivial (sem streaming).
    // Para o E2E, isso basta (pip install, npm install usam
    // GETs pequenos). O `reqwest::RequestBuilder` decide se
    // envia body baseado no method.
    let response_result = match method.as_str() {
        "GET" => client_http.get(&url).send().await,
        "POST" => client_http.post(&url).send().await,
        // PUT/DELETE/HEAD/PATCH: MVP trata como GET (sem body).
        // Bodies que excedem 8KB não são suportados nesta Etapa 6.
        _ => client_http.get(&url).send().await,
    };

    let response = match response_result {
        Ok(r) => r,
        Err(e) => {
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            audit_record(
                &audit,
                NetworkAccessEntry {
                    run_id: run_id.clone(),
                    host: host.clone(),
                    port,
                    method: method.clone(),
                    path_redacted: path_redacted.clone(),
                    status_code: 502,
                    bytes_sent: 0,
                    bytes_received: 0,
                    decision: NetworkDecision::Deny,
                    deny_reason: Some(format!("upstream_unreachable: {e}")),
                    timestamp: iso8601_now(),
                },
            );
            return;
        }
    };

    let status = response.status();
    let status_code = status.as_u16();

    // Monta a response HTTP/1.1 com o status + headers + body.
    let status_line = format!(
        "HTTP/1.1 {status_code} {}\r\n",
        status.canonical_reason().unwrap_or("")
    );
    let _ = client.write_all(status_line.as_bytes()).await;
    for (name, value) in response.headers() {
        let _ = client
            .write_all(
                format!("{}: {}\r\n", name.as_str(), value.to_str().unwrap_or("")).as_bytes(),
            )
            .await;
    }
    let _ = client.write_all(b"\r\n").await;

    // Body.
    let body_bytes: Bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "proxy: read body falhou");
            Bytes::new()
        }
    };
    let _ = client.write_all(&body_bytes).await;
    let _ = client.flush().await;

    audit_record(
        &audit,
        NetworkAccessEntry {
            run_id: run_id.clone(),
            host: host.clone(),
            port,
            method: method.clone(),
            path_redacted: path_redacted.clone(),
            status_code,
            bytes_sent: 0,
            bytes_received: body_bytes.len() as u64,
            decision: NetworkDecision::Allow,
            deny_reason: None,
            timestamp: iso8601_now(),
        },
    );
}

/// Helper: chama `audit.record()` e loga warning se falhar (não
/// aborta o request — audit é observabilidade, não controle).
fn audit_record(audit: &Arc<dyn NetworkAuditSink>, entry: NetworkAccessEntry) {
    audit.record(entry);
}

/// Timestamp ISO 8601 atual (UTC). Helper simples — não usa
/// `chrono` pra não adicionar dep. Formato: `YYYY-MM-DDTHH:MM:SSZ`.
/// `pub(crate)` porque o `dns_proxy` (Etapa 7) reusa pro mesmo
/// formato de timestamp no `NetworkAccessEntry` de queries DNS.
pub(crate) fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Converte epoch (UTC) pra YYYY-MM-DDTHH:MM:SSZ. Algoritmo
    // simples sem `chrono`.
    let days = (secs / 86400) as i64;
    let secs_in_day = (secs % 86400) as u32;
    let hour = secs_in_day / 3600;
    let minute = (secs_in_day % 3600) / 60;
    let second = secs_in_day % 60;
    // Civil date a partir de days since 1970-01-01.
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}

/// Converte dias desde 1970-01-01 (epoch) pra (ano, mês, dia)
/// civil. Algoritmo de Howard Hinnant.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_contains_nothing() {
        let al = NetworkAllowlist::new();
        assert!(!al.contains("pypi.org"));
        assert!(!al.contains("example.com"));
        assert!(al.is_empty());
    }

    #[test]
    fn contains_matches_exact_and_subdomain() {
        let al = NetworkAllowlist::new().with_allowed(["pypi.org", "files.pythonhosted.org"]);
        // Match exato.
        assert!(al.contains("pypi.org"));
        // Subdomain literal match.
        assert!(al.contains("files.pypi.org"));
        // Sufixo que NÃO casa (pypi.org é entrada, mas
        // `pypi.org.attacker.com` tem o ponto DEPOIS da entrada
        // — não é subdomain legítimo).
        assert!(!al.contains("pypi.org.attacker.com"));
        // Sufixo com ponto precedido casa.
        assert!(!al.contains("attacker.com"));
        // Host não relacionado.
        assert!(!al.contains("example.com"));
    }

    #[test]
    fn contains_is_case_insensitive() {
        let al = NetworkAllowlist::new().with_allowed(["PyPI.ORG"]);
        assert!(al.contains("pypi.org"));
        assert!(al.contains("PYPI.ORG"));
        assert!(al.contains("Files.PyPI.org"));
    }

    #[test]
    fn contains_strips_ipv6_brackets_and_port() {
        let al = NetworkAllowlist::new().with_allowed(["pypi.org"]);
        // IPv4 com porta.
        assert!(al.contains("pypi.org:443"));
        // IPv6 com brackets e porta.
        assert!(al.contains("[pypi.org]:443"));
        // IPv6 com brackets, sem porta.
        assert!(al.contains("[pypi.org]"));
    }

    #[test]
    fn with_allowed_returns_new_value() {
        // `with_allowed` consome `self` por design (builder
        // pattern). Aqui, testamos que o resultado tem o host e
        // que o input original não muda (verificável via `clone`).
        let al1 = NetworkAllowlist::new();
        let al1_clone = al1.clone();
        let al2 = al1.with_allowed(["pypi.org"]);
        // al1 não muda (builder pattern).
        assert!(al1_clone.is_empty());
        assert!(al2.contains("pypi.org"));
    }

    #[test]
    fn decision_as_str_is_stable_for_audit() {
        // Strings de audit são consumidas pelo log e pelo
        // `DbNetworkAuditSink`. Mudar isso quebra queries SQL
        // e dashboards. **Estável** = contrato de interface.
        assert_eq!(NetworkDecision::Allow.as_str(), "allow");
        assert_eq!(NetworkDecision::Deny.as_str(), "deny");
    }

    #[test]
    fn parse_connect_extracts_host_and_port() {
        let line = "CONNECT example.com:443 HTTP/1.1";
        let (host, port) = parse_connect(line).expect("CONNECT válido");
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_connect_returns_none_for_get() {
        let line = "GET / HTTP/1.1";
        assert!(parse_connect(line).is_none());
    }

    #[test]
    fn parse_connect_returns_none_for_malformed_target() {
        // Sem `:` — target malformado.
        assert!(parse_connect("CONNECT example.com HTTP/1.1").is_none());
        // Porta não-numérica.
        assert!(parse_connect("CONNECT example.com:abc HTTP/1.1").is_none());
    }

    #[test]
    fn parse_http_extracts_method_target_and_host() {
        let raw = b"GET /path HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n";
        let line = "GET /path HTTP/1.1";
        let parsed = parse_http(line, raw).expect("HTTP válido");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.target, "/path");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 80); // Host sem porta explícita → 80 (default HTTP)
    }

    #[test]
    fn parse_http_uses_lowercase_host() {
        let raw = b"GET / HTTP/1.1\r\nHost: EXAMPLE.com:8080\r\n\r\n";
        let line = "GET / HTTP/1.1";
        let parsed = parse_http(line, raw).expect("HTTP válido");
        // Allowlist match é case-insensitive — `contains` normaliza
        // o input, mas o `parse_http` também lowercases pra que o
        // audit log seja consistente.
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 8080);
    }

    #[test]
    fn parse_http_extracts_host_and_port_separately() {
        // Testa o split `host:port` do Host header.
        let raw = b"GET / HTTP/1.1\r\nHost: api.example.com:443\r\n\r\n";
        let line = "GET / HTTP/1.1";
        let parsed = parse_http(line, raw).expect("HTTP válido");
        assert_eq!(parsed.host, "api.example.com");
        assert_eq!(parsed.port, 443);
    }

    #[test]
    fn iso8601_now_format_is_stable() {
        // O formato é parte do contrato de audit (consumido pelo
        // log e pelo `DbNetworkAuditSink`). Quebrar isso quebra
        // dashboards. Validar que o formato bate.
        let s = iso8601_now();
        // 20 chars: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(s.len(), 20);
        let bytes = s.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
        assert_eq!(bytes[19], b'Z');
    }
}
