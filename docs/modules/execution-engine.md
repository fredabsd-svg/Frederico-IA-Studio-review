<!--
Estado: implementado
Verificado contra o código em: 2026-08-07
Fase correspondente: 3 (Etapa 4 + 4.x + 4.x.y) + Fase de Ligação (Etapa 1) + Fase 6 (Etapa 2 + Etapa 3 PR 1 + PR 2 + Etapa 4 PR 1 + Etapa 4 PR 2)
-->

# `frederico-execution-engine` — RunExecutor

> **Fase 3, Etapa 4** (jul/2026) + **Fase de Ligação, Etapa 1**
> (ago/2026). O coração do "Fluxo vertical 1" do `PROMPT MESTRE`
> §33: fecha o loop `tool_call` consumindo o stream do
> `ProviderAdapter`, validando a `ToolCall` no `ToolRegistry`,
> executando a `Tool` concreta, emitindo o `ToolResult` pro
> modelo e persistindo tudo no SQLite — tudo dentro da máquina
> de estados de 22 arestas definida no `frederico-agent-engine`.

**Mudança da Etapa 1 da Fase de Ligação (ADR-0022 §D3):** o
`RunExecutor` recebe `Arc<dyn JailResolver>` em vez de `Jail`
direto. O `Jail` efetivo é resolvido uma vez por run (no
início do `run()` via `RunRepo::get(run_id)` para extrair o
`conversation_id` + `JailResolver::resolve(&cid)`), e cacheado
para uso pela validação (Passo 7 do `validate_tool_call`) e
pelo `ToolContext` entregue a `Tool::execute` por tool_call.
**Breaking change** em `RunExecutor::new` (substitui `jail:
Jail` por `jail_resolver: Arc<dyn JailResolver>`) — registrado
no `CHANGELOG.md` da Etapa 1.

## O que este módulo faz

```text
            ┌──────────────────────────────────────────────────────────┐
            │                   RunExecutor::run                      │
            │                                                          │
 ChatMessage ┐                                                        │
             │   1. tick budget (max_steps)                           │
             ├─► 2. monta ChatRequest com tools: (Registry × allowed) │
             │   3. adapter.stream(req) ──► BoxStream<StreamEvent>    │
             │      │                                                  │
             │      │  para cada ev:                                   │
             │      │    a. persist_journal(ev) (transaction-safe)     │
             │      │    b. match ev:                                  │
             │      │       Delta  → accumulated + Message.set_content│
             │      │       Usage  → prompt/completion tokens         │
             │      │       ToolCall → guarda pro próximo round      │
             │      │       Done   → sai do while                     │
             │      │       Error  → finaliza Failed                  │
             │      │       Cancelled → finaliza Cancelled            │
             │      │                                                  │
             │      ▼                                                  │
             │   4. tool_call collected?                              │
             │      sim:                                              │
             │        a. validate_tool_call (10 passos)               │
             │        b. tool.execute(args) ──► ToolResult            │
             │        c. StreamEvent::ToolResult { id, ok, output }   │
             │        d. persist_journal(tool_result)                 │
             │        e. ChatMessage::tool(...) → contexto            │
             │        f. volta ao passo 1 (próximo round)             │
             │      não:                                             │
             │        a. Message.status = Completed                   │
             │        b. Run.status    = Completed                    │
             │        c. retorna RunOutcome                           │
             └──────────────────────────────────────────────────────────┘
```

## Decisões não óbvias

### 1. `BEGIN IMMEDIATE; ...; COMMIT;` é da Etapa 5, não desta

Cada `persist_journal` é um `INSERT` único no SQLite, que já é
atômico por si (single statement). A regra de **"journal-then-emit"**
(spec `chat-and-providers.md`) é mantida: o evento é gravado no
journal **antes** de ser processado. Falha na persistência aborta
o loop com `Err`.

A Etapa 5 (watchdog) introduz `BEGIN IMMEDIATE; ...; COMMIT;`
explícito quando precisar agrupar appends numa transação maior (ex.:
checkpoint de fim de round).

### 2. Custo em microcents = 0 (placeholder)

A Etapa 4 não usa `CostModel` do adapter. O `MessageRepo::set_usage_and_cost`
é chamado com `cost_microcents: 0`. A Etapa 4.x (ou Etapa 5)
reintroduz o cálculo de custo a partir do `frederico-model-catalog`
quando o `ChatOrchestrator` for refatorado pra consumir o
`RunExecutor` no lugar do `run_stream_loop` da Fase 2.

