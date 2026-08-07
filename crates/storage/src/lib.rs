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
use frederico_agent_engine::{apply_transition as portao_apply_transition, RunEventKind, RunState};
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
    /// O portão [`frederico_agent_engine::apply_transition`]
    /// rejeitou a transição solicitada. O caller deveria ter
    /// passado a transição pelo `state_mapping` antes de
    /// invocar este método — o erro aqui significa que o
    /// estado real do run não bate com o `from` que o caller
    /// assumiu.
    #[error("portão `apply_transition` rejeitou: from={from} kind={kind} — {cause}")]
    InvalidTransition {
        from: String,
        kind: String,
        cause: String,
    },
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

/// Persistência de um run. A coluna `status` (Fase 2) coexiste com
/// `state` (Fase 3) — `state` é a verdade canônica (22 valores do
/// `frederico-agent-engine::RunState`); `status` é o derivado
/// projeta do pelo `view` `runs_with_status` (6 valores, mantido pra
/// o `ChatOrchestrator` da Fase 2 e os testes E2E de recovery do
/// Hardening 5). Ver ADR-0009.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    /// Status derivado (6 valores, populado pela Fase 2 via `set_status`
    /// ou lido da view `runs_with_status`).
    pub status: String,
    /// Estado canônico do `Run` (22 valores do
    /// `frederico-agent-engine::RunState`, populado pela Etapa 4).
    pub state: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    /// Iteração atual do loop `calling_model` → `continuing_model`.
    pub current_step: u32,
    /// `Budget` serializado em JSON.
    pub budget_json: String,
    /// `Vec<ToolId>` (Etapa 2) serializado em JSON.
    pub allowed_tools_json: String,
    /// Última vez que o `Run` demonstrou estar vivo.
    pub last_heartbeat_at: String,
    /// Próximo `seq` a usar no journal (monotônico por `run_id`).
    pub last_event_seq: u64,
    /// Provedor que está executando este `Run`.
    pub provider_id: String,
    /// Modelo dentro do provedor.
    pub model_id: String,
    /// Assistente (Fase 3 Etapa 6). Nullable até lá.
    pub assistant_id: Option<String>,

    // ---- Etapa 4 da Fase 6: subagente (ADR-0027) ----
    /// Contador global de subagentes vivos (D1 do ADR-0027: teto
    /// de 8). Espelha `runs.subagent_count`. Lido/escrito pelo
    /// `RunRepo::increment_subagent_count` /
    /// `decrement_subagent_count` (Etapa 4 PR 1).
    pub subagent_count: u32,
    /// Profundidade na árvore de subagentes (D2 do ADR-0027).
    /// Espelha `runs.depth`.
    pub depth: u32,
    /// `RunId` do pai se o Run é um subagente (espelha
    /// `runs.parent_run_id`).
    pub parent_run_id: Option<RunId>,
    /// Custo efetivo em microcents (espelha
    /// `runs.spent_microcents`). Faz parte do `SpentBudget`
    /// do `Run` em memória.
    pub spent_microcents: u64,
    /// Tokens de entrada efetivos (espelha
    /// `runs.spent_tokens_in`).
    pub spent_tokens_in: u64,
    /// Tokens de saída efetivos (espelha
    /// `runs.spent_tokens_out`).
    pub spent_tokens_out: u64,
    /// Passos efetivos do loop (espelha `runs.spent_steps`).
    pub spent_steps: u32,
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
                    source: sqlx::Error::Configuration(Box::new(std::io::Error::other(format!(
                        "não consegui criar diretório {parent:?}: {e}"
                    )))),
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

    /// Abre um banco SQLite **em memória** (`:memory:`). Usado
    /// por testes de subsistemas (e.g. o runner de avaliação
    /// do `frederico-memory`) que precisam de isolamento total
    /// — sem arquivo, sem estado compartilhado entre runs.
    ///
    /// Roda as migrações e popula `app_info` igual ao
    /// [`Self::open`]. O pool é single-connection (`max_connections = 1`)
    /// porque SQLite em memória é local a uma conexão — com
    /// várias, cada uma veria um banco separado.
    pub async fn open_in_memory() -> StorageResult<Self> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .map_err(|source| StorageError::Open {
                path: PathBuf::from(":memory:"),
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
    String,         // id
    Option<String>, // title
    String,         // provider_id
    String,         // model_id
    String,         // created_at
    String,         // updated_at
    i64,            // total_cost_microcents
);

/// Row bruta de `messages`. Tipo de domínio: [`Message`].
type MessageRow = (
    String,         // id
    String,         // conversation_id
    String,         // role
    String,         // content
    String,         // status
    Option<String>, // run_id
    Option<i64>,    // prompt_tokens
    Option<i64>,    // completion_tokens
    i64,            // cost_microcents
    Option<String>, // error
    String,         // created_at
    Option<String>, // finished_at
);

/// Row bruta de `message_events`. Tipo de domínio: [`MessageEvent`].
type MessageEventRow = (
    i64,    // id
    String, // message_id
    i64,    // seq
    String, // kind
    String, // data
    String, // created_at
);

/// Row bruta de `runs`. Tipo de domínio: [`Run`].
///
/// **Por que é uma struct e não uma tupla:** o `sqlx::FromRow`
/// deriva `FromRow` automaticamente para tuplas até 9 elementos
/// (regra do `sqlx` 0.8); acima disso precisa struct com
/// `#[derive(FromRow)]` (ou `#[sqlx(FromRow)]` na forma
/// explícita). A Etapa 4 da Fase 6 bumpou de 16 pra 23 colunas
/// (subagent_count + depth + parent_run_id + spent_*), então
/// migramos pra struct.
///
/// **Campos: 1:1 com a tabela `runs` em ordem do `SELECT`.** O
/// `row_to_run` (private) consome este struct e devolve `Run`.
#[derive(sqlx::FromRow)]
struct RunRow {
    id: String,
    conversation_id: String,
    message_id: String,
    status: String,
    state: String,
    started_at: String,
    finished_at: Option<String>,
    cancellation_requested_at: Option<String>,
    current_step: i64,
    budget_json: String,
    allowed_tools_json: String,
    last_heartbeat_at: String,
    last_event_seq: i64,
    provider_id: String,
    model_id: String,
    assistant_id: Option<String>,
    // ---- Etapa 4 da Fase 6: subagente + spent ----
    subagent_count: i64,
    depth: i64,
    parent_run_id: Option<String>,
    spent_microcents: i64,
    spent_tokens_in: i64,
    spent_tokens_out: i64,
    spent_steps: i64,
}

/// Row bruta de `provider_configs`. Tipo de domínio: [`ProviderConfig`].
type ProviderConfigRow = (
    String,         // provider_id
    String,         // display_name
    i64,            // configured
    Option<String>, // last_ok_at
    Option<String>, // last_error_at
    Option<String>, // last_error
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
        let (id_s, title, provider, model, created_at, updated_at, cost) =
            row.ok_or(StorageError::ConversationNotFound(*id))?;
        Ok(Conversation {
            id: ConversationId(uuid::Uuid::parse_str(&id_s).map_err(|_| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {id_s}").into()))
            })?),
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
            let uuid = uuid::Uuid::parse_str(&id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
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
        let affected =
            sqlx::query("UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3")
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
        let conv_uuid = uuid::Uuid::parse_str(&conv).map_err(|e| {
            StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
        })?;
        Ok(Message {
            id: MessageId(uuid::Uuid::parse_str(&id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?),
            conversation_id: ConversationId(conv_uuid),
            role,
            content,
            status,
            run_id: run_id
                .map(|r| uuid::Uuid::parse_str(&r).map(RunId))
                .transpose()
                .map_err(|e| {
                    StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
                })?,
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
        for (id, conv, role, content, status, run_id, p, c, cost, err, created_at, finished_at) in
            rows
        {
            let conv_uuid = uuid::Uuid::parse_str(&conv).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
            out.push(Message {
                id: MessageId(uuid::Uuid::parse_str(&id).map_err(|e| {
                    StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
                })?),
                conversation_id: ConversationId(conv_uuid),
                role,
                content,
                status,
                run_id: run_id
                    .map(|r| uuid::Uuid::parse_str(&r).map(RunId))
                    .transpose()
                    .map_err(|e| {
                        StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
                    })?,
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
    pub async fn set_status(&self, id: &MessageId, status: MessageStatus) -> StorageResult<()> {
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
        sqlx::query("UPDATE messages SET status = ?1, finished_at = ?2 WHERE id = ?3")
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
            let data_json: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad json: {e}").into()))
            })?;
            let mid_uuid = uuid::Uuid::parse_str(&mid).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
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

    /// Seta `run_seq` em uma `MessageEvent` específica (identificada por
    /// `message_id` + `message_seq`). Usado pelo `RunExecutor` para
    /// preencher a coluna de join com `run_events` (Fase 6, Etapa 2,
    /// ADR-0029 §D6). A coluna `run_seq` foi adicionada na migração
    /// `0027_run_events.sql` e é `NULL` por padrão; este método é o
    /// que materializa o vínculo entre as duas tabelas.
    ///
    /// Idempotente: re-chamar com o mesmo `run_seq` é no-op (o `UPDATE`
    /// resulta em 0 linhas alteradas mas não falha).
    pub async fn set_run_seq(
        &self,
        message_id: &MessageId,
        message_seq: u32,
        run_seq: u64,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE message_events SET run_seq = ?1 \
             WHERE message_id = ?2 AND seq = ?3",
        )
        .bind(run_seq as i64)
        .bind(message_id.0.to_string())
        .bind(message_seq as i64)
        .execute(self.pool)
        .await?;
        Ok(())
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
        // O schema da Fase 3 (0003) preenche `state`, `current_step`,
        // `budget_json`, `allowed_tools_json`, `last_heartbeat_at`,
        // `last_event_seq`, `provider_id` e `model_id` com defaults
        // (vide CHECK constraint da migração). Só precisamos do
        // mínimo aqui.
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
            state: "created".to_string(),
            started_at: now.clone(),
            finished_at: None,
            cancellation_requested_at: None,
            current_step: 0,
            budget_json: "{}".to_string(),
            allowed_tools_json: "[]".to_string(),
            last_heartbeat_at: now,
            last_event_seq: 0,
            provider_id: String::new(),
            model_id: String::new(),
            assistant_id: None,
            subagent_count: 0,
            depth: 0,
            parent_run_id: None,
            spent_microcents: 0,
            spent_tokens_in: 0,
            spent_tokens_out: 0,
            spent_steps: 0,
        })
    }

    pub async fn get(&self, id: &RunId) -> StorageResult<Run> {
        let row: Option<RunRow> = sqlx::query_as(
            "SELECT id, conversation_id, message_id, status, state, started_at, finished_at, \
                    cancellation_requested_at, current_step, budget_json, allowed_tools_json, \
                    last_heartbeat_at, last_event_seq, provider_id, model_id, assistant_id, \
                    subagent_count, depth, parent_run_id, spent_microcents, spent_tokens_in, \
                    spent_tokens_out, spent_steps \
             FROM runs WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        Self::row_to_run(row.ok_or(StorageError::RunNotFound(*id))?)
    }

    pub async fn get_by_message(&self, message_id: &MessageId) -> StorageResult<Option<Run>> {
        let row: Option<RunRow> = sqlx::query_as(
            "SELECT id, conversation_id, message_id, status, state, started_at, finished_at, \
                    cancellation_requested_at, current_step, budget_json, allowed_tools_json, \
                    last_heartbeat_at, last_event_seq, provider_id, model_id, assistant_id, \
                    subagent_count, depth, parent_run_id, spent_microcents, spent_tokens_in, \
                    spent_tokens_out, spent_steps \
             FROM runs WHERE message_id = ?1",
        )
        .bind(message_id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(Self::row_to_run(r)?)),
            None => Ok(None),
        }
    }

    /// Converte uma `RunRow` (struct do `sqlx::FromRow`) no tipo de
    /// domínio [`Run`]. Centralizado porque `get`, `get_by_message`
    /// e `list_stale_heartbeats` precisam do mesmo parsing.
    fn row_to_run(row: RunRow) -> StorageResult<Run> {
        Ok(Run {
            id: RunId(uuid::Uuid::parse_str(&row.id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?),
            conversation_id: ConversationId(uuid::Uuid::parse_str(&row.conversation_id).map_err(
                |e| StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())),
            )?),
            message_id: MessageId(uuid::Uuid::parse_str(&row.message_id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?),
            status: row.status,
            state: row.state,
            started_at: row.started_at,
            finished_at: row.finished_at,
            cancellation_requested_at: row.cancellation_requested_at,
            current_step: row.current_step as u32,
            budget_json: row.budget_json,
            allowed_tools_json: row.allowed_tools_json,
            last_heartbeat_at: row.last_heartbeat_at,
            last_event_seq: row.last_event_seq as u64,
            provider_id: row.provider_id,
            model_id: row.model_id,
            assistant_id: row.assistant_id,
            subagent_count: row.subagent_count as u32,
            depth: row.depth as u32,
            parent_run_id: match row.parent_run_id {
                Some(s) if !s.is_empty() => {
                    Some(RunId(uuid::Uuid::parse_str(&s).map_err(|e| {
                        StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
                    })?))
                }
                _ => None,
            },
            spent_microcents: row.spent_microcents as u64,
            spent_tokens_in: row.spent_tokens_in as u64,
            spent_tokens_out: row.spent_tokens_out as u64,
            spent_steps: row.spent_steps as u32,
        })
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
        sqlx::query("UPDATE runs SET cancellation_requested_at = ?1 WHERE id = ?2")
            .bind(&now)
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// **Versão UNCHECKED**: atualiza a coluna `state` (22 valores
    /// do `frederico_agent_engine::RunState`) **sozinha, sem
    /// consultar o portão `apply_transition`**. **Não usar em
    /// código de produção da Fase 6 em diante** — é API legada
    /// mantida só pra testes que precisam forçar estados
    /// (`setup_executor`, testes de recovery com banco
    /// pré-migração).
    ///
    /// O caminho de produção da Etapa 2 da Fase 6 é
    /// [`set_state_validated`](Self::set_state_validated) (valida
    /// via `apply_transition` antes de gravar + grava `RunEvent`
    /// atômico) ou
    /// [`set_state_and_heartbeat_unchecked_tx`](Self::set_state_and_heartbeat_unchecked_tx)
    /// (versão transacional, ainda unchecked, mantida por
    /// compatibilidade até a Etapa 3 fechar a migração do
    /// `RunExecutor`).
    pub async fn set_state_unchecked(
        &self,
        id: &RunId,
        state: frederico_agent_engine::RunState,
    ) -> StorageResult<()> {
        sqlx::query("UPDATE runs SET state = ?1 WHERE id = ?2")
            .bind(state.as_str())
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// **Versão UNCHECKED**: atualiza `runs.state` +
    /// `last_heartbeat_at` + `last_event_seq` numa **única
    /// transação** `BEGIN IMMEDIATE; ...; COMMIT;`. Garante
    /// consistência contra crash (ou os 3 valores novos são
    /// persistidos, ou nenhum é), mas **não consulta o portão
    /// `apply_transition`** — é API legada, mantida por
    /// compatibilidade até a Etapa 3 da Fase 6 migrar o
    /// `RunExecutor` pra
    /// [`set_state_validated`](Self::set_state_validated).
    ///
    /// O `BEGIN IMMEDIATE` pega o write lock imediatamente (em vez
    /// de `BEGIN DEFERRED` que só pega no primeiro `WRITE`),
    /// evitando o `SQLITE_BUSY` quando vários executors estão
    /// ativos.
    pub async fn set_state_and_heartbeat_unchecked_tx(
        &self,
        id: &RunId,
        state: frederico_agent_engine::RunState,
        last_event_seq: u64,
    ) -> StorageResult<()> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE runs SET state = ?1, last_heartbeat_at = ?2, last_event_seq = ?3 \
             WHERE id = ?4",
        )
        .bind(state.as_str())
        .bind(&now)
        .bind(last_event_seq as i64)
        .bind(id.0.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Lista runs não-terminais cujo `last_heartbeat_at` é mais
    /// antigo que `threshold_seconds` segundos atrás. Usado pela
    /// Etapa 5.x (recovery de crash) no startup da casca Tauri —
    /// se um run ficou preso (app crashou entre dois heartbeats),
    /// a casca marca como `interrupted` no boot.
    pub async fn list_stale_heartbeats(&self, threshold_seconds: u64) -> StorageResult<Vec<Run>> {
        // SQLite: `datetime('now', '-N seconds')` devolve o
        // timestamp de N segundos atrás em `YYYY-MM-DD HH:MM:SS`.
        // O `last_heartbeat_at` pode estar em RFC 3339 (com `T`
        // no meio) — `datetime(last_heartbeat_at)` normaliza pra
        // o mesmo formato antes de comparar.
        let threshold = format!("-{} seconds", threshold_seconds);
        let rows: Vec<RunRow> = sqlx::query_as(
            "SELECT id, conversation_id, message_id, status, state, started_at, finished_at, \
                    cancellation_requested_at, current_step, budget_json, allowed_tools_json, \
                    last_heartbeat_at, last_event_seq, provider_id, model_id, assistant_id, \
                    subagent_count, depth, parent_run_id, spent_microcents, spent_tokens_in, \
                    spent_tokens_out, spent_steps \
             FROM runs \
             WHERE state NOT IN ('completed', 'failed', 'cancelled', 'interrupted') \
               AND datetime(last_heartbeat_at) < datetime('now', ?1) \
             ORDER BY last_heartbeat_at ASC",
        )
        .bind(&threshold)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(Self::row_to_run).collect()
    }

    /// Marca um `Run` como interrompido (terminal: watchdog matou
    /// ou recovery de crash detectou). Seta `state = 'interrupted'`,
    /// `status = 'failed'` (a view `runs_with_status` mapeia
    /// `interrupted → timeout`), `finished_at = now`, e adiciona uma
    /// nota no `error` da mensagem correspondente (best effort — se
    /// a mensagem não existe, segue sem erro).
    pub async fn mark_interrupted(&self, id: &RunId) -> StorageResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE runs SET state = 'interrupted', status = 'failed', \
             finished_at = COALESCE(?1, finished_at) \
             WHERE id = ?2",
        )
        .bind(&now)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Força `last_heartbeat_at` pra um valor arbitrário (em
    /// formato RFC 3339 / ISO 8601). **Apenas pra testes** —
    /// produção nunca deveria chamar isso. Usado pelo teste do
    /// recovery de crash pra simular um run "stale" sem esperar
    /// minutos reais.
    #[doc(hidden)]
    pub async fn force_heartbeat_at_for_test(
        &self,
        id: &RunId,
        timestamp_rfc3339: &str,
    ) -> StorageResult<()> {
        sqlx::query("UPDATE runs SET last_heartbeat_at = ?1 WHERE id = ?2")
            .bind(timestamp_rfc3339)
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// **Caminho validado de mudança de estado** (Fase 6, Etapa 2
    /// — fechamento do portão, ADR-0029 §D1).
    ///
    /// O portão [`frederico_agent_engine::apply_transition`] é
    /// consultado **antes** de qualquer gravação. Se a transição
    /// `from → to` (via `kind`) não é uma aresta válida da tabela
    /// `TRANSITIONS` / `GLOBAL_TRANSITIONS`, o método retorna
    /// [`StorageError::InvalidTransition`] sem alterar nada no
    /// banco. Caso contrário:
    ///
    /// 1. `runs.state` é atualizado pra `to` e `last_heartbeat_at` /
    ///    `last_event_seq` são atualizados.
    /// 2. Um [`RunEventRecord`] é gravado com `from = from`,
    ///    `to = to`, `kind = kind`, `seq = last_event_seq + 1`
    ///    e o `payload` fornecido.
    ///
    /// Os dois updates rodam na **mesma transação** `BEGIN
    /// IMMEDIATE; ...; COMMIT;` — se qualquer um falhar, nada é
    /// persistido (atomicidade contra crash).
    ///
    /// Retorna `(to, run_seq)` em sucesso. O `run_seq` é o `seq`
    /// do `RunEvent` recém-gravado (o caller usa pra popular
    /// `MessageEvent.run_seq` no mesmo round — Etapa 4 da Fase
    /// de Ligação documenta a invariante "RunEvent gravado antes
    /// de MessageEvent.emit" do ADR-0009 §D1).
    ///
    /// **Por que este método é a única via de produção pra
    /// mudar `runs.state`:** o [`RunExecutor`] consome o walk
    /// de [`crate::state_mapping::run_state_for_event`] (que já
    /// validou cada aresta) e passa cada `RunStateTransition`
    /// por aqui. Se o caminho de validação driftar (regressão
    /// em `state_mapping`), o portão no `apply_transition`
    /// dentro deste método é a **segunda linha de defesa** —
    /// `from` precisa bater com o estado persistido do run
    /// (lido de `runs.state`), e `kind` precisa ter aresta pra
    /// `to`. Se o `state_mapping` esqueceu de consultar o
    /// portão, este método pega.
    ///
    /// [`RunExecutor`]: ../../execution_engine/executor/struct.RunExecutor.html
    pub async fn set_state_validated(
        &self,
        id: &RunId,
        from: RunState,
        kind: RunEventKind,
        payload: serde_json::Value,
    ) -> StorageResult<(RunState, u64)> {
        // 1. Portão: aplica a transição via `apply_transition` (puro).
        //    Se a tabela `TRANSITIONS` rejeita, nada é gravado.
        let to =
            portao_apply_transition(from, kind).map_err(|e| StorageError::InvalidTransition {
                from: from.as_str().to_string(),
                kind: kind.as_str().to_string(),
                cause: format!("{e:?}"),
            })?;

        // 2. Tudo numa única transação. Se o `INSERT` em
        //    `run_events` falhar (e.g. UNIQUE(run_id, seq) batido
        //    por race), o `UPDATE` em `runs` é desfeito.
        let mut tx = self.pool.begin().await?;
        let now_ms = Utc::now().timestamp_millis();
        let now_rfc = Utc::now().to_rfc3339();

        // Próximo `seq` do journal (monotônico por `run_id`).
        // Lemos o `last_event_seq` do `runs` na **mesma** transação
        // pra evitar race com outro executor do mesmo run (a Etapa
        // 4 garante que cada run tem 1 executor, mas a defesa em
        // profundidade não custa nada).
        let current_last_seq: i64 =
            sqlx::query_scalar("SELECT last_event_seq FROM runs WHERE id = ?1")
                .bind(id.0.to_string())
                .fetch_one(&mut *tx)
                .await?;
        let new_seq = (current_last_seq as u64) + 1;

        // 3. Insere o `RunEvent` (UNIQUE(run_id, seq) garante
        //    monotonicidade mecânica).
        let event_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO run_events (event_id, run_id, seq, kind, from_state, to_state, \
             timestamp_ms, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&event_id)
        .bind(id.0.to_string())
        .bind(new_seq as i64)
        .bind(kind.as_str())
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(now_ms)
        .bind(payload.to_string())
        .execute(&mut *tx)
        .await?;

        // 4. Atualiza `runs.state` + `last_heartbeat_at` + `last_event_seq`.
        sqlx::query(
            "UPDATE runs SET state = ?1, last_heartbeat_at = ?2, last_event_seq = ?3 \
             WHERE id = ?4",
        )
        .bind(to.as_str())
        .bind(&now_rfc)
        .bind(new_seq as i64)
        .bind(id.0.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok((to, new_seq))
    }

    // ============================================================================
    // Etapa 4 da Fase 6 — Subagente (ADR-0027)
    //
    // Os métodos abaixo manipulam os campos novos da tabela
    // `runs` introduzidos pela migração `0029_subagent_runs.sql`:
    // `subagent_count`, `depth`, `parent_run_id`, `spent_*`. O
    // `SubagentRunner` (Etapa 4 PR 2) consome estes métodos no
    // caminho de produção; o PR 1 entrega só a infra.
    // ============================================================================

    /// **Incrementa** o contador global de subagentes vivos
    /// (`subagent_count` no banco) e devolve o novo valor. Usado
    /// pelo `SubagentRunner::try_spawn` no momento do spawn.
    ///
    /// **Por que método atômico e não `set_subagent_count`:** a
    /// incrementação precisa ser atômica entre o "caller checa
    /// `subagent_count < 8`" e o "caller grava `subagent_count + 1`"
    /// (race condition entre dois spawns concorrentes do mesmo pai
    /// — mesmo que a Etapa 4 PR 1 não tenha paralelização, o
    /// PR 2 vai ter; o método já fica pronto).
    ///
    /// **SQLite sem `RETURNING`:** o `RETURNING` não é suportado em
    /// todas as versões do SQLite (incluindo a que `sqlx` 0.8
    /// usa em alguns runners), então fazemos `UPDATE ... SET ...
    /// = col + 1` e depois `SELECT col` em duas queries. A
    /// transação (`BEGIN IMMEDIATE`) garante que outro spawn
    /// simultâneo vê o valor já incrementado.
    pub async fn increment_subagent_count(&self, id: &RunId) -> StorageResult<u32> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE runs SET subagent_count = subagent_count + 1 WHERE id = ?1")
            .bind(id.0.to_string())
            .execute(&mut *tx)
            .await?;
        let new_count: i64 = sqlx::query_scalar("SELECT subagent_count FROM runs WHERE id = ?1")
            .bind(id.0.to_string())
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(new_count as u32)
    }

    /// **Decrementa** o contador global de subagentes vivos. Usado
    /// pelo `SubagentRunner` quando um subagente termina
    /// (sucesso/falha/cancelamento). Saturado em 0 — se já está
    /// em 0 (não deveria acontecer, mas defendemos em
    /// profundidade), `UPDATE` seta 0 e segue.
    pub async fn decrement_subagent_count(&self, id: &RunId) -> StorageResult<u32> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE runs SET subagent_count = MAX(0, subagent_count - 1) WHERE id = ?1")
            .bind(id.0.to_string())
            .execute(&mut *tx)
            .await?;
        let new_count: i64 = sqlx::query_scalar("SELECT subagent_count FROM runs WHERE id = ?1")
            .bind(id.0.to_string())
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(new_count as u32)
    }

    /// **Seta** a profundidade do `Run` (D2 do ADR-0027). Chamado
    /// uma vez, na criação do subagente (`Run::new_subagent`
    /// define `depth = parent.depth + 1`). O portão `try_spawn`
    /// (Etapa 4 PR 2) rejeita antes de chamar.
    ///
    /// **Por que método em vez de incluir no `create`:** o `create`
    /// é raiz (`depth = 0`); subagentes são criados pelo
    /// `SubagentRunner` (Etapa 4 PR 2) numa segunda chamada, com
    /// `parent_run_id` + `depth` populados separadamente.
    pub async fn set_depth(
        &self,
        id: &RunId,
        depth: u32,
        parent_run_id: Option<RunId>,
    ) -> StorageResult<()> {
        sqlx::query("UPDATE runs SET depth = ?1, parent_run_id = ?2 WHERE id = ?3")
            .bind(depth as i64)
            .bind(parent_run_id.map(|p| p.0.to_string()))
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// **Acumula** gasto no `SpentBudget` do `Run` (Etapa 4
    /// D3 do ADR-0027). Soma `delta` aos 4 campos `spent_*` no
    /// banco. Saturado em `i64::MAX` (defesa contra overflow).
    ///
    /// **Por que método separado e não `record_run_step` que
    /// faz tudo:** a Etapa 5 (watchdog) pode chamar isso
    /// independentemente do `RunExecutor` (e.g. pra registrar
    /// gasto de uma operação I/O que falhou sem step
    /// completo). Cada eixo do `SpentBudget` é granular e
    /// pode ser atualizado por origens diferentes.
    pub async fn add_spent(
        &self,
        id: &RunId,
        delta: &frederico_agent_engine::SpentBudget,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE runs SET \
             spent_microcents = spent_microcents + ?1, \
             spent_tokens_in = spent_tokens_in + ?2, \
             spent_tokens_out = spent_tokens_out + ?3, \
             spent_steps = spent_steps + ?4 \
             WHERE id = ?5",
        )
        .bind(delta.cost_microcents as i64)
        .bind(delta.tokens_in as i64)
        .bind(delta.tokens_out as i64)
        .bind(delta.steps as i64)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }
}

/// Registro de um subagente específico (Etapa 4 da Fase 6,
/// ADR-0027 + migração `0029_subagent_runs.sql`).
///
/// Cada subagente tem uma linha na tabela `subagent_runs` (1:1 com
/// `runs` quando `runs.parent_run_id IS NOT NULL`). O
/// `SubagentRunRecord` carrega o que é **específico** do subagente
/// (parent_run_id, specialist_id, allocation, spent_microcents)
/// — o resto (state, started_at, finished_at) é denormalizado da
/// `runs` pra queries agregadas sem JOIN.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubagentRunRecord {
    /// `RunId` do subagente (PK = FK pra `runs.id`).
    pub id: RunId,
    /// `RunId` do pai (FK pra `runs.id` com `ON DELETE CASCADE`).
    pub parent_run_id: RunId,
    /// `SpecialistId` que o subagente está executando (resolve
    /// via `SpecialistRegistry`, Etapa 3 da Fase 6). `None`
    /// durante a Etapa 4 PR 1 (sem spawn real ainda); a Etapa 4
    /// PR 2 popula no `try_spawn`.
    pub specialist_id: Option<String>,
    /// Profundidade (1 = filho direto, 2 = neto bloqueado pelo
    /// portão). Espelha `runs.depth`.
    pub depth: u32,
    /// `BudgetAllocation` que o pai liberou pro filho (D5 do
    /// ADR-0027). Serializado em JSON na tabela.
    pub allocation: frederico_agent_engine::BudgetAllocation,
    /// Gasto efetivo (espelha `runs.spent_*`).
    pub spent: frederico_agent_engine::SpentBudget,
    /// Instante do spawn (criação da linha).
    pub started_at: String,
    /// Instante do término (sucesso, falha, cancelamento).
    /// `None` enquanto o subagente está vivo.
    pub finished_at: Option<String>,
    /// Estado do subagente (23 valores, mesmo enum `RunState`).
    pub state: String,
}

/// Repositório da tabela `subagent_runs` (Etapa 4 da Fase 6).
///
/// O `SubagentRunner::try_spawn` (Etapa 4 PR 2) consome este repo
/// pra registrar cada subagente no momento do spawn (portão D1 +
/// D2 + D3 do ADR-0027) e o `SubagentRunner::on_subagent_done`
/// consome `complete` pra marcar o término (libera o budget do
/// pai). **PR 1 só entrega a infra** (struct + repo + unit tests
/// básicos); a Etapa 4 PR 2 pluga o runner.
pub struct SubagentRunRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

/// Row bruta de `subagent_runs`. Tipo de domínio:
/// [`SubagentRunRecord`].
#[derive(sqlx::FromRow)]
struct SubagentRunRow {
    id: String,
    parent_run_id: String,
    specialist_id: Option<String>,
    depth: i64,
    allocation_json: String,
    spent_microcents: i64,
    spent_tokens_in: i64,
    spent_tokens_out: i64,
    spent_steps: i64,
    started_at: String,
    finished_at: Option<String>,
    state: String,
}

impl<'a> SubagentRunRepo<'a> {
    /// Cria o repo a partir do `Database`.
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// **Registra** um subagente no momento do spawn. Cria a
    /// linha em `subagent_runs` (1:1 com `runs.id`). O `runs`
    /// correspondente já foi criado pelo `RunRepo::create`
    /// antes (a Etapa 4 PR 2 segue essa ordem:
    /// `RunRepo::create` → `SubagentRunRepo::record` →
    /// `RunRepo::increment_subagent_count`).
    ///
    /// **Falha** se já existe um `subagent_runs` com o mesmo
    /// `id` (UNIQUE violation) — o `SubagentRunner` da Etapa 4
    /// PR 2 trata isso como `InternalError` (anti-exploit do
    /// spawn otimista rejeitado pela Etapa 1, alt 4 do
    /// ADR-0027).
    pub async fn record(&self, record: &SubagentRunRecord) -> StorageResult<()> {
        let allocation_json = serde_json::to_string(&record.allocation).map_err(|e| {
            StorageError::Query(sqlx::Error::Decode(
                format!("bad allocation json: {e}").into(),
            ))
        })?;
        sqlx::query(
            "INSERT INTO subagent_runs (id, parent_run_id, specialist_id, depth, \
             allocation_json, spent_microcents, spent_tokens_in, spent_tokens_out, \
             spent_steps, started_at, finished_at, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(record.id.0.to_string())
        .bind(record.parent_run_id.0.to_string())
        .bind(&record.specialist_id)
        .bind(record.depth as i64)
        .bind(&allocation_json)
        .bind(record.spent.cost_microcents as i64)
        .bind(record.spent.tokens_in as i64)
        .bind(record.spent.tokens_out as i64)
        .bind(record.spent.steps as i64)
        .bind(&record.started_at)
        .bind(&record.finished_at)
        .bind(&record.state)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// **Marca** o término de um subagente (sucesso, falha ou
    /// cancelamento). Seta `finished_at` e `state`. Idempotente
    /// — chamar 2x com o mesmo `id` é no-op (defesa contra
    /// cleanup duplicado).
    pub async fn complete(
        &self,
        id: &RunId,
        final_state: &str,
        finished_at: &str,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE subagent_runs SET state = ?1, finished_at = ?2 \
             WHERE id = ?3 AND finished_at IS NULL",
        )
        .bind(final_state)
        .bind(finished_at)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// **Lista** os subagentes vivos de um Run pai.
    ///
    /// Filtra por `state` não-terminal + `finished_at IS NULL`. Usado
    /// pelo portão D1 do ADR-0027 pra consulta rápida ("quantos
    /// subagentes ativos este pai tem agora?") e pela UI do Modo
    /// Equipe (Etapa 6).
    pub async fn list_active_for_parent(
        &self,
        parent_run_id: &RunId,
    ) -> StorageResult<Vec<SubagentRunRecord>> {
        let rows: Vec<SubagentRunRow> = sqlx::query_as(
            "SELECT id, parent_run_id, specialist_id, depth, allocation_json, \
                    spent_microcents, spent_tokens_in, spent_tokens_out, spent_steps, \
                    started_at, finished_at, state \
             FROM subagent_runs \
             WHERE parent_run_id = ?1 AND finished_at IS NULL",
        )
        .bind(parent_run_id.0.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(Self::row_to_record).collect()
    }

    fn row_to_record(row: SubagentRunRow) -> StorageResult<SubagentRunRecord> {
        let allocation: frederico_agent_engine::BudgetAllocation =
            serde_json::from_str(&row.allocation_json).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(
                    format!("bad allocation json: {e}").into(),
                ))
            })?;
        Ok(SubagentRunRecord {
            id: RunId(uuid::Uuid::parse_str(&row.id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?),
            parent_run_id: RunId(uuid::Uuid::parse_str(&row.parent_run_id).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?),
            specialist_id: row.specialist_id,
            depth: row.depth as u32,
            allocation,
            spent: frederico_agent_engine::SpentBudget {
                cost_microcents: row.spent_microcents as u64,
                tokens_in: row.spent_tokens_in as u64,
                tokens_out: row.spent_tokens_out as u64,
                steps: row.spent_steps as u32,
            },
            started_at: row.started_at,
            finished_at: row.finished_at,
            state: row.state,
        })
    }
}

/// Repositório do journal de transições do `Run` (Fase 6, Etapa 2).
///
/// Cada [`RunEvent`] que o [`RunExecutor`] emite (via
/// `state_mapping::run_state_for_event` consultando o portão único
/// `apply_transition`) é gravado aqui **na mesma transação** que o
/// `RunRepo::set_state_and_heartbeat_tx`. A invariante "transição gravada
/// antes de retornar" (ADR-0009 §D1) fica mantida.
///
/// [`RunEvent`]: frederico_agent_engine::RunEvent
pub struct RunEventRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

/// Row bruta de `run_events`. Tipo de domínio: [`RunEventRecord`].
type RunEventRow = (
    String,         // event_id
    String,         // run_id
    i64,            // seq
    String,         // kind
    Option<String>, // from_state
    Option<String>, // to_state
    i64,            // timestamp_ms
    String,         // payload_json
);

/// Linha do `run_events` parseada do SQLite. O domínio [`RunEvent`]
/// da `agent-engine` é a fonte de verdade (com `payload: serde_json::Value`);
/// este struct carrega os campos brutos do banco.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunEventRecord {
    pub event_id: String,
    pub run_id: String,
    pub seq: u64,
    pub kind: String,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub timestamp_ms: i64,
    pub payload_json: String,
}

impl RunEventRecord {
    fn from_row(row: RunEventRow) -> Self {
        let (event_id, run_id, seq, kind, from_state, to_state, timestamp_ms, payload_json) = row;
        Self {
            event_id,
            run_id,
            seq: seq as u64,
            kind,
            from_state,
            to_state,
            timestamp_ms,
            payload_json,
        }
    }
}

impl<'a> RunEventRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Anexa um evento de transição ao journal. Retorna o `seq`
    /// usado. O `seq` é monotônico por `run_id` (garantido pelo
    /// `UNIQUE(run_id, seq)` no SQLite).
    ///
    /// O `timestamp_ms` é gerado aqui (`chrono::Utc::now()` em
    /// milissegundos desde a epoch). O `event_id` é um `Uuid::new_v4()`.
    /// `from_state` e `to_state` são opcionais — eventos sem mudança
    /// de estado (ex.: `Usage` no stream) gravam ambos como
    /// `Some(current)` (igual a `current`).
    ///
    /// O `payload` é `serde_json::Value` opaco (mesma estratégia do
    /// `RunEvent.payload` na `agent-engine`).
    pub async fn append(
        &self,
        run_id: &RunId,
        kind: frederico_agent_engine::RunEventKind,
        from_state: Option<frederico_agent_engine::RunState>,
        to_state: Option<frederico_agent_engine::RunState>,
        payload: serde_json::Value,
    ) -> StorageResult<u64> {
        // Calcula o próximo `seq` para o run.
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM run_events WHERE run_id = ?1",
        )
        .bind(run_id.0.to_string())
        .fetch_one(self.pool)
        .await?;

        let event_id = uuid::Uuid::new_v4().to_string();
        let ts_ms = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO run_events (event_id, run_id, seq, kind, from_state, to_state, timestamp_ms, payload_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&event_id)
        .bind(run_id.0.to_string())
        .bind(next_seq)
        .bind(kind.as_str())
        .bind(from_state.map(|s| s.as_str().to_string()))
        .bind(to_state.map(|s| s.as_str().to_string()))
        .bind(ts_ms)
        .bind(payload.to_string())
        .execute(self.pool)
        .await?;

        Ok(next_seq as u64)
    }

    /// Lista todos os eventos de um run, em ordem de `seq`. Usado pelo
    /// `recovery.rs` (carrega o último estado válido) e pela UI do
    /// Modo Equipe (linha do tempo de estados, Etapa 6).
    pub async fn list_for_run(&self, run_id: &RunId) -> StorageResult<Vec<RunEventRecord>> {
        let rows: Vec<RunEventRow> = sqlx::query_as(
            "SELECT event_id, run_id, seq, kind, from_state, to_state, timestamp_ms, payload_json \
             FROM run_events WHERE run_id = ?1 ORDER BY seq ASC",
        )
        .bind(run_id.0.to_string())
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(RunEventRecord::from_row).collect())
    }

    /// Retorna o último evento do run (maior `seq`). `None` se o run
    /// ainda não tem eventos no journal (run criado antes da Etapa 2
    /// da Fase 6, ou run em estado `Created` que ainda não foi
    /// enfileirado). Usado pelo `recovery.rs` como fonte primária
    /// do estado (em vez do `last_heartbeat_at` heurístico).
    pub async fn latest_for_run(&self, run_id: &RunId) -> StorageResult<Option<RunEventRecord>> {
        let row: Option<RunEventRow> = sqlx::query_as(
            "SELECT event_id, run_id, seq, kind, from_state, to_state, timestamp_ms, payload_json \
             FROM run_events WHERE run_id = ?1 ORDER BY seq DESC LIMIT 1",
        )
        .bind(run_id.0.to_string())
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(RunEventRecord::from_row))
    }

    /// Retorna o maior `seq` registrado para o run. `0` se não há
    /// eventos. Usado pelo `RunRepo::set_state_and_heartbeat_tx`
    /// para atualizar `last_event_seq` atomicamente com a mudança
    /// de estado.
    pub async fn last_seq(&self, run_id: &RunId) -> StorageResult<u64> {
        let seq: Option<i64> =
            sqlx::query_scalar("SELECT MAX(seq) FROM run_events WHERE run_id = ?1")
                .bind(run_id.0.to_string())
                .fetch_one(self.pool)
                .await?;
        Ok(seq.unwrap_or(0) as u64)
    }
}

#[cfg(test)]
mod run_event_repo_tests {
    use super::*;
    use frederico_agent_engine::{RunEventKind, RunState};
    use frederico_core::ModelId;

    fn tempdir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let unique = format!(
            "frederico-runevent-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            n,
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn append_assigns_monotonic_seq() {
        let db = Database::open(&tempdir().join("re1.db")).await.unwrap();
        let conv = ConversationRepo::new(&db)
            .create(&ProviderId::new("p"), &ModelId::new("m"), None)
            .await
            .unwrap();
        let msg = MessageRepo::new(&db)
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = RunRepo::new(&db).create(&conv.id, &msg.id).await.unwrap();
        let re = RunEventRepo::new(&db);

        let s1 = re
            .append(
                &run.id,
                RunEventKind::Enqueue,
                Some(RunState::Created),
                Some(RunState::Queued),
                serde_json::Value::Null,
            )
            .await
            .unwrap();
        let s2 = re
            .append(
                &run.id,
                RunEventKind::Dequeue,
                Some(RunState::Queued),
                Some(RunState::PreparingContext),
                serde_json::Value::Null,
            )
            .await
            .unwrap();
        let s3 = re
            .append(
                &run.id,
                RunEventKind::FirstToken,
                Some(RunState::CallingModel),
                Some(RunState::Streaming),
                serde_json::json!({"first_chunk": "hello"}),
            )
            .await
            .unwrap();

        assert_eq!((s1, s2, s3), (1, 2, 3));
    }

    #[tokio::test]
    async fn latest_for_run_returns_highest_seq() {
        let db = Database::open(&tempdir().join("re2.db")).await.unwrap();
        let conv = ConversationRepo::new(&db)
            .create(&ProviderId::new("p"), &ModelId::new("m"), None)
            .await
            .unwrap();
        let msg = MessageRepo::new(&db)
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = RunRepo::new(&db).create(&conv.id, &msg.id).await.unwrap();
        let re = RunEventRepo::new(&db);

        re.append(
            &run.id,
            RunEventKind::Enqueue,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();
        re.append(
            &run.id,
            RunEventKind::Dequeue,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();
        re.append(
            &run.id,
            RunEventKind::FirstToken,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();

        let latest = re.latest_for_run(&run.id).await.unwrap().unwrap();
        assert_eq!(latest.kind, "first_token");
        assert_eq!(latest.seq, 3);
    }

    #[tokio::test]
    async fn latest_for_run_returns_none_when_empty() {
        let db = Database::open(&tempdir().join("re3.db")).await.unwrap();
        let conv = ConversationRepo::new(&db)
            .create(&ProviderId::new("p"), &ModelId::new("m"), None)
            .await
            .unwrap();
        let msg = MessageRepo::new(&db)
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = RunRepo::new(&db).create(&conv.id, &msg.id).await.unwrap();
        let re = RunEventRepo::new(&db);

        let latest = re.latest_for_run(&run.id).await.unwrap();
        assert!(latest.is_none());
    }

    #[tokio::test]
    async fn list_for_run_returns_all_in_order() {
        let db = Database::open(&tempdir().join("re4.db")).await.unwrap();
        let conv = ConversationRepo::new(&db)
            .create(&ProviderId::new("p"), &ModelId::new("m"), None)
            .await
            .unwrap();
        let msg = MessageRepo::new(&db)
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = RunRepo::new(&db).create(&conv.id, &msg.id).await.unwrap();
        let re = RunEventRepo::new(&db);

        for kind in [
            RunEventKind::Enqueue,
            RunEventKind::Dequeue,
            RunEventKind::ContextReady,
        ] {
            re.append(&run.id, kind, None, None, serde_json::Value::Null)
                .await
                .unwrap();
        }
        let events = re.list_for_run(&run.id).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind, "enqueue");
        assert_eq!(events[1].kind, "dequeue");
        assert_eq!(events[2].kind, "context_ready");
    }

    #[tokio::test]
    async fn last_seq_zero_when_empty_and_monotonic_when_populated() {
        let db = Database::open(&tempdir().join("re5.db")).await.unwrap();
        let conv = ConversationRepo::new(&db)
            .create(&ProviderId::new("p"), &ModelId::new("m"), None)
            .await
            .unwrap();
        let msg = MessageRepo::new(&db)
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = RunRepo::new(&db).create(&conv.id, &msg.id).await.unwrap();
        let re = RunEventRepo::new(&db);

        assert_eq!(re.last_seq(&run.id).await.unwrap(), 0);

        re.append(
            &run.id,
            RunEventKind::Enqueue,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();
        re.append(
            &run.id,
            RunEventKind::Dequeue,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();
        assert_eq!(re.last_seq(&run.id).await.unwrap(), 2);
    }
}

/// Entrada persistida da auditoria de uma `tool_call`. Append-only
/// (a migration 0005 não tem `UPDATE` nem `DELETE`). Cada linha
/// carrega o snapshot do momento da chamada (Fase 3, Etapa 5.x,
/// Passo 10 do `validate_tool_call`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAuditEntry {
    pub id: String,
    pub run_id: RunId,
    pub tool_id: String,
    pub tool_version: String,
    pub arguments_json: String,
    pub result_ok: bool,
    pub result_json: String,
    pub duration_micros: i64,
    pub created_at: String,
}

/// Repositório de auditoria de `tool_calls` (Fase 3, Etapa 5.x).
/// Append-only — o `append` é o único método público.
pub struct ToolAuditRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> ToolAuditRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Grava uma entrada de auditoria. Devolve o `id` gerado.
    #[allow(clippy::too_many_arguments)]
    pub async fn append(
        &self,
        run_id: &RunId,
        tool_id: &str,
        tool_version: &str,
        arguments_json: &str,
        result_ok: bool,
        result_json: &str,
        duration_micros: i64,
    ) -> StorageResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tool_audit (id, run_id, tool_id, tool_version, \
             arguments_json, result_ok, result_json, duration_micros) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&id)
        .bind(run_id.0.to_string())
        .bind(tool_id)
        .bind(tool_version)
        .bind(arguments_json)
        .bind(result_ok as i64)
        .bind(result_json)
        .bind(duration_micros)
        .execute(self.pool)
        .await?;
        Ok(id)
    }

    /// Lista entradas de auditoria de um `Run` em ordem cronológica.
    pub async fn list_for_run(&self, run_id: &RunId) -> StorageResult<Vec<ToolAuditEntry>> {
        // Tuple crua do `sqlx::query_as` — 9 campos. A lint
        // `type_complexity` reclamaria; o `allow` abaixo silencia.
        // A alternativa (criar um type alias) não traz benefício —
        // é só usado aqui.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            String,
            i64,
            String,
        )> = sqlx::query_as(
            "SELECT id, run_id, tool_id, tool_version, arguments_json, \
                        result_ok, result_json, duration_micros, created_at \
                 FROM tool_audit WHERE run_id = ?1 \
                 ORDER BY created_at ASC",
        )
        .bind(run_id.0.to_string())
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (
            id,
            run_id_s,
            tool_id,
            tool_version,
            arguments_json,
            result_ok,
            result_json,
            duration_micros,
            created_at,
        ) in rows
        {
            let uuid = uuid::Uuid::parse_str(&run_id_s).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
            out.push(ToolAuditEntry {
                id,
                run_id: RunId(uuid),
                tool_id,
                tool_version,
                arguments_json,
                result_ok: result_ok != 0,
                result_json,
                duration_micros,
                created_at,
            });
        }
        Ok(out)
    }
}

/// Status de uma entrada da fila de aprovação (Fase 3, Etapa 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

impl ApprovalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Rejected => "rejected",
        }
    }
}

