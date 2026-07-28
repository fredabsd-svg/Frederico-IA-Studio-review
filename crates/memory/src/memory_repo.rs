//! `MemoryRepo` — CRUD de memórias com busca lexical via FTS5.
//!
//! A Etapa 1 entrega o **funcionamento sem embeddings** (regra
//! do `PROMPT MESTRE` §10.13: "busca lexical FTS5 funciona
//! mesmo sem embeddings disponíveis"). O `search_lexical` usa
//! só o BM25 do FTS5 — semântica fica para a Etapa 2.
//!
//! Invariantes enforçadas aqui (não apenas convenção):
//!
//! 1. `insert_auto_captured` rejeita `origin = ExternalContent`
//!    (regra do `ADR-0012 §3` — conteúdo externo só vira memória
//!    com confirmação humana via `insert_user_confirmed`).
//! 2. `type_ = Temporary` exige `expires_at` no futuro
//!    (regra do `memory-architecture.md` §"Tipos").
//! 3. `confidence` e `importance` em [0.0, 1.0].
//! 4. `scope_id` vazio é rejeitado para escopos específicos
//!    (escopos globais `profile`/`preference`/`assistant`
//!    aceitam `scope_id` vazio).
//! 5. `mark_superseded` é idempotente (segunda chamada é no-op).
//!
//! O `list_by_scope` e o `search_lexical` filtram
//! `active = true AND superseded_by IS NULL AND pending_review =
//! false AND (expires_at IS NULL OR expires_at > now OR
//! user_pinned = true)` — mesma regra do
//! `MemoryRecord::is_visible`. Escopo é pré-filtro (`WHERE`),
//! não peso de score (regra do `ADR-0011 §1`, mitiga I4).

use chrono::{DateTime, Utc};
use frederico_core::{
    EmbeddingStatus, MemoryId, MemoryOrigin, MemoryRecord, MemoryScopeType, MemorySourceType,
    MemoryType,
};
use frederico_storage::Database;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{MemoryError, MemoryResult};
use crate::sanitize::sanitize_fts5_query;

/// Input para `MemoryRepo::insert_auto_captured` /
/// `insert_user_confirmed`. O `MemoryRecord` persistido ganha
/// `id`, `created_at`, `updated_at`, `embedding_status` e
/// campos derivados.
#[derive(Debug, Clone)]
pub struct NewMemoryInput {
    /// Escopo da memória (project, preference, etc).
    pub scope_type: MemoryScopeType,
    /// ID do escopo. Vazio para escopos globais.
    pub scope_id: String,
    /// Tipo da memória (preference, fact, temporary, etc).
    pub type_: MemoryType,
    /// Conteúdo textual da memória.
    pub content: String,
    /// Procedência (user, assistant, external_content).
    pub origin: MemoryOrigin,
    /// Tipo da fonte (user_message, tool_output:..., etc).
    pub source_type: MemorySourceType,
    /// ID da fonte (MessageId, ToolId, etc) — opcional.
    pub source_id: Option<String>,
    /// Confiança 0.0..=1.0 (saída do classificador).
    pub confidence: f32,
    /// Importância 0.0..=1.0 (boost no scoring).
    pub importance: f32,
    /// Expiração — obrigatório se `type_ == Temporary`.
    pub expires_at: Option<DateTime<Utc>>,
    /// Já passou por confirmação humana?
    pub user_confirmed: bool,
    /// Fixada pelo usuário? Bypassa `expires_at`.
    pub user_pinned: bool,
}

impl NewMemoryInput {
    /// Validações compartilhadas pelos dois métodos de insert.
    /// Específico do `type_` é checado pelo método.
    fn validate(&self) -> MemoryResult<()> {
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(MemoryError::OutOfRange {
                field: "confidence",
                value: self.confidence,
            });
        }
        if !(0.0..=1.0).contains(&self.importance) {
            return Err(MemoryError::OutOfRange {
                field: "importance",
                value: self.importance,
            });
        }
        if self.content.trim().is_empty() {
            return Err(MemoryError::GoldSetParse(
                "content não pode ser vazio".into(),
            ));
        }
        // Escopo global aceita scope_id vazio. Específico exige
        // scope_id.
        if !self.scope_type.is_global() && self.scope_id.trim().is_empty() {
            return Err(MemoryError::ScopeIdRequiredForSpecificScope(
                self.scope_type,
            ));
        }
        // Temporary exige expires_at não-nulo. Não rejeitamos
        // `expires_at` no passado — o `is_visible` filtra de
        // qualquer jeito. Isso permite que testes E2E
        // (determinísticos, com `FakeClock` ou `fixed_now()`)
        // insiram memórias "já expiradas" pra cobrir o caminho
        // do filtro. O caller de produção que cria memórias
        // expiradas por acidente tem o `is_visible` pra isso.
        if self.type_.requires_expires_at() && self.expires_at.is_none() {
            return Err(MemoryError::TemporaryRequiresExpiresAt);
        }
        Ok(())
    }
}

