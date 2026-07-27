//! Camada de persistência do Frederico IA Studio (núcleo).
//!
//! SQLite via `sqlx` com migrações numeradas. O caminho do banco é resolvido
//! via [`AppPaths`] (trait de `frederico-security`) — o storage **não**
//! importa nada de plataforma nem assume path fixo.
//!
//! A Fase 1 entregou a infraestrutura mínima. A Fase 2 adiciona
//! `0002_chat_core.sql` com as tabelas do motor de chat
//! (`conversations`, `messages`, `message_events`, `runs`,
//! `provider_configs`) e os repositórios correspondentes.

use chrono::Utc;
use frederico_core::{AppVersion, ConversationId, MessageId, ModelId, ProviderId, RunId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("falha ao abrir o banco SQLite em {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("falha ao rodar migração: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("query falhou: {0}")]
    Query(#[from] sqlx::Error),
    #[error("registro `app_info` ausente após migração")]
    AppInfoMissing,
    #[error("conversa {0} não encontrada")]
    ConversationNotFound(ConversationId),
    #[error("mensagem {0} não encontrada")]
    MessageNotFound(MessageId),
    #[error("run {0} não encontrado")]
    RunNotFound(RunId),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Trait para o caminho do banco. O `frederico-security` implementa isto
/// para Windows e os testes usam um fake. Manter o trait no storage evita
/// que o storage conheça o sistema de arquivos.
pub trait AppPaths {
    fn database_path(&self) -> PathBuf;
}

/// Estado persistido da primeira (e única, por enquanto) linha de `app_info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub started_at: String,
    pub last_seen_at: String,
}

/// Status de uma mensagem (espelha `MessageStatus` no spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Pending,
    Streaming,
    Completed,
    Failed,
    Cancelled,
    Timeout,
}

impl MessageStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MessageStatus::Pending => "pending",
            MessageStatus::Streaming => "streaming",
            MessageStatus::Completed => "completed",
            MessageStatus::Failed => "failed",
            MessageStatus::Cancelled => "cancelled",
            MessageStatus::Timeout => "timeout",
        }
    }
}

/// Status de um run (espelha `RunStatus` no spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Created,
    Running,
    Completed,
    Failed,
    Cancelled,
    Timeout,
}

impl RunStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Created => "created",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Timeout => "timeout",
        }
    }
}

/// Persistência de uma conversa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: Option<String>,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub created_at: String,
    pub updated_at: String,
    pub total_cost_microcents: u64,
}

/// Persistência de uma mensagem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub role: String,
    pub content: String,
    pub status: String,
    pub run_id: Option<RunId>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cost_microcents: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// Persistência de um evento do journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub id: i64,
    pub message_id: MessageId,
    pub seq: u32,
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: String,
}

/// Persistência de um run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
}

/// Configuração pública de um provedor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_id: ProviderId,
    pub display_name: String,
    pub configured: bool,
    pub last_ok_at: Option<String>,
    pub last_error_at: Option<String>,
    pub last_error: Option<String>,
}

/// Handle do banco de dados. Clonável (sqlx::SqlitePool é Arc internamente).
#[derive(Debug, Clone)]
pub struct Database {
    pool: sqlx::SqlitePool,
}

impl Database {
    /// Abre o banco no caminho dado, roda migrações e grava a linha inicial
    /// de `app_info` se for a primeira vez. Thread-safe.
    pub async fn open(path: &Path) -> StorageResult<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Open {
                    path: path.to_path_buf(),
                    source: sqlx::Error::Configuration(Box::new(std::io::Error::other(
                        format!("não consegui criar diretório {parent:?}: {e}"),
                    ))),
                })?;
        }

        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .map_err(|source| StorageError::Open {
                path: path.to_path_buf(),
                source,
            })?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO app_info (id, version, started_at, last_seen_at) \
             VALUES (1, ?1, ?2, ?2) \
             ON CONFLICT(id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
        )
        .bind(frederico_core::APP_VERSION.to_string())
        .bind(&now)
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    /// Versão registrada no `app_info` (a primeira escrita na inicialização).
    pub async fn app_info(&self) -> StorageResult<AppInfo> {
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT version, started_at, last_seen_at FROM app_info WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((version, started_at, last_seen_at)) => Ok(AppInfo {
                version,
                started_at,
                last_seen_at,
            }),
            None => Err(StorageError::AppInfoMissing),
        }
    }

    /// Versão de runtime esperada (vinda de `frederico-core`).
    pub fn expected_version() -> AppVersion {
        frederico_core::APP_VERSION
    }

    /// Acesso ao pool para os repositórios.
    pub fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
}