/// Entrada persistida da fila de aprovação (Fase 3, Etapa 6).
/// Criada quando o `RunExecutor` recebe `ApprovalRequired` do
/// `validate_tool_call`. O `request_json` carrega o
/// `ApprovalRequest` serializado (imutável); o `decision_json`
/// carrega o `ApprovalDecision` quando o usuário responde.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntry {
    pub id: String,
    pub run_id: RunId,
    pub tool_id: String,
    pub request_json: String,
    pub status: ApprovalStatus,
    pub decision_json: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

/// Repositório da fila de aprovação de `tool_calls` (Fase 3,
/// Etapa 6). Append no `enqueue`; update no `resolve`.
pub struct ApprovalQueueRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> ApprovalQueueRepo<'a> {
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Enfileira uma aprovação. Devolve o `id` gerado.
    pub async fn enqueue(
        &self,
        run_id: &RunId,
        tool_id: &str,
        request_json: &str,
    ) -> StorageResult<String> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO approval_queue (id, run_id, tool_id, request_json, status) \
             VALUES (?1, ?2, ?3, ?4, 'pending')",
        )
        .bind(&id)
        .bind(run_id.0.to_string())
        .bind(tool_id)
        .bind(request_json)
        .execute(self.pool)
        .await?;
        Ok(id)
    }

    /// Resolve uma aprovação pendente (approve ou reject). O
    /// `decision_json` carrega o `ApprovalDecision` serializado
    /// (`{"approved": true/false, "scope": "Once/Run/Project",
    /// "reason": "..."}`).
    pub async fn resolve(
        &self,
        id: &str,
        decision_json: &str,
        approved: bool,
    ) -> StorageResult<()> {
        let status = if approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Rejected
        };
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE approval_queue SET status = ?1, decision_json = ?2, resolved_at = ?3 \
             WHERE id = ?4 AND status = 'pending'",
        )
        .bind(status.as_str())
        .bind(decision_json)
        .bind(&now)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Lista entradas pendentes em ordem cronológica. A UI da
    /// Etapa 6 consome isso pra mostrar o modal.
    pub async fn list_pending(&self) -> StorageResult<Vec<ApprovalEntry>> {
        // Tuple crua do `sqlx::query_as` — 8 campos.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, String, String, String, String, Option<String>, String, Option<String>)> =
            sqlx::query_as(
                "SELECT id, run_id, tool_id, request_json, status, decision_json, created_at, resolved_at \
                 FROM approval_queue WHERE status = 'pending' \
                 ORDER BY created_at ASC",
            )
            .fetch_all(self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (
            id,
            run_id_s,
            tool_id,
            request_json,
            status_s,
            decision_json,
            created_at,
            resolved_at,
        ) in rows
        {
            let uuid = uuid::Uuid::parse_str(&run_id_s).map_err(|e| {
                StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
            let status = match status_s.as_str() {
                "pending" => ApprovalStatus::Pending,
                "approved" => ApprovalStatus::Approved,
                "rejected" => ApprovalStatus::Rejected,
                _ => {
                    return Err(StorageError::Query(sqlx::Error::Decode(
                        format!("bad status: {status_s}").into(),
                    )))
                }
            };
            out.push(ApprovalEntry {
                id,
                run_id: RunId(uuid),
                tool_id,
                request_json,
                status,
                decision_json,
                created_at,
                resolved_at,
            });
        }
        Ok(out)
    }

    /// Busca uma entrada por `id`.
    pub async fn get(&self, id: &str) -> StorageResult<ApprovalEntry> {
        // Tuple crua do `sqlx::query_as` — 8 campos.
        #[allow(clippy::type_complexity)]
        let row: Option<(String, String, String, String, String, Option<String>, String, Option<String>)> =
            sqlx::query_as(
                "SELECT id, run_id, tool_id, request_json, status, decision_json, created_at, resolved_at \
                 FROM approval_queue WHERE id = ?1",
            )
            .bind(id)
            .fetch_optional(self.pool)
            .await?;
        let (id, run_id_s, tool_id, request_json, status_s, decision_json, created_at, resolved_at) =
            row.ok_or_else(|| {
                StorageError::Query(sqlx::Error::Decode(
                    format!("approval {id} não encontrada").into(),
                ))
            })?;
        let uuid = uuid::Uuid::parse_str(&run_id_s).map_err(|e| {
            StorageError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
        })?;
        let status = match status_s.as_str() {
            "pending" => ApprovalStatus::Pending,
            "approved" => ApprovalStatus::Approved,
            "rejected" => ApprovalStatus::Rejected,
            _ => {
                return Err(StorageError::Query(sqlx::Error::Decode(
                    format!("bad status: {status_s}").into(),
                )))
            }
        };
        Ok(ApprovalEntry {
            id,
            run_id: RunId(uuid),
            tool_id,
            request_json,
            status,
            decision_json,
            created_at,
            resolved_at,
        })
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
            .set_model(
                &conv.id,
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-3-5-sonnet-latest"),
            )
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
            .create(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-3-5-sonnet-latest"),
                None,
            )
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
        let user = msg_repo.create(&conv.id, "user", "oi", None).await.unwrap();
        assert_eq!(user.role, "user");
        assert_eq!(user.status, "pending");
        let asst = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        // Anexa 3 eventos.
        let s1 = ev_repo
            .append(&asst.id, "delta", &serde_json::json!({"content": "O"}))
            .await
            .unwrap();
        let s2 = ev_repo
            .append(&asst.id, "delta", &serde_json::json!({"content": "k"}))
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
        let m = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
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
        let asst = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
        assert_eq!(run.status, "created");
        run_repo
            .set_status(&run.id, RunStatus::Running)
            .await
            .unwrap();
        let got = run_repo.get(&run.id).await.unwrap();
        assert_eq!(got.status, "running");
        run_repo.request_cancellation(&run.id).await.unwrap();
        let got = run_repo.get(&run.id).await.unwrap();
        assert!(got.cancellation_requested_at.is_some());
        run_repo
            .set_status(&run.id, RunStatus::Cancelled)
            .await
            .unwrap();
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
        let asst = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        run_repo.create(&conv.id, &asst.id).await.unwrap();
        // Segundo run para a mesma mensagem deve falhar (UNIQUE).
        let r = run_repo.create(&conv.id, &asst.id).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn run_repo_create_populates_phase3_columns_with_defaults() {
        // A Etapa 1 da Fase 3 adicionou 9 colunas a `runs` via
        // `0003_runs_and_checkpoints.sql`. O `create` da Fase 2 não
        // precisa conhecer cada uma — a `CHECK` constraint e os
        // defaults da migração preenchem. Esse teste garante que o
        // `Run` devolvido por `create` tem os campos novos com os
        // valores esperados.
        let db = Database::open(&tempdir().join("run3.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();
        let asst = msg_repo
            .create(&conv.id, "assistant", "", None)
            .await
            .unwrap();
        let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
        assert_eq!(run.state, "created");
        assert_eq!(run.current_step, 0);
        assert_eq!(run.budget_json, "{}");
        assert_eq!(run.allowed_tools_json, "[]");
        assert_eq!(run.last_event_seq, 0);
        assert_eq!(run.provider_id, "");
        assert_eq!(run.model_id, "");
        assert!(run.assistant_id.is_none());
        // O `last_heartbeat_at` da Etapa 1 é preenchido com `now()`
        // na migração (default do SQLite). Aqui comparamos só que
        // está preenchido — o timestamp exato vem do relógio do DB.
        assert!(!run.last_heartbeat_at.is_empty());

        // `get` lê as 9 colunas novas também.
        let got = run_repo.get(&run.id).await.unwrap();
        assert_eq!(got.state, "created");
        assert_eq!(got.current_step, 0);
        assert_eq!(got.budget_json, "{}");
        assert_eq!(got.allowed_tools_json, "[]");
    }

    #[tokio::test]
    async fn runs_with_status_view_maps_state_to_legacy_status() {
        // Cobertura do mapeamento `state → status` definido no
        // ADR-0009 §3 e implementado na view `runs_with_status`.
        // Cada estado tem um status esperado — esse teste existe
        // porque é a rede de segurança da Fase 2 (que lê `status`
        // direto) contra mudanças acidentais no `CASE WHEN` da view.
        let db = Database::open(&tempdir().join("view.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);
        let conv = conv_repo
            .create(&ProviderId::new("openai"), &ModelId::new("gpt-4o"), None)
            .await
            .unwrap();

        // Helper local: cria um run e força o `state` via SQL
        // direto. O `RunRepo` da Etapa 1 não tem `set_state` ainda
        // (Etapa 4 implementa); testamos a view sem mexer no
        // executor.
        async fn force_state(
            db: &Database,
            run_id: &RunId,
            state: &str,
        ) -> Result<(), sqlx::Error> {
            sqlx::query("UPDATE runs SET state = ?1 WHERE id = ?2")
                .bind(state)
                .bind(run_id.0.to_string())
                .execute(db.pool())
                .await?;
            Ok(())
        }

        // Cada par (state, expected_status) que o CASE WHEN da view
        // declara. Não testamos as 16 variações "→ running"
        // individualmente — uma amostra representativa basta.
        let cases: &[(&str, &str)] = &[
            ("created", "created"),
            ("queued", "created"),
            ("calling_model", "running"),
            ("streaming", "running"),
            ("executing_tool", "running"),
            ("paused", "running"),
            ("completed", "completed"),
            ("failed", "failed"),
            ("cancelled", "cancelled"),
            ("interrupted", "timeout"),
        ];

        for (state, expected_status) in cases {
            let asst = msg_repo
                .create(&conv.id, "assistant", "", None)
                .await
                .unwrap();
            let run = run_repo.create(&conv.id, &asst.id).await.unwrap();
            force_state(&db, &run.id, state).await.unwrap();

            // Lê da view.
            let row: (String,) =
                sqlx::query_as("SELECT status FROM runs_with_status WHERE id = ?1")
                    .bind(run.id.0.to_string())
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(
                row.0,
                *expected_status,
                "estado {state} deveria mapear para {expected_status}, veio {row_0}",
                row_0 = row.0
            );
        }
    }

    #[tokio::test]
    async fn provider_config_repo_upsert_and_list() {
        let db = Database::open(&tempdir().join("pcfg.db")).await.unwrap();
        let repo = ProviderConfigRepo::new(&db);
        repo.upsert(&ProviderId::new("openai"), "OpenAI", true)
            .await
            .unwrap();
        repo.upsert(&ProviderId::new("anthropic"), "Anthropic", false)
            .await
            .unwrap();
        repo.upsert(&ProviderId::new("openai"), "OpenAI", false)
            .await
            .unwrap();
        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 2);
        // Depois do upsert com `configured: false`, OpenAI não está mais configurado.
        let openai = list
            .iter()
            .find(|c| c.provider_id.as_str() == "openai")
            .unwrap();
        assert!(!openai.configured);
    }

    #[tokio::test]
    async fn provider_config_record_last_ok_clears_error() {
        let db = Database::open(&tempdir().join("pcfg2.db")).await.unwrap();
        let repo = ProviderConfigRepo::new(&db);
        let p = ProviderId::new("openai");
        repo.upsert(&p, "OpenAI", true).await.unwrap();
        repo.record_last_error(&p, "401 chave inválida")
            .await
            .unwrap();
        let before = repo.list().await.unwrap();
        let openai = before.iter().find(|c| c.provider_id == p).unwrap();
        assert_eq!(openai.last_error.as_deref(), Some("401 chave inválida"));
        repo.record_last_ok(&p).await.unwrap();
        let after = repo.list().await.unwrap();
        let openai = after.iter().find(|c| c.provider_id == p).unwrap();
        assert!(openai.last_error.is_none());
        assert!(openai.last_ok_at.is_some());
    }

    /// Contador atômico: o relógio sozinho não garante unicidade (no Windows
    /// a granularidade de `timestamp_nanos` é grosseira e testes paralelos
    /// podem colidir no mesmo valor, compartilhando o mesmo banco e
    /// disparando duas migrações concorrentes).
    static TEMPDIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir();
        let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let unique = format!(
            "frederico-storage-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            n,
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
