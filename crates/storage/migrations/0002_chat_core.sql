-- Migração 0002 — núcleo de chat (Fase 2).
--
-- Adiciona as tabelas de domínio: conversas, mensagens, eventos de
-- mensagem (o "journal" que permite recarregar a janela no meio do
-- stream), runs e configuração de provedores.
--
-- Princípios:
-- 1. **Append-only.** Mensagens e eventos nunca são editados; correções
--    do usuário viram mensagens novas (Fase 4 Memória).
-- 2. **Sem segredo.** `provider_configs` guarda só `configured: bool`
--    e metadados. A chave vive no `CredentialStore` (DPAPI / Windows
--    Credential Manager), nunca no SQLite.
-- 3. **Journal primeiro.** Cada `StreamEvent` do adapter é gravado
--    em `message_events` ANTES de ser emitido para a janela Tauri
--    (ver `docs/architecture/chat-and-providers.md` §"Journal").
-- 4. **Custo em microcents.** `u64`, sem ponto flutuante no banco.

-- Conversa. Modelo e provedor são **por conversa** (mutáveis).
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    title TEXT,
    provider_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    total_cost_microcents INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_conversations_updated_at
    ON conversations(updated_at DESC);

-- Mensagem. Role pode ser user/assistant/system. Status segue
-- `MessageStatus` no spec.
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
    content TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK(status IN ('pending', 'streaming', 'completed', 'failed', 'cancelled', 'timeout')),
    run_id TEXT,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    cost_microcents INTEGER NOT NULL DEFAULT 0,
    error TEXT,                            -- JSON do ProviderErrorView (PT-BR + ação)
    created_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_created
    ON messages(conversation_id, created_at);

CREATE INDEX IF NOT EXISTS idx_messages_run
    ON messages(run_id)
    WHERE run_id IS NOT NULL;

-- Journal de eventos por mensagem. Sequência monotônica por
-- `message_id`. **Append-only** — a Etapa 5 da casca lê daqui pra
-- reconstruir uma mensagem em `streaming` no reload de janela.
CREATE TABLE IF NOT EXISTS message_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('delta', 'usage', 'tool_call', 'done', 'error', 'cancelled')),
    data TEXT NOT NULL,                    -- JSON do payload específico
    created_at TEXT NOT NULL,
    UNIQUE(message_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_message_events_message_seq
    ON message_events(message_id, seq);

-- Run: 1:1 com a Message Assistant que disparou. Status segue
-- `RunStatus` no spec.
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('created', 'running', 'completed', 'failed', 'cancelled', 'timeout')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    cancellation_requested_at TEXT
);

-- Configuração pública de um provedor. **Sem segredo** — `configured`
-- é a única coisa que a UI consome daqui.
CREATE TABLE IF NOT EXISTS provider_configs (
    provider_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    configured INTEGER NOT NULL DEFAULT 0,  -- boolean (0/1)
    last_ok_at TEXT,
    last_error_at TEXT,
    last_error TEXT                          -- PT-BR, sem a chave
);
