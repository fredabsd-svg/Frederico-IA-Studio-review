//! Protocolo IPC do `process-architecture`.
//!
//! O envelope [`IpcMessage`] é o **contrato** entre o app principal
//! e qualquer worker sidecar. É serializado como JSON line-delimited
//! (uma `IpcMessage` por linha, terminada em `\n`) sobre named
//! pipes no Windows. O `protocol_version` no envelope é global; a
//! `op` carrega o schema do `payload` (referenciado por ID no
//! `shared-contracts`).
//!
//! **Convenções:**
//!
//! 1. Toda request tem um `request_id` (UUID v4). A response
//!    obrigatória carrega o mesmo `request_id` — o caller casa
//!    pelo ID (múltiplas requests podem estar em voo).
//! 2. `auth` carrega um token de curta duração (≤ 15 min,
//!    `PROMPT MESTRE` §22.5). Worker que reapresenta token revogado
//!    é morto. v0.1: o token é um `String` opaco — a revogação
//!    entra na Etapa 2B quando o `document-worker` consumir.
//! 3. `payload` é `serde_json::Value` arbitrário — o `op` carrega
//!    o schema. O JSON Schema do `IpcMessage` é gerado em runtime
//!    via `schemars` (mesma estratégia do `document-engine`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Versão do envelope IPC. Bump **MAJOR** em mudanças
/// incompatíveis (campo obrigatório novo, opcode renomeado).
/// Bump **MINOR** em adições compatíveis (campo opcional novo).
const PROTOCOL_VERSION: u32 = 1;

/// `request_id` — UUID v4 por mensagem. O caller gera, o worker
/// ecoa na response.
pub type RequestId = Uuid;

/// `worker_id` — string opaca (ex.: `"document-worker"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerId(pub String);

impl WorkerId {
    /// Cria um `WorkerId` a partir de qualquer `Into<String>`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// A string por baixo.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Token de autenticação de curta duração (`PROMPT MESTRE` §22.5).
/// v0.1: `String` opaco. Worker que reapresenta token revogado é
/// morto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerAuth(pub String);

impl WorkerAuth {
    /// Cria um token novo.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// A string por baixo (útil pra `Debug`/`Display` em logs —
    /// **nunca** pra devolver ao worker depois de revogado).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Saúde observada do worker. Spec §7.1 (mesma enum do
/// `tool-registry`, intencionalmente compatível — o
/// `ToolManifest::health` consome este tipo).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealth {
    /// Worker respondendo a healthcheck, sem erros recentes.
    Ok,
    /// Worker está respondendo mas reportou degradação (ex.:
    /// Tesseract indisponível, mas o resto funciona).
    Degraded,
    /// Worker não está respondendo ou reportou erro fatal.
    #[default]
    Unhealthy,
}

/// Dependência externa do worker (ex.: `python-docx 1.1.0`,
/// `Tesseract 5.3.0`, `tesseract-ocr-por 4.1.0`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// Nome legível (ex.: `"python-docx"`, `"Tesseract"`).
    pub name: String,
    /// Versão reportada pelo worker (string livre — semver
    /// preferido).
    pub version: String,
    /// Fonte da versão (ex.: `"pip show"`, `"tesseract --version"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Informação de compatibilidade declarada pelo worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityInfo {
    /// OS mínimo (ex.: `"Windows 10 1903"`).
    pub min_os: String,
    /// Arquitetura (ex.: `"x86_64"`).
    pub arch: String,
    /// Runtime mínimo do worker (ex.: `"Python 3.11+"`).
    pub min_runtime: String,
}

/// Manifesto do worker — `worker.hello` carrega isto. Spec §7.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerManifest {
    /// ID único do worker (ex.: `"document-worker"`).
    pub worker_id: WorkerId,
    /// Versão semântica do **worker** (não do protocolo — esse é
    /// `IpcMessage::protocol_version`).
    pub version: String,
    /// Capacidades oferecidas (ex.: `"docx.write"`, `"ocr.run"`).
    /// Strings livres — usadas pelo `ToolRegistry` pra filtrar
    /// ferramentas (`process-architecture.md` §"Descoberta na
    /// inicialização").
    pub capabilities: Vec<String>,
    /// Dependências externas.
    pub dependencies: Vec<Dependency>,
    /// Saúde observada (default: `Unhealthy` — só vira `Ok` depois
    /// do primeiro healthcheck positivo).
    pub health: WorkerHealth,
    /// Compatibilidade.
    pub compatibility: CompatibilityInfo,
}

