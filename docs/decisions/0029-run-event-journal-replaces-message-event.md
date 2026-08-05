# 0029 — `RunEvent` (com `seq` monotonicamente crescente) substitui `MessageEvent` como journal de transições do `Run` (Etapa 2 da Fase 6)

## Contexto

O ADR-0025 §Fato (auditoria de 2026-08-04) documentou o seguinte:

> 6. **O journal de eventos do spec (`agent-state-machine.md §147` "Cada `Run` tem um `RunEvent` por transição, com `seq` monotonicamente crescente por `run_id`") não existe no produto.** O que existe é `MessageEvent` no SQLite (Fase 3 Etapa 4.x, migração `0004_tool_result_kind.sql`), que é **outra** entidade — registra eventos do stream, não transições da máquina.

A `MessageEvent` foi introduzida na Fase 3 Etapa 4.x para registrar `kind = 'tool_result'` no stream (resposta do tool ao modelo). É uma entidade útil e que deve continuar existindo — ela é parte do **stream** que o modelo lê, não do **journal** de transições da máquina de estados.

O que falta é o `RunEvent` com `seq` monotonicamente crescente por `run_id`, que é o que `agent-state-machine.md §147` prometeu. Sem ele:

1. **A máquina de estados não tem journal próprio.** O `state_mapping.rs` mapeia `StreamEvent → RunState` por match puro, e o `RunRepo::set_state` grava direto. Se uma transição for inválida pela tabela `TRANSITIONS` (ex.: `Failed → Completed` direto), ninguém sabe — a transição inválida é gravada sem ser questionada.
2. **Retomada de run interrompido é heurística.** Quando o app reabre, o `recovery.rs` carrega o último `MessageEvent` e tenta adivinhar o estado. Sem `RunEvent`, não há fonte de verdade: o "último estado válido" é inferido do stream, não registrado.
3. **Debugging de run é por log, não por tabela.** Investigar "por que esse run ficou em `WaitingToolCall` por 30 minutos" exige ler o stream e inferir; com `RunEvent`, é uma query SQL simples (`SELECT * FROM run_events WHERE run_id = ? ORDER BY seq`).
4. **Auditoria de quem mudou o quê, quando, é impossível.** Sem journal de transições, o caminho de "Run foi de `Running` para `Failed` em t0" não tem rastro.

A Etapa 1 da Fase 3 prometeu (ADR-0009 §D1): "transição gravada antes de retornar". A Etapa 4 da Fase 3 não entregou. A pendência ficou nomeada no ADR-0025 §D3, com trabalho designado para a Fase 6.

A Fase 6 é o lugar natural: a Etapa 2 (portão único) é a única infraestrutura que faz o `RunEvent` ser **emitido consistentemente** (toda transição passa pelo `RunExecutor`).

## Decisões

### D1 — `RunExecutor` é o portão único de mudança de estado

`crates/execution-engine/src/state_mapping.rs` é reescrito:

- **Antes** (`301a222`): `pub fn run_state_for_event(event: &StreamEvent) -> Option<RunState> { match event { ... } }` — match puro, sem consultar `TRANSITIONS`, sem chamar `apply_transition`.
- **Depois** (Etapa 2 da Fase 6): `pub fn run_state_for_event(current: RunState, event: &StreamEvent) -> Result<Option<RunState>, TransitionError> { ... }` — recebe o estado atual, consulta `apply_transition(current, event)` da `agent-engine`, e:
  - Se `apply_transition` retorna `Ok(new_state)`: retorna `Ok(Some(new_state))`.
  - Se `apply_transition` retorna `Err(InvalidTransition)`: retorna `Err(TransitionError::Invalid { from: current, event: event.clone() })`.
  - Se o evento não dispara transição (ex.: `Delta` em estado terminal): retorna `Ok(None)` (sem mudança).

O `RunExecutor` consome `Result<Option<RunState>, TransitionError>`:
- `Ok(Some(new_state))` → grava `RunEvent { seq, from: current, to: new_state, ... }` no journal, chama `RunRepo::set_state(run_id, new_state)`.
- `Ok(None)` → no-op (evento sem mudança de estado).
- `Err(TransitionError::Invalid)` → grava `RunEvent { kind: RejectedInvalid, ... }` no journal, **não** chama `RunRepo::set_state`, retorna erro pro `RunState::Failed` (transição válida `Running → Failed` via `apply_transition`, registrada).

