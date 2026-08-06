-- Migração 0028 — `permission_profiles` (Fase 6, Etapa 3 PR 2).
--
-- Carrega o `PermissionSet` real do `assistant` / `project` / `user`
-- no caminho de produção do `ChatOrchestrator` (Etapa 3 da Fase 3
-- Etapa 4 deixou a frase "a Etapa 4 carrega o `PermissionSet` real
-- do `assistant` / `project` / `user` antes de validar — pendente"
-- no `docs/modules/tool-registry.md §3`; este PR fecha essa
-- pendência via `permission_loader` + `PermissionSet::merge`
-- fail-closed, ADR-0030 §D3).
--
-- ## Decisão CRÍTICA: cacheia o **parse**, não a **decisão**
--
-- Decisão registrada na conversa de 2026-08-06 (PR 2): cache de
-- permissão é fonte clássica de **privilégio obsoleto**. Se o cache
-- guardasse a **decisão** (o `PermissionSet` final merged), revogar
-- uma permissão num layer (ex.: user desliga `network`) não
-- propagaria até o cache expirar — o usuário continuaria com
-- `network: true` por horas/dias.
--
-- A solução estrutural: o cache **guarda o parse** (a
-- `PermissionSet` derivada de **uma** TOML, por layer), chaveado
-- por `(path, content_hash)`. Quando o conteúdo do TOML muda (novo
-- hash), o cache invalida. A **decisão** (o effective set) é
-- computada **a cada chamada** via `PermissionSet::merge3`
-- fail-closed — sem cache de decisão, sem janela de privilégio
-- obsoleto.
--
-- ## Schema
--
-- | coluna         | tipo      | semântica                                                  |
-- | -------------- | --------- | --------------------------------------------------------- |
-- | path           | TEXT PK   | caminho absoluto do TOML (user/project/assistant)         |
-- | content_hash   | TEXT NOT  | hash do `content_toml` (BLAKE3 ou FNV64) — chave de       |
-- |                | NULL      | invalidação                                                |
-- | content_toml   | TEXT NOT  | conteúdo bruto (re-parse on load — schema evolui sem     |
-- |                | NULL      | ter que migrar cache)                                     |
-- | parsed_at      | INTEGER   | Unix timestamp (ms) da última escrita                     |
-- | schema_version | INTEGER   | versão do parser de TOML (Etapa 6 começa em 1; bump em    |
-- |                | NOT NULL  | mudança incompatível)                                     |
--
-- **Por que guardar o TOML bruto** (não a `PermissionSet` já
-- desserializada): se o schema mudar (campo novo, enum renomeado),
-- a `schema_version` invalida o cache automaticamente (query com
-- `WHERE schema_version = $current` retorna vazio → re-parse).
-- Storing the parsed JSON would silently desynchronize from the
-- current schema; storing the source-of-truth (TOML) makes the
-- cache correct-by-construction on schema bumps.
--
-- **Cache em memória** (não-DB) ainda existe: o `PermissionLoader`
-- mantém um `Arc<Mutex<HashMap<(PathBuf, String), PermissionSet>>>`
-- chaveado por `(path, content_hash)`. Hit → retorno imediato;
-- miss → read do DB → parse → cache. A tabela DB é a **fonte
-- persistente** do TOML (cross-restart); o cache em memória é
-- apenas pra evitar re-parse em chamadas sucessivas no mesmo
-- run.
--
-- ## Invalidação no caminho de produção
--
-- O `permission_loader` faz:
-- 1. Lê o arquivo do disco (fonte primária).
-- 2. Calcula o `content_hash`.
-- 3. Cache hit (em memória) → retorna.
-- 4. Cache miss → lê do DB (`SELECT content_toml WHERE path =
--    ? AND schema_version = $current`).
--    - DB hit → parse, atualiza cache em memória, retorna.
--    - DB miss → insere no DB (`content_hash`, `content_toml`,
--      `parsed_at = now`, `schema_version = $current`), parse,
--      cache, retorna.
-- 5. Se o hash em memória não bate com o do disco (mudou
--    desde o último load), invalida entrada e recomeça do
--    passo 1.
--
-- **A Etapa 4 (subagente) consome esse mesmo `PermissionLoader`**
-- pra resolver o `effective_permission_set` do subagente: user
-- ∩ project ∩ assistant ∩ parent_permissions ∩
-- `SpecialistDefinition.allowed_tools − denied_tools`. O
-- `permission_loader` já está em produção quando a Etapa 4
-- entrar.

PRAGMA foreign_keys = OFF;

CREATE TABLE permission_profiles (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    content_toml TEXT NOT NULL,
    parsed_at INTEGER NOT NULL,
    -- Versão do parser. Bump quando um campo do TOML mudar de
    -- tipo, nome, ou semântica. A Etapa 6 começa em 1; a
    -- próxima bump (ex.: enum GitPermission ganhando nova
    -- variante) incrementa pra 2 e invalida TODOS os perfis
    -- cached.
    schema_version INTEGER NOT NULL DEFAULT 1
);

-- Query 1: lookup por path + schema_version atual. O `WHERE
-- schema_version = $current` é a chave de invalidação por
-- schema bump (sem ele, o cache serve o JSON velho indefinidamente).
CREATE INDEX IF NOT EXISTS idx_permission_profiles_path_schema
    ON permission_profiles (path, schema_version);

PRAGMA foreign_keys = ON;
