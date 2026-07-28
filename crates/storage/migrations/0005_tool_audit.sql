-- Migração 0005 — auditoria de tool calls (Fase 3, Etapa 5.x, Passo 10).
--
-- O spec `tool-registry-specification.md` §7.7 lista 10 passos
-- pra validação de uma `tool_call`. Os 9 primeiros estão
-- implementados na Etapa 2/3 do tool-registry. O Passo 10 é a
-- **entrada de auditoria** — registrar toda `tool_call`
-- validada (aprovada, rejeitada, ou com erro de execução) com
-- timestamp, tool_id, arguments e resultado.
--
-- A Etapa 5.x introduz uma tabela dedicada `tool_audit` (1:N com
-- `runs`). Cada linha é uma `tool_call` que o `RunExecutor`
-- executou (ou tentou executar). A UI da Fase 5/6 consome isso
-- pra mostrar o "o que o agente fez" pra o usuário.
--
-- Princípios:
-- 1. **Append-only.** Não há update nem delete — auditoria não
--    pode ser reescrita (mitiga a ameaça I7 do `security-threat-model.md`).
-- 2. **Snapshot no momento da chamada.** O `arguments_json` é o
--    JSON exato que o modelo passou (sem normalização); o
--    `result_json` é a saída da ferramenta (ou erro estruturado).
--    Se o tool falhou, `result_json` carrega o código de erro
--    canônico do `ToolErrorCode`.
-- 3. **FK pra `runs` com `ON DELETE CASCADE`.** Se um run for
--    deletado (Fase 5/6 — privacidade), a auditoria vai junto.
-- 4. **Índices úteis:** `(run_id, created_at)` pra "lista de
--    tool_calls do run em ordem", e `(tool_id, created_at)` pra
--    "todas as vezes que `files.read` foi chamado".

CREATE TABLE IF NOT EXISTS tool_audit (
    -- ID opaco (UUID, mesmo padrão dos outros IDs).
    id TEXT PRIMARY KEY,

    -- Run ao qual a tool_call pertence.
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,

    -- Ferramenta chamada.
    tool_id TEXT NOT NULL,

    -- Versão do manifesto no momento da chamada (a Etapa 2 valida
    -- isso no Passo 2; aqui é o registro).
    tool_version TEXT NOT NULL,

    -- Argumentos como JSON (raw, sem normalização).
    arguments_json TEXT NOT NULL,

    -- `true` se a ferramenta executou com sucesso (`ToolResult.ok`).
    -- `false` se rejeitada pelo validador ou se a execução falhou.
    result_ok INTEGER NOT NULL CHECK(result_ok IN (0, 1)),

    -- Output conforme `output_schema` (sucesso) ou erro estruturado
    -- (`{"error_code": "TOOL_...", "error_message": "..."}`).
    result_json TEXT NOT NULL,

    -- Duração da execução em microssegundos. `0` se a validação
    -- rejeitou antes de executar (a Etapa 4.x.y também usa isso
    -- pra observabilidade — qual ferramenta demora mais?).
    duration_micros INTEGER NOT NULL DEFAULT 0,

    -- Timestamp da chamada. Default do SQLite.
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- "lista de tool_calls do run em ordem"
CREATE INDEX IF NOT EXISTS idx_tool_audit_run_created
    ON tool_audit(run_id, created_at);

-- "todas as vezes que `files.read` foi chamado"
CREATE INDEX IF NOT EXISTS idx_tool_audit_tool_created
    ON tool_audit(tool_id, created_at);
