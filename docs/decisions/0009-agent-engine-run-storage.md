# 0009 — `agent-engine` como crate novo, SQLite como fonte da verdade do estado de `Run`

## Contexto

A Fase 2 entregou um `Run` simples no `provider-engine` — um registro com 6 status (`created`, `running`, `completed`, `failed`, `cancelled`, `timeout`) e o `RunRegistry` em memória (mapa de `RunId` para `CancellationToken`). A Fase 3 precisa de uma máquina de estados com 22 estados explícitos ([`agent-state-machine.md`](../architecture/agent-state-machine.md), vindo do `PROMPT MESTRE` §6.1), checkpoints, transições válidas/inválidas e invariantes que são verificáveis em teste.

A pergunta que esse ADR responde é estrutural e tem três dimensões: **onde mora o código** (qual crate), **onde mora o estado em processo** (que estrutura de dados), e **onde mora o estado em disco** (qual coluna, qual tabela). Decidir as três coisas em conjunto evita que a Fase 4 (integração com `ChatOrchestrator`) tenha que reescrever a Etapa 1 porque a forma da persistência não casou com a forma da máquina.

### Onde mora o código

A Fase 2 introduziu dois crates do núcleo (`provider-engine`, `model-catalog`) e dois caminhos plausíveis para o novo subsistema:

- **(A)** Submódulo de `frederico-core`. Pró: tudo num lugar só, sem novo `Cargo.toml`, sem nova entrada em `scripts/check-core-purity.ps1`. Contra: `core` é o lugar de tipos compartilhados (erros, IDs opacos, `AppVersion`). Adicionar a ele um sistema de 23 estados com tabela de transições, eventos e `Budget` faz o `core` deixar de ser "tipos fundamentais" e virar "miolo + tipos fundamentais" — o que o `core` foi explicitamente projetado para não ser.
- **(B)** Submódulo de `frederico-provider-engine`. Pró: já tem `RunRegistry` e o `ChatOrchestrator` que vai consumir o motor. Contra: o `provider-engine` é **adapter + orquestrador de I/O de rede**, não motor de execução. A mistura dos dois quebra a separação que o `process-architecture.md` traçou e o `PROMPT MESTRE` §5.5 pediu. Pior: cria ciclo de dependência — o `provider-engine` já depende de `storage`, `model-catalog` e `security`; um motor dentro dele impede que o motor seja usado por um subagente que não fala com provedor.
- **(C)** Crate novo `frederico-agent-engine`. Pró: respeita a fronteira do `software-architecture.md` §"Crates previstos na fundação" (que já lista `agent-engine` como crate distinto). Pró: `provider-engine`, `execution-engine` (futuro) e o `subagent-engine` (Fase 6) podem depender dele sem ciclo. Contra: mais um `Cargo.toml`, mais uma entrada no workspace, mais uma seção em `docs/modules/`.

### Onde mora o estado em processo

A Fase 2 mantém o `RunRegistry` em memória como mapa `RunId → CancellationToken`. O estado do `Run` em si é reconstruído a partir do SQLite a cada `send_message` (via `RunRepo::get`). A Fase 3 precisa de um cache em memória com três usos:

- chamar `apply_transition` no caminho quente sem ir ao banco a cada evento;
- expor o estado atual pra UI sem `SELECT`;
- segurar o `last_event_seq` e o `Budget` atualizados sem `UPDATE` por delta.

A pergunta é: **o cache é autoridade ou é derivação?**

- **(A)** Cache como autoridade (escreve no cache, escreve no SQLite depois). Pró: caminho quente rápido. Contra: crash no meio do batch perde transições. A Etapa 5 (recovery) tem que reconciliar divergência, e divergência é bug esperando pra acontecer.
- **(B)** SQLite como autoridade, cache como derivação. Pró: o banco é a fonte de verdade; o cache é só uma otimização. Toda transição grava no SQLite **antes** de retornar. Contra: adiciona um `INSERT` por evento. Mitigável com `BEGIN IMMEDIATE; ...; COMMIT;` (default do sqlx em SQLite) — uma transação curta por evento custa microssegundos.

### Onde mora o estado em disco

A Fase 2 tem uma tabela `runs` com colunas `id`, `conversation_id`, `message_id`, `status` (6 valores, `CHECK` constraint), `started_at`, `finished_at`, `cancellation_requested_at`. A Fase 3 precisa estender isso. Duas opções:

- **(A)** Estender `runs` em **uma** tabela com colunas novas (`state`, `current_step`, `budget_json`, `allowed_tools_json`, `last_heartbeat_at`, `last_event_seq`, `provider_id`, `model_id`, `assistant_id`) e criar `checkpoints` separada. O `status` da Fase 2 é mantido como **coluna derivada** (mapeada por view ou trigger), para não quebrar o `ChatOrchestrator` que ainda lê `status` em `'running'`, `'completed'`, etc. Pró: uma linha por `Run`, fácil de consultar, replica o `Run` struct do spec quase 1:1. Contra: tabela larga, alguns campos opcionais na v1 (`assistant_id` pode ser `NULL`).
- **(B)** Tabela `runs` só com o mínimo de Fase 2, e uma tabela `run_state` 1:1 com as colunas de domínio da Fase 3. Pró: separação clara de Fase 2 vs Fase 3. Contra: dois `SELECT` por query, mais `JOIN`s, complica o `RunRepo` sem ganho real.

A migração também precisa decidir o que fazer com o `runs.status`. Três opções:

- **(A1)** Manter a coluna, atualizar a `CHECK` constraint pra aceitar o vocabulário dos 23 estados. A Fase 2 continua escrevendo `running`; a Fase 3 escreve o estado detalhado. Os dois coexistem, possivelmente divergem.
- **(A2)** Manter a coluna `status` como `CHECK` de 6 valores (Fase 2) e adicionar coluna `state` (23 valores, `CHECK` separado). Atualização: a `state` é a verdade; a `status` é projeção. Uma **view** `runs_with_status` mantém a coerência. Pró: a Fase 2 não muda; a Fase 3 tem o que precisa. Contra: dois lugares para manter sincronizados.
- **(A3)** Renomear `status` → `state` e atualizar todos os usos do `provider-engine` (que lê `status` em vários lugares). Pró: limpo. Contra: mudança breaking na Fase 2 fechada, regredir a suíte de recovery que testa o `status` final no banco (CHANGELOG da Fase 2 Hardening 5).

## Decisão

### 1. Crate novo `frederico-agent-engine`

A máquina de estados vai em crate novo `crates/agent-engine/`, **sem dependência de plataforma** (mesma regra de `frederico-core` e `frederico-storage` — `unsafe_code = "forbid"`, sem `tauri`/`windows`/`winapi`, verificado por `scripts/check-core-purity.ps1`). O `software-architecture.md` §"Crates previstos na fundação" já lista `agent-engine` como crate distinto; essa decisão concretiza o que o spec prometeu.

- `frederico-agent-engine` depende **apenas** de `frederico-core` na Etapa 1. Não depende de `frederico-storage`, nem de `frederico-provider-engine`, nem de `frederico-security`.
- `frederico-provider-engine` ganha, na Etapa 4, dependência de `frederico-agent-engine` (direção: provider → agent). Não o inverso.
- `frederico-agent-engine` exporta a enum `RunState` (22 variantes — ver nota sobre a divergência entre a contagem do spec e a lista abaixo), o `RunEvent` + `RunEventKind`, o `Budget`, a struct `Run` (tipo de domínio em memória), a tabela `TRANSITIONS` e a função `apply_transition`. A Etapa 4 adiciona o `RunExecutor` que conecta a máquina ao `storage::RunRepo` e ao `provider::ProviderAdapter`.

> **Nota sobre a contagem de estados.** O preâmbulo do spec `agent-state-machine.md` (estado `especificado`) dizia "23 estados" e listava 22. A Etapa 1 da Fase 3 reconciliou: a enum `RunState` tem 22 variantes e a `CHECK` constraint do SQLite usa 22 valores. A divergência virou nota no spec com a regra do `REGRAS §1.13` ("Alterado em relação ao plano original: motivo").
- O `docs/modules/agent-engine.md` (template do §1.4) é criado no mesmo commit.

### 2. SQLite como autoridade; cache em processo como derivação

A fonte da verdade do estado de cada `Run` é o SQLite. Toda transição gravada passa por:

```text
BEGIN IMMEDIATE;
  apply_transition_in_memory(run) -> new_state
  INSERT INTO message_events (...);        -- journal
  UPDATE runs SET state = ?, last_heartbeat_at = ?, last_event_seq = ?, ...;
  -- se a transição for "antes de X" (spec §6.2), INSERT INTO checkpoints (...);
COMMIT;
```

