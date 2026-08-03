//! Adapter `WorkerInvoker` (do `frederico-core`) para
//! `WorkerHandle` (do `process-architecture`).
//!
//! ADR-0024 (Etapa 2.B da Fase de Ligação).
//!
//! ## Por que mora no `process-architecture`
//!
//! Regra do Rust (orphan rule): "only traits defined in the
//! current crate can be implemented for types defined outside
//! of the crate". O trait `WorkerInvoker` é definido no
//! `frederico-core`; o `WorkerHandle` é definido **aqui**, no
//! `process-architecture`. O `impl` precisa estar em um dos
//! dois. A escolha: o `process-architecture` é o crate que
//! conhece o `WorkerHandle` intimamente, e o `core` é puro
//! (não pode importar `process-architecture` — regra de
//! pureza do `core`).
//!
//! ## Por que o helper `process_to_invoke_error` está inline
//!
//! O mesmo helper existe no `tool-registry` (que é onde
//! originalmente coloquei), mas o `tool-registry` depende
//! do `process-architecture` — se o `process-architecture`
//! dependesse do `tool-registry` também, teríamos ciclo. A
//! duplicação (~25 linhas) é o preço de manter o grafo
//! limpo. O `app` (que importa ambos os crates) tem o seu
//! próprio helper inline também.
//!
//! ## O que muda no `process-architecture` em si
//!
//! Estritamente **nada**. O `WorkerHandle` struct continua
//! igual (Fase 5 fechada, Etapa 3). O `WorkerManager::invoke`
//! continua idêntico, o modelo de ator (ADR-0015) continua
//! idêntico, `health_snapshot` continua idêntico. Só
//! adicionamos um `impl` para um trait novo (definido no
//! `core`). É estritamente aditivo.

use async_trait::async_trait;
use frederico_core::{InvokeError, WorkerInvoker};
use serde_json::Value;

use crate::{ProcessError, WorkerHandle};

/// Converte `ProcessError` em `InvokeError` (1:1, exceto
/// categorização). Espelha o helper em
/// `frederico_tool_registry::worker_invoker::process_to_invoke_error`
/// e em `frederico_app::launcher` (worker_error_to_invoke_error).
/// **Duplicado intencionalmente** — cada crate que precisa
/// do mapeamento tem o seu (evita ciclo no grafo de
/// dependências).
#[must_use]
fn process_to_invoke_error(e: ProcessError) -> InvokeError {
    match e {
        ProcessError::Protocol { message } => InvokeError::Protocol { message },
        ProcessError::Transport { message } => InvokeError::Transport { message },
        ProcessError::Timeout { .. } => InvokeError::Timeout,
        // `Cancelled` é do watchdog — message genérica.
        // A informação completa está no journal, não no erro.
        ProcessError::Cancelled { .. } => InvokeError::Unhealthy {
            message: "cancelado pelo watchdog (passou do budget)".to_string(),
        },
        ProcessError::Unhealthy { message, .. } => InvokeError::Unhealthy { message },
        ProcessError::Platform { message } => InvokeError::Platform { message },
    }
}

#[async_trait]
impl WorkerInvoker for WorkerHandle {
    async fn invoke(&self, payload: Value) -> Result<Value, InvokeError> {
        // `WorkerHandle::invoke` é o método existente
        // (Fase 5). Capturamos o `ProcessError` e
        // convertemos pro `InvokeError` neutro do `core`.
        WorkerHandle::invoke(self, payload)
            .await
            .map_err(process_to_invoke_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_to_invoke_error_protocol_passes_message() {
        let pe = ProcessError::Protocol {
            message: "JSON malformado".to_string(),
        };
        let ie = process_to_invoke_error(pe);
        assert!(matches!(ie, InvokeError::Protocol { ref message } if message == "JSON malformado"));
    }

    #[test]
    fn process_to_invoke_error_timeout_maps_to_timeout() {
        let pe = ProcessError::Timeout {
            worker_id: "document-worker".to_string(),
            timeout_ms: 30_000,
        };
        let ie = process_to_invoke_error(pe);
        assert!(matches!(ie, InvokeError::Timeout));
    }

    #[test]
    fn process_to_invoke_error_cancelled_maps_to_unhealthy() {
        let pe = ProcessError::Cancelled {
            worker_id: "document-worker".to_string(),
            reason: "passou do budget".to_string(),
        };
        let ie = process_to_invoke_error(pe);
        assert!(matches!(ie, InvokeError::Unhealthy { .. }));
    }

    #[test]
    fn process_to_invoke_error_unhealthy_strips_worker_id() {
        let pe = ProcessError::Unhealthy {
            worker_id: "document-worker".to_string(),
            message: "saúde degradada".to_string(),
        };
        let ie = process_to_invoke_error(pe);
        match ie {
            InvokeError::Unhealthy { message } => {
                assert_eq!(message, "saúde degradada");
            }
            _ => panic!("esperava Unhealthy"),
        }
    }
}
