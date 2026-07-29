# Módulo `frederico-process-architecture`

> Etapa 2A da Fase 5 — **fechada** (commit desta entrega). Manager redesenhado como **ator** (sem `Arc<Mutex<Box<dyn Pipe>>>`) — ver [ADR-0015](../decisions/0015-process-architecture-actor-not-mutex.md) (decisão) e [ADR-0016](../decisions/0016-process-architecture-ator-impl.md) (implementação). Estado: parcial. Verificado contra o código em 2026-07-29.

## 1. O que este módulo faz

Define o **envelope IPC** (`IpcMessage` + 8 `IpcOp`) usado entre o app principal e os workers sidecar (`document-worker`, `sandbox-runner`, `browser-worker`, `runtime-manager`). É o **contrato**: line-delimited JSON sobre named pipes (Windows) ou qualquer transporte que implemente as traits `PipeReader` + `PipeWriter`. A Etapa 2A entrega o **manager** (modelo de ator) + o **fake in-process** que valida o design. A Etapa 2B adiciona o transporte real (`WindowsPipeReader`/`Writer` via `windows` crate gateado) e o `spawn_external` (`tokio::process::Command`).

O que está no repo e funciona:

- **Protocolo**: `IpcMessage` v1 com `protocol_version`, `request_id` (UUID v4 por mensagem), `op` (8 opcodes estáveis em snake_case), `payload` (JSON arbitrário), `auth` (token de curta duração opcional).
- **Manifesto**: `WorkerManifest` com ID, versão, capabilities, dependências, saúde, compatibilidade.
- **Auth**: `WorkerAuth` — `String` opaco v0.1; revogação entra na Etapa 2B.
- **Transporte**: traits separadas `PipeReader` (não `Clone`, leitura serializada pela task do ator) + `PipeWriter` (`Clone` + `Send` + `Sync`, escrita concorrente via `mpsc::Sender` ou `Arc<NamedPipeServer>`). Sem trait única — a divisão é o que destrava o modelo de ator (ADR-0015).
- **env_allowlist**: constrói o env do worker a partir de uma allowlist explícita. O env do pai **não** é lido (regra do `process-architecture.md` §Invariantes).
- **fake**: worker simulado in-process (`FakeWorker` + `LineBuffer`) que implementa `PipeReader` + `PipeWriter` sobre `mpsc::channel`. Envia `worker.hello` no **boot** (modelando o que o worker real faz ao subir). Suporta `slow_response_ms` (atraso artificial pra testes de timeout). Coberto por 2 unit tests + 10 integration tests.
- **manager**: `WorkerManager` + `WorkerHandle` (clonável). Modelo de ator — uma task é dona exclusiva do pipe; `invoke`/`ping`/`shutdown` mandam comandos por `mpsc` interno + `oneshot` correlacionado por `request_id`. Suporta invocações concorrentes sem lock no caminho do `invoke`.

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- `IpcMessage`, `IpcOp` (8 variantes), `RequestId`.
- `WorkerManifest`, `WorkerId`, `WorkerAuth`, `WorkerHealth`, `WorkerHealthSnapshot`, `Dependency`, `CompatibilityInfo`.
- `PipeReader`, `PipeWriter`, `PipeName` (newtype validado).
- `EnvEntry`, `build_worker_env`, `build_worker_env_with_defaults`.
- `ProcessError` (6 variantes), `ProcessErrorKind` (6 categorias).
- `WorkerManager`, `WorkerHandle`, `WorkerSpawnConfig`.
- `FakeWorkerConfig`, `FakeWorkerHandle`, `FakePipeReader`, `FakePipeWriter`, `spawn_fake_worker`, `unique_pipe_name`.

**Fora desta entrega (Etapa 2B):**

- `WindowsPipeReader`/`Writer` — `windows` crate gateado em `#[cfg(windows)]`.
- `spawn_external` — `tokio::process::Command` para abrir o `document-worker.exe`.

**Não-público (interno):**

- `LineBuffer` (em `fake.rs`) — busca por `\n` no `read_line`.
- `protocol::PROTOCOL_VERSION` — bump MAJOR em mudanças incompatíveis do envelope.
- `ManagerCommand`, `WorkerState`, `PendingResponse`, `PendingKind` (em `manager.rs`) — tipos internos do ator.
- `run_actor`, `handle_command`, `handle_incoming`, `drain_pending_with_error` (em `manager.rs`) — funções internas do loop.

## 3. Do que depende e quem depende dele

**Dependências (`Cargo.toml`):**

