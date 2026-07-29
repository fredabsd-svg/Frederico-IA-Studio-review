//! Testes de integração: storage + diagnostics juntos.
//!
//! A Fase 1 ainda não tem fluxos verticais (chegam nas fases 2-3), mas
//! este arquivo já existe para garantir o caminho de compilação de
//! `tests/integration/`. O teste de fato repete o de `crates/storage`
//! só para provar que o diretório está sendo varrido por `cargo test`.

use frederico_storage::Database;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Contador atômico: o relógio sozinho não garante unicidade (no Windows a
/// granularidade de `timestamp_nanos` é grosseira e testes paralelos podem
/// colidir no mesmo valor, compartilhando o mesmo arquivo de banco).
static TEMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique = format!(
        "frederico-it-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        n,
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn integration_open_db_and_read_info() {
    let dir = tempdir();
    let db_path = dir.join("integration.db");
    let db = Database::open(&db_path).await.expect("abre db");
    let info = db.app_info().await.expect("lê app_info");
    assert_eq!(info.version, "0.1.0");
}
