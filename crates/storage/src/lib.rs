//! Camada de persistência do Frederico IA Studio (núcleo).
//!
//! SQLite via `sqlx` com migrações numeradas. O caminho do banco é resolvido
//! via [`AppPaths`] (trait de `frederico-security`) — o storage **não**
//! importa nada de plataforma nem assume path fixo.
//!
//! A Fase 1 entrega a infraestrutura mínima: abertura, migração inicial
//! (`0001_initial.sql`) e leitura/escrita da tabela `app_info`. As tabelas
//! de domínio (runs, conversas, memórias, artefatos) entram nas fases
//! 2-5 conforme o roadmap.

use chrono::Utc;
use frederico_core::AppVersion;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("falha ao abrir o banco SQLite em {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("falha ao rodar migração: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("query falhou: {0}")]
    Query(#[from] sqlx::Error),
    #[error("registro `app_info` ausente após migração")]
    AppInfoMissing,
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Trait para o caminho do banco. O `frederico-security` implementa isto
/// para Windows e os testes usam um fake. Manter o trait no storage evita
/// que o storage conheça o sistema de arquivos.
pub trait AppPaths {
    fn database_path(&self) -> PathBuf;
}

/// Estado persistido da primeira (e única, por enquanto) linha de `app_info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub started_at: String,
    pub last_seen_at: String,
}

/// Handle do banco de dados. Clonável (sqlx::SqlitePool é Arc internamente).
#[derive(Debug, Clone)]
pub struct Database {
    pool: sqlx::SqlitePool,
}

impl Database {
    /// Abre o banco no caminho dado, roda migrações e grava a linha inicial
    /// de `app_info` se for a primeira vez. Thread-safe.
    pub async fn open(path: &Path) -> StorageResult<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Open {
                    path: path.to_path_buf(),
                    source: sqlx::Error::Configuration(Box::new(std::io::Error::other(
                        format!("não consegui criar diretório {parent:?}: {e}"),
                    ))),
                })?;
        }

        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .map_err(|source| StorageError::Open {
                path: path.to_path_buf(),
                source,
            })?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO app_info (id, version, started_at, last_seen_at) \
             VALUES (1, ?1, ?2, ?2) \
             ON CONFLICT(id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
        )
        .bind(frederico_core::APP_VERSION.to_string())
        .bind(&now)
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    /// Versão registrada no `app_info` (a primeira escrita na inicialização).
    pub async fn app_info(&self) -> StorageResult<AppInfo> {
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT version, started_at, last_seen_at FROM app_info WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((version, started_at, last_seen_at)) => Ok(AppInfo {
                version,
                started_at,
                last_seen_at,
            }),
            None => Err(StorageError::AppInfoMissing),
        }
    }

    /// Versão de runtime esperada (vinda de `frederico-core`).
    pub fn expected_version() -> AppVersion {
        frederico_core::APP_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_in_memory_and_runs_migration() {
        // Para `:memory:` cada conexão é um banco separado; pedimos
        // `max_connections=1` e `before_acquire` para garantir que todas
        // as queries veem o mesmo schema. Aqui usamos um arquivo temp
        // porque `sqlx::migrate!` precisa de persistência.
        let dir = tempdir();
        let db_path = dir.join("test.db");
        let db = Database::open(&db_path).await.expect("abre");
        let info = db.app_info().await.expect("lê app_info");
        assert_eq!(info.version, "0.1.0");
        assert!(!info.started_at.is_empty());
    }

    #[tokio::test]
    async fn second_open_updates_last_seen_only() {
        let dir = tempdir();
        let db_path = dir.join("test2.db");
        let db1 = Database::open(&db_path).await.expect("abre 1");
        let info1 = db1.app_info().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let db2 = Database::open(&db_path).await.expect("abre 2");
        let info2 = db2.app_info().await.unwrap();
        assert_eq!(info1.started_at, info2.started_at);
        assert_ne!(info1.last_seen_at, info2.last_seen_at);
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "frederico-storage-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
