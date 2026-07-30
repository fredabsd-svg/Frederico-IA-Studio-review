<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-29
Fase correspondente: 5
-->

> Última verificação: 2026-07-29. Reflete a **Fase 5** (Documentos) — `WorkerManager` redesenhado como ator (Etapa 2A, ADR-0015 + ADR-0016), transporte real sobre named pipes do Windows via Tokio (Etapa 2B, ADR-0017) com smoke test com pipes reais fechando em < 100ms (2 testes `windows_pipe_server_client_roundtrip` + `windows_pipe_sequential_read_then_write` saíram do `#[ignore]` na Etapa 2B continuação), handshake `worker.hello` / `app.ack` com `WorkerAuth` (UUID v4) já implementado, e o envelope IPC completo com 8 opcodes estáveis em snake_case. A integração com o `ToolRegistry` (Etapa 3) — onde o `docs.generate` consome o `WorkerHandle::invoke` — ainda não começou. O detalhe da implementação (manager + transport + testes + armadilhas) vive em [`docs/modules/process-architecture.md`](../modules/process-architecture.md) (atualizado na Etapa 2B continuação).

# Arquitetura de Processos

O Frederico IA Studio roda como um **app principal** que embute o núcleo Rust e a casca Tauri, mais um conjunto de **workers sidecar** que são processos separados. Cada worker é um executável distribuído com o app (ou baixado como pacote oficial assinado). Nenhum worker abre API em `localhost` (`PROMPT MESTRE` §5.3).

## Topologia

```text
┌─────────────────────────────────────┐
│  App principal (apps/desktop)        │
│   ├── Tauri runtime (WebView)        │
│   ├── Núcleo Rust (in-process)       │
│   └── Gerenciador de workers (ator)  │
└──┬──────┬──────┬──────┬────────────┘
   │      │      │      │  IPC (JSON line-delimited sobre named pipes, sem localhost)
   ▼      ▼      ▼      ▼
[document-worker] [sandbox-runner] [browser-worker] [runtime-manager]
  (Python)         (Rust)          (Rust+headless)    (Rust)
```

Os workers iniciais são previstos no `PROMPT MESTRE` §5.3. Workers adicionais só nascem com ADR.

## Contratos

### Manifesto do worker (handshake inicial, `PROMPT MESTRE` §7.3)

```rust
struct WorkerManifest {
    worker_id: WorkerId,
    version: SemVer,                  // handshake de versão do worker
    capabilities: Vec<String>,        // "docx.write", "ocr.run", "sandbox.exec.python", etc.
    dependencies: Vec<Dependency>,    // python-docx 1.1.0, tesseract 5.3, lang pack por.traineddata, etc.
    health: WorkerHealth,             // ok | degraded | unhealthy
    compatibility: CompatibilityInfo, // OS mínimo, arquitetura, runtime mínimo
}
```

### Mensagem IPC (envelope genérico)

```rust
struct IpcMessage {
    protocol_version: u32,            // versão do ENVELOPE (vs. WorkerManifest::version que é do worker)
    request_id: Uuid,                 // correlação request/response — toda response carrega o mesmo id
    op: IpcOp,                        // opcode estável em snake_case
    payload: serde_json::Value,       // validado por schema específico do `op`
    auth: Option<WorkerAuth>,         // token de curta duração (vazio em `hello`/`ack` inicial)
}
```

8 opcodes estáveis (versão de envelope atual = 1, bump MAJOR em mudanças incompatíveis): `worker.hello`, `app.ack`, `app.ping`, `worker.pong`, `app.shutdown`, `worker.error`, `tool.invoke`, `tool.result`. Implementação e invariantes em `crates/process-architecture/src/protocol.rs` (ver [`docs/modules/process-architecture.md`](../modules/process-architecture.md) §2).

### Descoberta na inicialização (`PROMPT MESTRE` §7.3)

Sequência obrigatória no boot do app:

1. Carrega ferramentas **internas** (registradas estaticamente no binário principal).
2. Spawn de cada worker registrado em `config/workers.toml` (Etapa 2B continuação: `WorkerManager::spawn_external` lê `READY <pipe_name>` do stdout do filho e usa pra fazer `connect_pipe_client`).
3. **Handshake síncrono** `worker.hello` → manifest + schema + saúde. O manager lê o `hello`, gera um `WorkerAuth` (UUID v4), e responde com `app.ack` carregando o token. **Daí em diante**, toda `tool.invoke` carrega o token — o worker valida contra o auth que recebeu.
4. Validação de `protocol_version` (incompatível = falha de boot).
5. Healthcheck ativo (ping com TTL curto).
6. Inventário consolidado no `ToolRegistry` (Etapa 3).
7. Workers que falham no handshake são marcados `unhealthy`; suas ferramentas ficam `unavailable` na UI e no cálculo de interseção (ver [`tool-registry-specification.md`](./tool-registry-specification.md)).

### Transporte (Windows)

**Named pipes** via `tokio::net::windows::named_pipe` (sem `crate windows`, sem `unsafe` no nosso código — ADR-0017 §Decisão 1). Modo **byte stream** (default Tokio; casa com line-delimited JSON — ADR-0017 §Decisão 3). **Inversão do handshake** (ADR-0017 §Decisão 2): o worker cria o `NamedPipeServer` e anuncia o nome via stdout `READY <pipe_name>`; o app se conecta como `NamedPipeClient` e resolve herança de handle sem complicar (o `HANDLE` é criado pelo filho, não passado pelo pai). **Conectando:** o server chama `pipe.connect().await` (envelopa `ConnectNamedPipe` Win32); o client (pós `ClientOptions::open`) chama `pipe.ready(Interest::READABLE | Interest::WRITABLE).await`. **NÃO** use `ready(READABLE | WRITABLE)` no server pré-connect — trava esperando readiness que nunca chega (Etapa 2B original usou, deadlockou; Etapa 2B continuação trocou por `connect()` e o smoke test com pipes reais passou em 0.01s).

