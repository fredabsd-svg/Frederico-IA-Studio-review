//! `DbNetworkAuditSink` — implementação do `NetworkAuditSink` que
//! grava em SQLite (Fase 7, Etapa 6, ADR-0033).
//!
//! ## O que faz
//!
//! Cada `NetworkAccessEntry` que o `network::start_proxy` produz
//! é gravado em `network_audit` via `NetworkAuditRepo::append`.
//! Append-only — sem `UPDATE` nem `DELETE`. A UI da Etapa 7 da
//! Fase 7 consome via `NetworkAuditRepo::list_for_run` (aba
//! "Sandbox" do run).
//!
//! ## Por que aqui, não no `execution-engine`?
//!
//! O trait `NetworkAuditSink` é definido em `crates/security`
//! (mesma crate que o `start_proxy`). O `DbNetworkAuditSink` é a
//! implementação concreta que **fala com o storage** — pode viver
//! na mesma crate (a `frederico-security` já depende de
//! `frederico-storage` via `Cargo.toml`).
//!
//! Diferente do `DbAuditSink` da Fase 3 (que vive em
//! `execution-engine` por bridging `tool-registry` e `storage`),
//! o network audit não precisa de bridge — `security` já fala com
//! `storage` diretamente.
//!
//! ## Thread-safety
//!
//! O `DbNetworkAuditSink` é `Send + Sync` (o `Database` é
//! `Arc<SqlitePool>` internamente). O `start_proxy` carrega um
//! `Arc<dyn NetworkAuditSink>` e chama do loop async — tudo
//! thread-safe.

use frederico_core::RunId;
use frederico_storage::{Database, NetworkAuditRepo, StorageResult};
use tracing::warn;

use crate::network::{NetworkAccessEntry, NetworkAuditSink};

/// `NetworkAuditSink` que grava cada acesso em `network_audit`
/// (SQLite). Append-only. Falha de I/O é logada via
/// `tracing::warn!` mas **não** é propagada — o proxy não aborta
/// por causa de erro de auditoria.
///
/// **`network_audit.run_id` tem FK CASCADE pra `runs.id`.** Se o
/// `run_id` da entrada não corresponde a uma linha existente em
/// `runs`, o `INSERT` falha (`FOREIGN KEY constraint failed`) —
/// e, por causa do "não propaga" do parágrafo acima, isso vira
/// só um `tracing::warn!`, silenciosamente, sem quebrar o
/// request HTTP/CONNECT que estava sendo auditado. Em produção
/// isso não deveria acontecer (o `RunExecutor` cria a linha em
/// `runs` antes de despachar qualquer `tool_call`), mas testes
/// que constroem um `ToolContext` direto (sem passar pelo
/// `RunExecutor`) precisam de uma linha real em `runs` pro
/// `run_id` usado — ver
/// `e2e_network_proxy_wired_into_exec_python.rs::exec_python_network_deny_is_persisted_via_db_network_audit_sink`.
pub struct DbNetworkAuditSink {
    db: Database,
    /// `RunId` da execução atual. O sink é construído por `Run` e
    /// sabe em qual run está gravando. `None` se o proxy for
    /// usado fora de contexto de run (ex.: health check, testes
    /// que não vinculam a um run específico).
    run_id: Option<RunId>,
}

impl DbNetworkAuditSink {
    /// Constrói um sink vinculado a um `Run`. O caller
    /// (tipicamente o `RunExecutor::run()` ou o setup do sandbox
    /// da Etapa 6) passa o `Database` e o `RunId` da execução
    /// atual.
    #[must_use]
    pub fn new(db: Database, run_id: RunId) -> Self {
        Self {
            db,
            run_id: Some(run_id),
        }
    }

    /// Constrói um sink sem vínculo a um `Run` (proxies de
    /// health check, testes que não simulam o contexto de run).
    #[must_use]
    pub fn new_unbound(db: Database) -> Self {
        Self { db, run_id: None }
    }

    /// Helper que abre o `NetworkAuditRepo` e chama `append`. O
    /// `block_in_place` é usado porque o `NetworkAuditSink::record`
    /// é síncrono (trait object) e o `sqlx` exige contexto async.
    ///
    /// **`run_id` vem da `entry`, não do `self.run_id`.** O
    /// `start_proxy` já recebe um `run_id` por chamada (Etapa 6
    /// da Fase 7) e estampa cada `NetworkAccessEntry` com ele —
    /// é essa identidade, não a do momento em que o sink foi
    /// construído, que corresponde à invocação real. Um sink
    /// construído uma única vez com `new_unbound` (compartilhado
    /// entre todas as invocações de `exec.python`/`exec.node`,
    /// mesmo padrão do `AuditSink` de arquivo) e usado com
    /// `self.run_id` fixo gravaria **o mesmo `run_id` errado (ou
    /// `None`) pra toda invocação depois da primeira** — silencioso,
    /// sem panic, sem teste pegando, exatamente a categoria de bug
    /// desta sessão inteira. `self.run_id` só entra como fallback
    /// pra quando a `entry` não carrega run_id (proxies fora de
    /// contexto de run, ex.: health check).
    fn append_sync(&self, entry: &NetworkAccessEntry) -> StorageResult<String> {
        let db = self.db.clone();
        let run_id = entry
            .run_id
            .as_deref()
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .map(RunId)
            .or(self.run_id);
        let entry = entry.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let repo = NetworkAuditRepo::new(&db);
                let deny_reason = entry.deny_reason.as_deref();
                repo.append(
                    run_id.as_ref(),
                    &entry.host,
                    entry.port,
                    &entry.method,
                    &entry.path_redacted,
                    entry.status_code,
                    entry.bytes_sent,
                    entry.bytes_received,
                    entry.decision.as_str(),
                    deny_reason,
                )
                .await
            })
        })
    }
}

impl NetworkAuditSink for DbNetworkAuditSink {
    fn record(&self, entry: NetworkAccessEntry) {
        match self.append_sync(&entry) {
            Ok(id) => {
                tracing::debug!(
                    audit_id = %id,
                    host = %entry.host,
                    method = %entry.method,
                    decision = %entry.decision.as_str(),
                    "network_audit gravada"
                );
            }
            Err(e) => {
                warn!(
                    host = %entry.host,
                    method = %entry.method,
                    error = %e,
                    "falha ao gravar network_audit (best effort — proxy segue)"
                );
            }
        }
    }
}