- `tokio` (`rt-multi-thread`, `macros`, `sync`, `time`, `fs`, `net`, `io-util`, `process`).
- `async-trait` (pra traits `PipeReader`/`PipeWriter` async).
- `serde` + `serde_json`.
- `uuid`, `chrono`, `thiserror`, `tracing`.

A Etapa 2B adiciona `windows` crate via `[target.'cfg(windows)'.dependencies]` (gateado). O `check-core-purity.ps1` já reconhece `process-architecture` na `allowedPlatformCrates`.

**Quem depende dele:**

- Nenhum crate do workspace ainda. A Etapa 3 (`docs.generate` no `ToolRegistry`) consome `WorkerHandle::invoke`.
- O `document-worker` Python (Etapa 2B) consome o envelope JSON sobre named pipes.

## 4. Decisões não óbvias e armadilhas conhecidas

- **WorkerManager redesenhado como ator (ADR-0015).** A Etapa 2A original usou `Arc<Mutex<Box<dyn Pipe>>>` partilhado entre `invoke` e a task de leitura. O `MutexGuard` ficava segurado durante `tx.send().await` (no write) e `rx.recv().await` (no read) — deadlock clássico de lock segurado em `.await`. Pior: o mesmo design quebrava **duas invocações concorrentes**. Redesenhado como ator: uma task é dona exclusiva do pipe; `invoke`/`ping`/`shutdown` mandam comandos por `mpsc::Sender<ManagerCommand>` interno + `oneshot::Sender` correlacionado por `request_id` (que o **ator** gera, não o caller). Resolve o deadlock e o caso de invokes paralelos de brinde.

- **Trava no CI (medida 4 do ADR-0015):** `verify.ps1` Step 2 e `ci.yml` rodam `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock`. Mesmo espírito do `check-core-purity.ps1` (ADR-0003) — a máquina coíbe a classe, em vez de depender de revisão manual.

- **Todo teste de integração de worker passa por `with_test_timeout` (medida 2).** Helper em `crates/test-support/` (5s default). Deadlock vira falha com nome do teste em 5s, não sessão pendurada com saída cortada. É o antídoto direto pra classe de bug que produziu o deadlock da Etapa 2A original.

- **`std::sync::Mutex` no `pending` (não `tokio::sync::Mutex`).** Proposital: as operações dentro do lock (`insert`/`remove`) são síncronas e curtas, e o `MutexGuard` **nunca** segura um `.await`. É o que o `-D clippy::await_holding_lock` enforça em tempo de compilação.

- **`PipeWriter` é `Clone` + `Send` + `Sync`; `PipeReader` não é `Clone`.** A leitura é serializada pela task do ator. Concorrência não vem de readers paralelos — vem de **múltiplas requests em voo** com `request_id` distinto (correlacionadas via `oneshot`).

- **Handshake síncrono no `WorkerManager::spawn_in_process`.** Lê o `worker.hello` (que o fake envia no boot), gera o `WorkerAuth`, envia o `app.ack`, e **só então** spawna a task do ator. O ator não vê o `hello` inicial — o estado já está montado.

- **Envelope é JSON line-delimited** (cada `IpcMessage` termina em `\n`). O `IpcMessage::encode_line` adiciona o `\n`; `decode_line` exige o `\n` final (rejeita sem). O `LineBuffer` do `FakePipeReader` busca por `\n` em chunks.

- **`protocol_version = 1` é global do envelope** (vs. `WorkerManifest::version` que é versão do worker). Bump MAJOR em mudanças incompatíveis do envelope (campo obrigatório novo, opcode renomeado); bump MINOR em adições compatíveis (campo opcional novo).

- **`auth` é `Option<WorkerAuth>`** — vazio nas mensagens iniciais (`hello`/`ack`). O `FakeWorker` valida o token em toda `tool.invoke` contra o `auth` salvo no `app.ack`; o `document-worker` Python repete a checagem.