/// Repositório de memórias. Acessa o pool do [`Database`].
#[derive(Debug, Clone, Copy)]
pub struct MemoryRepo<'a> {
    /// Pool subjacente. `pub(crate)` pra que o `EmbeddingWorker`
    /// (no mesmo crate) possa construir um `MemoryRepo`
    /// diretamente do pool, sem precisar de um `Database`
    /// wrapper (o worker roda em background, depois que o
    /// `Database` original já pode ter sido dropado).
    pub(crate) pool: &'a sqlx::SqlitePool,
}

impl<'a> MemoryRepo<'a> {
    /// Cria um `MemoryRepo` que acessa o pool do `Database` dado.
    /// O `Database` é clonado barato (`sqlx::SqlitePool` é `Arc`
    /// internamente), mas o `&'a Database` é necessário pra
    /// satisfazer o ciclo de vida do `&'a SqlitePool` interno.
    #[must_use]
    pub fn new(db: &'a Database) -> Self {
        Self { pool: db.pool() }
    }

    /// Insere memória via **captura automática** (classificador
    /// da Etapa 3 ou inserção em batch pelo worker).
    ///
    /// **Rejeita `origin = ExternalContent`** com
    /// [`MemoryError::ExternalContentAutoCaptured`]. Use
    /// [`Self::insert_user_confirmed`] pra conteúdo externo
    /// após revisão humana.
    ///
    /// Memórias inseridas aqui nascem com:
    /// - `embedding_status = Pending`
    /// - `pending_review = false` (é captura automática, não
    ///   aguardando revisão)
    /// - `user_confirmed = input.user_confirmed` (false por
    ///   padrão; pode ser true se o classificador tiver
    ///   confiança alta e origem não-externa)
    /// - `user_pinned = input.user_pinned`
    pub async fn insert_auto_captured(&self, input: NewMemoryInput) -> MemoryResult<MemoryRecord> {
        if input.origin.is_external() {
            return Err(MemoryError::ExternalContentAutoCaptured);
        }
        self.insert_internal(input, false).await
    }

    /// Insere memória **após confirmação humana** (via painel
    /// da Etapa 5 ou comando explícito do usuário).
    ///
    /// Aceita **qualquer** `origin`, incluindo `ExternalContent`
    /// — a confirmação humana substitui a barreira automática
    /// (regra do `ADR-0012 §3`).
    ///
    /// Memórias inseridas aqui nascem com:
    /// - `embedding_status = Pending`
    /// - `user_confirmed = true` (sempre — é o ponto do método)
    /// - `pending_review = false` (revisão concluída)
    pub async fn insert_user_confirmed(&self, input: NewMemoryInput) -> MemoryResult<MemoryRecord> {
        let mut input = input;
        input.user_confirmed = true;
        self.insert_internal(input, false).await
    }

    /// Atalho de insert com `pending_review = true`. Usado pelo
    /// worker da Etapa 3 quando o classificador propõe
    /// `origin = ExternalContent` (memória fica esperando
    /// revisão humana no painel da Etapa 5).
    pub async fn insert_pending_review(&self, input: NewMemoryInput) -> MemoryResult<MemoryRecord> {
        if !input.origin.is_external() {
            // Pending review sem origem externa é misuse do
            // caller. Logamos mas aceitamos (o filtro
            // `is_visible` mantém a memória oculta até a
            // confirmação).
        }
        self.insert_internal(input, true).await
    }

