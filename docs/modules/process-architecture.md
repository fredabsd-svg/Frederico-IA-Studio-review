# Módulo `frederico-process-architecture`

> Etapa 2B da Fase 5 — **parcial** (transporte real adicionado, smoke test com named pipes reais em `#[ignore]` aguardando diagnóstico de deadlock; `spawn_external` + `document-worker` Python ficam pra próxima sessão). Ver [ADR-0015](../decisions/0015-process-architecture-actor-not-mutex.md) (ator) + [ADR-0016](../decisions/0016-process-architecture-ator-impl.md) (impl ator, Etapa 2A) + [ADR-0017](../decisions/0017-process-architecture-windows-pipes.md) (Tokio, inversão do handshake, byte stream). Estado: parcial. Verificado contra o código em 2026-07-29.

## 1. O que este módulo faz

Define o **envelope IPC** (`IpcMessage` + 8 `IpcOp`) usado entre o app principal e os workers sidecar (`document-worker`, `sandbox-runner`, `browser-worker`, `runtime-manager`). É o **contrato**: line-delimited JSON sobre named pipes (Windows) ou qualquer transporte que implemente as traits `PipeReader` + `PipeWriter`. A Etapa 2A entrega o **manager** (modelo de ator) + o **fake in-process** que valida o design. A Etapa 2B adiciona o **transporte real** (`WindowsPipeReader`/`Writer` via `tokio::net::windows::named_pipe` — ADR-0017) e deixa pendente o `spawn_external` (`tokio::process::Command`) + o `document-worker` Python.

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

**Adicionado na Etapa 2B (parcial):**

- `WindowsPipeReader<R>` + `WindowsPipeWriter<W>` (genéricos sobre `AsyncRead`/`AsyncWrite`) + `shared_pipe_pair(inner)` — sobre `tokio::net::windows::named_pipe` (ADR-0017). Gateado em `#[cfg(windows)]`; o CI em Linux compila o `lib.rs` sem o módulo.
- `create_pipe_server(name)` + `connect_pipe_client(name)` + `full_pipe_path(name)` + `PIPE_PREFIX` — helpers de bootstrap do pipe. Caller é responsável pelo `ready()` antes do primeiro read/write.
- Modo **byte stream** (default Tokio, casa com line-delimited JSON do envelope).
- **Inversão do handshake** (ADR-0017): worker cria o server, app se conecta como client; worker anuncia o nome via stdout `READY <pipe_name>`. `spawn_external` é a próxima peça (Etapa 2B continuação).

**Fora desta entrega (próxima sessão):**

- `spawn_external(command, args, env)` — `tokio::process::Command` que abre o `document-worker.exe`, lê o `READY <pipe_name>` do stdout, e chama `connect_pipe_client`.
- `bootstrap.ps1` em `workers/document-worker/` (Python embeddable + libs + Tesseract + fontes "Tinta & Latão" — ADR-0004).
- `document-worker` Python (manifest, protocol, server, handlers stub).

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

A Etapa 2B **não precisou** adicionar a `windows` crate — o `tokio::net::windows::named_pipe` (já no grafo via `tokio = features = ["net"]`) envelopa o `HANDLE` Win32 com `AsyncRead` + `AsyncWrite` sem `unsafe` no nosso código (ADR-0017 §Decisão 1). O `check-core-purity.ps1` já reconhece `process-architecture` na `allowedPlatformCrates`.

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

- **`tokio::sync::Mutex` no `WindowsPipeReader`/`Writer` (ADR-0017).** O `tokio::sync::MutexGuard` é `Send`; o `std::sync::MutexGuard` é `!Send`. Como o `async-trait` exige que o future retornado seja `Send`, o guard precisa ser `Send` — força o uso do `tokio::sync::Mutex`. O `-D clippy::await_holding_lock` **não** flagra `tokio::sync::Mutex` (o guard do tokio é desenhado pra ser segurado em `.await`s). O `Arc<tokio::sync::Mutex<>>` permite `Clone` do writer e o compartilhamento do mesmo `HANDLE` Win32 entre reader e writer (caso do `NamedPipeServer`/`Client`).

- **Inversão do handshake (ADR-0017 §Decisão 2).** Worker cria o `NamedPipeServer`, gera o `PipeName` único via `unique_pipe_name()`, e escreve `READY <pipe_name>` no stdout antes de entrar no loop. O `WorkerManager::spawn_external` (próxima sessão) lê essa linha do stdout do filho e usa o nome pra `connect_pipe_client`. Resolve herança de handle sem complicar — `tokio::process::Command` no Windows herda stdin/stdout/stderr automaticamente; o `HANDLE` do pipe é criado pelo filho, não passado pelo pai.

- **`unsafe_code = "deny"` no `process-architecture`** (não `forbid` — abre a porta pra `windows` crate na Etapa 3+ se virar necessário, ex.: security descriptor customizado). O `windows_pipes.rs` é `#![cfg(windows)]` e não usa `unsafe` — a Tokio envelopa o `HANDLE` Win32 de forma segura (ADR-0017 §Decisão 5).

