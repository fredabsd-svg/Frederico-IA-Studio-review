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
pub fn init() {
    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tao=warn"));
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true).with_level(true).compact())
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