/// Opcode — `op` do `IpcMessage`. Lista **fechada** — qualquer
/// opcode desconhecido é erro de protocolo (futuro: dinâmico via
/// `ToolRegistry`; v0.1 é hardcoded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcOp {
    /// Worker → app: anuncia manifesto + pede auth token.
    Hello,
    /// App → worker: ack do `hello` + token de curta duração.
    Ack,
    /// App → worker: ping (healthcheck ativo).
    Ping,
    /// Worker → app: pong (resposta ao ping).
    Pong,
    /// App → worker: encerra com calma (termina o loop, fecha
    /// pipes, mata processo).
    Shutdown,
    /// Worker → app: reporta erro fatal (encerra a si mesmo).
    Error,
    /// App → worker: executa tool / op arbitrária.
    ToolInvoke,
    /// Worker → app: response de tool / op.
    ToolResult,
}

impl IpcOp {
    /// Nome em `snake_case` (estável, vai pro JSON).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hello => "worker.hello",
            Self::Ack => "app.ack",
            Self::Ping => "app.ping",
            Self::Pong => "worker.pong",
            Self::Shutdown => "app.shutdown",
            Self::Error => "worker.error",
            Self::ToolInvoke => "tool.invoke",
            Self::ToolResult => "tool.result",
        }
    }
}

/// Envelope IPC genérico. Spec §"Mensagem IPC".
///
/// O `payload` é `serde_json::Value` arbitrário — o `op` carrega o
/// schema (referenciado por ID no `shared-contracts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcMessage {
    /// Versão do **envelope** (vs. `WorkerManifest::version` que é
    /// versão do worker). Bump MAJOR em mudanças incompatíveis.
    pub protocol_version: u32,
    /// Request ID. Toda response obrigatória carrega o mesmo ID —
    /// o caller casa pelo ID.
    pub request_id: RequestId,
    /// Opcode.
    pub op: IpcOp,
    /// Payload arbitrário (validado pelo schema específico do `op`).
    pub payload: serde_json::Value,
    /// Auth token (vazio nas mensagens iniciais `hello`/`ack`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WorkerAuth>,
}

impl IpcMessage {
    /// Versão atual do protocolo.
    #[must_use]
    pub const fn current_protocol_version() -> u32 {
        PROTOCOL_VERSION
    }

    /// Cria uma mensagem `worker.hello` carregando o manifesto.
    #[must_use]
    pub fn hello(manifest: WorkerManifest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4(),
            op: IpcOp::Hello,
            payload: serde_json::to_value(&manifest).expect("WorkerManifest sempre serializa"),
            auth: None,
        }
    }

    /// Cria um `app.ack` com token de auth.
    #[must_use]
    pub fn ack(request_id: RequestId, auth: WorkerAuth) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            op: IpcOp::Ack,
            payload: serde_json::json!({"status": "ok"}),
            auth: Some(auth),
        }
    }

    /// Cria um `app.ping`.
    #[must_use]
    pub fn ping(request_id: RequestId, auth: Option<WorkerAuth>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            op: IpcOp::Ping,
            payload: serde_json::json!({}),
            auth,
        }
    }

    /// Cria um `app.shutdown` (graceful).
    #[must_use]
    pub fn shutdown(request_id: RequestId, auth: Option<WorkerAuth>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            op: IpcOp::Shutdown,
            payload: serde_json::json!({"reason": "app_shutdown"}),
            auth,
        }
    }

    /// Cria um `tool.invoke` com payload arbitrário.
    #[must_use]
    pub fn tool_invoke(
        request_id: RequestId,
        auth: WorkerAuth,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            op: IpcOp::ToolInvoke,
            payload,
            auth: Some(auth),
        }
    }

    /// Serializa como **uma linha** (line-delimited JSON).
    /// O `\n` no final é o separador de mensagens.
    ///
    /// # Erros
    /// Retorna `ProcessError::Protocol` se a serialização falhar
    /// (improvável com `serde_json::to_vec`).
    pub fn encode_line(&self) -> Result<Vec<u8>, crate::error::ProcessError> {
        let mut buf =
            serde_json::to_vec(self).map_err(|e| crate::error::ProcessError::Protocol {
                message: format!("IpcMessage não serializa: {e}"),
            })?;
        buf.push(b'\n');
        Ok(buf)
    }

    /// Desserializa uma **única** linha. Devolve a mensagem e o
    /// número de bytes consumidos (útil para callers com buffer
    /// parcial).
    ///
    /// # Erros
    /// - `Protocol` se a linha não tem `\n` terminal, se o JSON é
    ///   inválido, ou se o `protocol_version` não bate.
    pub fn decode_line(line: &[u8]) -> Result<(Self, usize), crate::error::ProcessError> {
        let line =
            line.strip_suffix(b"\n")
                .ok_or_else(|| crate::error::ProcessError::Protocol {
                    message: "linha sem \\n terminal".to_string(),
                })?;
        let msg: Self =
            serde_json::from_slice(line).map_err(|e| crate::error::ProcessError::Protocol {
                message: format!("JSON inválido: {e}"),
            })?;
        if msg.protocol_version != PROTOCOL_VERSION {
            return Err(crate::error::ProcessError::Protocol {
                message: format!(
                    "protocol_version {} não é a atual {}",
                    msg.protocol_version, PROTOCOL_VERSION
                ),
            });
        }
        Ok((msg, line.len() + 1))
    }
}

