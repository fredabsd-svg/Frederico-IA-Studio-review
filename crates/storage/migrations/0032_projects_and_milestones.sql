-- Migração 0032 — projetos e marcos (Fase 8, Etapa 4, ADR-0042).
--
-- Duas tabelas para os dois conceitos que o ADR-0042 §D1 separou
-- depois que a confusão custou um ADR inteiro:
--
-- - **Marco de projeto** (`project_milestones`, aqui): estado
--   nomeado do workspace, criado deliberadamente pelo usuário, que
--   **sobrevive** a runs e conversas.
-- - **Checkpoint de run** (`checkpoints`, migração 0003): ponto de
--   retomada interno da máquina de estados, amarrado ao run e
--   apagado com ele por `ON DELETE CASCADE`.
--
-- A tabela `checkpoints` continua **sem dono em código** (ADR-0042
-- §D5) e não é tocada aqui. Ela não é ancestral desta: estender
-- aquele schema faria o marco do usuário ser apagado junto com o run
-- que o originou — perda de dados silenciosa embutida no `CASCADE`,
-- que é a alternativa 1 que o ADR-0042 rejeitou.
--
-- **O dado de verdade do marco não mora aqui.** Ele é uma tag
-- anotada no repositório Git do workspace (ADR-0042 §D2). Esta
-- tabela guarda os metadados que o Git não tem lugar para guardar:
-- qual conversa originou o marco, e se ele foi criado pelo usuário
-- ou automaticamente antes de uma restauração. Consequência
-- declarada: se o usuário apagar a tag com o `git` dele, o marco
-- some do produto — e é por isso que `commit_id` fica gravado, para
-- o app conseguir dizer *o que* sumiu em vez de só falhar.

CREATE TABLE IF NOT EXISTS projects (
    -- ID opaco (UUID), mesmo padrão dos outros.
    id TEXT PRIMARY KEY,

    -- Caminho do diretório de workspace. **Único**: um projeto É um
    -- diretório (ADR-0042 §D4), então dois projetos no mesmo caminho
    -- seriam o mesmo projeto com dois nomes.
    caminho TEXT NOT NULL UNIQUE,

    -- Nome humano dado pelo usuário.
    nome TEXT NOT NULL,

    -- Perfil de permissão aplicável. `NULL` = o default do app.
    -- Texto e não FK: os perfis vivem em arquivo
    -- (`permission_loader`), não em tabela.
    perfil_permissao TEXT,

    criado_em TEXT NOT NULL DEFAULT (datetime('now')),

    -- Último acesso, para a UI ordenar por "mais recentes".
    ultimo_acesso TEXT NOT NULL DEFAULT (datetime('now'))
);

-- "projetos mais recentes primeiro" — ordenação da lista na UI.
CREATE INDEX IF NOT EXISTS idx_projects_ultimo_acesso
    ON projects(ultimo_acesso DESC);

CREATE TABLE IF NOT EXISTS project_milestones (
    id TEXT PRIMARY KEY,

    -- Projeto dono do marco. `CASCADE` aqui é seguro e desejado:
    -- apagar o projeto do app apaga os metadados dos marcos dele.
    -- As tags continuam no repositório Git do usuário — o app
    -- esquece, o Git não.
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    -- Nome humano do marco. Único **por projeto**: dois marcos com o
    -- mesmo nome no mesmo repositório colidiriam na tag.
    nome TEXT NOT NULL,

    descricao TEXT NOT NULL DEFAULT '',

    -- SHA-1 do commit que a tag aponta. Gravado para o app
    -- conseguir dizer o que sumiu se a tag for apagada por fora.
    commit_id TEXT NOT NULL,

    -- Conversa que originou o marco. `NULL` quando o marco nasce
    -- fora de conversa (automático, ou criado pela UI). Sem FK
    -- **de propósito**: o marco sobrevive à conversa (§D1), e uma FK
    -- com `CASCADE` reintroduziria exatamente a perda de dados que
    -- este ADR evitou; uma FK com `RESTRICT` impediria o usuário de
    -- apagar a conversa.
    conversa_origem TEXT,

    -- `1` quando o marco foi criado pelo app antes de uma
    -- restauração (ADR-0042 §D3), `0` quando o usuário pediu. A UI
    -- usa isso para não poluir a lista com marcos automáticos.
    automatico INTEGER NOT NULL DEFAULT 0 CHECK (automatico IN (0, 1)),

    criado_em TEXT NOT NULL DEFAULT (datetime('now')),

    UNIQUE (project_id, nome)
);

-- "marcos deste projeto, do mais novo para o mais antigo" — a
-- consulta que a lista de marcos faz.
CREATE INDEX IF NOT EXISTS idx_milestones_project_criado
    ON project_milestones(project_id, criado_em DESC);