// ============================================================================
// Repositórios
// ============================================================================

/// Repositório de conversas. Append-only no sentido de que mensagens
/// filhas são imutáveis, mas a própria conversa pode ser renomeada
/// ou ter o modelo trocado.
pub struct ConversationRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

/// Row bruta de `conversations`. Usada só para `query_as` — o tipo de
/// domínio é [`Conversation`].
type ConversationRow = (
    String,             // id
    Option<String>,     // title
    String,             // provider_id
    String,             // model_id
    String,             // created_at
    String,             // updated_at
    i64,                // total_cost_microcents
);

/// Row bruta de `messages`. Tipo de domínio: [`Message`].
type MessageRow = (
    String,             // id
    String,             // conversation_id
    String,             // role
    String,             // content
    String,             // status
    Option<String>,     // run_id
    Option<i64>,        // prompt_tokens
    Option<i64>,        // completion_tokens
    i64,                // cost_microcents
    Option<String>,     // error
    String,             // created_at
    Option<String>,     // finished_at
);

/// Row bruta de `message_events`. Tipo de domínio: [`MessageEvent`].
type MessageEventRow = (
    i64,                // id
    String,             // message_id
    i64,                // seq
    String,             // kind
    String,             // data
    String,             // created_at
);

/// Row bruta de `runs`. Tipo de domínio: [`Run`].
type RunRow = (
    String,             // id
    String,             // conversation_id
    String,             // message_id
    String,             // status
    String,             // started_at
    Option<String>,     // finished_at
    Option<String>,     // cancellation_requested_at
);

/// Row bruta de `provider_configs`. Tipo de domínio: [`ProviderConfig`].
type ProviderConfigRow = (
    String,             // provider_id
    String,             // display_name
    i64,                // configured
    Option<String>,     // last_ok_at
    Option<String>,     // last_error_at
    Option<String>,     // last_error
);

impl<'a> ConversationRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Cria uma conversa nova.
    pub async fn create(
        &self,
        provider: &ProviderId,
        model: &ModelId,
        title: Option<&str>,
    ) -> StorageResult<Conversation> {
        let id = ConversationId::new();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO conversations (id, title, provider_id, model_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )
        .bind(id.0.to_string())
        .bind(title)
        .bind(provider.as_str())
        .bind(model.as_str())
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(Conversation {
            id,
            title: title.map(str::to_string),
            provider_id: provider.clone(),
            model_id: model.clone(),
            created_at: now.clone(),
            updated_at: now,
            total_cost_microcents: 0,
        })
    }

    pub async fn get(&self, id: &ConversationId) -> StorageResult<Conversation> {
        let row: Option<ConversationRow> = sqlx::query_as(
            "SELECT id, title, provider_id, model_id, created_at, updated_at, total_cost_microcents \
             FROM conversations WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        let (id_s, title, provider, model, created_at, updated_at, cost) = row
            .ok_or(StorageError::ConversationNotFound(*id))?;
        Ok(Conversation {
            id: ConversationId(uuid::Uuid::parse_str(&id_s).map_err(|_| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {id_s}").into())))?),
            title,
            provider_id: ProviderId::new(provider),
            model_id: ModelId::new(model),
            created_at,
            updated_at,
            total_cost_microcents: cost as u64,
        })
    }

    /// Lista conversas em ordem de atualização decrescente. Sem
    /// mensagens — o caller carrega separadamente.
    pub async fn list_recent(&self, limit: u32) -> StorageResult<Vec<Conversation>> {
        let rows: Vec<ConversationRow> = sqlx::query_as(
            "SELECT id, title, provider_id, model_id, created_at, updated_at, total_cost_microcents \
             FROM conversations ORDER BY updated_at DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, title, provider, model, created_at, updated_at, cost) in rows {
            let uuid = uuid::Uuid::parse_str(&id)
                .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?;
            out.push(Conversation {
                id: ConversationId(uuid),
                title,
                provider_id: ProviderId::new(provider),
                model_id: ModelId::new(model),
                created_at,
                updated_at,
                total_cost_microcents: cost as u64,
            });
        }
        Ok(out)
    }

    /// Renomeia a conversa.
    pub async fn rename(&self, id: &ConversationId, title: Option<&str>) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(title)
        .bind(&now)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StorageError::ConversationNotFound(*id));
        }
        Ok(())
    }

    /// Troca o modelo da conversa.
    pub async fn set_model(
        &self,
        id: &ConversationId,
        provider: &ProviderId,
        model: &ModelId,
    ) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE conversations SET provider_id = ?1, model_id = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(provider.as_str())
        .bind(model.as_str())
        .bind(&now)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(StorageError::ConversationNotFound(*id));
        }
        Ok(())
    }

    /// Acumula custo na conversa.
    pub async fn add_cost(&self, id: &ConversationId, microcents: u64) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE conversations SET total_cost_microcents = total_cost_microcents + ?1, \
             updated_at = ?2 WHERE id = ?3",
        )
        .bind(microcents as i64)
        .bind(&now)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &ConversationId) -> StorageResult<()> {
        let affected = sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(StorageError::ConversationNotFound(*id));
        }
        Ok(())
    }
}