A regra do ADR-0025 §D3 fica implementada: "transição inválida é rejeitada com erro, não gravada direto".

### D2 — `RunEvent` com `seq` monotonicamente crescente por `run_id`

Nova struct em `crates/agent-engine/src/event.rs`:

```rust
pub struct RunEvent {
    pub event_id: Uuid,
    pub run_id: RunId,
    pub seq: u64,                  // monotonicamente crescente por run_id
    pub kind: RunEventKind,        // Started, StreamDelta, ToolCallRequested, ToolResultReceived, Completed, Failed, Cancelled, RejectedInvalid, ...
    pub from_state: Option<RunState>,
    pub to_state: Option<RunState>,
    pub timestamp_ms: i64,
    pub payload: Option<Value>,    // JSON arbitrário por kind (custo, tool_call_id, error_message, etc.)
}
```

`seq` é atribuído pelo `RunRepo::next_seq(run_id) -> u64` dentro da **mesma transação SQLite** que grava o evento. Atomicidade garante: duas threads concurrentes não pegam o mesmo `seq`. (O `RunExecutor` é single-thread por run, mas a garantia é no banco pra permitir leituras concorrentes.)

E2E em `crates/e2e/tests/e2e_portao_transicao_e2e.rs::run_event_seq_monotonic_through_orchestrator`:
- Lança 3 transições válidas em sequência (Running → Streaming → WaitingToolCall → Completed).
- Assert: `SELECT seq FROM run_events WHERE run_id = ? ORDER BY seq` retorna `[1, 2, 3]`, sem buracos, sem duplicatas.
- Tudo via `build_chat_orchestrator` — não testa `agent-engine` isolado.

### D3 — `MessageEvent` continua existindo, com papel explícito

A `MessageEvent` (Fase 3 Etapa 4.x, `kind = 'tool_result'`, etc.) **não é removida**. Ela registra eventos do **stream** que o modelo lê:

- `kind = 'delta'` — chunk de texto do modelo.
- `kind = 'tool_result'` — resposta do tool ao modelo.
- `kind = 'usage'` — tokens consumidos no chunk.

São eventos do **stream de mensagens**, não transições da **máquina de estados**. A distinção é:

- `RunEvent` é o journal da **máquina** (RunState). Pergunta que responde: "em que estado o run está, e como chegou aqui?"
- `MessageEvent` é o journal do **stream**. Pergunta que responde: "que conteúdo o modelo viu, em que ordem?"

A UI do Modo Equipe lê **as duas** para renderizar: `RunEvent` para a linha do tempo de estados, `MessageEvent` para o conteúdo de cada estado.

E2E em `::valid_transition_persists_in_run_event_journal`:
- Provoca 1 transição válida (Running → Streaming).
- Assert: 1 linha em `run_events` (com `seq=1`, `from=Running`, `to=Streaming`).
- Assert: linhas em `message_events` (deltas) **não contêm** `from_state`/`to_state` — são eventos de stream, não de transição.
- As duas tabelas coexistem, com papéis distintos.

### D4 — Migração `0027_run_events.sql` no `frederico-storage`

Nova migração (Etapa 2):

```sql
CREATE TABLE run_events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    from_state TEXT,
    to_state TEXT,
    timestamp_ms INTEGER NOT NULL,
    payload_json TEXT,
    UNIQUE (run_id, seq)
);
CREATE INDEX idx_run_events_run_id_seq ON run_events (run_id, seq);
CREATE INDEX idx_run_events_kind ON run_events (kind);
```

A constraint `UNIQUE (run_id, seq)` é a garantia mecânica: se duas threads tentarem o mesmo `seq`, o `INSERT` falha e o `RunRepo` retorna `Err(SeqConflict)` (que é uma situação anormal — o `RunExecutor` é single-thread por run).

A migração roda na Etapa 2. Suíte existente do `frederico-storage` continua verde (a nova tabela é aditiva).

