//! Helper [`with_test_timeout`] — embrulha um teste de integração
//! de worker num [`tokio::time::timeout`] de 5 segundos, com
//! mensagem que identifica o teste pelo nome.
//!
//! A motivação é o deadlock do `WorkerManager::invoke` na Etapa 2A
//! da Fase 5 (ver ADR-0015). Sem este helper, um deadlock fica
//! pendurado no `cargo test` e o `tail` do log vem truncado — fica
//! difícil saber **qual** teste travou. Com o helper, qualquer
//! `.await` que não resolva em 5s vira:
//!
//! ```text
//! thread '<unnamed>' panicked at 'teste `invoke_with_short_timeout`
//! excedeu 5000ms — provável deadlock: future não completou'
//! ```
//!
//! O tempo default (5s) é folgado o suficiente pra testes que
//! esperam `time::sleep` em cenários de timeout, e curto o
//! suficiente pra que um deadlock real vire falha visível em vez
//! de sessão pendurada.

use std::future::Future;
use std::time::Duration;

/// Erro devolvido por [`with_test_timeout`] quando o future não
/// completa dentro do prazo. É um erro "nomeado" — `Display`
/// imprime o nome do teste, o tempo limite e a recomendação
/// ("provável deadlock"), que é o que aparece no `cargo test` em
/// caso de pânico.
#[derive(Debug)]
pub struct TestTimeoutError {
    /// Nome do teste (o que o caller passou como primeiro arg).
    pub name: String,
    /// Tempo limite que foi excedido.
    pub elapsed: Duration,
}

impl std::fmt::Display for TestTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "teste `{}` excedeu {}ms — provável deadlock: future não completou",
            self.name,
            self.elapsed.as_millis()
        )
    }
}

impl std::error::Error for TestTimeoutError {}

/// Tempo default (5 segundos). Curto o suficiente pra que deadlock
/// vire falha visível em vez de sessão pendurada; longo o bastante
/// pra acomodar `time::sleep` deliberados em testes de timeout
/// (ex.: testar que um invoke com 50ms de budget falha com
/// `Timeout`, não com deadlock do próprio helper).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Roda `fut` sob um [`tokio::time::timeout`] de
/// [`DEFAULT_TIMEOUT`] (5s). Devolve `Err(TestTimeoutError)` se o
/// future não completar dentro do prazo.
///
/// O `name` é só pra mensagem — o helper **não** roda o `name` no
/// output, é o `Display` do erro que carrega. É por isso que o
/// panic do `cargo test` mostra o nome do teste, e não um
/// genérico "future timed out".
///
/// # Quando usar
///
/// ```ignore
/// #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// async fn invoke_roundtrip() {
///     with_test_timeout("invoke_roundtrip", async {
///         // … teste que pode deadlockar …
///     })
///     .await
///     .expect("teste não deadlockou");
/// }
/// ```
///
/// # Por que o `expect` no caller
///
/// O helper devolve `Result<T, TestTimeoutError>`. Forçar o caller
/// a lidar com `Err` explicitamente impede que um deadlock vire
/// um `Ok(())` silencioso — bug que o helper existe justamente
/// pra evitar.
pub async fn with_test_timeout<F, T>(name: &str, fut: F) -> Result<T, TestTimeoutError>
where
    F: Future<Output = T>,
{
    with_test_timeout_at(name, DEFAULT_TIMEOUT, fut).await
}

/// Versão com tempo limite customizado. Usada por testes que
/// esperam `time::sleep` deliberados (ex.: teste de cancelamento
/// de sleep) — 5s default não caberia.
pub async fn with_test_timeout_at<F, T>(
    name: &str,
    timeout: Duration,
    fut: F,
) -> Result<T, TestTimeoutError>
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(value) => Ok(value),
        Err(_elapsed) => Err(TestTimeoutError {
            name: name.to_string(),
            elapsed: timeout,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn with_test_timeout_resolves_quickly() {
        let result: i32 = with_test_timeout("fast", async { 42 })
            .await
            .expect("fast não deve estourar");
        assert_eq!(result, 42);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn with_test_timeout_times_out() {
        let result: Result<(), TestTimeoutError> =
            with_test_timeout_at("slow", Duration::from_millis(50), async {
                // `start_paused = true` faz o tokio pular o tempo —
                // este sleep nunca completa dentro do budget.
                tokio::time::sleep(Duration::from_secs(60)).await;
            })
            .await;
        let err = result.expect_err("devia ter estourado");
        assert_eq!(err.name, "slow");
        assert_eq!(err.elapsed, Duration::from_millis(50));
    }
}
