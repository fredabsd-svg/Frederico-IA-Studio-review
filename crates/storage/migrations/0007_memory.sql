-- Migração 0007 — Memória e continuidade (Fase 4, Etapa 1).
--
-- Cinco tabelas novas:
--
-- 1. `memory_records` — registro principal de cada memória. 24
--    colunas; `scope_type` e `type` têm `CHECK` com 9 e 12
--    valores respectivamente (canonizados em
--    `frederico_core::MemoryScopeType` e `MemoryType` —
--    adicionar valor exige update na enum + na `CHECK` no
--    mesmo commit, regra do `REGRAS §1.13`).
--
-- 2. `memory_fts` — virtual table FTS5 sobre `content`. Permite
--    busca lexical BM25 via operador `MATCH`. Triggers
--    mantêm em sincronia com `memory_records` (insert, delete,
--    update). A Etapa 1 usa o BM25 como **único** sinal de
--    scoring; a Etapa 2 adiciona cosine sobre
--    `memory_embeddings`.
--
-- 3. `memory_embeddings` — embeddings por `(memory_id,
--    provider, model)`. PK composta pra permitir reindexação
--    por modelo sem perder histórico (a Etapa 2 introduz o
--    `ReindexWorker` que escreve novas linhas com o modelo
--    novo e mantém as antigas pra audit).
--
-- 4. `embedding_reindex_jobs` — log de jobs de reindexação.
--    Lido pelo painel da Etapa 5 (progresso). Append-only.
--
-- 5. Regras de ouro (ADR-0010, ADR-0011, ADR-0012, ADR-0013,
--    ADR-0014):
--    - `origin` com `CHECK` em 3 valores (User, Assistant,
--      ExternalContent). ExternalContent **nunca** vira
--      memória automática (regra do `ADR-0012 §3`).
--    - `pending_review = 1` → memória aguardando revisão
--      humana; `MemoryRecord::is_visible` filtra.
--    - `superseded_by` FK pra `memory_records(id)` ON DELETE
--      SET NULL (cascade seria agressivo demais — preferir
--      histórico preservado).
--    - `expires_at` obrigatório quando `type = 'temporary'`
--      (validado no `MemoryRepo::insert`).
--    - `user_pinned` bypassa `expires_at` (não bypassa
--      `superseded_by`).

CREATE TABLE IF NOT EXISTS memory_records (
    -- ID opaco (UUID, mesmo padrão dos outros IDs).
    id TEXT PRIMARY KEY,

    -- 9 escopos. Ver `frederico_core::MemoryScopeType`.
    scope_type TEXT NOT NULL CHECK(scope_type IN (
        'profile', 'preference', 'assistant',
        'project', 'client', 'conversation',
        'document', 'task', 'session'
    )),

    -- ID do escopo (ProjectId, ConversationId, ...). Vazio
    -- para escopos globais (profile, preference, assistant).
    scope_id TEXT NOT NULL,

    -- 12 tipos. Ver `frederico_core::MemoryType`.
    type TEXT NOT NULL CHECK(type IN (
        'preference', 'fact', 'decision', 'correction',
        'project_instruction', 'client_context', 'procedure',
        'delivery_pattern', 'temporary', 'conversation_summary',
        'document_reference', 'user_pinned'
    )),

    -- Conteúdo da memória. Indexado por FTS5 (via `memory_fts`).
    content TEXT NOT NULL,

    -- 3 valores. Ver `frederico_core::MemoryOrigin`.
    -- ExternalContent NUNCA vira memória automática
    -- (`ADR-0012 §3`).
    origin TEXT NOT NULL CHECK(origin IN (
        'user', 'assistant', 'external_content'
    )),

    -- Tipo da fonte (string livre, valores típicos:
    -- "user_message", "assistant_message", "tool_output:<id>",
    -- "document_attachment:<id>", "manual").
    source_type TEXT NOT NULL,

    -- ID da fonte (MessageId, ToolId, etc) — opcional.
    source_id TEXT,

    -- Saída do classificador, [0.0, 1.0].
    confidence REAL NOT NULL DEFAULT 1.0
        CHECK(confidence >= 0.0 AND confidence <= 1.0),

    -- Importância da memória, [0.0, 1.0]. Boost no scoring
    -- (sinal 4 do `ADR-0011 §2`).
    importance REAL NOT NULL DEFAULT 0.5
        CHECK(importance >= 0.0 AND importance <= 1.0),

    -- Status do embedding.
    embedding_status TEXT NOT NULL DEFAULT 'pending'
        CHECK(embedding_status IN ('pending', 'ready', 'failed')),

    -- Identificação do modelo de embedding usado (preenchido
    -- pela Etapa 2). Partição em `memory_embeddings`.
    embedding_provider TEXT,
    embedding_model TEXT,
    embedding_dimensions INTEGER,

    -- Timestamps. Default do SQLite (UTC).
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,

    -- Obrigatório se type = 'temporary' (validado no
    -- `MemoryRepo::insert`). Bypassado se user_pinned = 1.
    expires_at TEXT,

    -- FK pra outra memória. ON DELETE SET NULL pra preservar
    -- histórico (a Etapa 4 introduz `mark_superseded`).
    superseded_by TEXT REFERENCES memory_records(id) ON DELETE SET NULL,
    superseded_at TEXT,

    -- Falsa por padrão. `ExternalContent` só vira memória
    -- após `insert_user_confirmed` (que seta = 1).
    user_confirmed INTEGER NOT NULL DEFAULT 0
        CHECK(user_confirmed IN (0, 1)),

    -- Bypassa `expires_at` (mas não `superseded_by`).
    user_pinned INTEGER NOT NULL DEFAULT 0
        CHECK(user_pinned IN (0, 1)),

    -- Soft delete (`ADR-0014 §3`: `purge_expired` faz
    -- `DELETE` físico; `active = 0` é só marca).
    active INTEGER NOT NULL DEFAULT 1
        CHECK(active IN (0, 1)),

    -- `true` = aguardando revisão humana (apenas memórias
    -- com `origin = external_content` antes da confirmação).
    -- `MemoryRecord::is_visible` filtra.
    pending_review INTEGER NOT NULL DEFAULT 0
        CHECK(pending_review IN (0, 1))
);

