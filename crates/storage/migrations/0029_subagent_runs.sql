-- Migração 0029 — Etapa 4 da Fase 6 (subagentes, PR 1: infraestrutura)
--
-- A Etapa 1 da Fase 6 decidiu no ADR-0027 o **portão de spawn** que
-- protege contra gasto recursivo descontrolado: teto global de 8
-- subagentes por run (D1), profundidade 2 (D2), budget herdado e
-- descontado do pai (D3), verificação no spawn com erro legível (D4),
-- `BudgetAllocation` como única superfície de alocação (D5), teto de
-- modelo fora de escopo (D6).
--
-- Esta migração entrega a **infra de banco** da Etapa 4:
--
-- 1. Adiciona colunas em `runs` que todo `Run` carrega (raiz OU
--    subagente): `subagent_count` (contador global de filhos vivos,
--    D1), `depth` (profundidade na árvore, 0 = raiz, D2),
--    `parent_run_id` (FK opcional pra raiz), `spent_microcents` /
--    `spent_tokens_in` / `spent_tokens_out` / `spent_steps`
--    (cópia do `SpentBudget` da Etapa 4 pra invariante de soma do
--    D3 e o desconto unidirecional do orçamento).
--
-- 2. Cria a tabela `subagent_runs` que registra **um subagente
--    específico** (parent_run_id + subagent_run_id + specialist_id +
--    allocation_json + spent_microcents + started_at + finished_at +
--    state). Cada subagente aparece **aqui** com sua alocação; a
--    coluna `subagent_count` em `runs` é o contador agregado
--    (pra checagem rápida de D1 no spawn).
--
-- Princípios:
--
-- - **Estender, não renomear.** Colunas novas têm default seguro
--   (0 / 0 / NULL) — o Run raiz continua funcionando sem migração
--   de dados. A Etapa 4 PR 2 (spawn real) vai popular via
--   `RunRepo::increment_subagent_count` e `record_subagent_run` no
--   `SubagentRunner::try_spawn`.
--
-- - **FK explícita.** `parent_run_id` em `subagent_runs` aponta pra
--   `runs.id` com `ON DELETE CASCADE` — se o run raiz for deletado,
--   os subagentes vão junto. A coluna `parent_run_id` em `runs` é
--   só redundância (denormalização) pra queries rápidas.
--
-- - **Mesma família de invariante do `RunEvent` journal** (migração
--   0027): `UNIQUE(parent_run_id, subagent_run_id)` em
--   `subagent_runs` impede que o mesmo subagente seja registrado 2x
--   pelo mesmo pai (anti-exploit do spawn paralelo otimista que a
--   Etapa 1 rejeitou — alternativa 4 do ADR-0027).
--
-- Por que não usar a própria `runs` pra tudo (em vez de tabela
-- separada)? O Run raiz e o subagente **são** o mesmo tipo de
-- domínio (`Run` com 23 estados), e o subagente é uma linha de
-- `runs` igual ao pai. A tabela `subagent_runs` carrega só o que é
-- **específico** do subagente (parent_run_id, specialist_id,
-- allocation, spent_microcents) — o resto está em `runs`. Mesma
-- estratégia do `RunEvent` (journal separado, FK pra runs) e do
-- `MessageEvent` (eventos do `Message` ficam em tabela própria).

-- ----------------------------------------------------------------------------
-- Extensão da tabela `runs` — campos do subagente + spent
-- ----------------------------------------------------------------------------

-- Contador de subagentes vivos pra este Run (D1 do ADR-0027:
-- "verificação no spawn": o `SubagentRunner` consulta
-- `subagent_count + 1 <= 8` antes de criar).
ALTER TABLE runs ADD COLUMN subagent_count INTEGER NOT NULL DEFAULT 0;

-- Profundidade na árvore de subagentes (D2 do ADR-0027: 0 pra
-- Run raiz; 1 pra filho direto; 2 bloqueado).
ALTER TABLE runs ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;

-- `parent_run_id` é NULL pro Run raiz (depth = 0). Pra
-- subagentes, aponta pro Run pai. Indexado pra queries tipo
-- "todos os subagentes deste Run pai".
ALTER TABLE runs ADD COLUMN parent_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE;

