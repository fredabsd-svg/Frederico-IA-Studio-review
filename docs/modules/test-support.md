# Módulo `frederico-test-support`

> Estado: implementado. Verificado contra o código em 2026-07-29.

## 1. O que este módulo faz

Helpers compartilhados entre os testes de integração do
workspace. Existe para **uma** razão: garantir que deadlock
durante um teste de integração vire **falha com nome do teste
em 5 segundos**, em vez de sessão pendurada com saída cortada.

A motivação é o deadlock do `WorkerManager::invoke` na Etapa 2A
da Fase 5 (ver [ADR-0015](../decisions/0015-process-architecture-actor-not-mutex.md)):
o teste travou > 60s e o `tail` do log veio truncado, dificultando
o diagnóstico. O helper [`timeout::with_test_timeout`] embrulha
uma `Future` num [`tokio::time::timeout`] e devolve um erro nomeado
se a operação não completar dentro do prazo.

É o **oposto** do que o `cargo test` faz por default (que pode
ficar pendurado para sempre em qualquer `.await` que nunca
resolve).

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- [`timeout::with_test_timeout(name, fut)`] — embrulha `fut` num
  [`tokio::time::timeout`] de 5s. Devolve `Err(TestTimeoutError)`
  se o future não completar dentro do prazo. O `name` é só pra
  mensagem — é o `Display` do erro que carrega, fazendo o panic
  do `cargo test` mostrar o nome do teste, não um genérico
  "future timed out".
- [`timeout::with_test_timeout_at(name, timeout, fut)`] — versão
  com tempo limite customizado. Usada por testes que esperam
  `time::sleep` deliberados (ex.: teste de cancelamento de sleep).
