-- Migração 0027 — `run_events` como journal de transições (Fase 6, Etapa 2).
--
-- A Fase 3 Etapa 1 introduziu `RunEvent`/`RunEventKind`/`RunEvent.payload`
-- em `crates/agent-engine/src/event.rs` (zero-chamada fora do crate até a
-- Etapa 4 da Fase de Ligação fechar com `apply_transition` documentado
-- como "não-exercitado no produto" — ADR-0025 §Fato). Esta migração
-- materializa a tabela no SQLite, fecha o portão único de transição
-- (ADR-0029 §D1) e a deixa pronta pra ser gravada em cada
-- `state_mapping` do `RunExecutor`.
--
-- Princípios:
-- 1. `seq` é monotonicamente crescente por `run_id`. A constraint
--    `UNIQUE(run_id, seq)` no SQLite é a garantia mecânica — duas
--    threads concorrentes pegando o mesmo `seq` causam `INSERT` falha
--    e o `RunEventRepo` retorna `StorageError`. O `RunExecutor` é
--    single-thread por run (mesma garantia que `MessageEventRepo`).
-- 2. `from_state` e `to_state` são `Option<RunState>` (nullable). Eventos
--    sem mudança de estado (`Usage` no stream, por exemplo) gravam
--    `from = to = current`. A invariante do `apply_transition` é que
--    `from != to` quando há aresta; a Etapa 4 (subagente) consome
--    `from != to` pra detectar transições reais vs no-op.
-- 3. `kind` aceita todas as 25 variantes de `RunEventKind`
--    (20 estruturais + 5 globais). A Etapa 2 não muda o enum; uma
--    variante nova (ex.: `RejectedInvalid`) só entra com ADR.
-- 4. `payload_json` é `serde_json::Value` opaco (mesma estratégia do
--    `RunEvent.payload` no `agent-engine`). Eventos diferentes
--    carregam dados diferentes: `FirstToken` carrega o primeiro
--    pedaço, `ToolCallEmitted` carrega `tool_call_id`, `UserCancel`
--    é vazio. A invariante `REGRAS §1.3` é que mudança de contrato
--    exige atualização do spec no mesmo commit, e adicionar uma
--    variante de `RunEventKind` já passa por esse gate.
-- 5. **Coluna `run_seq` em `message_events`** (nullable, índice
--    separado): quando o `RunExecutor` grava um `RunEvent` com
--    `seq=N`, o `MessageEvent` correspondente recebe `run_seq=N`
--    retroativamente. Isso permite que a UI do Modo Equipe (Etapa 6)
--    faça `LEFT JOIN message_events me ON me.run_seq = re.seq` para
--    renderizar a linha do tempo de estados com o conteúdo de cada
--    estado. A coluna é nullable pra não quebrar runs antigos
--    (criados antes da Etapa 2).

PRAGMA foreign_keys = OFF;

-- Tabela principal: journal de transições do Run.
CREATE TABLE run_events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    from_state TEXT,
    to_state TEXT,
    timestamp_ms INTEGER NOT NULL,
    payload_json TEXT NOT NULL DEFAULT 'null',
    UNIQUE (run_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_run_events_run_id_seq ON run_events (run_id, seq);
CREATE INDEX IF NOT EXISTS idx_run_events_kind ON run_events (kind);

-- Coluna `run_seq` em `message_events` (nullable; join com RunEvent).
-- SQLite não tem `ALTER TABLE ... ADD COLUMN ... NOT NULL DEFAULT` para
-- valor não-constante, mas aceita `NOT NULL DEFAULT NULL` que é o
-- trivial: as linhas antigas ficam `run_seq = NULL` e a Etapa 4 do
-- recovery pula essas (a query lê `WHERE run_seq IS NOT NULL`).
ALTER TABLE message_events ADD COLUMN run_seq INTEGER;

CREATE INDEX IF NOT EXISTS idx_message_events_run_seq
    ON message_events (message_id, run_seq);

PRAGMA foreign_keys = ON;
