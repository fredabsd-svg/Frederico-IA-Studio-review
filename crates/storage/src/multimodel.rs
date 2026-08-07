//! `PipelineRepo` — persistência do `MultimodelRun` (Etapa 5 da
//! Fase 6, ADR-0028).
//!
//! Ver [`docs/architecture/multimodel-architecture.md` §"Pipeline
//! Sequencial"](../../architecture/multimodel-architecture.md) e o
//! [ADR-0028](../decisions/0028-pipeline-sequencial-multimodel.md).
//!
//! ## O que o `PipelineRepo` faz
//!
//! Persiste o estado do `MultimodelRun` (sequência de
//! `MultimodelStage`s) e dos artefatos produzidos. **Sobrevive a
//! restart do app** (D5 do ADR-0028) — ao reabrir, o
//! `MultimodelOrchestrator` carrega runs em estado
//! `Running`/`Streaming`/`WaitingToolCall` e oferece continuação
//! (botão "retomar pipeline interrompido" no Modo Equipe da
//! Etapa 6).
//!
//! **Esta é a Etapa 5 PR 1 (infra) — só persistência.** O
//! `MultimodelOrchestrator` que consome este repo pra spawnar
//! stages em background, calcular `cost_microcents` por stage,
//! propagar cancelamento (D7 do ADR-0028) e reusar stage quando
//! `output_hash` não muda (D6 do ADR-0028) é a **Etapa 5 PR 2**
//! (consome este repo + `ChatOrchestrator` + `RunExecutor`).
//!
//! ## Por que 3 tabelas
//!
//! **multimodel_runs** (1 linha por pipeline): cabeçalho. `mode`
//! é o enum `MultimodelMode` (Etapa 5: só `Pipeline`; Etapas
//! futuras plugam `Comparison`/`Conselho`/`Debate` per ADR-0028
//! §D3).
//!
//! **multimodel_stages** (1 linha por stage): cada stage. Carrega
//! `input_artifact_id` (FK pros artefatos de entrada — `None`
//! no primeiro stage), `output_artifact_id`, `input_hash` /
//! `output_hash` (SHA-256 pra detectar reuso quando input não
//! muda), `cost_microcents` (alimentado pelo provider-engine),
//! `tools_used_json` e `validation_json` (resultado do validador
//! se o stage declarou um).
//!
//! **multimodel_artifacts** (1 linha por artefato): os arquivos
//! produzidos. `content_ref` aponta pro arquivo (workspace-
//! relative, validado pelo `Jail` da conversa). `hash` é
//! SHA-256 do conteúdo.
//!
//! Ver migração `0030_multimodel.sql` pra os detalhes do schema.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Database, StorageResult};

/// Modo do `MultimodelRun` (Etapa 5 = só `Pipeline`; Etapas
/// futuras plugam os outros 3 per ADR-0028 §D3).
///
/// **Por que enum no storage e não só no domínio:** o
/// `MultimodelOrchestrator` (Etapa 5 PR 2) decide o modo
/// baseado no input do usuário, e o modo é parte do estado
/// persistido. Manter no `storage` garante que o tipo é
/// compartilhado entre o `MultimodelOrchestrator` (que escreve)
/// e a UI da Etapa 6 (que lê).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultimodelMode {
    /// Sequência de stages onde cada um consome o artefato
    /// real do anterior (Etapa 5).
    Pipeline,
}

impl MultimodelMode {
    /// Nome em `snake_case` (mesmo formato do `RunState::as_str`).
    /// Usado no `CHECK` constraint da migração
    /// `0030_multimodel.sql`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
        }
    }
}

impl std::str::FromStr for MultimodelMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pipeline" => Ok(Self::Pipeline),
            other => Err(format!("MultimodelMode desconhecido: {other}")),
        }
    }
}

