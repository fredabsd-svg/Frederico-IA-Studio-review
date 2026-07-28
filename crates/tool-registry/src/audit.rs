//! Trait `AuditSink` — Passo 10 do `validate_tool_call` (Fase 3, Etapa 5.x).
//!
//! ## O que é o Passo 10
//!
//! O spec `tool-registry-specification.md` §7.7 lista 10 passos
//! pra validação de uma `tool_call`. Os 9 primeiros (existe,
//! versão, disponível, inventário, permissões, schema, jail,
//! limites, aprovação) estão implementados nas Etapas 2 e 3. O
//! **Passo 10** é a **entrada de auditoria** — registrar toda
//! `tool_call` (aprovada, rejeitada, ou com erro de execução) com
//! timestamp, tool_id, arguments e resultado.
//!
//! O `AuditSink` é o ponto de extensão: o `validate_tool_call` chama
//! o sink no Passo 10 (após todas as outras checagens passarem) e
//! passa um `AuditEntry` com o resultado da chamada. A
//! implementação concreta (no `execution-engine` ou na casca Tauri)
//! grava onde quiser — em SQLite, no `EventSink`, no log, etc.
//!
//! ## Por que trait, não função direta?
//!
//! O tool-registry **não depende de storage** (mantém pureza, ver
//! `scripts/check-core-purity.ps1`). O `AuditSink` é uma trait
//! abstrata que qualquer um implementa — a implementação concreta
//! `DbAuditSink` (em `execution-engine`) usa o `ToolAuditRepo` do
//! storage. O tool-registry só chama o sink; não sabe pra onde vai.
//!
//! ## Falha de auditoria é tolerada?
//!
//! **Sim.** Falha no `AuditSink` é logada mas **não** bloqueia a
//! execução da ferramenta. A auditoria é "best effort" —
//! preferimos executar a tool e perder o registro do que bloquear
//! a chamada por causa de um erro de I/O. O spec é explícito sobre
//! isso (Passo 10 é a última linha do `validate_tool_call`).

use std::time::Duration;

use frederico_core::ToolId;
use serde::Serialize;

/// Entrada de auditoria. Cada `tool_call` que passa por
/// `validate_tool_call` gera **uma** `AuditEntry` — passada pro
/// `AuditSink::record` (sink decide se grava ou não).
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    /// ID da ferramenta chamada.
    pub tool_id: ToolId,
    /// Versão que o modelo "viu" (Passo 2 do validador já
    /// confirmou que bate com a do manifesto).
    pub tool_version: String,
    /// Argumentos como JSON (raw, sem normalização).
    pub arguments_json: String,
    /// `true` se a ferramenta executou com sucesso
    /// (`ToolResult.ok`). `false` se rejeitada pelo validador
    /// ou se a execução falhou.
    pub result_ok: bool,
    /// Output conforme `output_schema` (sucesso) ou erro
    /// estruturado (`{"error_code": "TOOL_...",
    /// "error_message": "..."}`). Serializado como JSON.
    pub result_json: String,
    /// Duração da execução. `Duration::ZERO` se a validação
    /// rejeitou antes de executar.
    pub duration: Duration,
}

/// Trait `AuditSink` — o tool-registry chama isso no Passo 10 do
/// `validate_tool_call`. A implementação concreta decide onde a
/// auditoria vai (SQLite, log, IPC, etc.).
///
/// **Falha é tolerada**: o `validate_tool_call` loga via
/// `tracing::warn!` mas segue. O executor **não** aborta por
/// causa de erro de auditoria.
pub trait AuditSink: Send + Sync {
    /// Grava uma entrada de auditoria. Erros são propagados
    /// (`Result`), mas o caller decide se tolera ou não.
    fn record(&self, entry: AuditEntry) -> Result<(), String>;
}

/// `AuditSink` que descarta todas as entradas (no-op). É o
/// default quando o `RunExecutor` é construído sem sink
/// explícito (Etapa 4 do executor não tinha auditoria).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn record(&self, _entry: AuditEntry) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// `AuditSink` que acumula entradas em memória — pra testes
    /// verificarem o que foi gravado.
    #[derive(Debug, Default, Clone)]
    pub struct RecordingAuditSink {
        pub entries: Arc<Mutex<Vec<AuditEntry>>>,
    }

    impl RecordingAuditSink {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn count(&self) -> usize {
            self.entries.lock().unwrap().len()
        }

        pub fn last(&self) -> Option<AuditEntry> {
            self.entries.lock().unwrap().last().cloned()
        }
    }

    impl AuditSink for RecordingAuditSink {
        fn record(&self, entry: AuditEntry) -> Result<(), String> {
            self.entries.lock().unwrap().push(entry);
            Ok(())
        }
    }

    #[test]
    fn noop_sink_always_succeeds() {
        let sink = NoopAuditSink;
        let entry = AuditEntry {
            tool_id: ToolId::new("test"),
            tool_version: "0.1.0".to_string(),
            arguments_json: "{}".to_string(),
            result_ok: true,
            result_json: "{}".to_string(),
            duration: Duration::ZERO,
        };
        assert!(sink.record(entry).is_ok());
    }

    #[test]
    fn recording_sink_accumulates() {
        let sink = RecordingAuditSink::new();
        for i in 0..3 {
            let entry = AuditEntry {
                tool_id: ToolId::new(format!("test_{i}")),
                tool_version: "0.1.0".to_string(),
                arguments_json: "{}".to_string(),
                result_ok: true,
                result_json: "{}".to_string(),
                duration: Duration::ZERO,
            };
            sink.record(entry).unwrap();
        }
        assert_eq!(sink.count(), 3);
        assert_eq!(sink.last().unwrap().tool_id, ToolId::new("test_2"));
    }
}
