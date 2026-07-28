-- Migração 0004 — `kind = 'tool_result'` no journal (Fase 3, Etapa 4).
--
-- A Fase 2 deixou a CHECK constraint aceitando 'delta', 'usage',
-- 'tool_call', 'done', 'error', 'cancelled'. A Etapa 4 adiciona
-- 'tool_result' (a resposta da ferramenta que volta pro modelo).
--
-- Princípios:
-- 1. A Etapa 4 popula `tool_result` no `MessageEventRepo::append`
--    com o output do `Tool::execute` (em PT-BR com ação, se erro,
--    conforme spec `chat-and-providers.md`).
-- 2. Cada `tool_call` tem exatamente um `tool_result` emparelhado
--    (a Etapa 4 valida isso no executor e quebra o run se violar).
-- 3. O conteúdo do `tool_result` é a saída conforme o
--    `output_schema` do manifesto (quando `ok`), ou uma
--    mensagem PT-BR (quando erro).

-- SQLite não tem ALTER TABLE para CHECK constraints; precisamos
-- recriar a tabela. O caminho:
-- 1. PRAGMA foreign_keys = OFF (precisa ser antes de qualquer
--    transação da migração — `sqlx::migrate!` abre BEGIN
--    automaticamente; PRAGMA no nível de conexão toma efeito
--    imediato).
-- 2. Criar tabela nova com a constraint atualizada.
-- 3. Copiar os dados.
-- 4. Dropar a antiga.
-- 5. Renomear a nova.
-- 6. PRAGMA foreign_keys = ON (reativa pro resto da conexão).
-- 7. Recriar o índice idx_message_events_message_seq.
--
-- **Importante:** o `BEGIN TRANSACTION;` / `COMMIT;` explícito foi
-- removido em favor do que o `sqlx::migrate!` já abre
-- automaticamente — o `BEGIN` aninhado causava "cannot start a
-- transaction within a transaction". A atomicidade é mantida pelo
-- `sqlx::migrate!`.

PRAGMA foreign_keys = OFF;

CREATE TABLE message_events_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('delta', 'usage', 'tool_call', 'tool_result', 'done', 'error', 'cancelled')),
    data TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(message_id, seq)
);
INSERT INTO message_events_new (id, message_id, seq, kind, data, created_at)
SELECT id, message_id, seq, kind, data, created_at FROM message_events;
DROP TABLE message_events;
ALTER TABLE message_events_new RENAME TO message_events;

CREATE INDEX IF NOT EXISTS idx_message_events_message_seq
    ON message_events(message_id, seq);

PRAGMA foreign_keys = ON;
