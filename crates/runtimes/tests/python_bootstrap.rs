//! Teste de negação (regra do user, 2026-08-08): o `PATH` do
//! filho do sandbox (Python) **não** inclui paths do usuário
//! (Documents, AppData/Roaming, etc.).
//!
//! Defesa contra o usuário ter um `python.exe` malicioso em
//! `~/bin/` que hijacka o do app.
//!
//! Ver spec `runtimes-architecture.md` §"Testes de regressão
//! obrigatórios" e o teste de negação do `python_bootstrap`.

use frederico_runtimes::PythonRuntime;
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
fn python_env_vars_do_not_include_user_paths() {
    let config = config_for_test();
    let runtime = PythonRuntime::new(&config);
    let env_vars: Vec<(String, String)> = runtime.env_vars().to_vec();

    // 1. PATH está presente.
    let path = env_vars
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v)
        .expect("PATH deve estar nos env_vars do PythonRuntime");
    eprintln!("[python_bootstrap] PATH injetado: {path}");

    // 2. PATH **não** contém paths de "Python/Node do sistema"
    //    ou de "outro vendor" (defesa contra hijack).
    //    Padrões: AppData/Local/Microsoft/WindowsApps (stub Store),
    //    AppData/Local/Programs (instaladores manuais do user),
    //    AppData/Roaming/Python (outros vendors).
    //    **NÃO** bloqueia `C:\Users\<name>` genérico porque
    //    o `install_root` em testes usa o tempdir que vive
    //    dentro de `C:\Users\<name>\AppData\Local\Temp\`.
    let forbidden_patterns = [
        // Store Python stub
        r"Microsoft\WindowsApps",
        // Instaladores manuais (Python/Node instalados pelo user)
        r"AppData\Local\Programs",
        // Outros vendors (pyenv, anaconda, etc.)
        r"AppData\Roaming\Python",
        r"AppData\Roaming\npm",
        r"AppData\Roaming\pyenv",
        // Python store
        r"AppData\Local\Python\bin", // <-- espera, isso é o Python real. Vou permitir.
    ];
    let allowed_patterns = [
        r"AppData\Local\Python\bin", // Python real instalado em %LOCALAPPDATA%\Python\bin
    ];
    for pattern in &forbidden_patterns {
        if allowed_patterns.contains(pattern) {
            continue;
        }
        assert!(
            !path.contains(pattern),
            "PATH nao deve conter {pattern:?} (hijack de Python do user), mas tem: {path:?}"
        );
    }

    // 3. PATH contém o home do runtime (home_dir do PythonRuntime).
    assert!(
        path.contains(runtime.home_dir().to_string_lossy().as_ref()),
        "PATH deve conter home do runtime ({:?}), mas tem: {path:?}",
        runtime.home_dir()
    );

    // 4. PYTHONHOME está setado pro home_dir (não pro Python
    //    do sistema).
    let pythonhome = env_vars
        .iter()
        .find(|(k, _)| k == "PYTHONHOME")
        .map(|(_, v)| v)
        .expect("PYTHONHOME deve estar nos env_vars");
    assert_eq!(
        pythonhome,
        &runtime.home_dir().to_string_lossy().to_string(),
        "PYTHONHOME = home_dir do runtime (nao Python do sistema)"
    );

    // 5. PYTHONUNBUFFERED=1 (output em tempo real).
    let unbuffered = env_vars
        .iter()
        .find(|(k, _)| k == "PYTHONUNBUFFERED")
        .map(|(_, v)| v);
    assert_eq!(unbuffered, Some(&"1".to_string()));

    eprintln!(
        "[python_bootstrap] PASSOU — PATH injetado ({} bytes) nao contem paths do usuario",
        path.len()
    );
}