### 3. `runs.state` granular não é atualizado aqui

A Etapa 4 atualiza `messages.status` e `runs.status` (Fase 2 — 6
valores, derivado pela view `runs_with_status`). A coluna
`runs.state` (22 valores, `frederico_agent_engine::RunState`) é
populada pela Etapa 5 (watchdog) a partir do journal.

A máquina de estados **em memória** é carregada do `Run` quando o
`ChatOrchestrator` da Fase 2 passar o controle pro `RunExecutor`.
Quando o executor termina, ele devolve um `RunOutcome` com o
`RunState` final mapeado (ex.: `Completed` para `Stop`/`Length`,
`Failed` para budget estourado, `Cancelled` para cancel, etc.).

### 4. `ApprovalRequired` é erro nesta etapa

A Etapa 4 implementa o caminho `validate_tool_call → Approved →
execute → ToolResult` e `Rejected → ToolResult(erro)`. O caminho
`ApprovalRequired` (Etapa 2 do `validate_tool_call`) é aceito pelo
validador mas o executor **aborta** com
`ExecutorError::ApprovalRequired` porque a UI da Etapa 6 (modal de
aprovação) ainda não existe. A Etapa 6 substitui o abort por
enfileiramento na fila de aprovação.

### 5. `Role::Tool` adicionado ao `ChatMessage`

A Etapa 4 introduz a variante `Role::Tool` no `provider_engine::types`
e o campo `tool_call_id: Option<String>` no `ChatMessage`. O
`OpenAiCompatAdapter::role_to_str` traduz `Role::Tool` em `"tool"`
(o payload do OpenAI espera `{"role": "tool", "tool_call_id": "...",
"content": "..."}`). O `AnthropicAdapter` traduz em `"user"`
placeholder — o formato Anthropic correto (`{"type": "tool_result",
...}`) é trabalho da Etapa 4.1.

O `ChatMessage::tool(name, content, tool_call_id)` é o helper que o
executor usa pra adicionar a resposta da ferramenta ao contexto.

### 6. `Jail::resolve` re-chamado no `Tool::execute` (defesa em profundidade)

O `validate_tool_call` (Passo 7) já valida o path. O
`FilesReadTool::execute` re-chama `self.jail.resolve(...)` antes de
ler o arquivo. É redundante em produção mas defende contra chamadas
diretas de `Tool::execute` em testes que pulam o validador.

### 7. `ScriptedProviderAdapter` local nos testes E2E

Diferente do `FakeProviderAdapter` (em `provider-engine::fake`), que
sempre devolve o mesmo script, o executor precisa de **múltiplos
scripts** (um por `stream()` — round 1 = `Delta + ToolCall +
Done{ToolCalls}`, round 2 = `Delta + Done{Stop}`). O
`ScriptedProviderAdapter` em `tests/e2e.rs` implementa isso com um
`VecDeque<Vec<StreamEvent>>` interno.

## API pública

```rust
// Construtor
pub fn new(
    adapter: Arc<dyn ProviderAdapter>,
    registry: ToolRegistry,
    jail: Jail,
    db: Database,
    permissions: PermissionSet,
    allowed_for_run: Vec<ToolId>,
    tools: Vec<Arc<dyn Tool>>,
    budget: Budget,
    cancel: CancellationToken,
) -> Self

// Loop principal
pub async fn run(
    &mut self,
    message_id: MessageId,
    run_id: RunId,
    model_id: ModelId,
    initial_messages: Vec<ChatMessage>,
) -> ExecutorResult<RunOutcome>

// Tipos auxiliares
pub struct RunOutcome {
    pub stop_reason: StopReason,
    pub final_state: RunState,
    pub accumulated_content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub tool_calls: Vec<ToolCallLog>,
}

pub struct ToolCallLog {
    pub id: String,
    pub tool_id: ToolId,
    pub arguments: serde_json::Value,
    pub ok: bool,
    pub output: serde_json::Value,
}

pub enum ExecutorError {
    Storage(StorageError),
    Provider(ProviderError),
    Budget(BudgetError),
    ApprovalRequired,
    ToolNotExecutable(ToolId),
    UnknownTool(String),
}
```

## Cobertura de testes

