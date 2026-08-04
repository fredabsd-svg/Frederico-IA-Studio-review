# 0025 — Destino do `frederico-agent-engine` (Etapa 4 da Fase de Ligação)

## Contexto

O `frederico-agent-engine` foi criado na Fase 3 Etapa 1 (CHANGELOG
de 2026-07-27, ADR-0009) como o "dono da máquina de estados do
`Run`": 7 arquivos em `crates/agent-engine/src/`, enum `RunState` (22
variantes), `RunEventKind` (25 variantes), struct `RunEvent`, struct
`Budget`, tabela `TRANSITIONS` (21 arestas estruturais) +
`GLOBAL_TRANSITIONS` (5 arestas globais), função pura
`apply_transition`, e suíte de **46 testes por par** (cobre todas as
arestas + 4 estados terminais imutáveis + ausência de duplicatas).

O ADR-0009 §"Decisão 4" prometeu:

> O `Run` struct vive em `frederico-agent-engine`; o `storage::Run` é
> o registro persistido. (...) A função `apply_transition` da
> `agent-engine` é **pura** — recebe o estado atual, o evento, e
> devolve o novo estado (ou erro). Quem monta a transação SQL é o
> `RunExecutor` (Etapa 4), não a Etapa 1. A regra "transição gravada
> antes de retornar" mora no executor.

A Etapa 4 da Fase 3 introduziu o `RunExecutor` (Fase 3
"Fluxo vertical 1") "para conectar a máquina ao `storage::RunRepo` e
ao `provider::ProviderAdapter`" (mesma ADR-0009 §Consequências).
Depois a Etapa 4.x.y reorganizou os crates: o `RunExecutor` virou o
"ator" do loop, e o `ChatOrchestrator` foi movido pro
`execution-engine` que delega o loop pro `RunExecutor`.