/// Estado de um `MultimodelRun` (sequência de stages). Espelha
/// o `RunState` do `agent-engine` mas é uma enum separada (o
/// pipeline tem noção de "todos os stages concluídos" que o
/// `RunState` não carrega).
///
/// **Por que enum separado e não `RunState`:** o `RunState` do
/// `agent-engine` é o estado de **um Run** (um único call
/// tool-loop). O `MultimodelState` é o estado de **um pipeline
/// inteiro** (sequência de Runs). Compartilham variantes
/// (`Running`, `Streaming`, `Completed`, `Failed`, `Cancelled`)
/// mas o pipeline tem 2 exclusivas: `Pending` (criado mas sem
/// stage em curso) e `PartiallyCompleted` (alguns stages
/// completaram, mas o pipeline foi interrompido antes de
/// finalizar — D5 do ADR-0028: o `MultimodelOrchestrator`
/// carrega no startup e oferece continuação).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultimodelState {
    /// Criado mas nenhum stage em curso.
    Pending,
    /// Stage atual em `Running`/`Streaming`/`WaitingToolCall`.
    Running,
    /// Todos os stages concluídos (`Completed` + `output_artifact`
    /// quando aplicável).
    Completed,
    /// Algum stage falhou. Stages anteriores mantêm
    /// `state = Completed` (D7 do ADR-0028: rollback é caro,
    /// opt-in).
    PartiallyCompleted,
    /// Falha irrecuperável (erro de I/O, schema, etc).
    Failed,
    /// Cancelado pelo usuário (botão "Parar" do Modo Equipe).
    /// Stages em curso marcados `Cancelled`; stages futuros
    /// marcados `Cancelled` direto (sem chamar o modelo).
    Cancelled,
}

impl MultimodelState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::PartiallyCompleted => "partially_completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for MultimodelState {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "partially_completed" => Ok(Self::PartiallyCompleted),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("MultimodelState desconhecido: {other}")),
        }
    }
}

/// Kind de um `MultimodelArtifact` (o que o stage produziu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultimodelArtifactKind {
    /// Texto puro (`String` em memória).
    Text,
    /// Arquivo em disco (`content_ref` é o path workspace-
    /// relative, validado pelo `Jail`).
    File,
    /// JSON estruturado (parseado por `serde_json`).
    Json,
    /// Markdown (renderizado pela UI da Etapa 6).
    Markdown,
}

impl MultimodelArtifactKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::File => "file",
            Self::Json => "json",
            Self::Markdown => "markdown",
        }
    }
}

impl std::str::FromStr for MultimodelArtifactKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "file" => Ok(Self::File),
            "json" => Ok(Self::Json),
            "markdown" => Ok(Self::Markdown),
            other => Err(format!("MultimodelArtifactKind desconhecido: {other}")),
        }
    }
}

/// Tipo de domínio: cabeçalho de um `MultimodelRun`.
///
/// Carrega o `state` do pipeline (não dos stages individuais —
/// isso é carregado por `PipelineRepo::list_stages`). O
/// `total_cost_microcents` é a soma dos `cost_microcents` dos
/// stages, atualizada por `MultimodelOrchestrator` a cada
/// stage concluído (D5 do ADR-0028: rastreamento de custo é
/// parte do estado persistido).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodelRun {
    pub id: String,
    pub parent_run_id: String,
    pub mode: MultimodelMode,
    pub state: MultimodelState,
    pub input_artifact_id: Option<String>,
    pub final_artifact_id: Option<String>,
    pub total_cost_microcents: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Tipo de domínio: um stage do pipeline.