-- "lista memórias de um escopo" (pré-filtro de escopo no
-- `Retriever`).
CREATE INDEX IF NOT EXISTS idx_memory_records_scope
    ON memory_records(scope_type, scope_id);

-- "memórias ativas" (filtro de `active = 1`).
CREATE INDEX IF NOT EXISTS idx_memory_records_active
    ON memory_records(active);

-- "memórias expiradas pra purgar" (Etapa 4).
CREATE INDEX IF NOT EXISTS idx_memory_records_expires
    ON memory_records(expires_at);

-- "memórias superseded" (debug + audit).
CREATE INDEX IF NOT EXISTS idx_memory_records_superseded
    ON memory_records(superseded_by);

-- "memórias aguardando revisão humana" (painel da Etapa 5).
CREATE INDEX IF NOT EXISTS idx_memory_records_pending_review
    ON memory_records(pending_review);

-- "memórias sem embedding pronto" (worker da Etapa 2).
CREATE INDEX IF NOT EXISTS idx_memory_records_embedding_status
    ON memory_records(embedding_status);

-- ===========================================================================
-- FTS5 — busca lexical. `content='memory_records'` faz o FTS5 ler o
-- `content` da tabela externa. `tokenize='unicode61 remove_diacritics 2'`
-- remove acentos (bom pra português). Triggers mantêm em sincronia.
-- ===========================================================================
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    content,
    content='memory_records',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

-- Trigger: após insert em memory_records, indexa no FTS.
CREATE TRIGGER IF NOT EXISTS memory_fts_ai AFTER INSERT ON memory_records
BEGIN
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;

-- Trigger: após delete, remove do FTS.
CREATE TRIGGER IF NOT EXISTS memory_fts_ad AFTER DELETE ON memory_records
BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES('delete', old.rowid, old.content);
END;

-- Trigger: após update, re-indexa.
CREATE TRIGGER IF NOT EXISTS memory_fts_au AFTER UPDATE ON memory_records
BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES('delete', old.rowid, old.content);
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;

-- ===========================================================================
-- memory_embeddings — embeddings por (memory_id, provider, model).
-- PK composta permite reindexação por modelo sem perder histórico
-- (`ADR-0013 §1`).
-- ===========================================================================
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT NOT NULL REFERENCES memory_records(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    -- 4 bytes per f32 × dimensions. Decodificado pela Etapa 2
    -- (a Etapa 1 não consome — só o schema).
    vec_blob BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (memory_id, provider, model)
);

-- "embeddings de um provider/model" (ReindexWorker da Etapa 2).
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_provider_model
    ON memory_embeddings(provider, model);

-- ===========================================================================
-- embedding_reindex_jobs — log de jobs de reindexação
-- (`ADR-0013 §2`). Append-only; o painel da Etapa 5 lê daqui.
-- ===========================================================================
CREATE TABLE IF NOT EXISTS embedding_reindex_jobs (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    old_provider TEXT NOT NULL,
    old_model TEXT NOT NULL,
    new_provider TEXT NOT NULL,
    new_model TEXT NOT NULL,
    new_dimensions INTEGER NOT NULL,
    -- Snapshot do COUNT(*) no início do job (não atualiza
    -- durante — o total é a estimativa).
    total INTEGER NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK(status IN (
        'running', 'completed', 'failed', 'cancelled'
    ))
);

-- "jobs em andamento" (painel da Etapa 5 polea o último).
CREATE INDEX IF NOT EXISTS idx_embedding_reindex_jobs_status
    ON embedding_reindex_jobs(status, started_at);
