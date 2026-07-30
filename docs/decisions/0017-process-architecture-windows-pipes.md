# 0017 — `process-architecture`: `WindowsPipeReader`/`Writer` com Tokio, inversão do handshake

## Contexto

A [ADR-0015](0015-process-architecture-actor-not-mutex.md) deixou três decisões em aberto para a Etapa 2B:

1. **Qual crate usar para os named pipes do Windows** — `crate windows` (a já usada em `frederico-security`, ADR-0007) ou `tokio::net::windows::named_pipe` (já no grafo de dependências via `tokio = features = ["net"]`).
2. **Quem cria o pipe** — o app (server) ou o worker (server). O handshake do `PROMPT MESTRE` §7.3 não fixa o lado.
3. **Como o `WorkerManager::spawn_external` passa o nome do pipe do worker para o app** — o app pode saber o nome de antemão (config) ou o worker pode anunciá-lo (stdout/env).

A Etapa 2A usou `tokio::net::windows::named_pipe` só via `FakePipeClient` (que é `mpsc::channel`, não usa a API real). A Etapa 2B precisa implementar o transporte real sobre `HANDLE` Win32.

## Decisão

1. **`tokio::net::windows::named_pipe` em vez de `crate windows`.** A Tokio envelopa o `HANDLE` em `NamedPipeServer` / `NamedPipeClient` com `AsyncRead` + `AsyncWrite`; não usamos `unsafe` no nosso código. `crate windows` só seria necessária se quiséssemos security descriptor customizado, handle duplication, ou `CreateProcessW` com flags específicas — nenhum desses é requisito da Etapa 2B.

2. **Inversão do handshake: o worker cria o pipe (server), o app se conecta (client).** O worker, ao subir, gera um `PipeName` único (via `unique_pipe_name()`), cria o `NamedPipeServer`, e escreve no **stdout** uma linha `READY <pipe_name>` antes de entrar no loop. O `WorkerManager::spawn_external` lê essa linha do stdout do filho (com timeout curto, ex. 10s) e usa o nome para fazer `NamedPipeClient::connect`. Resolve herança de handle sem complicar — `tokio::process::Command` no Windows herda stdin/stdout/stderr automaticamente; o `HANDLE` do pipe é criado pelo filho, não passado pelo pai.

3. **Modo byte stream** (`ServerOptions::new()` sem `.message_mode(true)`). O protocolo `IpcMessage` é line-delimited JSON, e byte stream é o default da Tokio. Message mode traria fragmentação confusa (uma `IpcMessage` pode cair em duas mensagens do pipe) e exigiria framing extra.

4. **`tokio::sync::Mutex` no `WindowsPipeWriter` (não `std::sync::Mutex`).** O `tokio::sync::MutexGuard` é `Send`; o `std::sync::MutexGuard` é `!Send`. Como o `async-trait` exige que o future retornado seja `Send`, o guard precisa ser `Send` — isso força o uso do `tokio::sync::Mutex`. O `-D clippy::await_holding_lock` (no `verify.ps1` e no `ci.yml`, ADR-0015) **não** flagra `tokio::sync::Mutex` — o guard do tokio é explicitamente desenhado para ser segurado em `.await`s. O `Arc<tokio::sync::Mutex<W>>` permite `Clone` do writer.

5. **`unsafe_code = "deny"` no `process-architecture`** (já estava no `Cargo.toml` desde a Etapa 2A). O módulo `windows_pipes.rs` é `#![cfg(windows)]` e não usa `unsafe`. A porta fica aberta para `crate windows` na Etapa 3+ se virar necessário (ex.: security descriptor customizado); por enquanto a Tokio é suficiente.

## Travas de CI

- `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock` (no `verify.ps1` e no `ci.yml`, ADR-0015) continua sendo a trava mecânica. Como o `windows_pipes.rs` é `#[cfg(windows)]`, o CI no Linux compila o `lib.rs` sem o módulo — não há regressão no caminho multiplataforma.
- `scripts/check-core-purity.ps1` (ADR-0003) já reconhece `crates/process-architecture` como exceção legítima (usa `tokio` com features de plataforma).
- O `Cargo.toml` do `process-architecture` declara explicitamente `unsafe_code = "deny"` (não `forbid`), com comentário apontando que a Etapa 3+ pode liberar via `#![allow(unsafe_code)]` em módulo específico, mesmo padrão do `frederico-security` (ADR-0007 §Implementação Windows).

## Alternativas descartadas

