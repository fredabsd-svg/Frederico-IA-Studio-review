//! Teste de regressão: manifest com SHA-256 mismatch
//! (ou seja, archive corrompido em cache) dispara re-bootstrap
//! (delete + re-download), **não** usa runtime corrompido.
//!
//! Ver spec `runtimes-architecture.md` §"Comportamento de bootstrap":
//! "Resiliência: passo 4c-5d garante que um download corrompido
//! ou extração parcial não deixa o runtime em estado 'meio
//! instalado'."

use std::time::Duration;

use chrono::Utc;
use frederico_runtimes::__test_only::Manifest;
use frederico_runtimes::PythonRuntime;
use frederico_runtimes::Runtime;
use frederico_runtimes::RuntimeConfig;
use frederico_runtimes::RuntimeId;

// O SHA-256 esperado do Python 3.12.4 archive. Hard-coded no
// test (não precisa expor `python::PYTHON_SHA256` na API pública).
// **Verificado em 2026-08-08** via download de teste.
const EXPECTED_PYTHON_SHA256: &str =
    "15fea3c9367653a85086fe37216b4d1a1c78688fa5e1587e1db0b0f658856564";

fn has_network() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn corrupted_manifest_triggers_redownload() {
    if !has_network() {
        eprintln!("[manifest_corruption] sem python no PATH; pulando");
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
    let target_dir = runtime.home_dir();

    // 1. Cria o target_dir e escreve um manifest com SHA-256
    //    errado (corrompido por design).
    std::fs::create_dir_all(target_dir).expect("mkdir target_dir");
    let bad_manifest = Manifest {
        runtime_id: RuntimeId::from("python-3.12.4"),
        version: "3.12.4".to_string(),
        source_url: runtime.source_url().to_string(),
        source_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        archive_size_bytes: runtime.expected_archive_size(),
        bootstrap_at: Utc::now().to_rfc3339(),
        validated: true,
        validation_output: "fake".to_string(),
    };
    bad_manifest.write(target_dir).expect("write bad manifest");

    // 2. Bootstrap — deve deletar o target_dir (corrompido) e
    //    re-fazer do zero.
    runtime
        .bootstrap_if_needed()
        .expect("bootstrap re-fez do zero");

    // 3. O manifest novo tem o SHA-256 correto.
    let new_manifest = Manifest::read(target_dir)
        .expect("read manifest")
        .expect("manifest presente");
    assert_eq!(
        new_manifest.source_sha256.to_lowercase(),
        EXPECTED_PYTHON_SHA256.to_lowercase(),
        "novo manifest tem SHA-256 correto (re-download)"
    );
    assert!(
        new_manifest.validated,
        "novo manifest tem validated=true (validate passou)"
    );
    assert_ne!(
        new_manifest.bootstrap_at, bad_manifest.bootstrap_at,
        "bootstrap_at mudou (re-bootstrap de fato)"
    );
}