-- Gasto efetivo do Run (cópia do `SpentBudget` da Etapa 4).
-- **Cópia denormalizada**: a fonte da verdade é o executor a
-- cada iteração, mas o invariante de soma do D3 ("Σ filhos
-- ≤ pai.remaining_inicial − pai.gasto_atual") precisa de
-- leitura rápida por linha. Defaults zero = Run novo, sem
-- gasto.
ALTER TABLE runs ADD COLUMN spent_microcents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN spent_tokens_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN spent_tokens_out INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN spent_steps INTEGER NOT NULL DEFAULT 0;

-- Índice pra queries "todos os subagentes vivos do Run pai" e
-- "todos os Runs em profundidade = 1" (anti-explosão: watchdog
-- da Etapa 5 consulta por profundidade pra detectar anomalias).
CREATE INDEX IF NOT EXISTS idx_runs_parent_run_id
    ON runs(parent_run_id)
    WHERE parent_run_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_runs_depth
    ON runs(depth)
    WHERE depth > 0;

-- ----------------------------------------------------------------------------
-- Tabela `subagent_runs` — registro explícito de cada subagente
-- ----------------------------------------------------------------------------
--
-- Cada subagente tem uma linha aqui com sua alocação (D5 do
-- ADR-0027: `BudgetAllocation` é a única superfície de alocação)
-- e seu gasto efetivo (parte do desconto unidirecional do D3).
--
-- O `id` (PK) é o mesmo `RunId` do subagente (também tem linha
-- em `runs` com `parent_run_id` apontando pro pai). Mantém a
-- invariante "Run raiz e subagente são o mesmo tipo de domínio"
-- sem precisar de tipo `SubagentRunId` separado (o `RunId` já
-- carrega a identidade — `depth` e `parent_run_id` carregam o
-- papel).

CREATE TABLE IF NOT EXISTS subagent_runs (
    -- `RunId` do subagente. PK porque cada subagente tem 1 linha
    -- (registro 1:1 entre `runs` e `subagent_runs` quando o Run
    -- é um subagente, ou seja, `parent_run_id IS NOT NULL`).
    id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,

    -- `RunId` do Run pai. NÃO é a PK porque o mesmo pai pode ter
    -- N subagentes — a UNIQUE é composta (id, parent_run_id) na
    -- prática, mas como `id` já é PK, a UNIQUE(parent_run_id,
    -- id) é o que impede um mesmo subagente ser registrado 2x
    -- pelo mesmo pai.
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,

    -- `SpecialistId` que este subagente está executando. Nullable
    -- porque a Etapa 4 PR 2 vai popular no `try_spawn`; até lá,
    -- o PR 1 só entrega a infra (PR separado pela lição do
    -- PR #25 / PR #34).
    specialist_id TEXT,

    -- Profundidade (denormalização de `runs.depth` pra queries
    -- "todos os subagentes profundidade 1" sem JOIN).
    depth INTEGER NOT NULL CHECK(depth IN (1, 2)),

    -- `BudgetAllocation` serializado em JSON (D5). Estrutura:
    -- `{max_steps, max_tokens_in, max_tokens_out,
    --  max_cost_microcents, max_wall_clock_secs}`. Mesmo shape
    -- do `Budget` por enquanto — a Etapa 4 PR 2 pode decidir se
    -- subset (sem `max_wall_clock` por exemplo) é suficiente.
    allocation_json TEXT NOT NULL,

    -- Gasto efetivo do subagente (cópia do `runs.spent_*` pra
    -- queries agregadas "quanto os subagentes deste pai gastaram
    -- no total"). Default 0 = subagente sem gasto.
    spent_microcents INTEGER NOT NULL DEFAULT 0,
    spent_tokens_in INTEGER NOT NULL DEFAULT 0,
    spent_tokens_out INTEGER NOT NULL DEFAULT 0,
    spent_steps INTEGER NOT NULL DEFAULT 0,

    -- Instante do spawn (criação da linha).
    started_at TEXT NOT NULL DEFAULT (datetime('now')),

    -- Instante do término (sucesso, falha, cancelamento, ou
    -- `depth > 2` rejeitado). NULL enquanto o subagente está
    -- vivo.
    finished_at TEXT,

    -- Estado do subagente (mesmo enum `RunState` que o Run raiz
    -- — 23 valores). Carregado em `runs.state` (PK espelha);
    -- mantido aqui pra queries agregadas sem JOIN.
    state TEXT NOT NULL DEFAULT 'created'
        CHECK(state IN (
            'created', 'queued', 'preparing_context', 'retrieving_memory',
            'validating_capabilities', 'calling_model', 'streaming',
            'waiting_tool_call', 'validating_tool_call',
            'waiting_user_approval', 'executing_tool',
            'validating_tool_result', 'continuing_model',
            'generating_artifact', 'validating_artifact', 'checkpointing',
            'retrying', 'paused', 'completed', 'failed', 'cancelled',
            'interrupted'
        )),

    -- Anti-exploit: o mesmo subagente (id) não pode ser
    -- registrado 2x pelo mesmo pai. Defesa em profundidade
    -- contra o spawn otimista (rejeitado pela Etapa 1, alt 4
    -- do ADR-0027).
    UNIQUE(parent_run_id, id)
);

-- Índice pra queries "todos os subagentes vivos deste pai"
-- (não-terminais, sem `finished_at`).
CREATE INDEX IF NOT EXISTS idx_subagent_runs_parent_active
    ON subagent_runs(parent_run_id, state)
    WHERE finished_at IS NULL;

-- Índice pra queries de auditoria "todos os subagentes deste
-- especialista" (relatórios, debugging).
CREATE INDEX IF NOT EXISTS idx_subagent_runs_specialist_id
    ON subagent_runs(specialist_id)
    WHERE specialist_id IS NOT NULL;
