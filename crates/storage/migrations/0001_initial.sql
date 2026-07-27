-- Migração inicial da Fase 1.
-- Cria a tabela `app_info` que registra a versão do app e o instante de
-- primeira abertura. Tabela deliberadamente trivial: a Fase 1 é vertical
-- fina e ainda não modela domínio (memórias, runs, conversas chegam nas
-- fases 2-4).

CREATE TABLE IF NOT EXISTS app_info (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version TEXT NOT NULL,
    started_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);

-- Tabela de registro de execução das migrações. Usada por `sqlx::migrate!`
-- automaticamente; mantida aqui só para documentar o schema versionado.
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TEXT NOT NULL DEFAULT (datetime('now')),
    success INTEGER NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