| Cenário | Teste E2E | Status |
| --- | --- | --- |
| **Fluxo vertical 1** — `files.read` happy path, loop fechado | `e2e::executor_closes_tool_call_loop_with_files_read` | ✅ |
| Sem `tool_call` → `Completed` direto | `e2e::executor_completes_when_no_tool_call` | ✅ |
| `Error` do provider → `Failed` | `e2e::executor_fails_on_provider_error` | ✅ |
| `Rejected` pelo validador (Passo 4) → `ToolResult(erro)` | `e2e::executor_emits_tool_result_with_error_when_rejected` | ✅ |
| `Jail` rejeita path traversal → `ToolResult(TOOL_JAIL_VIOLATION)` | `e2e::executor_rejects_path_traversal_in_tool_call` | ✅ |
| Sanidade do `ScriptedAdapter` | `e2e::scripted_adapter_returns_one_script_per_call` | ✅ |
| **Cancelamento** — adapter emite `Cancelled` mid-stream | `cancel::cancel_via_provider_cancelled_event` | ✅ |
| **Cancelamento** — token cancelado antes do 1º tick | `cancel::cancel_via_token_before_first_tick` | ✅ |
| **Recovery** — journal persiste dentro do ciclo de vida do `RunExecutor` | `recovery::journal_persists_events_within_run_executor_lifecycle` | ✅ |
| **Recovery** — `final_state` (Completed) bate com `Run.status` no db | `recovery::final_state_persists_to_database` | ✅ |
| **Recovery** — `Error` do provider → `Run.status = Failed` | `recovery::provider_error_persists_as_failed` | ✅ |
| **Recovery** — journal inclui `tool_result` com `files.read` no meio | `recovery::journal_includes_tool_result_event` | ✅ |
| **Watchdog** — `event_timeout` estoura sem evento → `RunState::Interrupted` | `watchdog::watchdog_closes_run_after_event_timeout` | ✅ |
| **Integração** — `ChatOrchestrator` (no `execution-engine`) persiste user msg, dispara stream, finaliza com `Run.status` correto | `integration_orchestrator::send_message_persists_user_first` | ✅ |
| **Integração** — `ChatOrchestrator` persiste journal via `RunExecutor` e finaliza `Run.status = completed` | `integration_orchestrator::send_message_persists_journal_and_finalizes` | ✅ |
| **Integração** — `RunGetEvents` carrega janela com `since_seq` (após migração) | `integration_orchestrator::get_events_with_since_seq_skips_old` | ✅ |
| **Integração** — `RunCancel` marca `status = cancelling` no `Run` | `integration_orchestrator::cancel_run_marks_requested` | ✅ |
| **Integração** — provider desconhecido retorna erro estruturado | `integration_orchestrator::unknown_provider_returns_error` | ✅ |
| **Auditoria** (Etapa 5.x, Passo 10) — `files.read` aprovada+executada grava entrada em `tool_audit` via `DbAuditSink` | `audit::audit_records_files_read_execution` | ✅ |
| **Recovery** (Etapa 5.x) — runs com heartbeat velho são marcados como `interrupted` no startup | `recovery::recover_marks_only_stale_runs` | ✅ |
| **Recovery** (Etapa 5.x) — runs terminais com heartbeat velho são pulados | `recovery::recover_skips_terminal_runs` | ✅ |

**Cobertura total do crate:** 6 (e2e) + 2 (cancel) + 4 (recovery) + 1
(watchdog) + 5 (integration_orchestrator) + 1 (audit) = **19 testes E2E
verdes** + 13 unit (`budget::tick` + 10 `state_mapping` + 2 `recovery`)
= **32/32**. Suíte workspace: **266/266 verde** em 34 targets (era 252
da Etapa 4.x.y; +10 state_mapping + 2 recovery + 1 audit + 2
`cancel_http` no `provider-engine`).

**Cobertura do `provider-engine`** (5.x.1 — cancel real do HTTP):
2 testes em `tests/cancel_http.rs` (`cancel_drops_reqwest_connection` +
`stream_completes_normally_when_not_cancelled`).

## Onde o `RunExecutor` se encaixa na arquitetura

