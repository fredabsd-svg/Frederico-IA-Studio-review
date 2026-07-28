//! `DbAuditSink` — implementação do `AuditSink` que grava em SQLite
//! (Fase 3, Etapa 5.x, Passo 10 do `validate_tool_call`).
//!
//! ## O que faz
//!
//! Cada `AuditEntry` que o `RunExecutor` produz é gravado em
//! `tool_audit` via `ToolAuditRepo::append`. Append-only — sem
//! `UPDATE` nem `DELETE`. A UI da Fase 5/6 consome via
//! `ToolAuditRepo::list_for_run`.
//!
//! ## Por que aqui, não no `storage`?
//!
//! O `tool-registry` define a trait `AuditSink` (pura, sem
//! dependência de storage). O `DbAuditSink` é a implementação
//! concreta que **fala com o storage** — vive no `execution-engine`
//! que é a cola entre tool-registry e storage.
//!
//! ## Thread-safety
//!
//! O `DbAuditSink` é `Send + Sync` (o `Database` é `Arc<SqlitePool>`
//! internamente). O `RunExecutor` carrega um
//! `Arc<dyn AuditSink>` e chama do loop async — tudo thread-safe.

use frederico_core::RunId;
use frederico_storage::{Database, StorageResult, ToolAuditRepo};
use frederico_tool_registry::{AuditEntry, AuditSink};
use tracing::warn;

/// `AuditSink` que grava cada chamada em `tool_audit` (SQLite).
/// Append-only. Falha de I/O é logada via `tracing::warn!` mas
/// **não** é propagada — o `RunExecutor` não aborta por causa de
/// erro de auditoria.
pub struct DbAuditSink {
    db: Database,
    /// `RunId` da execução atual. O sink é construído por `Run`
    /// e sabe em qual run está gravando. **Limitação**: o mesmo
    /// sink só pode ser usado pra um `Run` por vez (o que é o
    /// caso do `RunExecutor`).
    run_id: RunId,
}

impl DbAuditSink {
    /// Constrói um sink vinculado a um `Run`. O caller
    /// (tipicamente o `ChatOrchestrator` ou o `RunExecutor`)
    /// passa o `Database` e o `RunId` da execução atual.
    pub fn new(db: Database, run_id: RunId) -> Self {
        Self { db, run_id }
    }

    /// Helper que abre o `ToolAuditRepo` e chama `append`. O
    /// `block_in_place` é usado porque o `AuditSink::record` é
    /// síncrono (trait object) e o `sqlx` exige contexto async.
    fn append_sync(&self, entry: &AuditEntry) -> StorageResult<String> {
        let db = self.db.clone();
        let run_id = self.run_id;
        let entry = entry.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let repo = ToolAuditRepo::new(&db);
                let args_json = entry.arguments_json.clone();
                let result_json = entry.result_json.clone();
                repo.append(
                    &run_id,
                    &entry.tool_id.to_string(),
                    &entry.tool_version,
                    &args_json,
                    entry.result_ok,
                    &result_json,
                    entry.duration.as_micros() as i64,
                )
                .await
            })
        })
    }
}

impl AuditSink for DbAuditSink {
    fn record(&self, entry: AuditEntry) -> Result<(), String> {
        match self.append_sync(&entry) {
            Ok(id) => {
                tracing::debug!(audit_id = %id, tool = %entry.tool_id, "audit gravada");
                Ok(())
            }
            Err(e) => {
                warn!(
                    tool = %entry.tool_id,
                    error = %e,
                    "falha ao gravar audit (best effort — execução segue)"
                );
                Ok(()) // tolerada
            }
        }
    }
}