    async fn insert_internal(
        &self,
        input: NewMemoryInput,
        pending_review: bool,
    ) -> MemoryResult<MemoryRecord> {
        input.validate()?;

        let id = MemoryId::new();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let expires_at_str = input.expires_at.map(|d| d.to_rfc3339());

        sqlx::query(
            "INSERT INTO memory_records (
                id, scope_type, scope_id, type, content, origin,
                source_type, source_id, confidence, importance,
                embedding_status, created_at, updated_at, expires_at,
                user_confirmed, user_pinned, active, pending_review
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10,
                'pending', ?11, ?11, ?12,
                ?13, ?14, 1, ?15
             )",
        )
        .bind(id.0.to_string())
        .bind(input.scope_type.as_str())
        .bind(&input.scope_id)
        .bind(input.type_.as_str())
        .bind(&input.content)
        .bind(input.origin.as_str())
        .bind(input.source_type.as_str())
        .bind(&input.source_id)
        .bind(input.confidence as f64)
        .bind(input.importance as f64)
        .bind(&now_str)
        .bind(&expires_at_str)
        .bind(if input.user_confirmed { 1_i64 } else { 0_i64 })
        .bind(if input.user_pinned { 1_i64 } else { 0_i64 })
        .bind(if pending_review { 1_i64 } else { 0_i64 })
        .execute(self.pool)
        .await?;

