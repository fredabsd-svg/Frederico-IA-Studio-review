-- Migração 0003 — motor de execução e checkpoints (Fase 3, Etapa 1).
--
-- A Fase 2 entregou a tabela `runs` com 6 status (created/running/
-- completed/failed/cancelled/timeout) e o `RunRegistry` em memória.
-- A Fase 3 substitui o modelo por uma máquina explícita de 22
-- estados (ver `agent-state-machine.md` §"Estados" e ADR-0009).
--
-- Princípios:
-- 1. **Estender, não renomear.** A coluna `runs.status` da Fase 2
--    continua existindo (6 valores, `CHECK` mantida) para não
--    regredir o `ChatOrchestrator` e os 3 testes E2E de recovery
--    (CHANGELOG Hardening 5). A nova coluna `runs.state` tem a
--    verdade canônica; a view `runs_with_status` mantém as duas
--    coerentes.
-- 2. **Tabela `runs` é 1:1 com `frederico-agent-engine::Run`.** A
--    migração adiciona as colunas do spec §"Contratos" do Run
--    (`current_step`, `budget_json`, `allowed_tools_json`,
--    `last_heartbeat_at`, `last_event_seq`, `provider_id`,
--    `model_id`, `assistant_id`).
-- 3. **Checkpoints em tabela separada.** Cada `checkpoint` é um
--    ponto de recuperação do `Run` (spec §6.2). A Etapa 4 grava
--    neles; a Etapa 5 lê para recuperar de crash.
-- 4. **View `runs_with_status` para compatibilidade.** O `RunRepo`
--    da Fase 2 lê dessa view; a Fase 3 lê a tabela `runs` direto.
--    O `status` derivado usa a tabela de mapeamento do ADR-0009 §3.

-- ----------------------------------------------------------------------------
-- Extensão da tabela `runs`
-- ----------------------------------------------------------------------------

-- 22 estados da máquina (ver `agent-state-machine.md` §"Estados" e
-- `frederico-agent-engine::RunState`). Os comentários em cada
-- literal são da Etapa 1 e podem ser removidos se ficar barulhento.
ALTER TABLE runs ADD COLUMN state TEXT NOT NULL DEFAULT 'created'
    CHECK(state IN (
        'created',                       -- Run criado, ainda não enfileirado
        'queued',                        -- Em fila, aguardando o escalonador
        'preparing_context',             -- Construindo o contexto da conversa
        'retrieving_memory',             -- Buscando memórias relevantes
        'validating_capabilities',       -- Validando modelo + tools + perms
        'calling_model',                 -- Chamada HTTP inicial ao provedor
        'streaming',                     -- Stream aberto, recebendo deltas
        'waiting_tool_call',             -- Modelo pediu uma ferramenta
        'validating_tool_call',          -- Validador aplicando os 10 passos
        'waiting_user_approval',         -- Fila de aprovação do usuário
        'executing_tool',                -- Ferramenta em execução
        'validating_tool_result',        -- Validando o ToolResult
        'continuing_model',              -- Voltando a chamar o modelo
        'generating_artifact',           -- Modelo pediu geração de artefato
        'validating_artifact',           -- Arquivo escrito, em validação
        'checkpointing',                 -- Serializando o checkpoint
        'retrying',                      -- Artefato inválido, pedindo de novo
        'paused',                        -- Usuário pausou, recuperável
        'completed',                     -- Terminal: sucesso
        'failed',                        -- Terminal: erro irrecuperável
        'cancelled',                     -- Terminal: usuário cancelou
        'interrupted'                    -- Terminal: watchdog / crash
    ));

-- Iteração atual do loop `calling_model` → ... → `continuing_model`.
-- Incrementado pelo executor da Etapa 4. Mitiga a ameaça D2
-- (modelo em loop) do `security-threat-model.md`.
ALTER TABLE runs ADD COLUMN current_step INTEGER NOT NULL DEFAULT 0;

-- Teto de uso do `Run`, serializado como JSON. Tipo: `Budget` em
-- `frederico-agent-engine`. Aplicação do budget mora no executor
-- (Etapa 4).
ALTER TABLE runs ADD COLUMN budget_json TEXT NOT NULL DEFAULT '{}';

-- Ferramentas permitidas para este `Run` específico, serializadas
-- como JSON. Tipo: `Vec<ToolId>` em `frederico-agent-engine`. A
-- Etapa 2 (tool-registry) calcula a interseção final e a Etapa 4
-- grava aqui.
ALTER TABLE runs ADD COLUMN allowed_tools_json TEXT NOT NULL DEFAULT '[]';

-- Última vez que o `Run` demonstrou estar vivo. Atualizado a cada
-- operação custosa pelo executor (Etapa 4) e pelo watchdog (Etapa 5).
ALTER TABLE runs ADD COLUMN last_heartbeat_at TEXT NOT NULL DEFAULT (datetime('now'));

