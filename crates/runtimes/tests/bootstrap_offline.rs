//! Teste de regressão: `bootstrap_if_needed` com
//! `allow_download = false` e cache vazio retorna
//! `Err(OfflineRequired)` (não panic, não tenta rede).
//!
//! Ver spec `runtimes-architecture.md` §"Comportamento esperado —
//! Launch air-gapped".

use std::time::Duration;

use frederico_runtimes::BootstrapError;
use frederico_runtimes::PythonRuntime;
use frederico_runtimes::Runtime;
use frederico_runtimes::RuntimeConfig;

#[test]
fn offline_returns_error_for_missing_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: false, // <-- air-gapped
        mirror_url: None,
        download_timeout: Duration::from_secs(60),
    };
    let runtime = PythonRuntime::new(&config);

    // Cache vazio (tempdir novo). allow_download=false.
    // Deve falhar com OfflineRequired, **não** panic.
    let result = runtime.bootstrap_if_needed();
    match result {
        Err(BootstrapError::OfflineRequired { id }) => {
            assert_eq!(id.as_str(), runtime.id().as_str());
        }
        Err(other) => panic!("esperava OfflineRequired, recebeu {other:?}"),
        Ok(()) => panic!("bootstrap nao deveria ter sucesso sem rede e sem cache"),
    }
}