```text
UI (Tauri)              ←───── StreamEvent ─────── EventSink
    │
    ▼
ChatOrchestrator (Fase 2)   ←── persiste message/user, cria run
    │                              │         cancel registry
    │                              ▼
    │                          RunExecutor (Etapa 4)  ◄── VOCÊ ESTÁ AQUI
    │                              │         │
    │                              │         ├── ProviderAdapter
    │                              │         ├── ToolRegistry (manifests)
    │                              │         ├── Jail (path)
    │                              │         ├── PermissionSet
    │                              │         ├── Tools concretas (FilesReadTool)
    │                              │         └── CancellationToken (Etapa 4.x)
    │                              ▼
    │                          SQLite (Database)
    │                              ├── messages
    │                              ├── message_events
    │                              │      ├── delta
    │                              │      ├── tool_call
    │                              │      ├── tool_result  ◄── NOVO Etapa 4
    │                              │      ├── done
    │                              │      └── error
    │                              └── runs
    ▼
EventSink.emit_run_event  (Tauri emite pra UI)
```

### Etapa 4.x — cancelamento e recovery no `RunExecutor`

A Etapa 4.x adiciona dois comportamentos ao `RunExecutor` que não
tinham ficado claros na Etapa 4:

- **Cancelamento** (testes em `tests/cancel.rs`): o `CancellationToken`
  passado no `new` é checado em **três pontos** do loop:
  1. Entre rounds do loop externo (passo 0) — antes do `tick` do
     `BudgetEnforcer`.
  2. Entre eventos do `stream.next()` — antes do `persist_journal`.
  3. Quando o adapter emite `StreamEvent::Cancelled` (que o
     `RunExecutor` traduz em `MessageStatus::Cancelled` /
     `RunStatus::Cancelled`).
  Quando o cancel chega, o executor fecha como Cancelled e
  retorna `RunOutcome { final_state: RunState::Cancelled, ... }`.
  **Limitação conhecida** (Etapa 5.x): o executor **não** interrompe
  a request HTTP em andamento no adapter — o `OpenAiCompatAdapter`
  recebe o `cancel: CancellationToken` no `ChatRequest`, mas o
  `tokio::select!` com `reqwest` observando o token é trabalho
  da Etapa 5.x. Por enquanto, a checagem cobre o caso "cancel
  chegou entre eventos".

- **Recovery** (testes em `tests/recovery.rs`): 4 testes E2E
  replicam a regra "Journal-then-emit" com o `RunExecutor`:
  1. O journal persiste os eventos durante o ciclo de vida do
     `RunExecutor` (5 deltas + 1 Done).
  2. O `Run.status` no db bate com o `final_state` do
     `RunOutcome`.
  3. `Error` do provider → `Run.status = Failed` no db.
  4. Journal inclui `tool_result` quando há `files.read` no meio
     do stream (específico da Etapa 4 — `tool_result` é uma
     variante nova do journal).

- **Watchdog** (teste em `tests/watchdog.rs`): 1 teste E2E
  (`watchdog_closes_run_after_event_timeout`) com
  `TrailingAdapter` que emite 1 delta e trava. O `event_timeout`
  (default 60s, configurável via
  [`RunExecutor::with_event_timeout`]) é o teto **entre**
  eventos. Quando estoura, o executor finaliza como
  `RunState::Interrupted` / `RunStatus::Timeout` (a view
  `runs_with_status` mapeia `interrupted → timeout`) e emite o
  journal do `Delta` que veio antes. A Etapa 5 não introduz
  cancelamento real do HTTP (`reqwest` observando o token) —
  isso é trabalho da Etapa 5.x porque exige mudanças no
  `OpenAiCompatAdapter` real.

### Por que dois testes de recovery?

O `provider-engine/tests/recovery.rs` da Fase 2 valida o mesmo
"Journal-then-emit" com o `run_stream_loop` da Fase 2. O
`execution-engine/tests/recovery.rs` da Etapa 4.x valida com o
`RunExecutor`. Quando a Etapa 4.x.y **reorganizou os crates**
(movendo o `ChatOrchestrator` pro `execution-engine` que delega
o loop pro `RunExecutor`), os 3 tests do `provider-engine` foram
atualizados pra usar o `ChatOrchestrator` do `execution-engine` via
dev-dep — a regra "Journal-then-emit" continua válida, agora via
`RunExecutor` por baixo. Até uma próxima reorganização, ter os
dois em paralelo dá segurança: se o `RunExecutor` quebrar, o do
`execution-engine` pega; se houver regressão na camada de
compatibilidade do `provider-engine`, o do `provider-engine` pega.

