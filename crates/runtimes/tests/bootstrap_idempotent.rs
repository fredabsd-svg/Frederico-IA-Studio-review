//! Teste de regressão: `bootstrap_if_needed` é idempotente.
//!
//! Ver spec `runtimes-architecture.md` §"Comportamento de bootstrap":
//! "Idempotência: passo 2 garante que bootstrap repetido é no-op
//! (sem rede, sem extraction) se o cache está válido."
//!
//! **Setup**: usa `tempfile::TempDir` como `install_root` (não toca
//! `%LOCALAPPDATA%`). Como o test baixa do python.org (rede real),
//! pula se `python --version` falha (= sem Python no PATH = sem
//! rede provável, mesma degradação que `tree_kill.rs` da Etapa 2).

use std::time::Duration;

use frederico_runtimes::__test_only::Manifest;
use frederico_runtimes::PythonRuntime;
use frederico_runtimes::Runtime;
use frederico_runtimes::RuntimeConfig;

fn has_network() -> bool {
    // Heurística: se Python está no PATH, a rede provavelmente está
    // acessível (mesma heurística do `tree_kill.rs`).
    std::process::Command::new("python")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn bootstrap_twice_is_noop() {
    if !has_network() {
        eprintln!("[bootstrap_idempotent] sem python no PATH; pulando (degradação controlada)");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: true,
        mirror_url: None,
        download_timeout: Duration::from_secs(60),
    };
    let runtime = PythonRuntime::new(&config);

    // 1. Primeiro bootstrap — pode ser lento (download).
    let start = std::time::Instant::now();
    runtime
        .bootstrap_if_needed()
        .expect("primeiro bootstrap OK");
    let first_duration = start.elapsed();
    eprintln!("[bootstrap_idempotent] primeiro bootstrap: {first_duration:?}");

    // Verifica manifest.
    let manifest_path = runtime.home_dir().join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json deve existir");
    let _manifest = Manifest::read(runtime.home_dir())
        .expect("read manifest")
        .expect("manifest presente");
    let mtime_before = std::fs::metadata(&manifest_path)
        .and_then(|m| m.modified())
        .expect("mtime");

    // 2. Espera 1s pra garantir mtime diferente se houver rewrite.
    std::thread::sleep(Duration::from_secs(1));

    // 3. Segundo bootstrap — deve ser no-op (cache hit, sem rede).
    let start = std::time::Instant::now();
    runtime
        .bootstrap_if_needed()
        .expect("segundo bootstrap OK (cache hit)");
    let second_duration = start.elapsed();
    eprintln!("[bootstrap_idempotent] segundo bootstrap: {second_duration:?}");

    // O segundo deve ser **significativamente** mais rápido que
    // o primeiro (cache hit não toca rede). Margem: 5x mais
    // rápido. Em prática, o segundo é <100ms; o primeiro é
    // 5-30s dependendo da rede.
    assert!(
        second_duration < first_duration / 5,
        "segundo bootstrap ({second_duration:?}) deveria ser >5x mais rapido \
         que o primeiro ({first_duration:?}) — cache hit nao tocou rede."
    );

    // Manifest não foi reescrito (mtime preservado).
    let mtime_after = std::fs::metadata(&manifest_path)
        .and_then(|m| m.modified())
        .expect("mtime");
    assert_eq!(
        mtime_before, mtime_after,
        "manifest nao foi reescrito (cache hit nao tocou disco alem do read)"
    );
}