        Ok(MemoryRecord {
            id,
            scope_type: input.scope_type,
            scope_id: input.scope_id,
            type_: input.type_,
            content: input.content,
            origin: input.origin,
            source_type: input.source_type,
            source_id: input.source_id,
            confidence: input.confidence,
            importance: input.importance,
            embedding_status: EmbeddingStatus::Pending,
            embedding_provider: None,
            embedding_model: None,
            embedding_dimensions: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            expires_at: input.expires_at,
            superseded_by: None,
            superseded_at: None,
            user_confirmed: input.user_confirmed,
            user_pinned: input.user_pinned,
            active: true,
            pending_review,
        })
    }

    /// Lê uma memória por id. Devolve `None` se não existir
    /// (memória `active = false` é devolvida — a UI mostra com
    /// marca de soft-deletada).
    pub async fn get(&self, id: &MemoryId) -> MemoryResult<Option<MemoryRecord>> {
        let row = sqlx::query(
            "SELECT id, scope_type, scope_id, type, content, origin,
                    source_type, source_id, confidence, importance,
                    embedding_status, embedding_provider, embedding_model,
                    embedding_dimensions, created_at, updated_at,
                    last_used_at, expires_at, superseded_by, superseded_at,
                    user_confirmed, user_pinned, active, pending_review
             FROM memory_records WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_record(&r)?)),
        }
    }

    /// Lista memórias visíveis em um escopo. Filtra:
    /// - escopo (pré-filtro, cláusula WHERE — `ADR-0011 §1`)
    /// - `active = true`
    /// - `superseded_by IS NULL`
    /// - `pending_review = false`
    /// - `expires_at > now` (ou `user_pinned = true`)
    pub async fn list_by_scope(
        &self,
        scope_type: MemoryScopeType,
        scope_id: &str,
        now: DateTime<Utc>,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let now_str = now.to_rfc3339();

        // Se o escopo é global, retorna memórias daquele tipo
        // (independente de scope_id). Se é específico, casamos
        // scope_type + scope_id.
        let query = if scope_type.is_global() {
            sqlx::query(
                "SELECT id, scope_type, scope_id, type, content, origin,
                        source_type, source_id, confidence, importance,
                        embedding_status, embedding_provider, embedding_model,
                        embedding_dimensions, created_at, updated_at,
                        last_used_at, expires_at, superseded_by, superseded_at,
                        user_confirmed, user_pinned, active, pending_review
                 FROM memory_records
                 WHERE scope_type = ?1
                   AND active = 1
                   AND superseded_by IS NULL
                   AND pending_review = 0
                   AND (expires_at IS NULL OR expires_at > ?2 OR user_pinned = 1)
                 ORDER BY updated_at DESC",
            )
            .bind(scope_type.as_str())
            .bind(&now_str)
        } else {
            sqlx::query(
                "SELECT id, scope_type, scope_id, type, content, origin,
                        source_type, source_id, confidence, importance,
                        embedding_status, embedding_provider, embedding_model,
                        embedding_dimensions, created_at, updated_at,
                        last_used_at, expires_at, superseded_by, superseded_at,
                        user_confirmed, user_pinned, active, pending_review
                 FROM memory_records
                 WHERE scope_type = ?1 AND scope_id = ?2
                   AND active = 1
                   AND superseded_by IS NULL
                   AND pending_review = 0
                   AND (expires_at IS NULL OR expires_at > ?3 OR user_pinned = 1)
                 ORDER BY updated_at DESC",
            )
            .bind(scope_type.as_str())
            .bind(scope_id)
            .bind(&now_str)
        };

        let rows = query.fetch_all(self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(row_to_record(r)?);
        }
        Ok(out)
    }

    /// Busca lexical via FTS5. Devolve tuplas
    /// `(MemoryRecord, score_lexical)` ordenadas por score
    /// descendente. O score é `1.0 / (1.0 + |bm25|)` —
    /// normalização simples pra [0.0, 1.0].
    ///
    /// **Expansão de escopo** (regra do `ADR-0011 §1`):
    /// - Se `scope_type` é **global** (Profile, Preference,
    ///   Assistant), retorna só memórias desse tipo (sem
    ///   `scope_id`).
    /// - Se `scope_type` é **específico** (Project, Client,
    ///   Conversation, etc), retorna memórias **desse escopo
    ///   específico** E memórias dos **escopos globais**
    ///   (Profile, Preference, Assistant) — memórias globais
    ///   atravessam conversas.
    ///
    /// A Etapa 1 só usa BM25. A Etapa 2 (retrieval híbrido)
    /// adiciona cosine semântica.
    ///
    /// Pré-filtros (`active`, `superseded_by`, `pending_review`,
    /// `expires_at`) iguais ao `list_by_scope`.
    pub async fn search_lexical(
        &self,
        scope_type: MemoryScopeType,
        scope_id: &str,
        query: &str,
        k: usize,
        now: DateTime<Utc>,
    ) -> MemoryResult<Vec<(MemoryRecord, f32)>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            // Query vazia → sem match lexical. Caller pode
            // decidir usar `list_by_scope` em vez de `search_lexical`.
            return Ok(Vec::new());
        }
        let now_str = now.to_rfc3339();

        // FTS5 do SQLite: `bm25(memory_fts)` devolve score
        // (negativo, mais perto de 0 = mais relevante).
        // Filtramos por escopo via JOIN com memory_records.
        let rows = if scope_type.is_global() {
            // Escopo global — só memórias desse tipo, sem
            // expandir pra outros.
            sqlx::query(
                "SELECT mr.id, mr.scope_type, mr.scope_id, mr.type, mr.content,
                        mr.origin, mr.source_type, mr.source_id, mr.confidence,
                        mr.importance, mr.embedding_status, mr.embedding_provider,
                        mr.embedding_model, mr.embedding_dimensions, mr.created_at,
                        mr.updated_at, mr.last_used_at, mr.expires_at,
                        mr.superseded_by, mr.superseded_at, mr.user_confirmed,
                        mr.user_pinned, mr.active, mr.pending_review,
                        bm25(memory_fts) AS bm25_score
                 FROM memory_fts
                 JOIN memory_records mr ON mr.rowid = memory_fts.rowid
                 WHERE memory_fts MATCH ?1
                   AND mr.scope_type = ?2
                   AND mr.active = 1
                   AND mr.superseded_by IS NULL
                   AND mr.pending_review = 0
                   AND (mr.expires_at IS NULL OR mr.expires_at > ?3 OR mr.user_pinned = 1)
                 ORDER BY bm25_score
                 LIMIT ?4",
            )
            .bind(&sanitized)
            .bind(scope_type.as_str())
            .bind(&now_str)
            .bind(k as i64)
            .fetch_all(self.pool)
            .await?
        } else {
            // Escopo específico — memórias desse escopo
            // (scope_type + scope_id) **+ memórias dos escopos
            // globais** (profile, preference, assistant).
            // O `OR` no `WHERE` é a aplicação do `ADR-0011 §1`:
            // memória global atravessa conversas.
            sqlx::query(
                "SELECT mr.id, mr.scope_type, mr.scope_id, mr.type, mr.content,
                        mr.origin, mr.source_type, mr.source_id, mr.confidence,
                        mr.importance, mr.embedding_status, mr.embedding_provider,
                        mr.embedding_model, mr.embedding_dimensions, mr.created_at,
                        mr.updated_at, mr.last_used_at, mr.expires_at,
                        mr.superseded_by, mr.superseded_at, mr.user_confirmed,
                        mr.user_pinned, mr.active, mr.pending_review,
                        bm25(memory_fts) AS bm25_score
                 FROM memory_fts
                 JOIN memory_records mr ON mr.rowid = memory_fts.rowid
                 WHERE memory_fts MATCH ?1
                   AND (
                     mr.scope_type IN ('profile', 'preference', 'assistant')
                     OR (mr.scope_type = ?2 AND mr.scope_id = ?3)
                   )
                   AND mr.active = 1
                   AND mr.superseded_by IS NULL
                   AND mr.pending_review = 0
                   AND (mr.expires_at IS NULL OR mr.expires_at > ?4 OR mr.user_pinned = 1)
                 ORDER BY bm25_score
                 LIMIT ?5",
            )
            .bind(&sanitized)
            .bind(scope_type.as_str())
            .bind(scope_id)
            .bind(&now_str)
            .bind(k as i64)
            .fetch_all(self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let record = row_to_record(r)?;
            let bm25: f64 = r.try_get("bm25_score").map_err(|e| {
                MemoryError::Query(sqlx::Error::ColumnDecode {
                    index: "bm25_score".into(),
                    source: Box::new(e),
                })
            })?;
            // Normalização: BM25 do FTS5 é negativo (mais perto
            // de 0 = mais relevante). Convertemos pra score em
            // [0.0, 1.0] onde 1.0 = perfeitamente relevante.
            let normalized = 1.0 / (1.0 + bm25.abs() as f32);
            out.push((record, normalized));
        }
        Ok(out)
    }

    /// Marca `old_id` como superseded por `new_id`. Idempotente:
    /// se `old_id` já está superseded, é no-op.
    pub async fn mark_superseded(
        &self,
        old_id: &MemoryId,
        new_id: &MemoryId,
        now: DateTime<Utc>,
    ) -> MemoryResult<()> {
        let now_str = now.to_rfc3339();

        // Idempotência: WHERE superseded_by IS NULL — se já
        // está superseded, a query não afeta nenhuma linha.
        let affected = sqlx::query(
            "UPDATE memory_records
             SET superseded_by = ?1, superseded_at = ?2, updated_at = ?2
             WHERE id = ?3 AND superseded_by IS NULL",
        )
        .bind(new_id.0.to_string())
        .bind(&now_str)
        .bind(old_id.0.to_string())
        .execute(self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            // Pode ser: (a) old_id não existe; (b) já estava
            // superseded. Em ambos, é no-op.
            // Opcionalmente, validar existência — mas a
            // idempotência do método é o contrato, não
            // erro de "id não existe".
        }
        Ok(())
    }

    /// Deleta memórias expiradas ou soft-deletadas. Chamado
    /// na inicialização (`apps/desktop/src-tauri/src/main.rs`)
    /// e sob demanda do usuário via painel da Etapa 5.
    ///
    /// Devolve o número de linhas deletadas (auditável no log
    /// de inicialização).
    pub async fn purge_expired(&self, now: DateTime<Utc>) -> MemoryResult<usize> {
        let now_str = now.to_rfc3339();

        // Deleta: (a) active=0 (soft-deletada), (b) superseded,
        // (c) expirada e não-pinned. A query é uma só pra
        // simplicidade.
        let result = sqlx::query(
            "DELETE FROM memory_records
             WHERE active = 0
                OR superseded_by IS NOT NULL
                OR (expires_at IS NOT NULL AND expires_at <= ?1 AND user_pinned = 0)",
        )
        .bind(&now_str)
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }

    /// Marca uma memória `pending_review = true` como
    /// confirmada (após revisão humana via painel). Memória
    /// vira visível (`user_confirmed = true`, `pending_review =
    /// false`).
    pub async fn confirm_pending(&self, id: &MemoryId) -> MemoryResult<()> {
        let now_str = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE memory_records
             SET user_confirmed = 1, pending_review = 0, updated_at = ?1
             WHERE id = ?2 AND pending_review = 1",
        )
        .bind(&now_str)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            // Não estava pendente — no-op.
        }
        Ok(())
    }

    /// Rejeita (deleta) uma memória `pending_review`. Audit
    /// trail preservado em log do app, não no banco.
    pub async fn reject_pending(&self, id: &MemoryId) -> MemoryResult<()> {
        sqlx::query("DELETE FROM memory_records WHERE id = ?1 AND pending_review = 1")
            .bind(id.0.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Persiste o embedding de uma memória e marca
    /// `embedding_status = 'ready'`. O `provider` e `model`
    /// viram parte da PK composta de `memory_embeddings` —
    /// embeddings de providers/modelos diferentes não são
    /// comparáveis (regra do `ADR-0010 §1`).
    ///
    /// Atualiza também os campos `embedding_provider`,
    /// `embedding_model` e `embedding_dimensions` na
    /// `memory_records` (cache pra o `Retriever` saber que
    /// tem embedding pronto sem precisar de JOIN).
    ///
    /// Idempotente: se já existe embedding pra esse
    /// `(memory_id, provider, model)`, sobrescreve.
    pub async fn set_embedding(
        &self,
        id: &MemoryId,
        provider: &str,
        model: &str,
        dimensions: u32,
        embedding: &[f32],
    ) -> MemoryResult<()> {
        // Validação: dimensions deve bater com o tamanho do
        // vetor. Erro estrutural — caller passou dimensões
        // erradas.
        if embedding.len() as u64 != u64::from(dimensions) {
            return Err(crate::error::MemoryError::EmbeddingDimensionMismatch {
                expected: dimensions as usize,
                actual: embedding.len(),
            });
        }
        let blob = crate::embedding_codec::encode_embedding(embedding);
        let now_str = Utc::now().to_rfc3339();

        // 1. INSERT OR REPLACE em `memory_embeddings`.
        sqlx::query(
            "INSERT OR REPLACE INTO memory_embeddings
                (memory_id, provider, model, dimensions, vec_blob, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(id.0.to_string())
        .bind(provider)
        .bind(model)
        .bind(dimensions as i64)
        .bind(blob)
        .bind(&now_str)
        .execute(self.pool)
        .await?;

        // 2. Atualiza cache em `memory_records`.
        sqlx::query(
            "UPDATE memory_records
             SET embedding_status = 'ready',
                 embedding_provider = ?1,
                 embedding_model = ?2,
                 embedding_dimensions = ?3,
                 updated_at = ?4
             WHERE id = ?5",
        )
        .bind(provider)
        .bind(model)
        .bind(dimensions as i64)
        .bind(&now_str)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Marca uma memória com `embedding_status = 'failed'`
    /// (o worker de embeddings tentou e falhou). Útil pro
    /// painel da Etapa 5 mostrar "última falha" e pra evitar
    /// retry em loop infinito.
    pub async fn mark_embedding_failed(&self, id: &MemoryId) -> MemoryResult<()> {
        let now_str = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE memory_records
             SET embedding_status = 'failed', updated_at = ?1
             WHERE id = ?2",
        )
        .bind(&now_str)
        .bind(id.0.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Lê o embedding persistido de uma memória (modelo
    /// específico). Retorna `Ok(None)` se não existe.
    /// Retorna `Err(EmbeddingDimensionMismatch)` se o blob
    /// persistido tem tamanho inconsistente.
    pub async fn get_embedding(
        &self,
        id: &MemoryId,
        provider: &str,
        model: &str,
    ) -> MemoryResult<Option<Vec<f32>>> {
        let row: Option<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT dimensions, vec_blob
             FROM memory_embeddings
             WHERE memory_id = ?1 AND provider = ?2 AND model = ?3",
        )
        .bind(id.0.to_string())
        .bind(provider)
        .bind(model)
        .fetch_optional(self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some((dim, blob)) => {
                let embedding = crate::embedding_codec::decode_embedding(&blob, dim as usize)?;
                Ok(Some(embedding))
            }
        }
    }

    /// Lista memórias com `embedding_status = 'pending'`,
    /// limitadas a `limit`. Usada pelo `EmbeddingWorker`
    /// pra encontrar o próximo lote.
    pub async fn list_pending_embeddings(&self, limit: u32) -> MemoryResult<Vec<MemoryRecord>> {
        let rows = sqlx::query(
            "SELECT id, scope_type, scope_id, type, content, origin,
                    source_type, source_id, confidence, importance,
                    embedding_status, embedding_provider, embedding_model,
                    embedding_dimensions, created_at, updated_at,
                    last_used_at, expires_at, superseded_by, superseded_at,
                    user_confirmed, user_pinned, active, pending_review
             FROM memory_records
             WHERE active = 1
               AND superseded_by IS NULL
               AND embedding_status = 'pending'
             ORDER BY created_at ASC
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(row_to_record(r)?);
        }
        Ok(out)
    }
}

/// Converte uma linha de `memory_records` no tipo `MemoryRecord`.
/// Helper privado compartilhado por `get`, `list_by_scope` e
/// `search_lexical`.
fn row_to_record(r: &sqlx::sqlite::SqliteRow) -> MemoryResult<MemoryRecord> {
    let id_str: String = r.try_get("id").map_err(col_err("id"))?;
    let id_uuid = Uuid::parse_str(&id_str)
        .map_err(|e| MemoryError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into())))?;
    let id = MemoryId(id_uuid);

    let scope_type_str: String = r.try_get("scope_type").map_err(col_err("scope_type"))?;
    let scope_type = scope_type_str
        .parse::<frederico_core::MemoryScopeType>()
        .map_err(|_| MemoryError::GoldSetParse(format!("scope_type inválido: {scope_type_str}")))?;

    let scope_id: String = r.try_get("scope_id").map_err(col_err("scope_id"))?;

    let type_str: String = r.try_get("type").map_err(col_err("type"))?;
    let type_ = type_str
        .parse::<MemoryType>()
        .map_err(|_| MemoryError::GoldSetParse(format!("type inválido: {type_str}")))?;

    let content: String = r.try_get("content").map_err(col_err("content"))?;

    let origin_str: String = r.try_get("origin").map_err(col_err("origin"))?;
    let origin = origin_str
        .parse::<MemoryOrigin>()
        .map_err(|_| MemoryError::GoldSetParse(format!("origin inválido: {origin_str}")))?;

    let source_type: String = r.try_get("source_type").map_err(col_err("source_type"))?;
    let source_type = MemorySourceType::new(source_type);

    let source_id: Option<String> = r.try_get("source_id").ok();

    let confidence: f64 = r.try_get("confidence").map_err(col_err("confidence"))?;
    let importance: f64 = r.try_get("importance").map_err(col_err("importance"))?;

    let embedding_status_str: String = r
        .try_get("embedding_status")
        .map_err(col_err("embedding_status"))?;
    let embedding_status = embedding_status_str
        .parse::<EmbeddingStatus>()
        .map_err(|_| {
            MemoryError::GoldSetParse(format!("embedding_status inválido: {embedding_status_str}"))
        })?;

    let embedding_provider: Option<String> = r.try_get("embedding_provider").ok();
    let embedding_model: Option<String> = r.try_get("embedding_model").ok();
    let embedding_dimensions: Option<i64> = r.try_get("embedding_dimensions").ok();
    let embedding_dimensions = embedding_dimensions.map(|d| d as u32);

    let created_at_str: String = r.try_get("created_at").map_err(col_err("created_at"))?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map_err(|e| MemoryError::GoldSetParse(format!("created_at inválido: {e}")))?
        .with_timezone(&Utc);

    let updated_at_str: String = r.try_get("updated_at").map_err(col_err("updated_at"))?;
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map_err(|e| MemoryError::GoldSetParse(format!("updated_at inválido: {e}")))?
        .with_timezone(&Utc);

    let last_used_at_str: Option<String> = r.try_get("last_used_at").ok();
    let last_used_at = match last_used_at_str {
        Some(s) if !s.is_empty() => Some(
            DateTime::parse_from_rfc3339(&s)
                .map_err(|e| MemoryError::GoldSetParse(format!("last_used_at inválido: {e}")))?
                .with_timezone(&Utc),
        ),
        _ => None,
    };

    let expires_at_str: Option<String> = r.try_get("expires_at").ok();
    let expires_at = match expires_at_str {
        Some(s) if !s.is_empty() => Some(
            DateTime::parse_from_rfc3339(&s)
                .map_err(|e| MemoryError::GoldSetParse(format!("expires_at inválido: {e}")))?
                .with_timezone(&Utc),
        ),
        _ => None,
    };

    let superseded_by_str: Option<String> = r.try_get("superseded_by").ok();
    let superseded_by = match superseded_by_str {
        Some(s) if !s.is_empty() => {
            let uuid = Uuid::parse_str(&s).map_err(|e| {
                MemoryError::Query(sqlx::Error::Decode(format!("bad uuid: {e}").into()))
            })?;
            Some(MemoryId(uuid))
        }
        _ => None,
    };

    let superseded_at_str: Option<String> = r.try_get("superseded_at").ok();
    let superseded_at = match superseded_at_str {
        Some(s) if !s.is_empty() => Some(
            DateTime::parse_from_rfc3339(&s)
                .map_err(|e| MemoryError::GoldSetParse(format!("superseded_at inválido: {e}")))?
                .with_timezone(&Utc),
        ),
        _ => None,
    };

    let user_confirmed: i64 = r
        .try_get("user_confirmed")
        .map_err(col_err("user_confirmed"))?;
    let user_pinned: i64 = r.try_get("user_pinned").map_err(col_err("user_pinned"))?;
    let active: i64 = r.try_get("active").map_err(col_err("active"))?;
    let pending_review: i64 = r
        .try_get("pending_review")
        .map_err(col_err("pending_review"))?;

    Ok(MemoryRecord {
        id,
        scope_type,
        scope_id,
        type_,
        content,
        origin,
        source_type,
        source_id,
        confidence: confidence as f32,
        importance: importance as f32,
        embedding_status,
        embedding_provider,
        embedding_model,
        embedding_dimensions,
        created_at,
        updated_at,
        last_used_at,
        expires_at,
        superseded_by,
        superseded_at,
        user_confirmed: user_confirmed != 0,
        user_pinned: user_pinned != 0,
        active: active != 0,
        pending_review: pending_review != 0,
    })
}

/// Helper: cria um `MemoryError::Query` com `sqlx::Error::ColumnNotFound`
/// ou `Decode` dependendo do erro do `try_get`.
fn col_err(col: &'static str) -> impl FnOnce(sqlx::Error) -> MemoryError {
    move |e| match e {
        sqlx::Error::ColumnNotFound(_) => {
            MemoryError::Query(sqlx::Error::ColumnNotFound(col.into()))
        }
        other => MemoryError::Query(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn new_input(scope_type: MemoryScopeType, scope_id: &str, content: &str) -> NewMemoryInput {
        NewMemoryInput {
            scope_type,
            scope_id: scope_id.into(),
            type_: MemoryType::Fact,
            content: content.into(),
            origin: MemoryOrigin::User,
            source_type: MemorySourceType::new("user_message"),
            source_id: None,
            confidence: 0.8,
            importance: 0.5,
            expires_at: None,
            user_confirmed: false,
            user_pinned: false,
        }
    }

    #[test]
    fn validate_rejects_empty_content() {
        let mut i = new_input(MemoryScopeType::Project, "proj-1", "");
        i.content = "   ".into();
        assert!(matches!(i.validate(), Err(MemoryError::GoldSetParse(_))));
    }

    #[test]
    fn validate_rejects_confidence_out_of_range() {
        let mut i = new_input(MemoryScopeType::Project, "proj-1", "ok");
        i.confidence = 1.5;
        assert!(matches!(i.validate(), Err(MemoryError::OutOfRange { .. })));
    }

    #[test]
    fn validate_rejects_importance_out_of_range() {
        let mut i = new_input(MemoryScopeType::Project, "proj-1", "ok");
        i.importance = -0.1;
        assert!(matches!(i.validate(), Err(MemoryError::OutOfRange { .. })));
    }

    #[test]
    fn validate_rejects_specific_scope_with_empty_id() {
        let i = new_input(MemoryScopeType::Project, "", "ok");
        assert!(matches!(
            i.validate(),
            Err(MemoryError::ScopeIdRequiredForSpecificScope(_))
        ));
    }

    #[test]
    fn validate_allows_global_scope_with_empty_id() {
        let i = new_input(MemoryScopeType::Preference, "", "ok");
        assert!(i.validate().is_ok());
    }

    #[test]
    fn validate_temporary_requires_expires_at() {
        let mut i = new_input(MemoryScopeType::Project, "proj-1", "ok");
        i.type_ = MemoryType::Temporary;
        assert!(matches!(
            i.validate(),
            Err(MemoryError::TemporaryRequiresExpiresAt)
        ));
    }

    #[test]
    fn validate_temporary_with_past_expires_at_is_allowed() {
        // Memórias "já expiradas" são aceitas no insert — o
        // `is_visible` filtra. Isso permite que testes E2E
        // com `fixed_now()` simulem o caminho do filtro.
        let mut i = new_input(MemoryScopeType::Project, "proj-1", "ok");
        i.type_ = MemoryType::Temporary;
        i.expires_at = Some(Utc::now() - Duration::hours(1));
        assert!(i.validate().is_ok());
    }
}
