-- Migração 0006 — fila de aprovação de tool calls (Fase 3, Etapa 6).
--
-- Quando o `validate_tool_call` devolve `ApprovalRequired` (spec
-- `tool-registry-specification.md` §7.7 Passo 9), o
-- `RunExecutor` enfileira o `ApprovalRequest` aqui em vez de
-- abortar o run com erro. O run fica em `RunState::Paused`
-- (22 valores do `frederico_agent_engine::RunState`) até o
-- usuário responder via IPC (`AppOp::ApprovalRespond`).
--
-- Princípios:
-- 1. **Fila 1:N com `runs`.** Cada run pode ter 0+ approvals
--    pendentes. A Etapa 6 enfileira 1 por vez; múltiplas em
--    paralelo é a Etapa 6.x (subagentes).
-- 2. **`status` com 3 valores:** `pending` (aguardando
--    resposta), `approved` (usuário aprovou, o run continua),
--    `rejected` (usuário negou, o run finaliza com
--    `RunState::Failed` e `RunStatus::Failed`).
-- 3. **Append-ish com update de status.** O `request_json`
--    (ApprovalRequest serializado) é imutável; o `status` é
--    atualizado quando o usuário responde. A Etapa 6 não
--    permite editar a request — é approve/reject.
-- 4. **FK pra `runs` com `ON DELETE CASCADE`.** Se o run for
--    deletado (Fase 5/6), a fila vai junto.
-- 5. **Índice útil:** `(status, created_at)` pra "lista de
--    approvals pendentes em ordem cronológica" (a UI da
--    Etapa 6 consome).

CREATE TABLE IF NOT EXISTS approval_queue (
    -- ID opaco (UUID, mesmo padrão dos outros IDs).
    id TEXT PRIMARY KEY,

    -- Run ao qual a aprovação pertence.
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,

    -- Tool que pediu aprovação.
    tool_id TEXT NOT NULL,

    -- `ApprovalRequest` serializado em JSON (request original —
    -- não muda quando o usuário responde).
    request_json TEXT NOT NULL,

    -- `pending` | `approved` | `rejected`. Default `pending` na
    -- Etapa 6 (fila 1-por-run; múltiplas em paralelo é 6.x).
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'approved', 'rejected')),

    -- Decisão do usuário (None até responder). Serializa o
    -- `ApprovalDecision` (scope + approved + reason) quando
    -- o status muda.
    decision_json TEXT,

    -- Timestamp da enfileiramento. Default do SQLite.
    created_at TEXT NOT NULL DEFAULT (datetime('now')),

    -- Timestamp da resposta (None até responder).
    resolved_at TEXT
);

-- "lista de approvals pendentes em ordem cronológica"
CREATE INDEX IF NOT EXISTS idx_approval_queue_status_created
    ON approval_queue(status, created_at);

-- "todas as approvals de um run" (debug/replay)
CREATE INDEX IF NOT EXISTS idx_approval_queue_run
    ON approval_queue(run_id);