A função `apply_transition` da `agent-engine` é **pura** — recebe o estado atual, o evento, e devolve o novo estado (ou erro). Quem monta a transação SQL é o `RunExecutor` (Etapa 4), não a Etapa 1. A regra "transição gravada antes de retornar" mora no executor; a Etapa 1 garante que a função pura é **testável sem SQLite** (cobertura por par 100% em memória).

A Etapa 5 (watchdog) introduz o cache em processo (`RunCache`) que consulta o SQLite na inicialização (recovery) e mantém o estado em memória para o caminho quente. O cache é **derivação** — divergência entre cache e SQLite é resolvida lendo o SQLite. Erro de leitura faz o run voltar pra `interrupted` (específica, não `failed`).

### 3. Estender `runs` com colunas novas; `status` da Fase 2 vira coluna derivada por view

A migração `0003_runs_and_checkpoints.sql` faz:

- **Estende** a tabela `runs` com colunas novas:
  - `state TEXT NOT NULL DEFAULT 'created' CHECK(state IN ('created', 'queued', 'preparing_context', 'retrieving_memory', 'validating_capabilities', 'calling_model', 'streaming', 'waiting_tool_call', 'validating_tool_call', 'waiting_user_approval', 'executing_tool', 'validating_tool_result', 'continuing_model', 'generating_artifact', 'validating_artifact', 'checkpointing', 'retrying', 'paused', 'completed', 'failed', 'cancelled', 'interrupted'))` (22 valores, conforme reconciliação feita na Etapa 1)
  - `current_step INTEGER NOT NULL DEFAULT 0`
  - `budget_json TEXT NOT NULL DEFAULT '{}'` — `Budget` serializado
  - `allowed_tools_json TEXT NOT NULL DEFAULT '[]'` — `Vec<ToolId>` serializado (Etapa 2; `ToolId` é o nome da ferramenta — só string)
  - `last_heartbeat_at TEXT NOT NULL DEFAULT (datetime('now'))`
  - `last_event_seq INTEGER NOT NULL DEFAULT 0`
  - `provider_id TEXT NOT NULL DEFAULT ''`
  - `model_id TEXT NOT NULL DEFAULT ''`
  - `assistant_id TEXT` (nullable na v1; populate na Etapa 6 quando o conceito de "assistente" entrar no fluxo)
- **Cria** a tabela `checkpoints`:
  - `id TEXT PRIMARY KEY`
  - `run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE`
  - `seq INTEGER NOT NULL`
  - `state TEXT NOT NULL` — espelha o `runs.state` no momento do checkpoint
  - `payload_json TEXT NOT NULL` — estado serializado do executor no momento (campos suficientes pra retomar; formato definido na Etapa 4)
  - `created_at TEXT NOT NULL DEFAULT (datetime('now'))`
  - `UNIQUE(run_id, seq)`
  - índice em `(run_id, seq DESC)` pra "último checkpoint do run"
- **Cria** uma view `runs_with_status` que devolve `runs.*` + uma coluna `status` derivada do `state` (vide tabela de mapeamento abaixo). O `RunRepo` da Fase 2 lê dessa view — uma única mudança no `SELECT` mantém o `ChatOrchestrator` funcionando sem mexer no código de aplicação.
- **Mapeamento** `state → status` (codificado na view via `CASE WHEN`):
  - `created`, `queued` → `created`
  - 16 estados não-terminais de execução (de `preparing_context` a `retrying`) → `running`
  - `paused` → `running` (ainda "vivo"; refinamento na Etapa 5 se a UI precisar de `paused` explícito)
  - `completed` → `completed`
  - `failed` → `failed`
  - `cancelled` → `cancelled`
  - `interrupted` → `timeout` (Etapa 5 pode refinar com coluna `interrupted_reason` separada)

A escolha de manter `status` derivado (em vez de renomear ou dropar) é deliberada: a Fase 2 fechou com testes E2E que verificam o `status` final persistido (CHANGELOG Hardening 5: "`error_to_view` com tabela PT-BR... status final persistido no db bate com o último status emitido pelo sink"). Renomear `status` → `state` regredir a suíte. A view dá a Fase 3 o vocabulário dos 23 estados sem quebrar a Fase 2.

### 4. O `Run` struct vive em `frederico-agent-engine`; o `storage::Run` é o registro persistido

