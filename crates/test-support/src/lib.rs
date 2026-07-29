//! Helpers compartilhados entre os testes de integração do
//! workspace.
//!
//! Este crate existe para **uma** razão: garantir que deadlock
//! durante um teste de integração vire **falha com nome do teste
//! em 5 segundos**, em vez de sessão pendurada com saída cortada.
//! A motivação é o deadlock do `WorkerManager::invoke` na Etapa 2A
//! da Fase 5 (ver [ADR-0015] e a entrada correspondente no
//! `CHANGELOG.md`) — o teste travou > 60s e o output do `cargo
//! test` ficou truncado, dificultando o diagnóstico.
//!
//! O helper [`with_test_timeout`] embrulha uma `Future` num
//! `tokio::time::timeout` e devolve um erro nomeado se a operação
//! não completar dentro do prazo. É o oposto do que o `cargo test`
//! faz por default (que pode ficar pendurado para sempre em
//! qualquer `.await` que nunca resolve).
//!
//! [ADR-0015]: ../../docs/decisions/0015-process-architecture-actor-not-mutex.md
//!
//! # Política de uso
//!
//! **Todo** teste de integração de worker (qualquer crate que
//! converse com `process-architecture`) é embrulhado em
//! `with_test_timeout`. A trava do CI no `verify.ps1` e no
//! `ci.yml` é `cargo clippy -D clippy::await_holding_lock` —
//! mesma filosofia deste helper: a máquina coibe a classe, em vez
//! de depender de revisão manual.

#![deny(missing_docs)]

pub mod timeout;

pub use timeout::{with_test_timeout, with_test_timeout_at, TestTimeoutError};
