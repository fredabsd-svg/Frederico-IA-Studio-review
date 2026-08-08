//! Teste de negação (regra do user, 2026-08-08): o `PATH` do
//! filho do sandbox (Node) **não** inclui paths do usuário.
//!
//! Espelha `python_bootstrap.rs::python_env_vars_do_not_include_user_paths`
//! para Node.

use frederico_runtimes::NodeRuntime;
use frederico_runtimes::Runtime;
use frederico_runtimes::RuntimeConfig;

fn config_for_test() -> RuntimeConfig {
    let tmp = tempfile::tempdir().expect("tempdir");
    RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: false, // só testamos env_vars
        mirror_url: None,
        download_timeout: std::time::Duration::from_secs(60),
    }
}

#[test]
fn node_env_vars_do_not_include_user_paths() {
    let config = config_for_test();
    let runtime = NodeRuntime::new(&config);
    let env_vars: Vec<(String, String)> = runtime.env_vars().to_vec();

    // 1. PATH está presente.
    let path = env_vars
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v)
        .expect("PATH deve estar nos env_vars do NodeRuntime");
    eprintln!("[node_bootstrap] PATH injetado: {path}");

    // 2. PATH não contém paths de "Node de outro vendor" (hijack).
    //    Não bloqueia `C:\Users\<name>` genérico (tempdir vive
    //    dentro de `C:\Users\<name>\AppData\Local\Temp\`).
    let forbidden_patterns = [
        r"Microsoft\WindowsApps",
        r"AppData\Local\Programs",
        r"AppData\Roaming\nvm",
        r"AppData\Roaming\npm",
        r"AppData\Local\nvm",
    ];
    for pattern in &forbidden_patterns {
        assert!(
            !path.contains(pattern),
            "PATH nao deve conter {pattern:?} (hijack de Node do user), mas tem: {path:?}"
        );
    }

    // 3. PATH contém o home do runtime.
    assert!(
        path.contains(runtime.home_dir().to_string_lossy().as_ref()),
        "PATH deve conter home do runtime ({:?})",
        runtime.home_dir()
    );

    // 4. NODE_PATH está setado pro home/node_modules.
    let node_path = env_vars
        .iter()
        .find(|(k, _)| k == "NODE_PATH")
        .map(|(_, v)| v)
        .expect("NODE_PATH deve estar nos env_vars");
    let expected_node_path = runtime
        .home_dir()
        .join("node_modules")
        .to_string_lossy()
        .to_string();
    assert_eq!(node_path, &expected_node_path);

    eprintln!(
        "[node_bootstrap] PASSOU — PATH injetado ({} bytes) nao contem paths do usuario",
        path.len()
    );
}