A "Etapa 4 da Fase de Ligação" (do plano de 2026-08-04, pós PR #24)
foi registrada como "decidir `frederico-agent-engine`" sem escopo
definido — justamente porque a Etapa 4 da Fase 3 não fechou o que
prometeu, e a Etapa 4 da Fase 3 fechou sem ninguém notar.

## Fato (verificável em 2026-08-04)

Auditoria feita pra este ADR, com `git grep` no `301a222`:

1. **`apply_transition` tem zero chamadas fora de `crates/agent-engine/`.** As únicas referências fora são em `docs/` (que descrevem o que deveria ser) e nos próprios testes do crate (`crates/agent-engine/src/transition.rs:321-496`). Nenhum `use frederico_agent_engine::apply_transition` em lugar nenhum do workspace.

2. **`RunEvent` e `RunEventKind` também têm zero chamadas fora de `crates/agent-engine/`.** Mesmo padrão: definidos e consumidos só dentro do crate. A enum é `pub use` no `lib.rs:43` mas nenhum crate vizinho importa.

3. **O caminho de produção ignora a máquina de estados inteira.** O `state_mapping.rs:42-56` em `crates/execution-engine/src/` mapeia `StreamEvent → RunState` por **match puro** (sem consultar `TRANSITIONS`, sem chamar `apply_transition`):

   ```rust
   pub fn run_state_for_event(event: &StreamEvent) -> Option<RunState> {
       match event {
           StreamEvent::Delta { .. } => Some(RunState::Streaming),
           StreamEvent::ToolCall { .. } => Some(RunState::WaitingToolCall),
           // ...
       }
   }
   ```

4. **O `RunRepo::set_state` é chamado direto, sem passar pela máquina.** Em `crates/execution-engine/src/recovery.rs:236`:

   ```rust
   .set_state(&run.id, frederico_agent_engine::RunState::Completed)
   ```

   sem chamar `apply_transition` antes. O estado "Completed" é gravado direto no SQLite; se a transição fosse inválida pela tabela (`TRANSITIONS` não permite `Failed → Completed` direto, por exemplo), ninguém saberia.

5. **O `agent-engine` é peça crítica como peça de tipos, não como peça de máquina.** `execution-engine` importa `RunState` (3+ lugares) e `Budget` (8+ lugares, todos em `tests/` e em `src/budget.rs`, `src/orchestrator.rs`, `src/executor.rs`). `storage` importa `RunState` (3+ lugares em `src/lib.rs`). `tool-registry` declara a dependência como preditiva (`docs/modules/tool-registry.md:184`): "não é usado ainda (a integração com a máquina de estados é Etapa 4)".

6. **O journal de eventos do spec (`agent-state-machine.md §147` "Cada `Run` tem um `RunEvent` por transição, com `seq` monotonicamente crescente por `run_id`") não existe no produto.** O que existe é `MessageEvent` no SQLite (Fase 3 Etapa 4.x, migração `0004_tool_result_kind.sql`), que é **outra** entidade — registra eventos do stream, não transições da máquina.

**Conclusão da auditoria:** os 46 testes do `agent-engine` protegem o invariante **se** alguém chamasse `apply_transition`. O caminho real (`RunExecutor → state_mapping → RunRepo::set_state`) não chama. A máquina de estados do `agent-engine` é, no produto, **documentação** — não é invariante ativo. A Etapa 1 da Fase 3 prometeu "transição gravada antes de retornar" (ADR-0009 §D1); a Etapa 4 da Fase 3 não entregou.

## Decisões

### D1 — Manter a fronteira `frederico-agent-engine`

O `agent-engine` fica como crate do workspace. **Razão:** é peça crítica para o **tipo** `RunState` (22 variantes) e `Budget` (default 50 steps / 200k tokens / 16k tokens out / $5 / 10min) — `execution-engine` e `storage` importam. Remover significa consolidar 499 linhas em outro crate, o que é trabalho de Fase 6 (decisão de "manter 14 crates" ou "consolidar"), não desta etapa.

A `Run` struct (domínio em memória) e o `apply_transition` (pura) também ficam. A função pura não está sendo chamada hoje, mas é pequena (50 linhas em `transition.rs:238-279`), testada, e — se um dia alguém implementar o portão único (D3) — já está pronta. Remover agora é trabalho de consolidação, não de decisão.

### D2 — Remover a dependência preditiva do `tool-registry`

`crates/tool-registry/Cargo.toml:14` declara `frederico-agent-engine = { workspace = true }` como dependência preditiva. `git grep "use frederico_agent_engine" crates/tool-registry/` retorna zero matches — não é usado. A "integração com a máquina de estados" prometida pelo `docs/modules/tool-registry.md:184` ("a Etapa 4 vai fazer `frederico-provider-engine` depender dele para o `RunExecutor` construir a interseção de inventário") é trabalho que **não pertence** à Fase de Ligação: depende de Fase 6 (UI de approval_queue) e de decisão de arquitetura (consolidação ou não). Mantê-la como preditiva é o tipo de "comentário aspiracional em código" que a lição 2 do PR #25 mandou evitar.

**Ação:** deletar a linha do `Cargo.toml`. Atualizar `docs/modules/tool-registry.md:184` (remover "depende preditiva"). Nenhum `cargo build` quebrou (auditoria de imports antes do commit).

### D3 — Declarar a máquina de estados como documentação, não como invariante ativo

A regra do projeto (do PR #25, memória cross-project) é: **mecanismos que nunca rodam no caminho real parecem funcionar até o dia que precisam; quando precisam, é tarde**. A `apply_transition` é exatamente isso: 46 testes passam, ninguém chama. O caminho real grava estado direto via `RunRepo::set_state`, sem consultar a tabela `TRANSITIONS`.

Até a Fase 6, o invariante da máquina de estados **não é exercitado no produto**. Os 46 testes do `agent-engine` são úteis (testam a função pura em si), mas não cobrem o que o produto faz. Qualquer revisão de PR que mexer no `state_mapping.rs` precisa ter isso em mente: a transição gravada pode ser inválida pela tabela, e ninguém vai pegar.

**Pendência nomeada pra Fase 6 (portão único de transição):** o `RunExecutor` deve ser o portão único de mudança de estado. O `state_mapping.rs` deve:
- (a) consultar `apply_transition(from, event)` antes de chamar `RunRepo::set_state`;
- (b) rejeitar a transição se `apply_transition` retornar `Err` (em vez de gravar o estado direto);
- (c) emitir um `RunEvent` (com `seq` monotonicamente crescente) que é a fonte de verdade do journal — substituindo ou complementando o `MessageEvent` atual.

Esse é o trabalho de trazer a máquina de estados de volta pro caminho de produção. Sem ele, a Etapa 1 da Fase 3 continua não cumprindo a promessa do ADR-0009.

## Consequências

- `crates/tool-registry/Cargo.toml` perde 1 linha de dependência. `cargo build` e `cargo test` continuam verdes (auditoria pré-commit).
- `docs/modules/tool-registry.md §3` perde o item "depende preditiva". Carimbo de verificação bumped.
- `docs/modules/agent-engine.md` ganha uma seção **"O que este módulo NÃO garante"** declarando que `apply_transition` não é chamada no produto até a Fase 6. Carimbo de verificação bumped.
- Os 46 testes do `agent-engine` **continuam** (não tocamos). Eles protegem a função pura. A auditoria deste ADR é que registra o limite dessa proteção.
- A Etapa 4 da Fase de Ligação **não** fecha o portão único de transição — apenas **declara** que ele está em aberto e onde vai ser resolvido. A Fase 6 do plano mestre (Multimodelo e subagentes) é o lugar natural: approval_queue + portão único de transição + UI de aprovação são trabalho coerente.
- A Fase 6 fica com **2 pendências nomeadas** que vêm da Fase de Ligação: (i) o portão único de transição (este ADR §D3); (ii) o "controle de memória na interface" do CHANGELOG da Etapa 3 da Fase de Ligação.
- O ADR é o registro de que a Etapa 1 da Fase 3 prometeu mais do que entregou. Mesma lição do PR #25 (defaults fail-open escondem o que nunca funcionou), agora em documento: **testes por par não cobrem se o produto chama a função**. O argumento é o mesmo: mecanismos que não rodam no caminho real parecem funcionar até o dia que precisam; quando precisam, é tarde.

## Alternativas consideradas

1. **Remover o `agent-engine` por inteiro (consolidar `RunState` e `Budget` no `core` ou no `execution-engine`).** Rejeitado porque: (a) consolidação é trabalho de Fase 6, não desta etapa; (b) o `agent-engine` é puro (`unsafe_code = "forbid"`, sem dependência de plataforma) e serve de **âncora de fronteira** — é mais barato manter a fronteira do que decidir consolidação agora; (c) sem a máquina de estados **como peça** (mesmo que não usada), a Fase 6 não tem o que religar.

2. **Implementar o portão único de transição agora (D3) na Etapa 4 da Fase de Ligação.** Rejeitado porque: (a) é trabalho que cruza 3 crates (`agent-engine`, `execution-engine`, `tool-registry`) e exige decisão sobre o journal (`RunEvent` vs `MessageEvent`); (b) o consumidor final é a UI de approval_queue da Fase 6, que ainda não existe; (c) fazer agora sem ter o consumidor cria exatamente o vício que o PR #25 documentou — mecanismo que roda sem ninguém ver.

3. **Marcar o `agent-engine` como `#[deprecated]` até a Fase 6.** Rejeitado porque: (a) `#[deprecated]` é decoração; o tipo continua sendo importado; (b) a verdade (este ADR §Fato) é mais útil que o aviso; (c) o carimbo de "Verificado contra o código em" + o §"O que este módulo NÃO garante" são o mecanismo certo.

4. **Manter a dependência preditiva do `tool-registry` "pra Fase 6 não esquecer".** Rejeitado porque: (a) o que a Fase 6 vai precisar é **decisão de arquitetura** (manter ou consolidar), não uma linha de `Cargo.toml` que ninguém lê; (b) ADR pendente já cobre (item "Pendências" deste ADR); (c) o `docs/modules/tool-registry.md` atualizado menciona a pendência explicitamente.

## Pendências

- **Portão único de transição (Fase 6 do plano mestre)**: o `RunExecutor` deve ser o portão único de mudança de estado; o `state_mapping` deve consultar `apply_transition` antes de `set_state`; o journal deve ser `RunEvent` (não `MessageEvent`). Trabalho da Fase 6, vai com Etapa própria quando a Fase 6 abrir.
- **Escolha "manter 14 crates" vs "consolidar" (Fase 6)**: a decisão de manter o `agent-engine` como peça separada **vale** ser revista quando a Fase 6 chegar. Aí a consolidação tem consumidor (UI de approval, journal de eventos) e dá pra avaliar com critério.
- **Carimbo de verificação do `docs/modules/agent-engine.md`**: o §"O que este módulo NÃO garante" entra neste PR. O carimbo de "Verificado contra o código em: 2026-08-04" entra junto.

## Histórico de revisão

- 2026-08-04 — versão inicial. Convergência da Etapa 4 da Fase de Ligação (decisão do plano de 2026-08-04 sobre "Etapa 6 antes da 3 e da 4"). Achado da auditoria: `apply_transition` e `RunEvent`/`RunEventKind` são zero-chamada fora do crate; o `state_mapping` mapeia direto; o `RunRepo::set_state` é chamado direto. Pendência do portão único de transição nomeada pra Fase 6.