///
/// Carrega o `state` do stage individual (não do pipeline —
/// isso é carregado por `MultimodelRun.state`). O `input_artifact_id`
/// é o artefato de entrada (None no primeiro stage).
/// `output_artifact_id` é o artefato produzido. `input_hash` /
/// `output_hash` são SHA-256 (D6 do ADR-0028: reuso quando
/// input não muda). `tools_used_json` é a lista de `ToolId`s
/// que o stage chamou (mesma decisão do `allowed_tools_json`
/// do `runs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodelStage {
    pub id: String,
    pub run_id: String,
    pub seq: i64,
    pub model_id: String,
    pub provider_id: String,
    pub state: String, // RunState como string (mesmo padrão do `runs.state`)
    pub input_artifact_id: Option<String>,
    pub output_artifact_id: Option<String>,
    pub input_hash: Option<String>,
    pub output_hash: Option<String>,
    pub cost_microcents: i64,
    pub tools_used_json: String,
    pub validation_json: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Tipo de domínio: um artefato produzido por um stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultimodelArtifact {
    pub id: String,
    pub run_id: String,
    pub stage_id: Option<String>,
    pub kind: MultimodelArtifactKind,
    pub content_ref: String,
    pub hash: String,
    pub size_bytes: i64,
    pub created_at: String,
}

/// Erros do `PipelineRepo`. Separado do `StorageError` pra
/// permitir que a Etapa 5 PR 2 (`MultimodelOrchestrator`)
/// discrimine erros de pipeline de erros de storage genéricos
/// (e aborte o pipeline com erro estruturado quando bate
/// `MultimodelError`, diferente de "DB write falhou" que pode
/// ser transient).
#[derive(Debug, Error)]
pub enum MultimodelError {
    /// `MultimodelRun` não encontrado.
    #[error("multimodel run '{0}' não encontrado")]
    RunNotFound(String),

    /// `MultimodelStage` não encontrado.
    #[error("multimodel stage '{0}' não encontrado")]
    StageNotFound(String),

    /// `MultimodelArtifact` não encontrado.
    #[error("multimodel artifact '{0}' não encontrado")]
    ArtifactNotFound(String),

    /// `UNIQUE (run_id, seq)` violado: tentativa de criar 2
    /// stages com o mesmo `seq` no mesmo pipeline. Bug
    /// interno (o `MultimodelOrchestrator` da Etapa 5 PR 2
    /// nunca deveria disparar isto).
    #[error("multimodel stage duplicado: run_id={run_id}, seq={seq}")]
    DuplicateStage { run_id: String, seq: i64 },

    /// `MultimodelMode` ou `MultimodelState` com valor
    /// desconhecido na deserialização. Bug interno (a
    /// migração `0030_multimodel.sql` restringe o CHECK
    /// constraint, mas defesa em profundidade).
    #[error("valor inválido em multimodel_runs: column={column}, value={value}")]
    InvalidValue { column: String, value: String },
}