- **`PipeName` é validado** (não vazio, sem `\`, sem `/`, ≤ 200 chars — limite do Windows para `\\.\pipe\<name>`). O construtor `PipeName::new` é o único ponto de entrada.

- **`env_allowlist` é função pura sem I/O.** Teste `env_allowlist_does_not_inherit_parent` injeta `OPENAI_API_KEY` no test runner e prova que ela **não** vaza — sobreviveu à retirada do `WorkerManager` quebrado.

- **Health é atualizado pelo ator a cada response.** `Pong` → `Ok`; `Error` → `Degraded`; outros → não mexe. `WorkerHandle::health_snapshot()` lê o `Arc<RwLock<WorkerHealthSnapshot>>` (lock curto).

## 5. Como testá-lo isoladamente

```powershell
cd C:\src\Frederico
$env:PATH = "$env:PATH;C:\Users\conta\.cargo\bin"
cargo test -p frederico-process-architecture --no-fail-fast > test.log 2>&1
Get-Content test.log -Tail 50
```

**9/9 unit + 10/10 integration verde** em ~0.5s.

| Regra / comportamento | Teste | Onde |
|---|---|---|
| Envelope IPC round-trip | `protocol::tests::encode_decode_roundtrip` | unit |
| `protocol_version` errado é rejeitado | `protocol::tests::decode_rejects_wrong_protocol_version` | unit |
| Linha sem `\n` é rejeitada | `protocol::tests::decode_rejects_line_without_terminator` | unit |
| Opcode strings estáveis | `protocol::tests::op_strings_are_snake_case_and_stable` | unit |
| Env allowlist não herda pai | `env_allowlist::tests::env_allowlist_does_not_inherit_parent` | unit |
| Env allowlist inclui entries explícitas | `env_allowlist::tests::env_allowlist_includes_explicit_entries` | unit |
| `with_defaults` merge `always_include` | `env_allowlist::tests::env_allowlist_with_defaults_merges_always_include` | unit |
| `FakePipeReader`/`FakePipeWriter` roundtrip | `fake::tests::fake_reader_writer_split_roundtrip` | unit |
| `FakePipeWriter` é `Clone` + `close` idempotente | `fake::tests::writer_is_clone_and_idempotent_close` | unit |
| Spawn + handshake + shutdown | `fake_worker::spawn_in_process_smoke` | integration |
| `invoke` roundtrip preserva payload | `fake_worker::invoke_roundtrip_preserves_payload` | integration |
| `ping` atualiza `health` pra `Ok` | `fake_worker::ping_updates_health_to_ok` | integration |
| Env allowlist end-to-end (worker não vê pai) | `fake_worker::worker_does_not_see_parent_env` | integration |
| Env allowlist end-to-end (worker vê explícito) | `fake_worker::worker_sees_explicit_allowlist` | integration |
| **Duas invokes em paralelo** | `fake_worker::concurrent_invocations_complete` | integration |
| `invoke_with_timeout` falha com `Timeout` | `fake_worker::invoke_with_short_timeout_fails` | integration |
| `shutdown` termina worker limpo | `fake_worker::shutdown_terminates_worker_cleanly` | integration |
| Auth é validado após handshake | `fake_worker::auth_token_is_required_after_handshake` | integration |
| Auth token custom via `WorkerSpawnConfig` | `fake_worker::custom_auth_token_is_used` | integration |

**Todos** os integration tests são embrulhados em
`with_test_timeout` (5s default) — deadlock vira falha com nome
do teste em 5s.

## 6. O que ele **não** faz

- **Não abre named pipes reais.** `WindowsPipeReader`/`Writer`
  (gateado em `#[cfg(windows)]`) entra na Etapa 2B.
- **Não spawna processos externos.** `spawn_external` via
  `tokio::process::Command` entra na Etapa 2B.
- **Não conhece Python nem o `document-worker`.** Sidecar
  Python entra na Etapa 2B.
- **Não tem revogação de token.** `WorkerAuth` é `String`
  opaco; revogação por lista negra entra na Etapa 2B.
- **Não tem schema JSON do envelope versionado.** O envelope
  é validado por tipos Rust; o schema (via `schemars` 0.8,
  mesma estratégia do `document-engine`) entra quando a Etapa 3
  for definir o `input_schema` do `docs.generate`.
- **Não tem cancelamento de invoke em voo.** O `WorkerHandle`
  carrega o `CancellationToken` no `execution-engine` (Etapa 3)
  — `WorkerManager` não tem. O invoke só termina por timeout,
  response do worker, ou morte do worker (EOF).

## Pendências para a próxima etapa (2B)

1. `WindowsPipeReader`/`Writer` via `windows` crate (gateado em
   `#[cfg(windows)]`).
2. `spawn_external(command, args, pipe_name)` que abre o
   `document-worker.exe` via `tokio::process::Command` e
   conecta o pipe real.
3. `bootstrap.ps1` em `workers/document-worker/` que baixa
   Python embeddable + libs + Tesseract + fontes "Tinta &
   Latão".
4. `document-worker` Python (manifest, protocol, server,
   handlers stub).
5. Revogação de token por lista negra.