/// Repositório de mensagens. Append-only.
pub struct MessageRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> MessageRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Acesso ao pool subjacente. Útil para o orquestrador construir
    /// outros repos que precisam do mesmo pool sem ter o `Database`
    /// em mãos.
    #[must_use]
    pub fn pool(&self) -> &'a sqlx::SqlitePool {
        self.pool
    }

    /// Cria uma mensagem nova. Para o caso comum, o `run_id` é None
    /// (mensagem do usuário) — o orquestrador da Leva 3 cria o run
    /// depois e atualiza a mensagem.
    pub async fn create(
        &self,
        conversation_id: &ConversationId,
        role: &str,
        content: &str,
        run_id: Option<RunId>,
    ) -> StorageResult<Message> {
        let id = MessageId::new();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content, status, run_id, cost_microcents, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, 0, ?6)",
        )
        .bind(id.0.to_string())
        .bind(conversation_id.0.to_string())
        .bind(role)
        .bind(content)
        .bind(run_id.map(|r| r.0.to_string()))
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(Message {
            id,
            conversation_id: *conversation_id,
            role: role.to_string(),
            content: content.to_string(),
            status: "pending".to_string(),
            run_id,
            prompt_tokens: None,
            completion_tokens: None,
            cost_microcents: 0,
            error: None,
            created_at: now,
            finished_at: None,
        })
    }

    pub async fn get(&self, id: &MessageId) -> StorageResult<Message> {
        let row: Option<MessageRow> = sqlx::query_as(
            "SELECT id, conversation_id, role, content, status, run_id, \
                    prompt_tokens, completion_tokens, cost_microcents, error, created_at, finished_at \
             FROM messages WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        let (id, conv, role, content, status, run_id, p, c, cost, err, created_at, finished_at) =
            row.ok_or(StorageError::MessageNotFound(*id))?;
        let conv_uuid = uuid::Uuid::parse_str(&conv)
            .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?;
        Ok(Message {
            id: MessageId(uuid::Uuid::parse_str(&id).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            conversation_id: ConversationId(conv_uuid),
            role,
            content,
            status,
            run_id: run_id
                .map(|r| uuid::Uuid::parse_str(&r).map(RunId))
                .transpose()
                .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?,
            prompt_tokens: p.map(|n| n as u32),
            completion_tokens: c.map(|n| n as u32),
            cost_microcents: cost as u64,
            error: err,
            created_at,
            finished_at,
        })
    }

    /// Lista mensagens de uma conversa, em ordem cronológica.
    pub async fn list_for_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> StorageResult<Vec<Message>> {
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT id, conversation_id, role, content, status, run_id, \
                    prompt_tokens, completion_tokens, cost_microcents, error, created_at, finished_at \
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )
        .bind(conversation_id.0.to_string())
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, conv, role, content, status, run_id, p, c, cost, err, created_at, finished_at) in rows {
            let conv_uuid = uuid::Uuid::parse_str(&conv)
                .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?;
            out.push(Message {
                id: MessageId(uuid::Uuid::parse_str(&id).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
                conversation_id: ConversationId(conv_uuid),
                role,
                content,
                status,
                run_id: run_id
                    .map(|r| uuid::Uuid::parse_str(&r).map(RunId))
                    .transpose()
                    .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?,
                prompt_tokens: p.map(|n| n as u32),
                completion_tokens: c.map(|n| n as u32),
                cost_microcents: cost as u64,
                error: err,
                created_at,
                finished_at,
            });
        }
        Ok(out)
    }

    /// Atualiza o status de uma mensagem.
    pub async fn set_status(
        &self,
        id: &MessageId,
        status: MessageStatus,
    ) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        let finished_at = if matches!(
            status,
            MessageStatus::Completed
                | MessageStatus::Failed
                | MessageStatus::Cancelled
                | MessageStatus::Timeout
        ) {
            Some(&now)
        } else {
            None
        };
        sqlx::query(
            "UPDATE messages SET status = ?1, finished_at = ?2 WHERE id = ?3",
        )
        .bind(status.as_str())
        .bind(finished_at)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Substitui o conteúdo da mensagem (para Assistant: o texto
    /// montado a partir dos deltas; em geral, atualizado pelo
    /// orquestrador conforme os eventos chegam).
    pub async fn set_content(&self, id: &MessageId, content: &str) -> StorageResult<()> {
        sqlx::query("UPDATE messages SET content = ?1 WHERE id = ?2")
            .bind(content)
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Grava usage e custo finais (chamado no `Done`).
    pub async fn set_usage_and_cost(
        &self,
        id: &MessageId,
        prompt_tokens: u32,
        completion_tokens: u32,
        cost_microcents: u64,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE messages SET prompt_tokens = ?1, completion_tokens = ?2, cost_microcents = ?3 \
             WHERE id = ?4",
        )
        .bind(prompt_tokens as i64)
        .bind(completion_tokens as i64)
        .bind(cost_microcents as i64)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Grava erro estruturado (PT-BR com ação).
    pub async fn set_error(&self, id: &MessageId, error_json: &str) -> StorageResult<()> {
        sqlx::query("UPDATE messages SET error = ?1 WHERE id = ?2")
            .bind(error_json)
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

/// Journal de eventos de mensagem. Append-only.
pub struct MessageEventRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> MessageEventRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Anexa um evento ao journal. Retorna o `seq` usado. O `seq` é
    /// monotônico por `message_id` (garantido pelo `UNIQUE(message_id, seq)`).
    pub async fn append(
        &self,
        message_id: &MessageId,
        kind: &str,
        data: &serde_json::Value,
    ) -> StorageResult<u32> {
        let now = Utc::now().to_rfc3339();
        // Calcula o próximo `seq` para a mensagem.
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM message_events WHERE message_id = ?1",
        )
        .bind(message_id.0.to_string())
        .fetch_one(self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO message_events (message_id, seq, kind, data, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(message_id.0.to_string())
        .bind(next_seq)
        .bind(kind)
        .bind(data.to_string())
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(next_seq as u32)
    }

    /// Lê eventos de uma mensagem com `seq > since_seq` (exclusivo).
    /// Use `since_seq = 0` para receber todos (o `seq` começa em 1).
    pub async fn list_for_message(
        &self,
        message_id: &MessageId,
        since_seq: u32,
    ) -> StorageResult<Vec<MessageEvent>> {
        let rows: Vec<MessageEventRow> = sqlx::query_as(
            "SELECT id, message_id, seq, kind, data, created_at \
             FROM message_events WHERE message_id = ?1 AND seq > ?2 ORDER BY seq ASC",
        )
        .bind(message_id.0.to_string())
        .bind(since_seq as i64)
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, mid, seq, kind, data, created_at) in rows {
            let data_json: serde_json::Value = serde_json::from_str(&data)
                .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad json: {e}").into())))?;
            let mid_uuid = uuid::Uuid::parse_str(&mid)
                .map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?;
            out.push(MessageEvent {
                id,
                message_id: MessageId(mid_uuid),
                seq: seq as u32,
                kind,
                data: data_json,
                created_at,
            });
        }
        Ok(out)
    }
}

/// Repositório de runs.
pub struct RunRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> RunRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    pub async fn create(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> StorageResult<Run> {
        let id = RunId::new();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO runs (id, conversation_id, message_id, status, started_at) \
             VALUES (?1, ?2, ?3, 'created', ?4)",
        )
        .bind(id.0.to_string())
        .bind(conversation_id.0.to_string())
        .bind(message_id.0.to_string())
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(Run {
            id,
            conversation_id: *conversation_id,
            message_id: *message_id,
            status: "created".to_string(),
            started_at: now,
            finished_at: None,
            cancellation_requested_at: None,
        })
    }

    pub async fn get(&self, id: &RunId) -> StorageResult<Run> {
        let row: Option<RunRow> = sqlx::query_as(
            "SELECT id, conversation_id, message_id, status, started_at, finished_at, \
                    cancellation_requested_at FROM runs WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        let (id_s, conv, msg, status, started_at, finished_at, cancellation_requested_at) =
            row.ok_or(StorageError::RunNotFound(*id))?;
        Ok(Run {
            id: RunId(uuid::Uuid::parse_str(&id_s).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            conversation_id: ConversationId(uuid::Uuid::parse_str(&conv).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            message_id: MessageId(uuid::Uuid::parse_str(&msg).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            status,
            started_at,
            finished_at,
            cancellation_requested_at,
        })
    }

    pub async fn get_by_message(&self, message_id: &MessageId) -> StorageResult<Option<Run>> {
        let row: Option<RunRow> = sqlx::query_as(
            "SELECT id, conversation_id, message_id, status, started_at, finished_at, \
                    cancellation_requested_at FROM runs WHERE message_id = ?1",
        )
        .bind(message_id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        let Some((id_s, conv, msg, status, started_at, finished_at, cancellation_requested_at)) = row else {
            return Ok(None);
        };
        Ok(Some(Run {
            id: RunId(uuid::Uuid::parse_str(&id_s).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            conversation_id: ConversationId(uuid::Uuid::parse_str(&conv).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            message_id: MessageId(uuid::Uuid::parse_str(&msg).map_err(|e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?),
            status,
            started_at,
            finished_at,
            cancellation_requested_at,
        }))
    }

    pub async fn set_status(&self, id: &RunId, status: RunStatus) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        let finished_at = if matches!(
            status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled | RunStatus::Timeout
        ) {
            Some(&now)
        } else {
            None
        };
        sqlx::query(
            "UPDATE runs SET status = ?1, finished_at = COALESCE(?2, finished_at) WHERE id = ?3",
        )
        .bind(status.as_str())
        .bind(finished_at)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Marca `cancellation_requested_at`. O orquestrador (Leva 3)
    /// usa isso para saber que o usuário pediu stop.
    pub async fn request_cancellation(&self, id: &RunId) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE runs SET cancellation_requested_at = ?1 WHERE id = ?2",
        )
        .bind(&now)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }
}

/// Repositório de configuração pública de provedores. **Sem segredo**
/// — só `configured: bool` e metadados.
pub struct ProviderConfigRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> ProviderConfigRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    pub async fn upsert(
        &self,
        provider_id: &ProviderId,
        display_name: &str,
        configured: bool,
    ) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO provider_configs (provider_id, display_name, configured) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(provider_id) DO UPDATE SET \
                display_name = excluded.display_name, \
                configured = excluded.configured",
        )
        .bind(provider_id.as_str())
        .bind(display_name)
        .bind(configured as i64)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self) -> StorageResult<Vec<ProviderConfig>> {
        let rows: Vec<ProviderConfigRow> = sqlx::query_as(
            "SELECT provider_id, display_name, configured, last_ok_at, last_error_at, last_error \
             FROM provider_configs ORDER BY display_name ASC",
        )
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, display_name, configured, last_ok_at, last_error_at, last_error) in rows {
            out.push(ProviderConfig {
                provider_id: ProviderId::new(id),
                display_name,
                configured: configured != 0,
                last_ok_at,
                last_error_at,
                last_error,
            });
        }
        Ok(out)
    }

    pub async fn record_last_ok(&self, provider_id: &ProviderId) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE provider_configs SET last_ok_at = ?1, last_error_at = NULL, last_error = NULL \
             WHERE provider_id = ?2",
        )
        .bind(&now)
        .bind(provider_id.as_str())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_last_error(
        &self,
        provider_id: &ProviderId,
        error_pt: &str,
    ) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE provider_configs SET last_error_at = ?1, last_error = ?2 WHERE provider_id = ?3",
        )
        .bind(&now)
        .bind(error_pt)
        .bind(provider_id.as_str())
        .execute(self.pool)
        .await?;
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_in_memory_and_runs_migration() {
        let dir = tempdir();
        let db_path = dir.join("test.db");
        let db = Database::open(&db_path).await.expect("abre");
        let info = db.app_info().await.expect("lê app_info");
        assert_eq!(info.version, "0.1.0");
        assert!(!info.started_at.is_empty());
    }

    #[tokio::test]
    async fn second_open_updates_last_seen_only() {
        let dir = tempdir();
        let db_path = dir.join("test2.db");
        let db1 = Database::open(&db_path).await.expect("abre 1");
        let info1 = db1.app_info().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let db2 = Database::open(&db_path).await.expect("abre 2");
        let info2 = db2.app_info().await.unwrap();
        assert_eq!(info1.started_at, info2.started_at);
        assert_ne!(info1.last_seen_at, info2.last_seen_at);
    }

    #[tokio::test]
    async fn conversation_repo_create_and_get() {
        let db = Database::open(&tempdir().join("conv.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let conv = conv_repo
            .create(
                &ProviderId::new("openai"),
                &ModelId::new("gpt-4o"),
                Some("teste"),
            )
            .await
            .unwrap();
        assert_eq!(conv.title.as_deref(), Some("teste"));
        let got = conv_repo.get(&conv.id).await.unwrap();
        assert_eq!(got.id, conv.id);
        assert_eq!(got.total_cost_microcents, 0);
    }

    #[tokio::test]
    async fn conversation_repo_rename_and_set_model() {
        let db = Database::open(&tempdir().join("conv2.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        conv_repo.rename(&conv.id, Some("novo")).await.unwrap();
        let got = conv_repo.get(&conv.id).await.unwrap();
        assert_eq!(got.title.as_deref(), Some("novo"));
        conv_repo
            .set_model(&conv.id, &ProviderId::new("anthropic"), &ModelId::new("claude-3-5-sonnet-latest"))
            .await
            .unwrap();
        let got = conv_repo.get(&conv.id).await.unwrap();
        assert_eq!(got.provider_id.as_str(), "anthropic");
        assert_eq!(got.model_id.as_str(), "claude-3-5-sonnet-latest");
    }

    #[tokio::test]
    async fn conversation_repo_add_cost_and_list() {
        let db = Database::open(&tempdir().join("conv3.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let c1 = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let c2 = conv_repo
            .create(&ProviderId::new("anthropic"), &ModelId::new("claude-3-5-sonnet-latest"), None)
            .await
            .unwrap();
        conv_repo.add_cost(&c1.id, 1234).await.unwrap();
        conv_repo.add_cost(&c2.id, 5678).await.unwrap();
        let list = conv_repo.list_recent(10).await.unwrap();
        assert_eq!(list.len(), 2);
        let sum: u64 = list.iter().map(|c| c.total_cost_microcents).sum();
        assert_eq!(sum, 1234 + 5678);
    }

    #[tokio::test]
    async fn message_repo_create_and_append_event() {
        let db = Database::open(&tempdir().join("msg.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let ev_repo = MessageEventRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let user = msg_repo
            .create(&conv.id, "user", "oi", None)
            .await
            .unwrap();
        assert_eq!(user.role, "user");
        assert_eq!(user.status, "pending");
        let asst = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        // Anexa 3 eventos.
        let s1 = ev_repo
            .append(
                &asst.id,
                "delta",
                &serde_json::json!({"content": "O"}),
            )
            .await
            .unwrap();
        let s2 = ev_repo
            .append(
                &asst.id,
                "delta",
                &serde_json::json!({"content": "k"}),
            )
            .await
            .unwrap();
        let s3 = ev_repo
            .append(
                &asst.id,
                "done",
                &serde_json::json!({"stop_reason": "stop"}),
            )
            .await
            .unwrap();
        assert_eq!((s1, s2, s3), (1, 2, 3));
        let events = ev_repo.list_for_message(&asst.id, 0).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind, "delta");
        assert_eq!(events[2].kind, "done");
    }

    #[tokio::test]
    async fn message_repo_set_status_records_finished_at() {
        let db = Database::open(&tempdir().join("msg2.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let m = msg_repo.create(&conv.id, "assistant", "", None).await.unwrap();
        msg_repo
            .set_status(&m.id, MessageStatus::Streaming)
            .await
            .unwrap();
        let got = msg_repo.get(&m.id).await.unwrap();
        assert_eq!(got.status, "streaming");
        assert!(got.finished_at.is_none());
        msg_repo
            .set_status(&m.id, MessageStatus::Completed)
            .await
            .unwrap();
        let got = msg_repo.get(&m.id).await.unwrap();
        assert_eq!(got.status, "completed");
        assert!(got.finished_at.is_some());
    }

    #[tokio::test]
    async fn run_repo_create_and_status_progression() {
        let db = Database::open(&tempdir().join("run.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let asst = msg_repo.create(&conv.id, "assistant", "", None).await.unwrap();
        let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
        assert_eq!(run.status, "created");
        run_repo.set_status(&run.id, RunStatus::Running).await.unwrap();
        let got = run_repo.get(&run.id).await.unwrap();
        assert_eq!(got.status, "running");
        run_repo.request_cancellation(&run.id).await.unwrap();
        let got = run_repo.get(&run.id).await.unwrap();
        assert!(got.cancellation_requested_at.is_some());
        run_repo.set_status(&run.id, RunStatus::Cancelled).await.unwrap();
        let got = run_repo.get(&run.id).await.unwrap();
        assert_eq!(got.status, "cancelled");
        assert!(got.finished_at.is_some());
    }

    #[tokio::test]
    async fn run_repo_unique_per_message() {
        let db = Database::open(&tempdir().join("run2.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let asst = msg_repo.create(&conv.id, "assistant", "", None).await.unwrap();
        run_repo.create(&conv.id, &asst.id).await.unwrap();
        // Segundo run para a mesma mensagem deve falhar (UNIQUE).
        let r = run_repo.create(&conv.id, &asst.id).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn provider_config_repo_upsert_and_list() {
        let db = Database::open(&tempdir().join("pcfg.db")).await.unwrap();
        let repo = ProviderConfigRepo::new(&db);
        repo.upsert(&ProviderId::new("openai"), "OpenAI", true).await.unwrap();
        repo.upsert(&ProviderId::new("anthropic"), "Anthropic", false).await.unwrap();
        repo.upsert(&ProviderId::new("openai"), "OpenAI", false).await.unwrap();
        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 2);
        // Depois do upsert com `configured: false`, OpenAI não está mais configurado.
        let openai = list.iter().find(|c| c.provider_id.as_str() == "openai").unwrap();
        assert!(!openai.configured);
    }

    #[tokio::test]
    async fn provider_config_record_last_ok_clears_error() {
        let db = Database::open(&tempdir().join("pcfg2.db")).await.unwrap();
        let repo = ProviderConfigRepo::new(&db);
        let p = ProviderId::new("openai");
        repo.upsert(&p, "OpenAI", true).await.unwrap();
        repo.record_last_error(&p, "401 chave inválida").await.unwrap();
        let before = repo.list().await.unwrap();
        let openai = before.iter().find(|c| c.provider_id == p).unwrap();
        assert_eq!(openai.last_error.as_deref(), Some("401 chave inválida"));
        repo.record_last_ok(&p).await.unwrap();
        let after = repo.list().await.unwrap();
        let openai = after.iter().find(|c| c.provider_id == p).unwrap();
        assert!(openai.last_error.is_none());
        assert!(openai.last_ok_at.is_some());
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "frederico-storage-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