### D5 — `recovery.rs` lê `RunEvent`, não `MessageEvent`

`crates/execution-engine/src/recovery.rs` (reescrito na Etapa 2):

- **Antes** (`301a222`): carrega o último `MessageEvent` e tenta adivinhar o estado (heurística, sem fonte de verdade).
- **Depois** (Etapa 2): carrega o último `RunEvent` com `to_state IS NOT NULL ORDER BY seq DESC LIMIT 1` e usa `to_state` como fonte de verdade. Se não há `RunEvent` (run criado antes da migração `0027`), fallback pro último `MessageEvent` com log warning.

A heurística do `MessageEvent` vira **caminho de migração**, não caminho primário. Runs novos sempre têm `RunEvent` desde o `Started`. Runs antigos (criados antes da Etapa 2) usam `MessageEvent` **uma vez** (na primeira retomada), e a partir daí a `RunEvent` é a fonte.

E2E em `::recovery_loads_state_from_run_event_journal` (criado na Etapa 2): lança run, mata o `ChatOrchestrator` no meio de um `Streaming`, recria via `build_chat_orchestrator`, afirma que o run retoma do estado `Streaming` lendo o `RunEvent` (não o `MessageEvent`).

### D6 — `MessageEvent` ganha coluna `seq` (opcional, pra coexistência)

A `MessageEvent` ganha coluna `seq` opcional (`BIGINT NULL`). Quando `RunEvent` é gravado com `seq=N`, o `MessageEvent` do mesmo step recebe `seq=N` retroativamente. Isso permite que a UI do Modo Equipe faça **join temporal** entre as duas tabelas:

```sql
SELECT re.kind, re.from_state, re.to_state, me.kind, me.payload_json
FROM run_events re
LEFT JOIN message_events me ON me.run_id = re.run_id AND me.seq = re.seq
WHERE re.run_id = ?
ORDER BY re.seq;
```

Sem o `seq` na `MessageEvent`, o join é por timestamp (impreciso — múltiplos eventos no mesmo ms). Com o `seq`, é exato.

A coluna `seq` é **nullable** pra não quebrar runs antigos (criados antes da migração `0027`). A UI trata `NULL` como "evento de stream sem transição associada" (ex.: `Delta` em `Streaming` que não muda estado).

Migração: `0027_run_events.sql` adiciona a coluna `seq` na `message_events` **e** o índice `CREATE INDEX idx_message_events_run_id_seq ON message_events (run_id, seq)`. Bump atômico: as duas mudanças no mesmo commit.

## Consequências