### Etapa 4.x.y — reorganização dos crates (`ChatOrchestrator` ↔ `RunExecutor`)

A Etapa 4 deixou o `RunExecutor` **vivo nos testes E2E** mas o
`ChatOrchestrator` da Fase 2 (`provider-engine::orchestrator`)
ainda rodava o `run_stream_loop` original — o "Fluxo vertical 1"
estava destravado só em testes, não em produção. A Etapa 4.x.y
**move** o `ChatOrchestrator` pro
`frederico-execution-engine::orchestrator` e o faz **delegar o
loop** pro `RunExecutor`.

**Por quê mover e não re-exportar?** Porque o grafo de
dependências era um ciclo: `provider-engine` re-exportava o
`ChatOrchestrator`, que dependia do `RunExecutor`, que dependia
do `provider-engine` (por `ProviderAdapter`, `StreamEvent`, etc.).
`cargo` rejeita isso. Movendo o `ChatOrchestrator` pro
`execution-engine`, o grafo fica
`provider-engine ──► execution-engine ──► provider-engine`
(sem ciclo — DAG).

**O que ficou no `provider-engine::orchestrator`:**
- `error_to_view` (função pura — tabela PT-BR com ação)
- `ProviderErrorView` (struct)
- `OrchestratorError`/`OrchestratorResult` definidos
  **localmente** (sem `pub use frederico_execution_engine::*` —
  causaria ciclo). A enum tem as mesmas 4 variantes da Fase 2
  (`Storage`, `Provider`, `ProviderNotFound`, `ModelNotFound`,
  `ModelWithoutPrice`) — `Executor` foi removido porque o
  `RunExecutor` roda em background (via `tokio::spawn`) e erros
  são logados na task, não propagam pro `send_message` síncrono.

**O que mudou no `ChatOrchestrator::new`:** ganhou 4 args novos:
- `tool_registry: ToolRegistry`
- `jail: Jail`
- `tools: Vec<Arc<dyn Tool>>`
- `allowed_for_run: Vec<ToolId>`

**O que mudou no `send_message`:** agora monta o `RunExecutor`
em background (`tokio::spawn`) e o loop roda lá dentro. Quando
termina, o `send_message` síncrono:
1. Lê o `RunOutcome` retornado.
2. Calcula custo real via `descriptor.cost_microcents(p, c)` (do
   `frederico-model-catalog` — o `ChatOrchestrator` carrega o
   `ModelCatalog` que era inerte na Etapa 4).
3. Persiste `MessageRepo::set_usage_and_cost(p, c, cost)` +
   `ConversationRepo::add_cost(conv_id, cost)` quando
   `Completed` (a Etapa 4 deixava o custo em 0).
4. Emite o `RunStatus` final pro `EventSink` e faz
   `RunRegistry::unregister(run_id)`.

**Testes migraram:** os 5 tests do `ChatOrchestrator` (eram
`provider-engine::orchestrator::tests`) viraram
`execution-engine::tests::integration_orchestrator.rs`:
`send_message_persists_user_first`,
`send_message_persists_journal_and_finalizes`,
`get_events_with_since_seq_skips_old`,
`cancel_run_marks_requested`,
`unknown_provider_returns_error`. Os 3 tests do
`provider-engine::tests::recovery.rs` da Fase 2 foram
atualizados pra usar o `ChatOrchestrator` do `execution-engine`
via dev-dep.

**Decisão de encapsulamento:** os campos do `ChatOrchestrator`
(`db`, `providers`, `runs`, `sink`, `catalog`, `tool_registry`,
`jail`, `tools`, `allowed_for_run`) ficaram `pub` (eram
`private` na Fase 2) pra que o executor de testes integre
direto. É um trade-off consciente — encapsulamento
relaxado por simplicidade, justificado pelo uso massivo de
E2E na Fase 3.

**Casca Tauri** (`apps/desktop/src-tauri/src/main.rs`):
constrói o `ChatOrchestrator` com o tooling inicial — `ToolRegistry`
com `files.read`, `Jail` no `current_dir`, `FilesReadTool` como
tool concreta, `[ToolId::new("files.read")]` como
`allowed_for_run`. É o "Fluxo vertical 1" rodando em produção.

### Etapa 5 — watchdog de 60s

A Etapa 5 (escopo mínimo) fecha a primeira parte do
"monitoramento de saúde" do executor: **watchdog entre eventos**.
O `tokio::select!` no loop interno ganhou 3 braços:

1. `cancel.cancelled()` (biased — prioridade alta)
2. `tokio::time::sleep(event_timeout)` (watchdog)
3. `stream.next()`

O `event_timeout` é o teto **entre** eventos (reseta a cada
`stream.next()` que retorna `Some`); default 60s, configurável
via `RunExecutor::with_event_timeout(Duration)` (encadeável, pra
não quebrar a assinatura do `new`).

Quando o watchdog estoura sem o adapter emitir nada, o executor
fecha como `MessageStatus::Timeout` / `RunStatus::Timeout` /
`RunState::Interrupted`. A view `runs_with_status` já mapeia
`interrupted → timeout` (reaproveita o mapeamento da Fase 2).
O journal do `Delta` que veio antes do timeout é preservado.

**Limitação documentada** (continua da Etapa 4.x): o executor
**não** interrompe a request HTTP em andamento no adapter
real — o `OpenAiCompatAdapter` recebe o
`cancel: CancellationToken` no `ChatRequest`, mas o
`tokio::select!` com `reqwest` observando o token é trabalho
da Etapa 5.x porque exige mudanças no adapter real (fora do
escopo deste `RunExecutor` puro).

### Etapa 5.x — cancel real do HTTP + recovery + transação + state granular + Passo 10

A Etapa 5.x é a "limpeza de débitos técnicos" do `RunExecutor` em
produção. Em 5 sub-etapas, ela fecha 5 frentes que estavam em
aberto desde as Etapas 4/4.x/4.x.y/5:

**5.x.1 — Cancel real do HTTP (full-stack).** O
`tokio::select!` com `cancel.cancelled()` no
`OpenAiCompatAdapter::stream` (e no `AnthropicAdapter::stream`)
**já existia** desde a Etapa 5 (foi adicionado junto com o
watchdog). O que faltava era um teste E2E que provasse que a
stack inteira é full-stack: usuário clica Parar →
`cancel_run` dispara o token → executor observa no `select!` →
adapter observa no `select!` → drop do `byte_stream` (= body da
`reqwest::Response`) → `reqwest` fecha a conexão TCP → server
detecta EOF → request abortada no upstream. O teste
`crates/provider-engine/tests/cancel_http.rs::cancel_drops_reqwest_connection`
sobe um `TcpListener` local, faz 1 request, lê 1 Delta, cancela
o token, e valida que o server detectou o EOF (read retornou 0)
em até 3s. **A Etapa 5.x.1 prova que o cancel É full-stack — não
era preciso mexer nos adapters.** Companion test
`stream_completes_normally_when_not_cancelled` é o sanity check
(sem cancel, o stream termina normalmente).

**5.x.4 + 5.x.3 — `RunState` granular a partir do journal +
`BEGIN IMMEDIATE`.** Até a Etapa 5, o `RunExecutor` só
atualizava `runs.status` (6 valores derivados, via view) — a
coluna `runs.state` (22 valores do
`frederico_agent_engine::RunState`) ficava em `created`/`running`
o tempo todo. A Etapa 5.x introduz um mapping `StreamEvent →
RunState` (no novo módulo `state_mapping` do `execution-engine`
— não no `agent-engine` pra evitar ciclo de dependência) e
chama esse mapping a cada evento do journal. Quando há mudança
de state, o executor persiste via
`RunRepo::set_state_and_heartbeat_tx` numa **única transação**
`BEGIN IMMEDIATE; ...; COMMIT;` que agrupa: `UPDATE runs SET
state = ?, last_heartbeat_at = ?, last_event_seq = ?`. O
`BEGIN IMMEDIATE` (em vez de `BEGIN DEFERRED` default) garante
que o write lock é pego imediatamente — se outra conexão já
tem, falha rápido (preferimos falhar visível a esperar
silenciosamente). Sem essa transação, um crash entre os 3
updates deixaria o journal inconsistente (state granular
defasado do `last_event_seq`). O `RunRepo` ganha 5 métodos
novos: `set_state` (legado), `set_state_and_heartbeat_tx`
(quente), `list_stale_heartbeats`, `mark_interrupted`, e
`force_heartbeat_at_for_test` (`#[doc(hidden)]`).

