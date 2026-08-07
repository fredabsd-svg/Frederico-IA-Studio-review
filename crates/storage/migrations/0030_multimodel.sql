-- Etapa 5 da Fase 6 (Pipeline Sequencial, ADR-0028).
--
-- Cria as 3 tabelas que persistem o estado do `MultimodelRun`
-- (sequência de `MultimodelStage`s) e dos artefatos produzidos
-- pelos stages. **Sobrevive a restart do app** (D5 do ADR-0028) —
-- ao reabrir, o `MultimodelOrchestrator` carrega runs em estado
-- `Running`/`Streaming`/`WaitingToolCall` e oferece continuação
-- (botão "retomar pipeline interrompido" no Modo Equipe).
--
-- ## Por que 3 tabelas
--
-- **multimodel_runs** (1 linha por pipeline): cabeçalho do
-- pipeline (parent_run_id, mode, state). `mode` é o enum
-- `MultimodelMode` (Etapa 5: só `Pipeline`; Etapas futuras
-- podem plugar `Comparison`/`Conselho`/`Debate` per ADR-0028 §D3).
--
-- **multimodel_stages** (1 linha por stage): cada stage do
-- pipeline. Carrega `input_artifact_id` (FK pros artefatos de
-- entrada — `None` no primeiro stage), `output_artifact_id` (FK
-- pro artefato de saída), `input_hash` / `output_hash` (SHA-256
-- pra detectar reuso do stage quando input não muda — D6 do
-- ADR-0028), `cost_microcents` (custo do stage, alimentado
-- pelo provider-engine via `descriptor.cost_microcents(p, c)`),
-- `tools_used_json` (lista de `ToolId`s que o stage chamou), e
-- `validation_json` (resultado do validador, se o stage
-- declarou um).
--
-- **multimodel_artifacts** (1 linha por artefato): os arquivos
-- produzidos pelos stages. `content_ref` aponta pro arquivo
-- (workspace-relative, validado pelo `Jail` da conversa — Etapa
-- 1 da Fase de Ligação, ADR-0022 §D3). `hash` é SHA-256 do
-- conteúdo (mesmo que `output_hash` do stage, mas armazenado no
-- artefato pra validar reuso entre pipelines diferentes).
--
-- ## Por que `UNIQUE(run_id, seq)` em `multimodel_stages`
--
-- O `(run_id, seq)` é a chave natural: stages são ordenados por
-- `seq` dentro do mesmo pipeline. `UNIQUE` previne que o
-- orchestrator crie 2 stages com o mesmo seq (quebraria a
-- ordenação).
--
-- ## Por que `FOREIGN KEY ... ON DELETE CASCADE`
--
-- Quando um `MultimodelRun` é deletado, os stages e artefatos vão
-- junto. Sem o cascade, o storage fica com linhas órfãs (sem
-- sentido — o pipeline raiz não existe mais).
--
-- ## Por que `tools_used_json` em vez de tabela `multimodel_stage_tools`
--
-- Mesma decisão do `allowed_tools_json` do `runs`: o array
-- costuma ter 0-3 elementos. Tabela separada seria overhead de
-- JOIN pra um campo que raramente é lido fora do "show do
-- pipeline" da UI da Etapa 6. JSON é o trade-off
-- escolhido.
--
-- ## Por que `state` como `TEXT` (não enum do SQLite)
--
-- Mesmo padrão do `runs.state` (Etapa 1 da Fase 3): o `state`
-- do stage é o mesmo `RunState` do `agent-engine` (23 valores).
-- Manter como `TEXT` evita CHECK constraint que quebra
-- quando o enum ganha variante (Etapa 2 da Fase 3 introduziu
-- o CHECK; a Etapa 2 da Fase 6 (PR #30) trocou por TEXT quando
-- a enum cresceu).
--
-- ## Por que `multimodel_artifacts.size_bytes` separado do `hash`
--
-- `size_bytes` é o tamanho do arquivo em disco (informação
-- operacional — UI mostra "5 KB" ao lado do nome). `hash` é
-- SHA-256 do conteúdo (validação). Separar evita "inferir
-- tamanho do hash" (hash é fixo, tamanho varia).

PRAGMA foreign_keys = ON;

-- ----------------------------------------------------------------------------
-- multimodel_runs
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS multimodel_runs (
    id                TEXT    PRIMARY KEY NOT NULL,
    parent_run_id     TEXT    NOT NULL,
    mode              TEXT    NOT NULL CHECK (mode IN ('pipeline', 'comparison', 'council', 'debate')),
    state             TEXT    NOT NULL,
    input_artifact_id TEXT,
    final_artifact_id TEXT,
    total_cost_microcents INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL,
    FOREIGN KEY (parent_run_id)     REFERENCES runs(id) ON DELETE CASCADE,
    FOREIGN KEY (input_artifact_id) REFERENCES multimodel_artifacts(id) ON DELETE SET NULL,
    FOREIGN KEY (final_artifact_id) REFERENCES multimodel_artifacts(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_multimodel_runs_parent_run_id ON multimodel_runs(parent_run_id);
CREATE INDEX IF NOT EXISTS idx_multimodel_runs_state ON multimodel_runs(state);
CREATE INDEX IF NOT EXISTS idx_multimodel_runs_updated_at ON multimodel_runs(updated_at);

-- ----------------------------------------------------------------------------
-- multimodel_stages
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS multimodel_stages (
    id                 TEXT    PRIMARY KEY NOT NULL,
    run_id             TEXT    NOT NULL,
    seq                INTEGER NOT NULL,
    model_id           TEXT    NOT NULL,
    provider_id        TEXT    NOT NULL,
    state              TEXT    NOT NULL,
    input_artifact_id  TEXT,
    output_artifact_id TEXT,
    input_hash         TEXT,
    output_hash        TEXT,
    cost_microcents    INTEGER NOT NULL DEFAULT 0,
    tools_used_json    TEXT    NOT NULL DEFAULT '[]',
    validation_json    TEXT,
    started_at         TEXT,
    finished_at        TEXT,
    UNIQUE (run_id, seq),
    FOREIGN KEY (run_id)             REFERENCES multimodel_runs(id) ON DELETE CASCADE,
    FOREIGN KEY (input_artifact_id)  REFERENCES multimodel_artifacts(id) ON DELETE SET NULL,
    FOREIGN KEY (output_artifact_id) REFERENCES multimodel_artifacts(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_multimodel_stages_run_id ON multimodel_stages(run_id);
CREATE INDEX IF NOT EXISTS idx_multimodel_stages_state ON multimodel_stages(state);
CREATE INDEX IF NOT EXISTS idx_multimodel_stages_output_hash ON multimodel_stages(output_hash);

-- ----------------------------------------------------------------------------
-- multimodel_artifacts
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS multimodel_artifacts (
    id          TEXT    PRIMARY KEY NOT NULL,
    run_id      TEXT    NOT NULL,
    stage_id    TEXT,
    kind        TEXT    NOT NULL CHECK (kind IN ('text', 'file', 'json', 'markdown')),
    content_ref TEXT    NOT NULL,
    hash        TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL,
    FOREIGN KEY (run_id)   REFERENCES multimodel_runs(id) ON DELETE CASCADE,
    FOREIGN KEY (stage_id) REFERENCES multimodel_stages(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_multimodel_artifacts_run_id ON multimodel_artifacts(run_id);
CREATE INDEX IF NOT EXISTS idx_multimodel_artifacts_stage_id ON multimodel_artifacts(stage_id);
CREATE INDEX IF NOT EXISTS idx_multimodel_artifacts_hash ON multimodel_artifacts(hash);