- **Modo byte stream (ADR-0017 §Decisão 3).** `ServerOptions::new()` sem `.message_mode(true)`. O envelope é line-delimited JSON, e byte stream é o default da Tokio. Message mode traria fragmentação (uma `IpcMessage` pode cair em duas mensagens do pipe se passar de 4 KB) e exigiria framing extra.

- **`ready()` antes do primeiro read/write (ADR-0017 §Ready).** O `NamedPipeServer` e o `NamedPipeClient` (depois de `connect`) exigem `pipe.ready(Interest::READABLE | Interest::WRITABLE).await` antes de qualquer read/write. O `shared_pipe_pair` em si não chama — o caller é responsável. O `WorkerManager::spawn_external` (próxima sessão) cuida.

## 5. Como testá-lo isoladamente

```powershell
cd C:\src\Frederico
$env:PATH = "$env:PATH;C:\Users\conta\.cargo\bin"
cargo test -p frederico-process-architecture --no-fail-fast > test.log 2>&1
Get-Content test.log -Tail 50
```

**9/9 unit + 10/10 integration verde** em ~0.5s (suite Etapa 2A). **Etapa 2B adiciona 3 unit tests no `windows_pipes.rs` (in-process com `tokio::io::duplex`) e 2 integration tests em `tests/windows_pipes_smoke.rs` marcados `#[ignore]`** (deadlock em diagnóstico — rodar com `cargo test -p frederico-process-architecture --test windows_pipes_smoke -- --ignored --nocapture`). Cobertura da abstração provada pelos unit tests; o smoke real fica pra próxima sessão.

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
| `WindowsPipeReader` lê linha via `duplex` | `windows_pipes::tests::windows_pipe_reader_reads_line` | unit |
| `WindowsPipeWriter` clone escreve serializado | `windows_pipes::tests::windows_pipe_writer_clone_writes` | unit |
| `shared_pipe_pair` compila e partilha `Arc<Mutex<>>` | `windows_pipes::tests::shared_pipe_pair_smoke_compiles` | unit |
| Named pipe real server↔client roundtrip (`#[ignore]`) | `windows_pipes_smoke::windows_pipe_server_client_roundtrip` | integration |
| Named pipe real read-then-write sequencial (`#[ignore]`) | `windows_pipes_smoke::windows_pipe_sequential_read_then_write` | integration |

**Todos** os integration tests são embrulhados em
`with_test_timeout` (5s default) — deadlock vira falha com nome
do teste em 5s.

## 6. O que ele **não** faz

- **Não abre named pipes reais em produção (ainda).** A
  abstração `WindowsPipeReader`/`Writer` está pronta e coberta
  pelos unit tests in-process (`tokio::io::duplex`). O smoke
  test com named pipes reais em `tests/windows_pipes_smoke.rs`
  está **`#[ignore]`** — deadlocka em runtime e entra na
  próxima sessão como diagnóstico. O contrato da abstração
  está provado pelos unit tests; o gap é o transporte real
  ponta-a-ponta, não a forma da API.
- **Não spawna processos externos.** `spawn_external` via
  `tokio::process::Command` entra na próxima sessão.
- **Não conhece Python nem o `document-worker`.** Sidecar
  Python entra na próxima sessão.
- **Não tem revogação de token.** `WorkerAuth` é `String`
  opaco; revogação por lista negra entra em hardening futuro.
- **Não tem schema JSON do envelope versionado.** O envelope
  é validado por tipos Rust; o schema (via `schemars` 0.8,
  mesma estratégia do `document-engine`) entra quando a Etapa 3
  for definir o `input_schema` do `docs.generate`.
- **Não tem cancelamento de invoke em voo.** O `WorkerHandle`
  carrega o `CancellationToken` no `execution-engine` (Etapa 3)
  — `WorkerManager` não tem. O invoke só termina por timeout,
  response do worker, ou morte do worker (EOF).

## Pendências para a próxima sessão (Etapa 2B continuação)

1. **Diagnosticar o deadlock do smoke test com named pipes reais**
   em `tests/windows_pipes_smoke.rs` (instrumentar com
   `tracing` no `ready()` e `lock().await`; considerar
   `tokio-console` ou Win32 ETW). Tira o `#[ignore]` quando
   o roundtrip com `NamedPipeServer`/`Client` reais
   completar em < 1s.
2. `WorkerManager::spawn_external(command, args, env)` que
   abre o `document-worker.exe` via `tokio::process::Command`,
   lê o `READY <pipe_name>` do stdout, faz `connect_pipe_client`,
   e devolve o `WorkerHandle` (mesma forma do `spawn_in_process`).
3. `bootstrap.ps1` em `workers/document-worker/` que baixa
   Python embeddable + libs + Tesseract + fontes "Tinta &
   Latão".
4. `document-worker` Python (manifest, protocol, server,
   handlers stub).
5. Revogação de token por lista negra (hardening).
