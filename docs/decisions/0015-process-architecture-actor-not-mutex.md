# 0015 — `process-architecture`: ator, não mutex

## Contexto

A Etapa 2A da Fase 5 introduziu o `frederico-process-architecture` —
o crate que vai gerenciar os workers sidecar (`document-worker`,
`sandbox-runner`, `browser-worker`, `runtime-manager`). A primeira
versão do `WorkerManager` usou `Arc<Mutex<Box<dyn Pipe>>>` para
compartilhar o transporte entre o `invoke`/`ping` (que escreve) e
uma task de leitura de background (que lê responses e despacha por
`request_id`).

Resultado: **deadlock**. Os 8 testes de integração do `WorkerManager`
em `tests/fake_worker.rs` (`invoke_roundtrip`, `ping_updates_health`,
`shutdown_terminates_worker`, `worker_does_not_see_parent_env`,
`worker_sees_explicit_allowlist`, `spawn_in_process_smoke`,
`spawn_and_shutdown_smoke`, `invoke_with_short_timeout_*`) travavam
> 60s. Diagnóstico:

- A task de leitura segura o `MutexGuard` durante
  `read_line().await` que, no `FakePipeClient`, faz
  `rx.recv().await` (um `await` point).
- O `invoke` precisa do **mesmo** guard para `write_line` que faz
  `tx.send().await` (outro `await` point).
- Em algum cenário os dois esperam pelo guard mutuamente —
  deadlock clássico de `lock` segurado através de `.await`.

Pior, o mesmo design quebraria **duas invocações concorrentes**:
a segunda `invoke` espera o guard, a primeira segura o guard
esperando a response, e nada anda. O bug ia aparecer em produção
quando o modelo fizesse duas chamadas paralelas (recurso
relativamente comum em chains de tool calls).

A Etapa 2A foi suspensa com o `WorkerManager` removido do
repositório; o que está commitado é só o protocolo IPC
(`IpcMessage` + 8 `IpcOp` + `WorkerManifest` + `WorkerAuth`),
a trait `Pipe` (que a Etapa 2B vai dividir em `PipeReader`/
`PipeWriter`), o `env_allowlist`, e o `fake.rs` com o `FakeWorker`
— suíte de 7 testes unitários verde.

## Decisão

O `WorkerManager` redesenhado vai usar **modelo de ator**, não
mutex compartilhado:

1. **Uma task dona exclusiva do pipe.** Ninguém mais segura o
   pipe. A task lê linhas, parseia `IpcMessage`, e despacha.
2. **`invoke` manda a request por um `mpsc` interno** (chamado
   `request_tx: mpsc::Sender<InvokeRequest>`), junto com um
   `oneshot::Sender<JsonValue>` pra receber a response.
3. **A task** mantém um `HashMap<Uuid, oneshot::Sender<JsonValue>>`
   indexado por `request_id`. Quando recebe `ToolResult` com o
   mesmo `request_id`, despacha pelo oneshot. O `invoke` faz
   `oneshot::Receiver::await` com `tokio::time::timeout`.
4. **Concorrência de `invoke`s paralelos**: cada `invoke` tem
   seu próprio oneshot. O `request_id` correlaciona. Resolve de
   brinde o caso "duas tool calls em paralelo".
5. **Trait `Pipe` se divide em `PipeReader` + `PipeWriter`** no
   construtor — o `WorkerHandle` recebe as duas metades, o
   `WorkerManager` segura a `Writer` (no `invoke`, sem lock) e a
   task fica com a `Reader`. A `Writer` é `Clone`-able (wrapper
   sobre `mpsc::Sender` ou sobre o lado de write do named pipe
   Windows, que é protegido internamente pelo SO).
6. **`windows` crate** entra na Etapa 2B com `unsafe_code =
   "deny"` global e `#![allow(unsafe_code)]` apenas no módulo
   `windows_pipes` (mesma estratégia do `frederico-security`,
   ADR-0007).

## Trava no CI

A classe de bug (mutex segurado em `.await`) é coibida por:

```powershell
cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock
```

