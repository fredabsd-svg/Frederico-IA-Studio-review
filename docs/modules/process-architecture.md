# Módulo `frederico-process-architecture`

> **Etapa 2B+Y fechada (2026-07-30):** 7º handler (`ocr.run`) entra no `document-worker` Python v0.3.0 + Tesseract 5.4.0.20240606 (UB-Mannheim GitHub Releases, silent install com admin detection) + `por`+`eng`+`osd` traineddata (`tessdata_fast` 4.1.0, SHA-256 fixo) + `pytesseract` 0.3.10+ + `pdf.read` com fallback OCR transparente (`text` e `ocr_text` sempre separados, parâmetro `ocr: "auto"|"never"|"only"`, teto de 20 páginas com `ocr_truncated`, `tesseract_version` no retorno) + 5 testes E2E novos (2 com Tesseract + 3 sem) + CI noturno isolado (`.github/workflows/ci-nightly.yml`, cron `0 4 * * *` UTC) + ADR-0019 com 5 decisões (Tesseract source com admin detection, `tessdata_fast` 4.1.0, `por+eng` como default + `lang` parametrizável com validação regex, `text`/`ocr_text` separados com procedência, CI noturno isolado). **Mudança visível do `pdf.read`:** PDF 100% escaneado que antes retornava `ok: false, code: pdf_scanned_no_ocr` agora pode retornar `ok: true` com `text` do OCR + `extraction: "ocr"` (CHANGELOG registra breaking change). Ver [`docs/modules/document-worker.md`](document-worker.md) + [ADR-0019](../decisions/0019-document-worker-ocr-tesseract.md). Estado: produção. Verificado contra o código em 2026-07-30.