- **`crate windows` direto (sem Tokio).** Descartada: o `HANDLE` Win32 raw é síncrono; envelopar em `AsyncRead`/`AsyncWrite` exigiria implementar manualmente o `poll_read` / `poll_write` com `OVERLAPPED` e `GetOverlappedResult`. A Tokio já fez isso. **Reabrir** se aparecer requisito de security descriptor customizado.
- **App como server, worker como client.** Descartada: o app tem múltiplos workers potenciais; ter o app como server de N pipes concorrentes complica a vida. Inversão do handshake (worker server) é o padrão do `pipe` em Unix e da própria `tokio::net::windows::named_pipe` (a Tokio é desenhada pra suportar os dois lados, mas o padrão "server anuncia nome" é mais natural pra nosso caso).
- **Passar o nome do pipe via env.** Descartada: o env já tem o `allowlist` (ADR-0015); misturar nome de pipe no env viola a separação. O stdout é o canal de boot natural.
- **Passar o nome do pipe via stdin (worker lê do pai).** Descartada: o worker precisa do nome **antes** de criar o pipe (server), então o pai teria que gerá-lo — mas o pai não sabe quantos workers rodam nem os nomes. Inversão (worker gera, anuncia via stdout) é mais limpa.
- **Modo message.** Descartada: traria fragmentação (uma `IpcMessage` pode cair em duas mensagens do pipe se passar de 4 KB) e exigiria framing extra (length prefix ou reassembly). Byte stream + line-delimited JSON é mais simples e robusto.

## Consequências

**Mais fácil:**

- Zero `unsafe` no nosso código — a Tokio envelopa o `HANDLE` Win32 de forma segura.
- `WindowsPipeReader<R>` e `WindowsPipeWriter<W>` são **genéricos** sobre `AsyncRead`/`AsyncWrite` — testáveis com `tokio::io::duplex` (sem precisar de named pipes reais nos unit tests). Os integration tests com pipes reais ficam em `tests/windows_pipes_smoke.rs`, gateado em `#[cfg(windows)]`.
- O design combina com o modelo de ator (ADR-0015): o `WindowsPipeReader` vai pra task do ator (não `Clone`); o `WindowsPipeWriter` é `Clone` (via `Arc<Mutex<W>>`) e fica no `WorkerHandle`.
- O nome do pipe é anunciado pelo worker via stdout — o app não precisa coordenar nomes com o filho, e cada worker gera o seu (sem colisão).
- CI do projeto continua multiplataforma (Linux compila o crate sem o módulo Windows; o `#[cfg(windows)]` é gate).

**Mais difícil:**

- `tokio::sync::Mutex` é mais caro que `std::sync::Mutex` (alocação, contention). Para um pipe com poucas escritas concorrentes, é overhead desprezível. **Reabrir** se profiling mostrar problema.
- A leitura de `READY <pipe_name>` do stdout do filho precisa ser feita com timeout — se o worker travar antes de imprimir, o `spawn_external` precisa falhar com `ProcessError::Platform` claro, não pendurar.
- O `tokio::process::Command` precisa de `tokio` com feature `process` (já está no `Cargo.toml` da Etapa 2A).

## Pendências para a próxima sessão

1. `WorkerManager::spawn_external(command, args, env)` que abre o `document-worker.exe` via `tokio::process::Command`, lê `READY <pipe_name>` do stdout, faz `connect_pipe_client`, e devolve o `WorkerHandle` (mesma forma do `spawn_in_process`).
2. Integration test E2E: spawnar um processo filho real (ex.: `cmd /c echo READY foo`), validar o handshake, e fechar.
3. `document-worker` Python (Etapa 2B, lado Python) — gera `READY <pipe_name>`, implementa o protocolo `IpcMessage` sobre line-delimited JSON, responde a `tool.invoke` com `tool.result` ou `worker.error`.

## Referências

- [`windows_pipes.rs`](../../crates/process-architecture/src/windows_pipes.rs) — implementação.
- [ADR-0015](0015-process-architecture-actor-not-mutex.md) — modelo de ator.
- [ADR-0016](0016-process-architecture-ator-impl.md) — implementação do ator (Etapa 2A fechada).
- [ADR-0007](0007-credential-store-trait.md) — `windows` crate gateada em `frederico-security` (mesma estratégia que pode voltar aqui na Etapa 3+).
- [`process-architecture.md`](../architecture/process-architecture.md) §Invariantes (env allowlist, zero polling, sem TCP).
- `PROMPT MESTRE` §5.3, §7.3, §22.5