em `scripts/verify.ps1` (Step 2). Mesmo espírito do
`check-core-purity.ps1`: a máquina passa a coibir a classe, em
vez de depender de revisão manual.

A segunda trava (medida 2 do user) é: **todo teste de integração
de worker é embrulhado em `tokio::time::timeout(5s)`**. Um
deadlock vira falha com nome do teste em 5 segundos, não sessão
pendurada com saída cortada. Helper:

```rust
// crates/test-support/src/worker_timeout.rs (próxima sessão)
pub async fn with_test_timeout<F>(name: &str, fut: F) -> ...
```

## Alternativas descartadas

- **Manter `Arc<Mutex<Box<dyn Pipe>>>` mas usar `parking_lot`
  com `try_lock` em loop**. Descartada: o problema não é o lock
  em si, é o `lock` segurado em `.await`. Tentar `try_lock` em
  loop não resolve o `await` e ainda introduz **busy wait**
  (viola a invariante "zero polling no app principal" do
  `process-architecture.md`).
- **Um `RwLock` em vez de `Mutex`.** Descartada: o problema é
  simétrico — `read` também segura através de `await`.
- **Two-phase commit com canal de controle**. Descartada: o
  modelo de ator com `mpsc` + `oneshot` correlacionado por
  `request_id` é mais simples e **resolve o caso concorrente
  de graça**.

## Consequências

**Mais fácil:**

- Concorrência de `invoke`s paralelos cai de graça (uma
  request por `oneshot`).
- Não tem mais a categoria de bug "mutex segurado em await".
- O `WorkerHandle` é `Send + Sync` trivialmente (sem `Mutex`
  interno).
- Debug mais fácil: o ator tem **um** lugar onde o pipe é
  acessado, e a `mpsc` de requests é a única porta de entrada.

**Mais difícil:**

- O `Pipe` trait se divide — `PipeReader::read_line` e
  `PipeWriter::write_line` separados. A Etapa 2B tem que
  implementar os dois no `WindowsPipe`. O `FakePipeClient`
  (em `fake.rs`) já tem `tx`/`rx` separados, fica mais natural.
- O `WorkerHandle` carrega **dois** handles (`request_tx` +
  `response_rx` indireto via `Arc<WorkerState>`), em vez de um
  `Arc<Mutex<Box<dyn Pipe>>>`. A `WorkerState` carrega o
  `pending: Mutex<HashMap<Uuid, oneshot::Sender<JsonValue>>>` —
  o **único** lock do design, e ele **não** segura `.await`
  longos (apenas `remove` + `send`).
- Mudar a assinatura de `invoke`/`ping`/`shutdown` — o que
  estava na Etapa 2A. A Etapa 3 (que consome o `invoke`) precisa
  da nova assinatura.

## Pendências para a próxima sessão

1. Reintroduzir o `WorkerManager` com o modelo de ator descrito
   acima.
2. Helper `with_test_timeout` em `crates/test-support/`.
3. Re-rodar a suíte `tests/fake_worker.rs` com o helper aplicado
   — agora com o design correto, deve passar em < 1s.
4. Etapa 2B: `WindowsPipeReader`/`Writer` via `windows` crate
   (gateado), `spawn_external` via `tokio::process::Command`.
5. `document-worker` Python (Etapa 2B, lado Python).
6. `docs/modules/process-architecture.md` (template §1.4) — só
   depois que o `WorkerManager` voltar, pra não documentar design
   que não está no repo.

## Referências

- `PROMPT MESTRE` §5.3, §7.3, §22.5
- [`process-architecture.md`](../architecture/process-architecture.md)
  §Invariantes (env allowlist, zero polling, sem TCP)
- [ADR-0003](0003-nucleo-desacoplado-da-casca-tauri.md) —
  `unsafe_code = "deny"` no núcleo
- [ADR-0007](0007-credential-store-trait.md) — `windows` crate
  gateada em `frederico-security` (modelo que `process-architecture`
  segue na Etapa 2B)
- `REGRAS-DO-PROJETO.md` §1.10 (verificação automática no CI)