/// Snapshot de saúde observado pelo `WorkerManager`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHealthSnapshot {
    /// Saúde observada no último healthcheck.
    pub health: WorkerHealth,
    /// Quando o último healthcheck foi feito.
    pub last_check_at: DateTime<Utc>,
    /// Mensagem opcional (ex.: motivo da degradação).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let manifest = WorkerManifest {
            worker_id: WorkerId::new("document-worker"),
            version: "0.1.0".to_string(),
            capabilities: vec!["docx.write".to_string(), "ocr.run".to_string()],
            dependencies: vec![Dependency {
                name: "python-docx".to_string(),
                version: "1.1.0".to_string(),
                source: Some("pip show".to_string()),
            }],
            health: WorkerHealth::Ok,
            compatibility: CompatibilityInfo {
                min_os: "Windows 10 1903".to_string(),
                arch: "x86_64".to_string(),
                min_runtime: "Python 3.11+".to_string(),
            },
        };
        let msg = IpcMessage::hello(manifest.clone());
        let line = msg.encode_line().expect("encode");
        let (decoded, consumed) = IpcMessage::decode_line(&line).expect("decode");
        assert_eq!(consumed, line.len());
        assert_eq!(decoded.protocol_version, PROTOCOL_VERSION);
        assert_eq!(decoded.op, IpcOp::Hello);
        // Re-decodifica o payload como WorkerManifest e compara.
        let decoded_manifest: WorkerManifest = serde_json::from_value(decoded.payload).unwrap();
        assert_eq!(decoded_manifest, manifest);
    }

    #[test]
    fn decode_rejects_wrong_protocol_version() {
        let raw = format!(
            r#"{{"protocol_version":999,"request_id":"{}","op":"worker.hello","payload":{{}}}}"#,
            Uuid::new_v4()
        );
        let mut line = raw.into_bytes();
        line.push(b'\n');
        let err = IpcMessage::decode_line(&line).expect_err("versão errada tem que falhar");
        assert!(matches!(err, crate::error::ProcessError::Protocol { .. }));
    }

    #[test]
    fn decode_rejects_line_without_terminator() {
        let raw = b"{\"protocol_version\":1,\"request_id\":\"00000000-0000-0000-0000-000000000000\",\"op\":\"app.ping\",\"payload\":{}}";
        let err = IpcMessage::decode_line(raw).expect_err("sem \\n tem que falhar");
        assert!(matches!(err, crate::error::ProcessError::Protocol { .. }));
    }

    #[test]
    fn op_strings_are_snake_case_and_stable() {
        // Esses strings são parte do **contrato** — mudar quebra
        // workers em campo.
        assert_eq!(IpcOp::Hello.as_str(), "worker.hello");
        assert_eq!(IpcOp::Ack.as_str(), "app.ack");
        assert_eq!(IpcOp::Ping.as_str(), "app.ping");
        assert_eq!(IpcOp::Pong.as_str(), "worker.pong");
        assert_eq!(IpcOp::Shutdown.as_str(), "app.shutdown");
        assert_eq!(IpcOp::ToolInvoke.as_str(), "tool.invoke");
        assert_eq!(IpcOp::ToolResult.as_str(), "tool.result");
    }
}
