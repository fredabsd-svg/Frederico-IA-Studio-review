<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 3 (Etapa 1)
-->

# `frederico-agent-engine`

Máquina de estados do `Run`, eventos, transições e `Budget`. Crate do
núcleo (sem dependência de plataforma) entregue na Etapa 1 da Fase 3.

## 1. O que este módulo faz

Codifica a máquina de estados de 22 estados do `Run` (vinda do
`PROMPT MESTRE` §6.1 e especificada em
[`docs/architecture/agent-state-machine.md`](../architecture/agent-state-machine.md))
como tipos Rust puros, em memória, e a função
[`apply_transition`](../../crates/agent-engine/src/transition.rs) que
decide o próximo estado dado o atual e o evento. A invariante
**"transições inválidas são erro estruturado"** (mesma do spec) é
garantida: [`apply_transition`](../../crates/agent-engine/src/transition.rs)
rejeita antes mesmo de consultar a tabela se o estado de origem é
terminal, e devolve `TransitionError::InvalidTransition` se o par
(`from`, `event`) não tem aresta.

A Etapa 1 entrega **a máquina pura**. O executor que conecta a máquina
ao SQLite e ao `provider-engine` (a transação `BEGIN IMMEDIATE; ...;
COMMIT;` que grava o journal e o checkpoint antes de retornar) entra
na Etapa 4.

## 2. O que ele expõe

- `RunState` — enum com 22 variantes; `is_terminal()`, `as_str()` (snake_case),
  `Display`, `FromStr`, `all()` (lista canônica), `COUNT`.
- `RunEventKind` — enum com 25 variantes (20 estruturais + 5 globais);
  `is_global()`, `as_str()` (snake_case), `Display`, `FromStr`.
- `RunEvent` — struct (`run_id`, `seq`, `ts`, `kind`, `payload: serde_json::Value`).
- `Transition` — struct (`from`, `event`, `to`) + tabela constante
  `TRANSITIONS` (21 arestas estruturais) e `GLOBAL_TRANSITIONS` (5
  arestas globais).
- `apply_transition(from: RunState, event: RunEventKind) -> Result<RunState, TransitionError>`
  — função pura, determinística.
- `TransitionError` — `FromTerminal { from }` ou `InvalidTransition { from, event, to }`.
- `RunStateParseError`, `RunEventKindParseError` — erros de parsing.
- `Budget` — struct (`max_steps`, `max_tokens_in`, `max_tokens_out`,
  `max_cost_microcents`, `max_wall_clock: Duration`) com `Default` razoável
  (50 steps, 200k tokens entrada, 16k saída, $5, 10 min).
- `Run` — struct em memória com os 13 campos do spec §"Contratos":
  `id`, `conversation_id`, `project_id`, `assistant_id: Option`,
  `provider_id`, `model_id`, `started_at`, `state`, `current_step`,
  `budget`, `allowed_tools: Vec<ToolId>`, `last_heartbeat_at`,
  `last_event_seq`. Métodos: `new`, `is_terminal`, `next_event_seq`,
  `heartbeat`, `transition(event) -> Result<(), TransitionError>`.

## 3. De quem depende e quem depende dele

**Depende de:**

- `frederico-core` — `RunId`, `ConversationId`, `ProjectId`,
  `AssistantId` (criado na Etapa 1), `CheckpointId`, `ProviderId`,
  `ModelId`, `ToolId` (criado na Etapa 1), `MessageId`, `ArtifactId`.
- `serde` / `serde_json` — serialização dos eventos e do `Run` para o
  journal e o `budget_json` / `allowed_tools_json` no SQLite.
- `chrono` — `DateTime<Utc>` para `started_at`, `last_heartbeat_at`,
  `RunEvent.ts`.
- `thiserror` — `derive(Error)` nos tipos de erro.

**Quem depende dele (hoje):**

- Ninguém ainda. A Etapa 4 (integração) vai fazer
  `frederico-provider-engine` depender dele para o `RunExecutor`. A
  Etapa 2 (tool-registry) e a Etapa 3 (permissões) podem consultá-lo
  para validar transições fora do executor (ex.: `apply_transition`
  num preview da UI).

**Quem vai depender dele (próximas etapas):**

- `frederico-execution-engine` (sugerido pelo spec
  `software-architecture.md` §"Crates previstos na fundação") — o
  coordenador entre motor, tools e persistência. Pode ser módulo
  dentro do `frederico-agent-engine` na Etapa 4, ou crate separado
  se crescer.
- `frederico-subagent-engine` (Fase 6) — o motor de subagentes
  precisa de uma máquina de estados; reusar a do `agent-engine` é o
  caminho natural.

## 4. Decisões não óbvias e armadilhas conhecidas

- **22 estados, não 23.** O spec original (`agent-state-machine.md`,
  estado `especificado` até a Etapa 1) dizia "23 estados" no
  preâmbulo mas listava 22. A Etapa 1 reconciliou: a enum tem 22
  variantes, o `COUNT` é 22, a `CHECK` constraint do SQLite e a
  view `runs_with_status` usam 22 valores. A divergência virou nota
  no spec e no ADR-0009 com a regra do `REGRAS §1.13`. Se a contagem
  mudar no futuro, **atualizar o spec, o ADR, a `CHECK` constraint e
  o `COUNT` no mesmo commit**.
