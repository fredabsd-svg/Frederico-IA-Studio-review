// Frederico IA Studio — casca Tauri + comandos IPC.
//
// Esta é a **casca** do app (ver ADR-0003). A casca:
//   1. Inicializa logs.
//   2. Resolve o caminho do banco de dados via diretórios do Windows.
//   3. Abre o banco SQLite rodando as migrações.
//   4. Expõe operações ao frontend via `tauri::command`.
//
// Toda a lógica de negócio vive nos crates de `crates/` — esta casca
// não a duplica.

use std::path::PathBuf;
use std::sync::Arc;

use frederico_diagnostics as diagnostics;
use frederico_security::fake::FakePlatform;
use frederico_shared_contracts::{AppOp, IpcRequest, IpcResponse};
use frederico_storage::Database;
use tauri::{Manager, State};

/// Estado compartilhado passado aos comandos Tauri.
struct AppState {
    db: Arc<Database>,
}

/// Resolve o caminho do banco de dados. Em produção vai usar
/// `directories::ProjectDirs` apontando para `%LOCALAPPDATA%`; por
/// enquanto usamos o diretório de trabalho para a Fase 1.
fn resolve_db_path() -> PathBuf {
    let proj = directories::ProjectDirs::from("studio", "frederico", "ia")
        .expect("diretórios do projeto resolvem em Windows");
    let dir = proj.data_local_dir();
    dir.join("frederico.db")
}

#[tauri::command]
async fn ipc_dispatch(
    request: IpcRequest,
    state: State<'_, AppState>,
) -> Result<IpcResponse, String> {
    let response = match request.op {
        AppOp::Ping => IpcResponse::ok(serde_json::json!({ "pong": true }))
            .unwrap_or_else(|e| IpcResponse::err(e.to_string())),
        AppOp::GetAppInfo => match state.db.app_info().await {
            Ok(info) => IpcResponse::ok(info).unwrap_or_else(|e| IpcResponse::err(e.to_string())),
            Err(e) => IpcResponse::err(e.to_string()),
        },
    };
    Ok(response)
}

#[tauri::command]
fn app_version() -> String {
    frederico_core::APP_VERSION.to_string()
}

fn main() {
    diagnostics::init();
    tracing::info!("Frederico IA Studio iniciando…");

    tauri::Builder::default()
        .setup(|app| {
            let db_path = resolve_db_path();
            tracing::info!(?db_path, "abrindo banco SQLite");

            // Runtime bloqueante para a abertura inicial do banco. Como
            // `Database::open` é async, usamos `tauri::async_runtime::block_on`.
            let db = tauri::async_runtime::block_on(async { Database::open(&db_path).await })
                .expect("abre o banco SQLite");

            // Garante que o AppState é Drop-safe (Database é Clone e
            // internamente Arc).
            app.manage(AppState { db: Arc::new(db) });

            // `FakePlatform` é suficiente na Fase 1 — o storage já
            // resolve o path sozinho. A casca vai implementar
            // `Platform` real na Fase 2.
            let _platform = FakePlatform::new(db_path.parent().unwrap().to_path_buf(), {
                use frederico_security::fake::FakeClock;
                FakeClock::new()
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![ipc_dispatch, app_version])
        .run(tauri::generate_context!())
        .expect("falha ao rodar app Tauri");
}
