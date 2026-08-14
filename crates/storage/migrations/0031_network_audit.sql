-- Migração 0031 — auditoria de acesso de rede do sandbox (Fase 7, Etapa 6).
--
-- O proxy do sandbox (Etapa 6 da Fase 7, ADR-0033) registra toda
-- tentativa de acesso HTTP/CONNECT num sink de audit. Esta tabela
-- é a implementação concreta (`DbNetworkAuditSink` em
-- `crates/storage/src/network_audit.rs`).
--
-- Princípios (mesmos do `tool_audit`, migração 0005):
--
-- 1. **Append-only.** Não há update nem delete — auditoria não
--    pode ser reescrita (mitiga a ameaça I7 do
--    `security-threat-model.md`).
-- 2. **Snapshot no momento da chamada.** Os campos do
--    `NetworkAccessEntry` (host, port, method, path_redacted,
--    status_code, bytes_sent, bytes_received, decision,
--    deny_reason, timestamp) são o que o proxy gravou na hora.
-- 3. **FK pra `runs` com `ON DELETE CASCADE`.** Se um run for
--    deletado, a auditoria vai junto.
-- 4. **`path_redacted` é a coluna de path** — sempre armazena o
--    path **sem query string** (D4 do ADR-0033). Em HTTPS via
--    `CONNECT`, é literalmente `'<redacted>'` porque o proxy não
--    enxerga o path (TLS é opaco). Honesto: o log não promete
--    mais do que entrega.
-- 5. **Índices úteis:** `(run_id, created_at)` pra "lista de
--    acessos do run em ordem cronológica" (UI da Etapa 7 da
--    Fase 7 mostra isso na aba "Sandbox"), e `(host, created_at)`
--    pra "todas as tentativas para um host específico"
--    (investigação pós-incidente).
--
-- **Por que `decision` e `deny_reason` separados:**
-- `decision` é o resultado binário (`allow`/`deny`) — indexável
-- como filtro. `deny_reason` é o porquê textual
-- (`not_in_allowlist`/`allowlist_empty`/`bad_request`/`timeout`/
-- `upstream_unreachable`) — indexável mas mais pra debug. Os
-- dois são texto (não enum) pra evitar CHECK constraint que
-- quebra quando ganhamos novas razões (mesma decisão do
-- `runs.state`).

CREATE TABLE IF NOT EXISTS network_audit (
    -- ID opaco (UUID, mesmo padrão dos outros IDs).
    id TEXT PRIMARY KEY,

    -- Run ao qual o acesso pertence. `NULL` se o proxy for usado
    -- fora de contexto de run (ex.: health check, futuro
    -- `NetworkAccessLog` independente).
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,

    -- Host que o cliente pediu (sem porta, lowercased).
    host TEXT NOT NULL,

    -- Porta que o cliente pediu. `0` se o parse falhou
    -- (bad_request) — útil pra distinguir "tentou conectar"
    -- de "request malformado".
    port INTEGER NOT NULL DEFAULT 0,

    -- Método (`GET`/`POST`/etc.) ou `CONNECT` para HTTPS.
    method TEXT NOT NULL,

    -- Path **sem query string** (D4). Em HTTPS via CONNECT, é
    -- `'<redacted>'` (o TLS é opaco pro proxy).
    path_redacted TEXT NOT NULL,

    -- Código de status. `0` se o proxy fechou antes de falar
    -- com o upstream (deny ou timeout).
    status_code INTEGER NOT NULL DEFAULT 0,

    -- Bytes enviados pelo cliente (request body).
    bytes_sent INTEGER NOT NULL DEFAULT 0,

    -- Bytes recebidos do upstream (response body). `0` se o
    -- proxy nunca chegou a falar com o upstream.
    bytes_received INTEGER NOT NULL DEFAULT 0,

    -- Decisão final. `allow` ou `deny`.
    decision TEXT NOT NULL CHECK (decision IN ('allow', 'deny')),

    -- Razão do deny (`NULL` se `decision = 'allow'`). Ver doc
    -- do `NetworkAccessEntry.deny_reason` em
    -- `crates/security/src/network.rs` pra lista estável.
    deny_reason TEXT,

    -- Timestamp ISO 8601. Texto (não INTEGER epoch) pra ficar
    -- legível em `sqlite3` CLI (`SELECT created_at FROM
    -- network_audit`).
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- "lista de acessos do run em ordem cronológica" — UI da Etapa 7
-- da Fase 7.
CREATE INDEX IF NOT EXISTS idx_network_audit_run_created
    ON network_audit(run_id, created_at);

-- "todas as tentativas para um host específico" — investigação
-- pós-incidente.
CREATE INDEX IF NOT EXISTS idx_network_audit_host_created
    ON network_audit(host, created_at);

-- "todos os denies" — filtro pra UI ("o que foi bloqueado").
CREATE INDEX IF NOT EXISTS idx_network_audit_decision_created
    ON network_audit(decision, created_at);
