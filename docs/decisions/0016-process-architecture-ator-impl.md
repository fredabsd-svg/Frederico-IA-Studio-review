# 0016 — `process-architecture`: implementação do ator (Etapa 2A fechada)

## Contexto

O [ADR-0015](0015-process-architecture-actor-not-mutex.md) registrou
a **decisão** de redesenhar o `WorkerManager` como ator (não
mutex). A Etapa 2A foi suspensa sem o manager no repo. Esta
entrega fecha a Etapa 2A com a **implementação** desse design.

## Decisão

Implementado conforme o ADR-0015 §Decisão, com três ajustes
mecânicos que o design original não fixou (escolhas locais da
implementação, não contradições):

1. **Handshake síncrono antes do ator entrar em cena.** O
   `WorkerManager::spawn_in_process` lê o `worker.hello` do fake
   (enviado no boot), gera o `WorkerAuth` (UUID v4 ou
   `WorkerSpawnConfig::auth_token` pré-definido), envia o
   `app.ack` e **só então** spawna a task do ator. O ator não
   precisa se preocupar com o `hello` inicial.
2. **Shutdown ignorando o payload do `oneshot`.** O
   `ManagerCommand::Shutdown::reply` usa o mesmo tipo
   `oneshot::Sender<Result<Value, ProcessError>>` que os outros
   (uniformidade do `HashMap` de pendings). O
   `drain_pending_with_error` envia `Ok(Value::Null)` pro
   pending de Shutdown. O `WorkerManager::shutdown` ignora o
   payload — a confirmação real de que o worker morreu vem do
   `actor_task.await`.
3. **`Box<dyn PipeReader>` e `Box<dyn PipeWriter>` movidos pra
   task do ator** (não `Arc`). Concorrência de invokes é
   resolvida pelo `request_id` no `handle_incoming`, não por
   readers paralelos.

## Travas de CI

A trava de CI do ADR-0015 (medida 4) já estava no `verify.ps1`
local; foi promovida também pro `ci.yml` (GitHub Actions):

```yaml
- name: Clippy
  run: cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock
```

A medida 2 (`with_test_timeout` em `crates/test-support/`,
5s default) é **aplicada em todo teste de integração** de
worker — o helper embrulha o teste em `tokio::time::timeout(5s)`
e devolve `Err(TestTimeoutError)` se a future não completar.
Deadlock vira falha com nome do teste em 5 segundos, não
sessão pendurada.

## Verificação

- `cargo test -p frederico-process-architecture --no-fail-fast`:
  **9 unit + 10 integration verde** em 0.52s.
- `cargo test --workspace`: 49 grupos, 0 falhas.
- `cargo clippy --workspace --all-targets -- -D warnings -D
  clippy::await_holding_lock`: limpo.
- `cargo fmt --all -- --check`: limpo.
- `check-core-purity.ps1`: OK.

Teste de **invocações concorrentes** (`concurrent_invocations_
complete`) prova o caso que quebrava a versão anterior: duas
`invoke` em paralelo via `tokio::join!`, ambas completam com o
`request_id` correto, sem serialização no caller.

## Consequências

- `WorkerHandle` é `Clone` (vários handles pro mesmo worker
  são permitidos — o `Arc<WorkerState>` partilha o estado).
- O `pending: Mutex<HashMap<...>>` é o único lock do design, e
  é usado em `insert`/`remove` **síncronos** (sem `.await`
  dentro do lock). É o que o `-D clippy::await_holding_lock`
  enforça.
- Próxima etapa: **Etapa 2B** (`WindowsPipeReader`/`Writer` via
  `windows` crate gateado, `spawn_external` via
  `tokio::process::Command`, bootstrap do `document-worker.exe`
  Python).

## Referências

- [ADR-0015](0015-process-architecture-actor-not-mutex.md) —
  decisão original.
- [`docs/architecture/process-architecture.md`](../architecture/process-architecture.md)
  §Invariantes.
- `REGRAS-DO-PROJETO.md` §1.3 (docs no mesmo commit) e §1.10
  (verificação automática no CI).