O `WindowsPipeReader`/`Writer` envelopa o handle em `Arc<tokio::sync::Mutex<>>` — o `tokio::sync::MutexGuard` é `Send` (necessário porque o `async-trait` exige `Send` no future; o `std::sync::MutexGuard` é `!Send`). Justificativa completa em [`docs/modules/process-architecture.md`](../modules/process-architecture.md) §4.

## Invariantes

- **Nenhum worker abre porta TCP** em `localhost` ou em qualquer interface de rede. Auditável: script de teste tenta `netstat -an` durante boot e falha se houver porta aberta por worker.
- **Variáveis de ambiente do processo pai não são herdadas pelos workers** (`PROMPT MESTRE` §22.5). O worker recebe um env construído por allowlist via `env_allowlist::build_worker_env` (função pura — o env do pai **não** é lido). Teste `env_allowlist_does_not_inherit_parent` injeta `OPENAI_API_KEY` no test runner e prova que ela **não** vaza pro env construído pela allowlist.
- **Comunicação é autenticada** — `WorkerAuth` (UUID v4 v0.1) de curta duração, carregado no `app.ack` e em toda `tool.invoke` subsequente. Worker que reapresenta token revogado é morto (revogação por lista negra — hardening futuro, `docs/modules/process-architecture.md` §6).
- **Workers são stateless do ponto de vista do app** — morte e ressubida não corrompem estado, porque estado vive no SQLite via checkpoints (ver [`agent-state-machine.md`](./agent-state-machine.md)).
- **Worker lento é morto por watchdog** no `timeoutMs` declarado na chamada (`WorkerHandle::invoke_with_timeout`, default 30s). Watchdog é configurável por chamada; o `ToolManifest` pode sugerir timeoutMs (integração Etapa 3).
- **Zero polling no app principal.** Comunicação só por eventos, jamais por loop de checagem (`PROMPT MESTRE` §23.7). Health é atualizado pelo ator a cada response (`Pong` → `Ok`; `Error` → `Degraded`).

## Manager: modelo de ator (ADR-0015, ADR-0016)

A Etapa 2A original usou `Arc<Mutex<Box<dyn Pipe>>>` partilhado entre o `invoke`/`ping` e a task de leitura de background — o `MutexGuard` ficava segurado durante `tx.send().await` (no write) e `rx.recv().await` (no read), **deadlock clássico de lock segurado em `.await`**. Pior: quebrava duas invocações concorrentes.

Redesenhado como **ator** (Etapa 2A redesenho, ADR-0015 + ADR-0016): uma task é dona exclusiva do pipe (`Box<dyn PipeReader>` + `Box<dyn PipeWriter>` movidos pra dentro dela); `invoke`/`ping`/`shutdown` mandam comandos por `mpsc::Sender<ManagerCommand>` interno + `oneshot::Sender` correlacionado por `request_id` que o **ator** gera (não o caller). `pending: Mutex<HashMap<...>>` é o único lock do design, usado em `insert`/`remove` **síncronos** (sem `.await` dentro do lock) — é o que o `-D clippy::await_holding_lock` (no `verify.ps1` e no `ci.yml`, ADR-0015 §Trava) enforça em tempo de compilação. Suporta invocações concorrentes sem lock no caminho do `invoke` (cada invoke tem seu próprio `oneshot`).

Detalhe completo em [`docs/modules/process-architecture.md`](../modules/process-architecture.md) §4 (decisão "WorkerManager redesenhado como ator").

## Não-objetivos

- Comunicação via HTTP local (mesmo com TLS) — viola a invariante.
- Workers como bibliotecas dinâmicas carregadas no processo principal — viola isolamento e o `PROMPT MESTRE` §22.
- Workers em rede (entre máquinas) — fora do escopo da v1.
- Mais de uma instância do mesmo worker simultânea na v1.

## Decisões

- [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md) — por que a comunicação é via contrato serializável.
- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — por que `document-worker` é Python embutido.
- [ADR-0015](../decisions/0015-process-architecture-actor-not-mutex.md) — `WorkerManager` como ator (não mutex partilhado). Resolve deadlock da Etapa 2A original e suporta invokes paralelos.
- [ADR-0016](../decisions/0016-process-architecture-ator-impl.md) — implementação do ator (Etapa 2A fechada).
- [ADR-0017](../decisions/0017-process-architecture-windows-pipes.md) — `WindowsPipeReader`/`Writer` com Tokio, inversão do handshake (worker server, app client), byte stream, `tokio::sync::Mutex` (não `std`).

## Referências

- `PROMPT MESTRE` §5.3 (processos auxiliares), §7.3 (descoberta), §7.8 (resultado de ferramenta), §22 (sandbox), §22.5 (segredos e rede)
- [`tool-registry-specification.md`](./tool-registry-specification.md)
- [`security-threat-model.md`](./security-threat-model.md)
- [`testing-strategy.md`](./testing-strategy.md)
- [`docs/modules/process-architecture.md`](../modules/process-architecture.md) — fonte detalhada do que está implementado (atualizada em 2026-07-29, Etapa 2B continuação)
