# Módulo `frederico-process-architecture`

> Etapa 2A da Fase 5 — **parcial** (commit próprio, `WorkerManager` fora — ver [ADR-0015](../decisions/0015-process-architecture-actor-not-mutex.md)). O que está **no repositório** e verde é o **protocolo IPC** + **Pipe trait** + **env allowlist** + **fake worker**. Estado: parcial. Verificado contra o código em 2026-07-29.

## 1. O que este módulo faz

Define o **envelope IPC** (`IpcMessage` + 8 `IpcOp`) usado entre o
app principal e os workers sidecar (`document-worker`,
`sandbox-runner`, `browser-worker`, `runtime-manager`). É o
**contrato**: line-delimited JSON sobre named pipes (Windows) ou
qualquer transporte que implemente a trait `Pipe`. A Etapa 2B
adiciona o transporte real (`WindowsPipeReader`/`Writer` via
crate `windows` gateado) e o `WorkerManager` redesenhado (modelo
de ator — ADR-0015).

O que está no repo e funciona:

- **Protocolo**: `IpcMessage` v1 com `protocol_version`,
  `request_id` (UUID v4 por mensagem), `op` (8 opcodes estáveis
  em snake_case), `payload` (JSON arbitrário), `auth` (token de
  curta duração opcional).
- **Manifesto**: `WorkerManifest` com ID, versão, capabilities,
  dependências, saúde, compatibilidade.
- **Auth**: `WorkerAuth` — `String` opaco v0.1; revogação entra
  na Etapa 2B.
- **Pipe trait**: abstração assíncrona de transporte
  (`read_line` + `write_line` + `close`). A Etapa 2B divide em
  `PipeReader`/`PipeWriter` (sem `Arc<Mutex<>>`).
- **env_allowlist**: constrói o env do worker a partir de uma
  allowlist explícita. O env do pai **não** é lido (regra do
  `process-architecture.md` §Invariantes).
- **fake**: worker simulado in-process (`FakeWorker` + `LineBuffer`)
  que entende os opcodes essenciais. Coberto por testes unitários
  do protocolo + o `fake_worker_handle_spawn_helper` (integration
  test que existia mas foi removido junto com o `WorkerManager` —
  volta com o redesenho).

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- `IpcMessage`, `IpcOp` (8 variantes), `RequestId`.
- `WorkerManifest`, `WorkerId`, `WorkerAuth`, `WorkerHealth`,
  `WorkerHealthSnapshot`, `Dependency`, `CompatibilityInfo`.
- `Pipe` (trait), `PipeName` (newtype validado).
- `EnvEntry`, `build_worker_env`, `build_worker_env_with_defaults`.
- `ProcessError` (6 variantes), `ProcessErrorKind` (6 categorias).
- `FakeWorkerConfig`, `FakePipeClient`, `FakeWorkerHandle`,
  `spawn_fake_worker`.

**Fora desta entrega (próxima sessão):**

- `WorkerManager`, `WorkerHandle`, `WorkerSpawnConfig` —
  removidos com o redesenho (ver ADR-0015). Entram de volta
  quando o modelo de ator for implementado.
- `WindowsPipeReader`/`Writer` — Etapa 2B.
- `spawn_external` — Etapa 2B.

**Não-público (interno):**

- `LineBuffer` (em `fake.rs`) — busca por `\n` no `read_line`.
- `protocol::PROTOCOL_VERSION` — bump MAJOR em mudanças
  incompatíveis do envelope.

## 3. Do que depende e quem depende dele

**Dependências (`Cargo.toml`):**

- `tokio` (`rt-multi-thread`, `macros`, `sync`, `time`, `fs`,
  `net`, `io-util`, `process`).
- `async-trait` (pra trait `Pipe` async).
- `serde` + `serde_json`.
- `uuid`, `chrono`, `thiserror`, `tracing`.

A Etapa 2B adiciona `windows` crate via
`[target.'cfg(windows)'.dependencies]` (gateado). O
`check-core-purity.ps1` foi atualizado pra reconhecer
`process-architecture` na `allowedPlatformCrates`.

**Quem depende dele:**

- Nenhum crate do workspace ainda. A Etapa 3 (`docs.generate`
  no `ToolRegistry`) consome `WorkerManager::invoke`.
- O `document-worker` Python (Etapa 2B) consome o envelope JSON
  sobre named pipes.

## 4. Decisões não óbvias e armadilhas conhecidas

- **WorkerManager saiu do repo (Etapa 2A → próxima sessão).** A
  primeira versão usou `Arc<Mutex<Box<dyn Pipe>>>` partilhado
  entre `invoke` e a task de leitura. O `MutexGuard` ficava
  segurado durante `tx.send().await` (no write) e
  `rx.recv().await` (no read) — deadlock clássico de lock
  segurado em `.await`. O redesenho (ADR-0015) troca por **ator**:
  uma task dona do pipe, `invoke` manda request por `mpsc`
  interno + `oneshot` correlacionado por `request_id`. Resolve o
  deadlock e o caso de `invoke`s paralelos.