> **Etapa 2B+X fechada (2026-07-30, PR #12):** 6 handlers reais do `document-worker` Python (docx.write/read, xlsx.write/read, pdf.write/read) + bootstrap estendido (python-docx, openpyxl, reportlab, pdfplumber, Adobe Source Sans 3 + Source Serif 4) + 6 testes E2E em `tests/external_doc_worker.rs` (rodam em CI, NAO `#[ignore]`) + `scripts/verify-external.ps1` + cache de `runtime/` no `.github/workflows/ci.yml` + novo ADR-0018 (handler = primitiva de baixo nível, OCR deferido pra Etapa 2B+Y). **Limitação conhecida do `pdf.read` (resolvida na 2B+Y):** PDFs 100% escaneados devolvem `code: pdf_scanned_no_ocr` no payload (sem OCR até 2B+Y).

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

**Adicionado na Etapa 2B (parcial) + continuação (esta entrega):**

- `WindowsPipeReader<R>` + `WindowsPipeWriter<W>` (genéricos sobre `AsyncRead`/`AsyncWrite`) + `shared_pipe_pair(inner)` — sobre `tokio::net::windows::named_pipe` (ADR-0017). Gateado em `#[cfg(windows)]`; o CI em Linux compila o `lib.rs` sem o módulo.
- `create_pipe_server(name)` + `connect_pipe_client(name)` + `full_pipe_path(name)` + `PIPE_PREFIX` — helpers de bootstrap do pipe. Caller é responsável pelo `ready()` antes do primeiro read/write.
- Modo **byte stream** (default Tokio, casa com line-delimited JSON do envelope).
- **Inversão do handshake** (ADR-0017): worker cria o server, app se conecta como client; worker anuncia o nome via stdout `READY <pipe_name>`.
- **`WorkerManager::spawn_external(config: ExternalSpawnConfig)`** (cfg windows) — `tokio::process::Command` que abre o worker, lê `READY <name>` do stdout com timeout 10s, faz `connect_pipe_client` + `ready(READABLE | WRITABLE).await`, segue o handshake `worker.hello`/`app.ack` (mesmo do `spawn_in_process`), e devolve `(WorkerManager, WorkerHandle)` indistinguível do fake. `WorkerManager` ganhou campo `child: Option<Child>`; o `shutdown` faz `child.wait()` com timeout 5s + `kill` se o worker não respondeu ao `app.shutdown`. **PATH do pai é injetado automaticamente** (exceção documentada do §Invariantes — workers precisam pra resolver DLLs/binários; PATH não é segredo).
- `ExternalSpawnConfig` (com `new`/`with_args`/`with_env`/`with_cwd`/`with_auth_token`/`with_ready_timeout`) — re-exportado em `lib.rs` (`#[cfg(windows)]`).
- **`IpcMessage::decode_line` tolera BOM UTF-8** e **`\r\n`** no fim (defesa em profundidade — `StreamWriter` do .NET e PowerShell enviam assim por default).
- **`IpcOp` serializa com o nome do contrato** (`worker.hello`, `app.ack`, etc) — a Etapa 2A tinha um bug sutil que serializava `Hello` como `"hello"` (sem prefixo `worker.`); descoberto quando o stub PowerShell enviou o nome correto. Custom `Serialize`/`Deserialize` substitui o `rename_all = "snake_case"` anterior.

**Fora desta entrega (próximas etapas da Fase 5):**

- `ToolRegistry` da Etapa 3 consome `WorkerHandle::invoke` (integração casca ↔ worker — `docs.generate`).
- Handlers reais do `document-worker` (`docx.write`/`docx.read`/... + OCR) — dependem do `document-engine` (Etapa 1, já fechado) e de Tesseract + fontes "Tinta & Latão" do `bootstrap.ps1` estendido.
- **Tesseract + fontes** no `bootstrap.ps1` (Etapa 2B+X) — esta entrega instala só Python + pip + `pywin32`; a próxima adiciona os binários pesados.

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

**24/24 verde** em < 1s (12 unit + 10 fake_worker integration + 2 windows_pipes_smoke integration). **Etapa 2B** adiciona 3 unit tests no `windows_pipes.rs` (in-process com `tokio::io::duplex`) e 2 integration tests em `tests/windows_pipes_smoke.rs` com **named pipes reais** — os 2 saíram do `#[ignore]` na Etapa 2B continuação e rodam na suíte normal (sem `--ignored`) em 0.00s e 0.01s respectivamente. Cobertura completa: abstração provada pelos unit tests, transporte real ponta-a-ponta provado pelos integration tests com `NamedPipeServer`/`Client` reais.

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
| Named pipe real server↔client roundtrip | `windows_pipes_smoke::windows_pipe_server_client_roundtrip` | integration (0.00s) |
| Named pipe real read-then-write sequencial | `windows_pipes_smoke::windows_pipe_sequential_read_then_write` | integration (0.01s) |

**Todos** os integration tests são embrulhados em
`with_test_timeout` (5s default) — deadlock vira falha com nome
do teste em 5s.

## 6. O que ele **não** faz

- **Não conhece Python nem o `document-worker`.** O
  `document-worker` Python stub (Etapa 2B continuação) vive
  em `workers/document-worker/` — o `process-architecture` só
  conhece o envelope IPC, não o sidecar específico. **Stub de
  PowerShell** (`tests/stubs/worker-stub.ps1`) prova o
  `spawn_external` E2E sem depender de Python no CI.
- **Não instala Tesseract via winget/choco.** O `bootstrap.ps1` da
  Etapa 2B+Y instala Tesseract 5.4.0 do GitHub Releases do
  UB-Mannheim via silent install (NSIS `/S /D=<path>`), em contexto
  elevado. Em dev local non-elevated, o bloco é pulado com
  warning + instruções. A instalação pro usuário final fica pro
  instalador NSIS do Tauri (Fase 9). Detalhes no
  [ADR-0019](../decisions/0019-document-worker-ocr-tesseract.md).
- **Não tem revogação de token.** `WorkerAuth` é `String`
  opaco; revogação por lista negra entra em hardening futuro
  (pendência herdada — precisa de ADR).
- **Não tem schema JSON do envelope versionado.** O envelope
  é validado por tipos Rust; o schema (via `schemars` 0.8,
  mesma estratégia do `document-engine`) entra quando a Etapa 3
  for definir o `input_schema` do `docs.generate`.
- **Não tem cancelamento de invoke em voo.** O `WorkerHandle`
  carrega o `CancellationToken` no `execution-engine` (Etapa 3)
  — `WorkerManager` não tem. O invoke só termina por timeout,
  response do worker, ou morte do worker (EOF).

## Pendências para a próxima sessão

1. **Etapa 3 (ToolRegistry + kits DocumentSpec):**
   `ToolManifest::allowed_paths` para path safety forte (a
   barreira atual no Python é rejeitar `..`; a forte é
   allowlist de diretórios por tool, validada no manager
   Rust antes do `invoke`). Os 7 handlers da v0.3.0 do
   `document-worker` sobrevivem à Etapa 3 sem reescrita
   (handler = primitiva, kit = renderer do DocumentSpec,
   conforme [ADR-0018](../decisions/0018-document-worker-handlers-primitive.md) §Decisão 1).
2. **Revogação de token por lista negra** (hardening) — o
   `WorkerAuth` é `String` opaco; revogação por lista negra
   é a próxima peça. **Decisão de arquitetura:** o que a
   lista negra contém (hash do token? ID explícito?) precisa
   de ADR antes da implementação.
3. **Tabela de capacidades dinâmica** — o `IpcOp` hoje é
   lista fechada (8 opcodes hardcoded); a Etapa 3 vai precisar
   que tools dinâmicas (do `ToolRegistry`) sejam roteáveis.
   Pode entrar via `op = "tool.invoke"` com `capability` no
   payload (como o `document-worker` já faz).
4. **`WorkerHealth::Unknown` (nunca observado) distinto de
   `Unhealthy` (observado e ruim)** — encontrada pela
   regressão do PR #24 (Etapa 5 da Fase de Ligação).
   O `WorkerHealth::Unhealthy` é o `#[default]` do enum
   (`src/protocol.rs`) E o valor inicial de
   `fresh_health_snapshot()` (`src/manager.rs`). O snapshot
   só vira `Ok` no primeiro `Pong` recebido pelo ator
   (`manager.rs:839`); o handshake `worker.hello`/`app.ack`
   do `spawn_external` NÃO emite `Pong`. Resultado:
   qualquer consumidor que chame `health_snapshot()` antes
   do primeiro `Pong` recebe `Unhealthy` indistinguível
   de "worker está ruim de verdade" — e age errado
   (recusa invoke, transita pra Restarting, etc.).
   **Workaround atual (Etapa 5):** o
   `DocumentWorkerLauncher::invoke` chama
   `ensure_first_pong(handle)` antes da checagem, fazendo
   um `ping` se a saúde for stale. **Correção de
   modelagem:** introduzir `WorkerHealth::Unknown`
   (nunca observado) e fazer a guarda recusar só em
   `Unhealthy` (observado). Aí o helper some — o
   `Unknown` é informação útil, não uma falha. Cuidado
   de migração: o default do enum muda; testes que
   comparam `== Unhealthy` precisam virar
   `== Unhealthy || == Unknown` durante a transição, ou
   rodar o `ensure_first_pong` no setup. ADR nova
   necessária (decisão de modelagem que afeta
   `protocol`, `manager`, `launcher`, e qualquer outro
   consumer de `health_snapshot`).

> **Resolvido na Etapa 2B continuação** (2026-07-29): os 2
> integration tests com named pipes reais em
> `tests/windows_pipes_smoke.rs` saíram do `#[ignore]`. Fix
> foi trocar `ready(READABLE | WRITABLE)` por `connect()` no
> `NamedPipeServer` — `ready()` num server pré-connect trava
> esperando readiness que nunca chega. Detalhes no header do
> `tests/windows_pipes_smoke.rs` e no §4 "connect() no server,
> ready() no client" do `src/windows_pipes.rs`.