- `crates/agent-engine/src/event.rs` ganha `RunEvent` + `RunEventKind` (25 variantes, já definidas — o que faltava era o consumer). Bump atômico.
- `crates/execution-engine/src/state_mapping.rs` é reescrito: recebe `current: RunState`, consulta `apply_transition`, retorna `Result<Option<RunState>, TransitionError>`. Bump atômico com a migração `0027`.
- `crates/execution-engine/src/recovery.rs` lê `RunEvent` como fonte primária, `MessageEvent` como fallback de migração. Bump atômico.
- `crates/storage/migrations/0027_run_events.sql`: tabela nova + coluna `seq` na `message_events` + 2 índices.
- `crates/storage/src/run_event_repo.rs` (novo): `insert(event: RunEvent) -> Result<(), RepoError>`, `next_seq(run_id: RunId) -> Result<u64, RepoError>`, `list_by_run(run_id: RunId) -> Result<Vec<RunEvent>, RepoError>`. 100% coberto por testes.
- `docs/modules/agent-engine.md §"O que este módulo NÃO garante"` (criado no PR #26) é **atualizado**: `apply_transition` agora é exercitada no caminho de produção. A frase muda para "este módulo garante: a função pura `apply_transition` é consultada em toda transição de `RunState` via `RunExecutor::state_mapping`. O que ainda não é exercitado: ...". Carimbo de verificação bumped.
- `docs/modules/execution-engine.md §"SubagentRunner"` (criado no ADR-0027) é complementado com "state portão" + bump no carimbo.
- E2E em `crates/e2e/tests/e2e_portao_transicao_e2e.rs` (Etapa 2): 4 testes, todos consumindo `build_chat_orchestrator`:
  - `run_executor_rejects_invalid_transition_through_orchestrator`
  - `run_event_seq_monotonic_through_orchestrator`
  - `valid_transition_persists_in_run_event_journal`
  - `recovery_loads_state_from_run_event_journal`
- O `cargo test --workspace` deve continuar verde. A adição é mecânica; nenhuma mudança de comportamento quebra testes existentes.
- A `transition_journal_e2e` do `crates/agent-engine/tests/` (rascunho antigo, se existir) é **removida** — o teste do invariante de transição agora é E2E em `crates/e2e/tests/`, conforme a regra do gate (caminho de produção, não de crate).

## Alternativas consideradas

1. **Manter `MessageEvent` como journal e adicionar `from_state`/`to_state` nela.** Rejeitado porque (a) `MessageEvent` tem papel distinto (stream, não máquina), misturar os dois é o bug que o ADR-0025 §Fato documentou, (b) a UI do Modo Equipe precisa de duas perspectivas (linha do tempo de estados + conteúdo de mensagens), forçar uma tabela a servir as duas é degradação.
2. **`RunEvent` sem `seq` monotonicamente crescente** (auto-increment do SQLite). Rejeitado porque (a) o spec `agent-state-machine.md §147` promete `seq` explícito por `run_id`, (b) `seq` por `run_id` (não por tabela) é o que permite o join com `MessageEvent` da D6, (c) `AUTOINCREMENT` global atrapalha o join.
3. **Implementar portão único sem journal** (transição rejeitada, mas sem `RunEvent` gravando a rejeição). Rejeitado porque (a) o journal da rejeição é o que permite auditar "tentativa de transição inválida", (b) sem journal de rejeição, debugging de "por que esse run falhou" perde informação.
4. **Manter o `state_mapping.rs` como match puro e adicionar o portão em outra camada** (ex.: wrapper no `RunRepo::set_state`). Rejeitado porque (a) o portão fica escondido no storage, longe do `agent-engine` que é a fonte de verdade da máquina, (b) viola a regra "fonte única de verdade" do ADR-0025 D1, (c) a Etapa 4 (subagente) precisa do portão **no `RunExecutor`**, não no storage — porque subagente transiciona pelo mesmo portão.
5. **Recriar `RunEvent` do zero em vez de reusar o tipo existente no `agent-engine`.** Rejeitado porque (a) `RunEvent` e `RunEventKind` já existem no `agent-engine` (criados na Fase 3 Etapa 1, ADR-0009), zero-chamada fora do crate, (b) recriar é trabalho que joga fora 25 variantes testadas.

## Pendências

- **Migração de runs antigos** (criados antes da Etapa 2): o `recovery.rs` lê `MessageEvent` como fallback, e na primeira retomada grava o `RunEvent` equivalente. Suíte de migração roda em `crates/storage/tests/migration_0027.rs` (cobre 3 cenários: run sem `RunEvent`, run com `RunEvent` parcial, run com `RunEvent` completo).
- **Visualização do `RunEvent` na UI do Modo Equipe** (Etapa 6 da Fase 6): linha do tempo de estados, custo por transição, latência por estado. Sem isso, o journal é técnico, não visível pro usuário. O desenho visual fica pra Etapa 6.
- **Política de retenção do `RunEvent`**: quanto tempo guardar. Default proposto: 90 dias. Decisão de produto, não desta ADR. Pendência nomeada.
- **Exportação do `RunEvent` para debug**: comando `cargo run --bin dump-run-events <run_id>` que imprime o journal em formato legível. Útil pra suporte, não crítico. Pendência nomeada.

## Histórico de revisão

- 2026-08-05 — versão inicial. Decisão da Etapa 1 da Fase 6. Validação pelo user: a cobertura E2E do portão tem que ser em `crates/e2e/tests/` (não em `crates/agent-engine/tests/`), porque os 46 testes do `agent-engine` provavam a máquina enquanto o caminho real a ignorava. O mesmo erro não pode se repetir.