- **CI trava a classe do bug (medida 4)**: `verify.ps1` Step 2
  roda `cargo clippy ... -D clippy::await_holding_lock`. Mesmo
  espírito do `check-core-purity.ps1` (ADR-0003) — a máquina
  coibe, em vez de depender de revisão manual.

- **Teste de worker embrulhado em 5s timeout (medida 2)**: regra
  da próxima sessão. Helper `with_test_timeout` em
  `crates/test-support/`. Deadlock vira falha com nome do
  teste em 5s — não sessão pendurada com saída cortada.

- **Envelope é JSON line-delimited** (cada `IpcMessage` termina
  em `\n`). O `IpcMessage::encode_line` adiciona o `\n`;
  `decode_line` exige o `\n` final (rejeita sem). O `LineBuffer`
  do `FakePipeClient` busca por `\n` em chunks.

- **`protocol_version = 1` é global do envelope** (vs.
  `WorkerManifest::version` que é versão do worker). Bump MAJOR
  em mudanças incompatíveis do envelope (campo obrigatório
  novo, opcode renomeado); bump MINOR em adições compatíveis
  (campo opcional novo).

- **`auth` é `Option<WorkerAuth>`** — vazio nas mensagens
  iniciais (`hello`/`ack`). Worker que não carrega token é
  rejeitado pelo fake (ver `fake.rs::ToolInvoke`); o
  `document-worker` Python repete a checagem.

- **`PipeName` é validado** (não vazio, sem `\`, sem `/`, ≤ 200
  chars — limite do Windows para `\\.\pipe\<name>`). O construtor
  `PipeName::new` é o único ponto de entrada.

- **`env_allowlist` é função pura sem I/O.** Teste
  `env_allowlist_does_not_inherit_parent` injeta `OPENAI_API_KEY`
  no test runner e prova que ela **não** vaza — é o teste que
  **sobreviveu** à retirada do `WorkerManager` quebrado.

## 5. Como testá-lo isoladamente

```powershell
cd C:\src\Frederico
$env:PATH = "$env:PATH;C:\Users\conta\.cargo\bin"
cargo test -p frederico-process-architecture > test.log 2>&1
Get-Content test.log -Tail 15
```

**7/7 unit verde** (4 do `protocol` + 3 do `env_allowlist`).

| Regra / comportamento | Teste |
|---|---|
| Envelope IPC round-trip | `protocol::tests::encode_decode_roundtrip` |
| `protocol_version` errado é rejeitado | `protocol::tests::decode_rejects_wrong_protocol_version` |
| Linha sem `\n` é rejeitada | `protocol::tests::decode_rejects_line_without_terminator` |
| Opcode strings estáveis | `protocol::tests::op_strings_are_snake_case_and_stable` |
| Env allowlist não herda pai | `env_allowlist::tests::env_allowlist_does_not_inherit_parent` |
| Env allowlist inclui entries explícitas | `env_allowlist::tests::env_allowlist_includes_explicit_entries` |
| `with_defaults` merge `always_include` | `env_allowlist::tests::env_allowlist_with_defaults_merges_always_include` |

## 6. O que ele **não** faz

- **Não gerencia workers.** O `WorkerManager` saiu do repo
  (Etapa 2A, ADR-0015). Volta na próxima sessão com modelo de
  ator.
- **Não abre named pipes reais.** A `Pipe` trait é a abstração;
  a impl real (`WindowsPipeReader`/`Writer`) entra na Etapa 2B.
- **Não spawna processos.** `spawn_external` via
  `tokio::process::Command` entra na Etapa 2B.
- **Não conhece Python nem o `document-worker`.** Sidecar
  Python entra na Etapa 2B.
- **Não tem revogação de token.** `WorkerAuth` é `String`
  opaco; revogação por lista negra entra na Etapa 2B.
- **Não tem schema JSON do envelope versionado.** O envelope
  é validado por tipos Rust; o schema (via `schemars` 0.8,
  mesma estratégia do `document-engine`) entra quando a Etapa 3
  for definir o `input_schema` do `docs.generate`.

## Pendências para a próxima sessão

1. Reintroduzir `WorkerManager` como **ator** (ADR-0015) — sem
   `Arc<Mutex<>>`, com `mpsc` de requests + `oneshot`
   correlacionado por `request_id`.
2. Helper `with_test_timeout` em `crates/test-support/`. Regra
   da próxima sessão: **todo** teste de integração de worker
   passa por ele (5s default). Deadlock vira falha com nome do
   teste em 5s.
3. Re-rodar a suíte `tests/fake_worker.rs` (10 tests) — agora
   com design correto, deve passar em < 1s.
4. Etapa 2B: `WindowsPipeReader`/`Writer` via `windows` crate
   (gateado em `#[cfg(windows)]`).
5. Etapa 2B: `document-worker` Python (bootstrap, manifest,
   protocol, server, handlers stub).
6. Atualizar este `docs/modules/process-architecture.md` com o
   `WorkerManager` redesenhado (template §1.4 só fecha quando o
   módulo estiver completo).
