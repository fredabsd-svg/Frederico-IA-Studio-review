//! Diagnóstico do Frederico IA Studio: logs estruturados.
//!
//! A Fase 1 entrega apenas o setup de `tracing` (formato, env-filter,
//! destino). Telemetria, tela de diagnóstico e exportadores avançados
//! chegam nas fases seguintes.

use std::sync::OnceLock;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: OnceLock<()> = OnceLock::new();

/// Inicializa o subscriber global de tracing. Idempotente — chamadas
/// repetidas são no-op. Lê `RUST_LOG` para o filtro (default: `info`).
///
/// **Por que `.with_writer(std::io::stderr)` explícito:** o
/// `fmt::layer()` sem `.with_writer()` usa `std::io::stdout` por
/// default (`MakeWriterDefault` em `tracing-subscriber` 0.3).
/// Isso confunde o smoke test `apps/desktop/src-tauri/tests/
/// smoke_startup.rs` que captura só stderr via `Stdio::piped()`
/// (a documentação do test assume que panics e `tracing::error!`
/// vão pro stderr). Forçar `std::io::stderr` aqui alinha o
/// comportamento com a convenção do projeto (logs em stderr,
/// dados em stdout) e com o gate de PR. Sem isso, o smoke test
/// não consegue detectar a nova classe de erro do startup
/// recovery (Etapa 6+).
pub fn init() {
    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tao=warn"));
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_level(true)
                    .compact(),
            )
            .init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        // Não conseguimos testar o init propriamente (afeta estado global),
        // mas garantimos que duas chamadas não paniquem.
        init();
        init();
    }
}