- **Eventos globais vs. estruturais.** A tabela `TRANSITIONS` tem
  arestas que casam um par (`from`, `event`); a
  `GLOBAL_TRANSITIONS` lista os 5 eventos que partem de qualquer
  estado não-terminal. A `apply_transition` consulta as duas nessa
  ordem, com a barreira de "estado terminal" antes. Adicionar uma
  nova aresta global exige entrar nas duas tabelas (e a
  `is_global()` do `RunEventKind`) — o teste
  `global_transitions_only_list_global_events` em
  `transition.rs` falha se as duas listas divergirem.
- **Transições implícitas.** Três arestas da tabela `TRANSITIONS`
  não estão literalmente no spec §"Tabela de transições" mas são
  necessárias pra fechar o grafo: `checkpointing → completed via
  CheckpointPersisted`, `retrying → calling_model via NextIteration`,
  `paused → preparing_context via Resume`. O comentário no início de
  `TRANSITIONS` documenta essas três. Se a Etapa 4 descobrir que o
  executor real precisa de uma aresta diferente (ex.: `retrying →
  validating_capabilities`), o desvio vira PR com a regra
  `REGRAS §1.3` (atualizar spec e código no mesmo commit).
- **`apply_transition` é pura.** Não toca SQLite, não dispara side
  effects. A regra "transição gravada antes de retornar" (regra do
  "Journal de eventos" do spec `chat-and-providers.md`) mora no
  executor da Etapa 4, não aqui. Isso permite que a suíte de testes
  por par cubra 23 estados × todas as combinações em <10ms sem I/O.
- **Mensagens de erro incluem o que o caller pediu, não o que
  deveria ter pedido.** `TransitionError::InvalidTransition` tem
  `to: Option<RunState>` — `None` se o caller só checou
  (`from`, `event`), `Some(t)` se o caller também tinha um `t` em
  mente. A assinatura pública da função é `apply_transition(from,
  event) -> Result<RunState, _>`, então `to` é sempre `None` no uso
  público. O campo existe para o `Run::transition` (que conhece
  ambos) anexar informação de diagnóstico.
- **`Run::transition` é separado de `apply_transition`.** O
  `Run::transition` encapsula a chamada (atualiza o `state` em
  sucesso, deixa intacto em erro). Não toca `seq`, `heartbeat` ou
  `step` — isso é responsabilidade do executor (Etapa 4).
- **`ToolId` foi criado em `frederico-core` (não em
  `frederico-agent-engine`).** O `Run::allowed_tools: Vec<ToolId>`
  precisa do tipo, e `ToolId` é um ID opaco de domínio (igual a
  `ProviderId` e `ModelId`) que vai ser usado em vários lugares
  (tool-registry, permissões, executor). Criar em `core` evita
  ciclo de dependência futuro.

## 5. Como testá-lo isoladamente

```pwsh
# Suíte do crate (unit + 0 integration; cobre par a par)
cargo test -p frederico-agent-engine

# Suíte de storage (valida a migração 0003 e a view `runs_with_status`)
cargo test -p frederico-storage

# Verificação completa
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
```

Cobertura por par atual (em `crates/agent-engine/src/transition.rs`):

- 4 estados terminais × 5 eventos globais = 20 casos (todos rejeitados
  como `FromTerminal`).
- 18 estados não-terminais × 5 eventos globais = 90 casos (todos
  levam ao destino global correto: `cancelled`, `interrupted`,
  `failed`, `paused`).
- Cada aresta estrutural listada tem um teste explícito (21 testes).
- 4 testes de par para arestas estruturais específicas
  (streaming bifurca, calling_model não-streaming, retrying volta,
  paused resume).
- 1 teste de invariante da tabela (sem pares duplicados).
- 2 testes de invariante da tabela global (só terminais, só eventos
  globais).

## 6. O que ele **não** faz

- **Não executa transições no banco.** A função `apply_transition` é
  pura. Quem grava é o `RunExecutor` da Etapa 4. Não há `INSERT` nem
  `UPDATE` no SQLite partindo do `agent-engine` (e nem dependência
  de `frederico-storage`).
- **Não conhece provedores, ferramentas, permissões ou workers.** O
  `Run` carrega `Vec<ToolId>`, mas o cálculo da interseção
  (`effective_tools`) é da Etapa 2 (tool-registry). A validação de
  permissões é da Etapa 3. A invocação da ferramenta em si é do
  executor (Etapa 4) ou do `tool-registry` (Etapa 2).
- **Não tem watchdog, `CancellationToken` ou recovery.** Esses
  entram na Etapa 5. O `agent-engine` apenas modela os estados
  `paused`, `cancelled` e `interrupted` — quem dispara a transição
  é código fora daqui.
- **Não tem `set_state`, `bump_step` ou `append_event` no
  `Run`.** A Etapa 4 (integração) vai criar o `RunExecutor` que
  monta a transação SQL `BEGIN IMMEDIATE; ...; COMMIT;` e usa o
  `Run` apenas como tipo de domínio em memória. Manter o `Run` sem
  métodos de mutação que tocam SQLite é o que garante que a
  `apply_transition` continua pura e 100% testável sem I/O.
- **Não conhece Windows, Tauri, paths do sistema, env vars.** Mesma
  regra do `core` (verificado por `scripts/check-core-purity.ps1`).
  A integração com a casca Tauri (comandos IPC, eventos de UI)
  entra na Etapa 6.
- **Não tem `Default` para `RunState`.** Estados não têm um "valor
  neutro" — `Created` é a única entrada válida, e o caller
  constrói `Run::new(...)` em vez de `Run::default()`. Estados
  terminais são explicitamente imutáveis (a `apply_transition`
  rejeita qualquer evento a partir deles).