- [`timeout::TestTimeoutError`] — struct com `name: String` e
  `elapsed: Duration`. Implementa `Display` ("teste `X` excedeu
  Yms — provável deadlock: future não completou") e
  `std::error::Error`.
- [`timeout::DEFAULT_TIMEOUT`] — `Duration::from_secs(5)`.

**Política de uso:** **todo** teste de integração de worker
(qualquer crate que converse com `process-architecture`) é
embrulhado em `with_test_timeout`. Mesma filosofia da trava
de CI no `verify.ps1` e no `ci.yml` (`-D clippy::await_holding_lock`):
a máquina coíbe a classe, em vez de depender de revisão manual.

## 3. Do que depende e quem depende dele

**Dependências (`Cargo.toml`):**

- `tokio` (`macros`, `rt-multi-thread`, `time`, `sync`).

**Quem depende dele:**

- `frederico-process-architecture` — `dev-dep` (os 10 testes de
  integração em `tests/fake_worker.rs` usam `with_test_timeout`).
- Qualquer crate futuro do workspace que tiver testes de
  integração de worker (Etapa 2B do `process-architecture`,
  Etapa 3 do `document-engine` quando rodar contra o
  `document-worker` Python, etc.).

## 4. Decisões não óbvias e armadilhas conhecidas

- **Default de 5s é folgado o suficiente pra testes que esperam
  `time::sleep` em cenários de timeout** (ex.: testar que um
  invoke com 50ms de budget falha com `Timeout`, não com
  deadlock do próprio helper), e **curto o suficiente** pra que
  um deadlock real vire falha visível em vez de sessão pendurada.
  Testes com `time::sleep` deliberados (ex.: > 5s) usam
  `with_test_timeout_at` com timeout customizado.

- **O `name` é `&str`, não `String`.** O helper faz `name.to_string()`
  internamente (dentro do `TestTimeoutError`). Custo: zero
  alocação no caminho feliz (o `Err` só constrói se o timeout
  estourar). Custo: alocação no caminho de erro, que é o que
  queremos — é a mensagem de erro que vai pro panic do `cargo test`.

- **`tokio::time::timeout` envelopa o future, não o spawn.** O
  `cargo test` do `cargo` (e o `tokio::test` que embrulha)
  fornece um runtime tokio. `with_test_timeout` **não** precisa
  spawnar nada — usa o runtime do caller. Custo: zero overhead
  no caminho feliz. Trade-off: o teste tem que estar rodando num
  contexto tokio (todos os `#[tokio::test]` do workspace estão).

- **Forçar o caller a lidar com `Err` explicitamente.** O helper
  devolve `Result<T, TestTimeoutError>`. Forçar o caller a
  `.expect("teste não deadlockou")` impede que um deadlock vire
  um `Ok(())` silencioso — bug que o helper existe justamente
  pra evitar.

- **Não mede o tempo decorrido.** `TestTimeoutError::elapsed` é
  o `Duration` que foi **estourado** (não o tempo real decorrido).
  A razão: `tokio::time::timeout` devolve `Elapsed` (sem o tempo),
  e medir o real exige um `Instant::now()` extra. Como o tempo
  default é 5s, a aproximação `elapsed` é o suficiente pra debug.

- **NÃO substitui o `cargo test` de fato.** É uma **segunda
  camada** de defesa contra deadlock em testes. A primeira é o
  `cargo test` rodar normal (e falhar se um teste falhar). A
  segunda é o `with_test_timeout` garantir que deadlock vire
  falha com nome em 5s, não sessão pendurada.

## 5. Como testá-lo isoladamente

```powershell
cd C:\src\Frederico
$env:PATH = "$env:PATH;C:\Users\conta\.cargo\bin"
cargo test -p frederico-test-support --no-fail-fast > test.log 2>&1
Get-Content test.log -Tail 20
```

**2/2 unit verde** (o crate é pequeno, são os 2 únicos testes):

| Regra / comportamento | Teste |
|---|---|
| `with_test_timeout` resolve rápido (devolve `Ok`) | `timeout::tests::with_test_timeout_resolves_quickly` |
| `with_test_timeout_at` estoura com `Err(TestTimeoutError)` em 50ms quando o future dorme 60s | `timeout::tests::with_test_timeout_times_out` |

Os 2 testes usam `#[tokio::test(flavor = "current_thread", start_paused = true)]` — `start_paused` faz o tokio pular o tempo virtualmente, então o `tokio::time::sleep(Duration::from_secs(60))` do `with_test_timeout_times_out` nunca completa dentro do budget de 50ms (sem precisar esperar 60s reais no CI).

## 6. O que ele **não** faz

- **Não detecta deadlock sem `with_test_timeout` envelopando o teste.** O helper só protege o future que ele envelopa. Testes que esquecem de chamar `with_test_timeout` ficam vulneráveis ao comportamento default do `cargo test` (que pode pendurar indefinidamente em `.await` que nunca resolve). A política é **todo teste de integração de worker passa por ele** — mas a aplicação depende de revisão humana + a trava do CI (`-D clippy::await_holding_lock`).

- **Não mede tempo de execução real do teste.** Só o `Duration` que estourou. Pra profiling de performance, use `cargo bench` (separado) ou wrappers de timing de teste.

- **Não aborta o `cargo test` inteiro quando estoura.** O `Err(TestTimeoutError)` acorda o `await` do caller; o caller faz `.expect()` (ou outra decisão). Se o caller decidir **continuar** o teste (sem abortar), o helper não impede. A política é sempre `.expect("teste não deadlockou")` — quem escreve o teste escolhe.

- **Não substitui detecção de deadlock em código de produção.** O `cargo test` roda testes de integração, não smoke tests de produção. Pra detectar deadlock em produção, tem o `-D clippy::await_holding_lock` no CI (medida 4 do ADR-0015) e tem o `drain_pending_with_error` no `WorkerManager` (limpa pendings quando o loop do ator termina).

## Pendências

- Nenhuma. O crate é estável e cobre a medida 2 do feedback do deadlock (Etapa 2A da Fase 5).
