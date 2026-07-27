<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 1
-->

# Módulo `storage`

> Crate: [`crates/storage/`](../../crates/storage/)
> Nome do pacote: `frederico-storage`

## O que faz

Camada de persistência. SQLite via `sqlx` com migrações numeradas.
A Fase 1 entrega a infraestrutura: abertura, migração inicial
(`0001_initial.sql`) e leitura/escrita da tabela `app_info`.

## O que expõe

- `Database` — handle do pool SQLite. Clone-compartilhado (`Arc` interno).
- `Database::open(&Path)` — abre o banco, cria diretórios pais, roda migrações e grava linha inicial de `app_info`.
- `Database::app_info()` — devolve o `AppInfo` persistido.
- `Database::expected_version()` — versão de runtime esperada.
- `AppInfo` — struct serializável com `version`, `started_at`, `last_seen_at`.
- `AppPaths` — trait para o caminho do banco (implementado por `frederico-security`).
- `StorageError`/`StorageResult` — erros do módulo.

## De quem depende / quem depende dele

- **Depende de:** `frederico-core` (AppVersion), `sqlx` (sqlite, tokio-rustls, migrate, chrono, uuid), `tokio`, `async-trait`, `serde`, `tracing`, `directories`.
- **Usado por:** `frederico-security` (re-exporta o trait `AppPaths`), `frederico-desktop` (abre o banco no `setup` da Tauri).

## Decisões não óbvias / armadilhas

- `sqlx::migrate!("./migrations")` é resolvido em **tempo de compilação** relativo ao `Cargo.toml` do crate. Se o crate for movido, a macro continua apontando para `crates/storage/migrations/`.
- O `app_info` é gravado em `ON CONFLICT DO UPDATE` para que reabrir o app atualize `last_seen_at` sem duplicar a linha.
- `:memory:` SQLite não funciona com `sqlx::migrate!` porque cada conexão é um banco separado; testes usam arquivo temporário.
- O pool padrão do `sqlx::SqlitePool` aceita múltiplas conexões; leituras concorrentes são seguras.

## Como testar isoladamente

```pwsh
cargo test -p frederico-storage
```

## O que este módulo **não** faz

- Não conhece a casca Tauri nem paths do Windows — o `AppPaths` é injetado.
- Não modela domínio: tabelas de runs, conversas, memórias, ferramentas, artefatos chegam nas fases 2-5.
- Não roda migrações destrutivas automaticamente. A política de migração (backups, validação) está em `docs/architecture/` e será detalhada nas fases seguintes.
