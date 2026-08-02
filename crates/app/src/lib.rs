//! Camada de composição do Frederico IA Studio.
//!
//! Detém **o que é montar o app** (catálogo de ferramentas,
//! permissões iniciais, resolvedor de jail por conversa,
//! construção do `ChatOrchestrator`) e **nada do que é rodar a
//! UI**. A casca Tauri (`apps/desktop/src-tauri`) continua sendo
//! a casca — `frederico-app` é o que ela importa.
//!
//! ## Por que um crate separado
//!
//! Duas razões combinadas (ver ADR-0022 §D1):
//!
//! 1. **Mesma função de composição para a casca e para os
//!    E2E da raiz** (`tests/e2e/`, Etapa 5 da Fase de Ligação).
//!    Como `apps/desktop/src-tauri` é crate binário, testes
//!    externos não conseguem `use` dele. `frederico-app`
//!    expõe `build_tool_registry`, `initial_permission_set`,
//!    `build_chat_orchestrator` etc. — a casca e os E2E
//!    chamam as mesmas funções, eliminando a possibilidade
//!    de drift entre "o que o teste exercita" e "o que a
//!    casca usa em produção".
//!
//! 2. **Reaproveitamento no modo servidor do PROMPT MESTRE
//!    §5.5 (VPS / headless)**. O crate é puro por construção
//!    (sem `tauri`, sem `windows` — passa em
//!    `scripts/check-core-purity.ps1` automaticamente). O
//!    `build_chat_orchestrator` recebe as dependências
//!    injetadas via `parts` e roda em qualquer runtime
//!    `tokio` com acesso ao DB e à rede. Manter essa pureza
//!    é regra do projeto (ADR-0022 §D1), não acidente: se
//!    alguém "simplificar" adicionando `tauri` ao
//!    `Cargo.toml` deste crate, o gate quebra o build.
//!
//! ## Estrutura
//!
//! - [`composition`] (populado no commit 4b da Etapa 1):
//!   `build_tool_registry(tools)`, `initial_permission_set()`,
//!   `ChatOrchestratorParts`, `build_chat_orchestrator(parts)`.
//! - [`jail`] (populado no commit 3 da Etapa 1):
//!   `JailResolver` trait + `FileSystemJailResolver` (default
//!   da Etapa 1; Etapa 7 substitui por `SecurityJailResolver`
//!   via `frederico-security`).
//!
//! ## Estado da Etapa 1
//!
//! Este commit (2) é o **skeleton**: o crate existe, o módulo
//! `lib.rs` documenta a intenção, e os testes cobrem o mínimo
//! (versão exposta). A lógica de composição entra nos commits
//! seguintes (3, 4a, 4b, 5) com testes próprios. Manter o
//! skeleton mínimo na Etapa 1 serve a dois propósitos: (a)
//! o build de `cargo test --workspace` continua verde desde
//! o commit 2, sem dependências quebradas; (b) a revisão
//! pode ler a forma do contrato (nomes de funções, structs,
//! traits) antes do comportamento entrar.

/// Versão do crate, derivada do workspace (`Cargo.toml` raiz).
///
/// Útil para logs e telemetria no startup da casca e do modo
/// servidor §5.5. Não é uma API "estável" no sentido
/// semver — apenas reflete a versão do workspace.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test do skeleton. A lógica de composição entra
    /// nos commits 3, 4a, 4b e 5 — até lá, `version()` é a
    /// única API pública.
    #[test]
    fn version_is_set() {
        // `env!("CARGO_PKG_VERSION")` é sempre uma string não
        // vazia (vem do `Cargo.toml` do workspace). Sanity
        // check: não é string vazia, e tem pelo menos um
        // caractere (o "0" do "0.1.0" atual).
        assert!(!version().is_empty());
    }
}