**5.x.2 — Recovery de crash no startup.** A Etapa 5.x assume
que o `RunExecutor` está vivo e mantém o `last_heartbeat_at`
fresco. Mas se a casca Tauri crashar no meio de um run (Tauri
crash, Windows reinicia, OOM, etc.), o run fica preso num
estado não-terminal com heartbeat velho. O novo módulo
`recovery::recover_stale_runs` lista runs não-terminais com
`last_heartbeat_at < datetime('now', '-N seconds')` e marca
cada um como `interrupted` (terminal — a view
`runs_with_status` mapeia `interrupted → timeout`). Threshold
default `DEFAULT_STALE_THRESHOLD_SECS = 120` (2 min — maior que
o `event_timeout` de 60s porque o executor pode estar esperando
o delta final do provider). A casca Tauri chama
`spawn_recover_stale_runs(db.clone(), Duration::from_secs(120))`
no `.setup`, em background (`tokio::spawn`) — não bloqueia o
startup. O helper recebe `Database` (não `RunRepo`) porque o
`RunRepo` tem borrow do `Database`, e a casca Tauri proíbe
`unsafe` (o `spawn` precisa de `'static`). O `RunRepo` é
construído dentro do closure da task.

**5.x.5 — Passo 10 de auditoria.** O spec
`tool-registry-specification.md` §7.7 lista 10 passos pra
validação de uma `tool_call`; os 9 primeiros estão na Etapa
2/3. O **Passo 10** é a entrada de auditoria — registrar toda
`tool_call` (aprovada, rejeitada) com timestamp, tool_id,
arguments e resultado. A Etapa 5.x introduz:

- Migration `0005_tool_audit.sql` com tabela
  `tool_audit(id, run_id, tool_id, tool_version, arguments_json,
  result_ok, result_json, duration_micros, created_at)` (FK pra
  `runs` com `ON DELETE CASCADE`, índices
  `idx_tool_audit_run_created` e `idx_tool_audit_tool_created`).
  **Append-only** — sem `UPDATE` nem `DELETE` (mitiga a ameaça
  R1 do `security-threat-model.md`).
- `ToolAuditRepo` no `storage` (`append` + `list_for_run`).
- Trait `AuditSink` no `tool-registry` (pura, sem dependência
  de storage). `NoopAuditSink` (default) + `RecordingAuditSink`
  (testes).
- `DbAuditSink` no `execution-engine::audit_sink` que
  implementa `AuditSink` via `ToolAuditRepo::append` (best
  effort — falha de I/O é logada via `tracing::warn!` mas não
  aborta a execução, conforme o spec do Passo 10).
- `RunExecutor::new` ganhou campo `audit_sink: Arc<dyn AuditSink>`
  + builder encadeável `RunExecutor::with_audit_sink(...)`. O
  `ChatOrchestrator::send_message` injeta
  `DbAuditSink::new(db.clone(), run_id)` ao montar o executor.
- `RunExecutor::handle_tool_call` chama `audit_sink.record(...)`
  em todos os caminhos: aprovado+executado (com `duration`
  medido via `Instant::now`), rejeitado pelo validador. O
  `result_json` é o JSON do output ou do erro estruturado. A
  UI da Fase 5/6 consome via `ToolAuditRepo::list_for_run`.

A Etapa 5.x é a primeira vez que `runs.state` (22 valores) é
populada em produção (a Etapa 4 só mexia em `runs.status`). A
view `runs_with_status` continua derivando `status` a partir de
`state` — o consumidor (Fase 2) não vê diferença.

## Próximas etapas

- **Etapa 4.1** (AnthropicAdapter tool calling + OpenAI-compat
  deltas de tool_call): tradução correta de `Role::Tool` em
  `{"type": "tool_result", ...}` no content_block Anthropic
  (hoje está em `"user"` placeholder); suporte a tool_call
  deltas em múltiplos chunks no parser SSE (o caso "tool_call
  completo em um chunk" é o implementado; deltas em múltiplos
  chunks ficam pra essa etapa).
- **Etapa 6** (UI de aprovação + E2E de produção): substitui o
  `ApprovalRequired` erro do executor por enfileiramento na
  fila de aprovação. Modal de aprovação consome o
  `ApprovalRequest` real (com `path`, `arguments`, `mandatory`)
  e o `PermissionSet` real do assistente/projeto. E2E em
  `tests/e2e/` com `files.read` de ponta a ponta (UI + Tauri
  + adapter real).