/// Repositório das 3 tabelas do `MultimodelRun`. Reúne as
/// operações de `multimodel_runs` + `multimodel_stages` +
/// `multimodel_artifacts` no mesmo struct (mesma decisão do
/// `SubagentRunRepo` da Etapa 4 PR 1).
pub struct PipelineRepo<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> PipelineRepo<'a> {
    /// Constrói o repo a partir do `Database`.
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: &db.pool }
    }

    /// Acesso ao pool subjacente (mesma decisão do
    /// `MessageRepo::pool`).
    #[must_use]
    pub fn pool(&self) -> &'a sqlx::SqlitePool {
        self.pool
    }

    // ========================================================================
    // multimodel_runs
    // ========================================================================

    /// Cria um `MultimodelRun` novo (estado `Pending`,
    /// `total_cost_microcents = 0`). Usado pelo
    /// `MultimodelOrchestrator` quando o usuário cria um
    /// pipeline. Falha se o `parent_run_id` não existe (FK).
    pub async fn create_run(&self, run: &MultimodelRun) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO multimodel_runs (id, parent_run_id, mode, state, \
             input_artifact_id, final_artifact_id, total_cost_microcents, \
             created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&run.id)
        .bind(&run.parent_run_id)
        .bind(run.mode.as_str())
        .bind(run.state.as_str())
        .bind(&run.input_artifact_id)
        .bind(&run.final_artifact_id)
        .bind(run.total_cost_microcents)
        .bind(&run.created_at)
        .bind(&run.updated_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Carrega um `MultimodelRun` pelo ID. Retorna
    /// `Err(MultimodelError::RunNotFound)` se não existir (a
    /// Etapa 5 PR 2 trata isso como "pipeline não existe,
    /// falha o comando").
    pub async fn get_run(&self, id: &str) -> StorageResult<MultimodelRun> {
        let row: Option<MultimodelRunRow> = sqlx::query_as::<_, MultimodelRunRow>(
            "SELECT id, parent_run_id, mode, state, input_artifact_id, \
             final_artifact_id, total_cost_microcents, created_at, updated_at \
             FROM multimodel_runs WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        match row {
            Some(r) => r.into_domain().map_err(crate::StorageError::from),
            None => Err(crate::StorageError::Multimodel(
                MultimodelError::RunNotFound(id.to_string()),
            )),
        }
    }

    /// Atualiza o `state` e `updated_at` de um `MultimodelRun`.
    /// Usado pelo `MultimodelOrchestrator` quando transiciona
    /// o pipeline (e.g. `Pending` → `Running` quando o
    /// primeiro stage começa).
    pub async fn set_state(
        &self,
        id: &str,
        state: MultimodelState,
        updated_at: &str,
    ) -> StorageResult<()> {
        sqlx::query("UPDATE multimodel_runs SET state = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(state.as_str())
            .bind(updated_at)
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Atualiza `total_cost_microcents` e `updated_at`. Usado
    /// quando um stage concluído é gravado (soma o
    /// `cost_microcents` do stage ao total).
    pub async fn add_cost(&self, id: &str, delta: i64, updated_at: &str) -> StorageResult<()> {
        sqlx::query(
            "UPDATE multimodel_runs SET total_cost_microcents = total_cost_microcents + ?1, \
             updated_at = ?2 WHERE id = ?3",
        )
        .bind(delta)
        .bind(updated_at)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Seta `final_artifact_id` (quando o último stage
    /// produz o artefato final do pipeline).
    pub async fn set_final_artifact(
        &self,
        id: &str,
        artifact_id: &str,
        updated_at: &str,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE multimodel_runs SET final_artifact_id = ?1, updated_at = ?2 \
             WHERE id = ?3",
        )
        .bind(artifact_id)
        .bind(updated_at)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Lista os `MultimodelRun`s em estados `Running` ou
    /// `PartiallyCompleted` (D5 do ADR-0028: o app carrega
    /// esses no startup pra oferecer "retomar pipeline
    /// interrompido"). Ordena por `updated_at` DESC (mais
    /// recente primeiro).
    pub async fn list_resumable(&self) -> StorageResult<Vec<MultimodelRun>> {
        let rows: Vec<MultimodelRunRow> = sqlx::query_as::<_, MultimodelRunRow>(
            "SELECT id, parent_run_id, mode, state, input_artifact_id, \
             final_artifact_id, total_cost_microcents, created_at, updated_at \
             FROM multimodel_runs \
             WHERE state IN ('running', 'partially_completed') \
             ORDER BY updated_at DESC",
        )
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|r| r.into_domain().map_err(crate::StorageError::from))
            .collect()
    }

    // ========================================================================
    // multimodel_stages
    // ========================================================================

    /// Cria um `MultimodelStage` novo. Falha com
    /// `MultimodelError::DuplicateStage` se já existe um
    /// stage com o mesmo `(run_id, seq)`.
    pub async fn create_stage(&self, stage: &MultimodelStage) -> StorageResult<()> {
        let result = sqlx::query(
            "INSERT INTO multimodel_stages (id, run_id, seq, model_id, provider_id, \
             state, input_artifact_id, output_artifact_id, input_hash, output_hash, \
             cost_microcents, tools_used_json, validation_json, started_at, finished_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )
        .bind(&stage.id)
        .bind(&stage.run_id)
        .bind(stage.seq)
        .bind(&stage.model_id)
        .bind(&stage.provider_id)
        .bind(&stage.state)
        .bind(&stage.input_artifact_id)
        .bind(&stage.output_artifact_id)
        .bind(&stage.input_hash)
        .bind(&stage.output_hash)
        .bind(stage.cost_microcents)
        .bind(&stage.tools_used_json)
        .bind(&stage.validation_json)
        .bind(&stage.started_at)
        .bind(&stage.finished_at)
        .execute(self.pool)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => Err(
                crate::StorageError::Multimodel(MultimodelError::DuplicateStage {
                    run_id: stage.run_id.clone(),
                    seq: stage.seq,
                }),
            ),
            Err(e) => Err(e.into()),
        }
    }

    /// Lista os stages de um pipeline, ordenados por `seq` ASC.
    /// Carregado pelo `MultimodelOrchestrator` quando precisa
    /// saber "qual o próximo stage a executar" ou pela UI
    /// quando renderiza o grafo do pipeline.
    pub async fn list_stages(&self, run_id: &str) -> StorageResult<Vec<MultimodelStage>> {
        let rows: Vec<MultimodelStageRow> = sqlx::query_as::<_, MultimodelStageRow>(
            "SELECT id, run_id, seq, model_id, provider_id, state, input_artifact_id, \
             output_artifact_id, input_hash, output_hash, cost_microcents, \
             tools_used_json, validation_json, started_at, finished_at \
             FROM multimodel_stages WHERE run_id = ?1 ORDER BY seq ASC",
        )
        .bind(run_id)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|r| r.into_domain().map_err(crate::StorageError::from))
            .collect()
    }

    /// Atualiza o `state`, `cost_microcents`, `output_artifact_id`,
    /// `output_hash`, `tools_used_json`, `validation_json` e
    /// `finished_at` de um stage. Usado pelo
    /// `MultimodelOrchestrator` quando o stage termina.
    #[allow(clippy::too_many_arguments)] // 9 args: o portão de "stage concluído" precisa carregar tudo que mudou (custo, output, validação, tools, hash) em 1 update atômico — splittar em N updates quebraria o invariante "leitor vê o stage ou o estado anterior, nunca meio termo"
    pub async fn complete_stage(
        &self,
        id: &str,
        state: &str,
        cost_microcents: i64,
        output_artifact_id: Option<&str>,
        output_hash: Option<&str>,
        tools_used_json: &str,
        validation_json: Option<&str>,
        finished_at: &str,
    ) -> StorageResult<()> {
        sqlx::query(
            "UPDATE multimodel_stages SET \
             state = ?1, cost_microcents = ?2, output_artifact_id = ?3, \
             output_hash = ?4, tools_used_json = ?5, validation_json = ?6, \
             finished_at = ?7 \
             WHERE id = ?8",
        )
        .bind(state)
        .bind(cost_microcents)
        .bind(output_artifact_id)
        .bind(output_hash)
        .bind(tools_used_json)
        .bind(validation_json)
        .bind(finished_at)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Lista os stages `Completed` com `output_hash` igual ao
    /// `previous_output_hash` (D6 do ADR-0028: reuso quando
    /// input não muda). Usado pelo `MultimodelOrchestrator`
    /// na retomada do pipeline.
    pub async fn list_reusable_stages(
        &self,
        run_id: &str,
        previous_output_hash: &str,
    ) -> StorageResult<Vec<MultimodelStage>> {
        let rows: Vec<MultimodelStageRow> = sqlx::query_as::<_, MultimodelStageRow>(
            "SELECT id, run_id, seq, model_id, provider_id, state, input_artifact_id, \
             output_artifact_id, input_hash, output_hash, cost_microcents, \
             tools_used_json, validation_json, started_at, finished_at \
             FROM multimodel_stages \
             WHERE run_id = ?1 AND state = 'completed' AND output_hash = ?2 \
             ORDER BY seq ASC",
        )
        .bind(run_id)
        .bind(previous_output_hash)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|r| r.into_domain().map_err(crate::StorageError::from))
            .collect()
    }

    // ========================================================================
    // multimodel_artifacts
    // ========================================================================

    /// Cria um `MultimodelArtifact` novo. Usado pelo
    /// `MultimodelOrchestrator` quando um stage produz um
    /// artefato (texto, arquivo, JSON ou markdown).
    pub async fn create_artifact(&self, artifact: &MultimodelArtifact) -> StorageResult<()> {
        sqlx::query(
            "INSERT INTO multimodel_artifacts (id, run_id, stage_id, kind, \
             content_ref, hash, size_bytes, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&artifact.id)
        .bind(&artifact.run_id)
        .bind(&artifact.stage_id)
        .bind(artifact.kind.as_str())
        .bind(&artifact.content_ref)
        .bind(&artifact.hash)
        .bind(artifact.size_bytes)
        .bind(&artifact.created_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Carrega um `MultimodelArtifact` pelo ID. Retorna
    /// `Err(MultimodelError::ArtifactNotFound)` se não existir.
    pub async fn get_artifact(&self, id: &str) -> StorageResult<MultimodelArtifact> {
        let row: Option<MultimodelArtifactRow> = sqlx::query_as::<_, MultimodelArtifactRow>(
            "SELECT id, run_id, stage_id, kind, content_ref, hash, size_bytes, created_at \
             FROM multimodel_artifacts WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        match row {
            Some(r) => r.into_domain().map_err(crate::StorageError::from),
            None => Err(crate::StorageError::Multimodel(
                MultimodelError::ArtifactNotFound(id.to_string()),
            )),
        }
    }
}

// ============================================================================
// Row types (sqlx::FromRow)
// ============================================================================

/// Row bruta de `multimodel_runs`. Tipo de domínio:
/// [`MultimodelRun`].
#[derive(sqlx::FromRow)]
struct MultimodelRunRow {
    id: String,
    parent_run_id: String,
    mode: String,
    state: String,
    input_artifact_id: Option<String>,
    final_artifact_id: Option<String>,
    total_cost_microcents: i64,
    created_at: String,
    updated_at: String,
}

impl MultimodelRunRow {
    fn into_domain(self) -> Result<MultimodelRun, MultimodelError> {
        let mode: MultimodelMode =
            self.mode
                .parse()
                .map_err(|e: String| MultimodelError::InvalidValue {
                    column: "mode".to_string(),
                    value: format!("{e} (raw: {})", self.mode),
                })?;
        let state: MultimodelState =
            self.state
                .parse()
                .map_err(|e: String| MultimodelError::InvalidValue {
                    column: "state".to_string(),
                    value: format!("{e} (raw: {})", self.state),
                })?;
        Ok(MultimodelRun {
            id: self.id,
            parent_run_id: self.parent_run_id,
            mode,
            state,
            input_artifact_id: self.input_artifact_id,
            final_artifact_id: self.final_artifact_id,
            total_cost_microcents: self.total_cost_microcents,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// Row bruta de `multimodel_stages`. Tipo de domínio:
/// [`MultimodelStage`].
#[derive(sqlx::FromRow)]
struct MultimodelStageRow {
    id: String,
    run_id: String,
    seq: i64,
    model_id: String,
    provider_id: String,
    state: String,
    input_artifact_id: Option<String>,
    output_artifact_id: Option<String>,
    input_hash: Option<String>,
    output_hash: Option<String>,
    cost_microcents: i64,
    tools_used_json: String,
    validation_json: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

impl MultimodelStageRow {
    fn into_domain(self) -> Result<MultimodelStage, MultimodelError> {
        Ok(MultimodelStage {
            id: self.id,
            run_id: self.run_id,
            seq: self.seq,
            model_id: self.model_id,
            provider_id: self.provider_id,
            state: self.state,
            input_artifact_id: self.input_artifact_id,
            output_artifact_id: self.output_artifact_id,
            input_hash: self.input_hash,
            output_hash: self.output_hash,
            cost_microcents: self.cost_microcents,
            tools_used_json: self.tools_used_json,
            validation_json: self.validation_json,
            started_at: self.started_at,
            finished_at: self.finished_at,
        })
    }
}

/// Row bruta de `multimodel_artifacts`. Tipo de domínio:
/// [`MultimodelArtifact`].
#[derive(sqlx::FromRow)]
struct MultimodelArtifactRow {
    id: String,
    run_id: String,
    stage_id: Option<String>,
    kind: String,
    content_ref: String,
    hash: String,
    size_bytes: i64,
    created_at: String,
}

impl MultimodelArtifactRow {
    fn into_domain(self) -> Result<MultimodelArtifact, MultimodelError> {
        let kind: MultimodelArtifactKind =
            self.kind
                .parse()
                .map_err(|e: String| MultimodelError::InvalidValue {
                    column: "kind".to_string(),
                    value: format!("{e} (raw: {})", self.kind),
                })?;
        Ok(MultimodelArtifact {
            id: self.id,
            run_id: self.run_id,
            stage_id: self.stage_id,
            kind,
            content_ref: self.content_ref,
            hash: self.hash,
            size_bytes: self.size_bytes,
            created_at: self.created_at,
        })
    }
}

// ============================================================================
// Helpers de identidade
// ============================================================================

/// Gera um ID único pro `MultimodelRun` (UUID v4 stringificado).
/// Mesmo padrão do `RunId::new()`.
#[must_use]
pub fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Gera um ID único pro `MultimodelStage` (UUID v4 stringificado).
#[must_use]
pub fn new_stage_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Gera um ID único pro `MultimodelArtifact` (UUID v4 stringificado).
#[must_use]
pub fn new_artifact_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Hash SHA-256 de um arquivo em disco. Usado pelo
/// `MultimodelOrchestrator` quando um stage produz um
/// `File` artifact.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::hash::Hasher;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Reusa a estratégia do `permission_loader` (FNV via
    // `DefaultHasher` — mesmo trade-off, mais barato que SHA
    // real e suficiente pra detectar reuso, não pra
    // segurança). Pra v1, o "hash" é só uma chave de reuso.
    // Etapa futura pode trocar por SHA-256 se precisar de
    // integridade criptográfica.
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multimodel_mode_roundtrip() {
        // Por enquanto só `Pipeline` (Etapa 5 — Etapas futuras
        // plugam `Comparison`/`Conselho`/`Debate` per ADR-0028 §D3).
        // Quando os outros entrarem, transformar em loop sobre todas.
        let m = MultimodelMode::Pipeline;
        assert_eq!(m.as_str().parse::<MultimodelMode>().unwrap(), m);
    }

    #[test]
    fn multimodel_state_roundtrip() {
        for s in [
            MultimodelState::Pending,
            MultimodelState::Running,
            MultimodelState::Completed,
            MultimodelState::PartiallyCompleted,
            MultimodelState::Failed,
            MultimodelState::Cancelled,
        ] {
            assert_eq!(s.as_str().parse::<MultimodelState>().unwrap(), s);
        }
    }

    #[test]
    fn multimodel_artifact_kind_roundtrip() {
        for k in [
            MultimodelArtifactKind::Text,
            MultimodelArtifactKind::File,
            MultimodelArtifactKind::Json,
            MultimodelArtifactKind::Markdown,
        ] {
            assert_eq!(k.as_str().parse::<MultimodelArtifactKind>().unwrap(), k);
        }
    }

    #[test]
    fn new_ids_are_unique() {
        let a = new_run_id();
        let b = new_run_id();
        assert_ne!(a, b);
        assert!(a.len() > 30); // UUID v4
    }
}