A `Run` struct que aparece no spec `agent-state-machine.md` §"Contratos" é o **tipo de domínio em memória**. Ela vive em `frederico-agent-engine::Run`. O `frederico_storage::Run` é o **registro persistido** (linha da tabela `runs`), com `state: String` em vez de `state: RunState` — pra não criar ciclo de dependência (`agent-engine` não pode depender de `storage` se quiser ser puro).

A Etapa 4 (integração) introduz a conversão `storage::Run ↔ agent_engine::Run` num módulo pequeno (`agent-engine::persistence`), único lugar que conhece os dois lados. `FromStr` / `Display` da enum `RunState` ficam em `agent-engine` e são a única ponte.

## Alternativas descartadas

- **Submódulo de `frederico-core`.** Descartada: o `core` é o lugar de tipos compartilhados (erros, IDs opacos, `AppVersion`). Adicionar a ele um sistema de 23 estados com tabela de transições, eventos e `Budget` faz o `core` deixar de ser "tipos fundamentais".
- **Submódulo de `frederico-provider-engine`.** Descartada: o `provider-engine` é adapter + orquestrador de I/O de rede, não motor. A mistura quebra a separação `process-architecture.md` / `PROMPT MESTRE` §5.5. Cria ciclo de dependência que impede o motor de ser usado por um subagente (Fase 6) que não fala com provedor.
- **Cache em processo como autoridade.** Descartada: crash no meio do batch perde transições. A Etapa 5 (recovery) teria que reconciliar divergência, e divergência entre dois lugares que ambos se dizem "verdade" é bug esperando pra acontecer.
- **Tabela `run_state` 1:1 separada.** Descartada: dois `SELECT` por query, mais `JOIN`s, complica o `RunRepo` sem ganho real. O `Run` do spec é 1:1 com a linha de `runs`.
- **Renomear `status` → `state` e atualizar todos os usos.** Descartada: mudança breaking na Fase 2 fechada; regredir a suíte de recovery E2E (Hardening 5) que verifica o `status` final persistido. View é mais barata e mantém a Fase 2 intocada.

## Consequências

**Mais fácil:**

- A função `apply_transition` é pura (em memória) e 100% testável sem SQLite. Suíte de testes por par roda em <1s e cobre 23 estados × todas as transições válidas/inválidas + 4 estados terminais imutáveis.
- A Etapa 4 (integração) monta a transação SQL uma vez (`BEGIN IMMEDIATE; ... COMMIT;`) e a Suíte continua crescendo sem mexer na máquina.
- A Fase 2 (`ChatOrchestrator` lendo `runs.status` via `RunRepo::get`) continua funcionando sem mudança de código. A view esconde a diferença.
- Adicionar a Etapa 5 (watchdog) é incremental: o `RunCache` lê a view, e o `Watchdog` consulta `last_heartbeat_at` que já existe.
- O `provider-engine` pode ser refatorado em fases (a Etapa 4 começa pelo caminho "tool_call ausente" e migra depois), porque `Run` na Fase 2 é independente da enum de 23 estados.

**Mais difícil:**

- A coluna `runs` ficou larga (15+ colunas). Mitigação: a maioria é `TEXT NOT NULL DEFAULT ''` ou `INTEGER NOT NULL DEFAULT 0`; a `state` é a única com `CHECK` de 23 valores. O `SELECT * FROM runs` continua barato em SQLite (B-tree de chave primária).
- A divergência eventual entre `state` e `status` (a Etapa 3 escreve um, a Fase 2 escreve o outro em momentos diferentes) pode aparecer em janelas curtas. Mitigação: a view é a única leitura; testes de recovery E2E da Fase 2 continuam batendo porque leem `status` e o `status` ainda é escrito pela Fase 2 nos mesmos pontos.
- A Etapa 4 vai ter que decidir se a Fase 2 ainda escreve `status` (e `state` é secundário) ou se a Fase 3 assume e a Fase 2 só lê da view. Decisão da Etapa 4; o ADR-0009 não apressa.
- O `storage::Run` ganhou `state: String` em vez de `state: RunState`. A conversão `FromStr`/`Display` na `agent-engine` é manual, e a Etapa 4 tem que escrever um teste pra cada variante (23 testes). Aceitável.
- A documentação do `Run` no `agent-state-machine.md` (spec) e o tipo `agent_engine::Run` (código) ficam próximos mas não idênticos. A seção "Contratos" do spec é **fonte de verdade do design**; a struct Rust é o reflexo atual. Divergências viram PR com `// Alterado em relação ao plano original: motivo` (REGRAS §1.13).