-- Próximo `seq` a usar no journal. Monotônico por `run_id`. A Etapa 4
-- incrementa antes de cada `INSERT INTO message_events`.
ALTER TABLE runs ADD COLUMN last_event_seq INTEGER NOT NULL DEFAULT 0;

-- Provedor de LLM que está executando este `Run`. Espelha o
-- `conversations.provider_id` mas pode divergir (Etapa 6 — assistentes
-- com provedor dedicado, sem precisar mudar a conversa).
ALTER TABLE runs ADD COLUMN provider_id TEXT NOT NULL DEFAULT '';

-- Modelo dentro do provedor.
ALTER TABLE runs ADD COLUMN model_id TEXT NOT NULL DEFAULT '';

-- Assistente (perfil de sistema + ferramentas permitidas). Nullable
-- até a Etapa 6 (UI de assistentes).
ALTER TABLE runs ADD COLUMN assistant_id TEXT;

-- Índice útil para o watchdog da Etapa 5: "todos os runs não-terminais
-- com `last_heartbeat_at < threshold`".
CREATE INDEX IF NOT EXISTS idx_runs_state_heartbeat
    ON runs(state, last_heartbeat_at)
    WHERE state NOT IN ('completed', 'failed', 'cancelled', 'interrupted');

-- ----------------------------------------------------------------------------
-- Tabela `checkpoints`
-- ----------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS checkpoints (
    -- CheckpointId (opaco, gerado em `frederico_core`).
    id TEXT PRIMARY KEY,

    -- Run ao qual o checkpoint pertence.
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,

    -- `seq` do último evento do `Run` no momento do checkpoint. Útil
    -- pra "checkpoint cobre eventos com `seq <= X`".
    seq INTEGER NOT NULL,

    -- Estado do `Run` no momento do checkpoint (espelha `runs.state`).
    state TEXT NOT NULL CHECK(state IN (
        'created', 'queued', 'preparing_context', 'retrieving_memory',
        'validating_capabilities', 'calling_model', 'streaming',
        'waiting_tool_call', 'validating_tool_call',
        'waiting_user_approval', 'executing_tool',
        'validating_tool_result', 'continuing_model',
        'generating_artifact', 'validating_artifact', 'checkpointing',
        'retrying', 'paused', 'completed', 'failed', 'cancelled',
        'interrupted'
    )),

    -- Estado do executor serializado (formato definido na Etapa 4 —
    -- o mínimo viável na Etapa 1 é `{"seq": N, "state": "..."}`).
    payload_json TEXT NOT NULL,

    -- Timestamp da gravação. Default do SQLite.
    created_at TEXT NOT NULL DEFAULT (datetime('now')),

    -- Garante que não há dois checkpoints com o mesmo `seq` para o
    -- mesmo `Run`. A Etapa 4 decide se escreve com `INSERT OR
    -- REPLACE` (atômico) ou `INSERT` (falha visível).
    UNIQUE(run_id, seq)
);

-- Índice do "último checkpoint do Run". A Etapa 5 (recovery) usa
-- este índice para reconstruir o estado no boot.
CREATE INDEX IF NOT EXISTS idx_checkpoints_run_seq_desc
    ON checkpoints(run_id, seq DESC);

-- ----------------------------------------------------------------------------
-- View `runs_with_status` — compatibilidade com a Fase 2
-- ----------------------------------------------------------------------------
--
-- O `RunRepo` da Fase 2 lê `runs.status` (6 valores) para alimentar
-- o `ChatOrchestrator` e os testes E2E de recovery. Esta view mantém
-- a Fase 2 funcionando sem mexer no código de aplicação: ela
-- devolve os campos de `runs` + uma coluna `status` derivada do
-- `state` (22 valores).
--
-- Tabela de mapeamento (ADR-0009 §3):
--   created, queued                                     → created
--   16 estados não-terminais de execução                → running
--   paused                                              → running
--   completed                                           → completed
--   failed                                              → failed
--   cancelled                                           → cancelled
--   interrupted                                         → timeout
--
-- A Etapa 5 (watchdog) pode refinar `interrupted` separando-o
-- definitivamente de `timeout` se a UI precisar; por enquanto, o
-- agrupamento é o mais conservador.
CREATE VIEW IF NOT EXISTS runs_with_status AS
SELECT
    r.id,
    r.conversation_id,
    r.message_id,
    CASE r.state
        WHEN 'created' THEN 'created'
        WHEN 'queued' THEN 'created'
        WHEN 'completed' THEN 'completed'
        WHEN 'failed' THEN 'failed'
        WHEN 'cancelled' THEN 'cancelled'
        WHEN 'interrupted' THEN 'timeout'
        ELSE 'running'
    END AS status,
    r.state,
    r.started_at,
    r.finished_at,
    r.cancellation_requested_at,
    r.current_step,
    r.budget_json,
    r.allowed_tools_json,
    r.last_heartbeat_at,
    r.last_event_seq,
    r.provider_id,
    r.model_id,
    r.assistant_id
FROM runs r;
