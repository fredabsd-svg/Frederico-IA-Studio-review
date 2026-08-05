# 0027 — Herança e desconto de Budget em subagentes + tetos anti-"explosão" (Etapa 1 da Fase 6)

## Contexto

Subagente é o **primeiro recurso do Frederico que gasta dinheiro de forma recursiva**. O `Run` pai (driver da conversa) delega para `SubagentRun`s filhos; cada filho pode, em tese, delegar para netos, e assim por diante. Sem política explícita de orçamento, um único `Run` do usuário pode gerar, em minutos, dezenas de chamadas de API em modelo pago — e o usuário só descobre na fatura.

A Fase 3 fechou a infra de `Budget` (ADR-0009): `Budget` struct em `frederico-agent-engine` carrega `max_steps` (50), `max_input_tokens` (200k), `max_output_tokens` (16k), `max_cost_microcents` (5 USD) e `timeout_ms` (10min). O `BudgetEnforcer` (Fase 3 Etapa 4) consome durante o loop do `RunExecutor`. O tipo está pronto; o que falta é a **política de herança** quando o executor tem filhos, e os **tetos numéricos** que protegem latência, complexidade e explosão de estado.

A política precisa ser **testável em invariante** (mesma família do `PermissionSet::is_subset_of` da Fase 3 Etapa 3). "Subagente nunca tem mais permissões que o pai" tem um teste que prova `perm(filho) ⊆ perm(pai)`. A herança de Budget precisa de análogo: "soma de Budget alocado aos filhos nunca excede Budget disponível do pai" — provado em teste, no caminho real, não só na estrutura.

Sem teto numérico, o invariante de soma pode ser respeitado mas o run fica com 100 filhos de $0.05 cada = $5, mesmo pai que gastou zero. A soma respeita, mas a **latência** estoura (100 rodadas sequenciais ou paralelas) e a **complexidade** da UI vira ilegível. Os tetos numéricos protegem outra coisa — não dinheiro, mas viabilidade prática.

## Decisões

### D1 — Teto global de subagentes por `Run` = 8

Cada `Run` carrega um contador `subagent_count: u32` (no domínio em memória, espelhado em coluna do SQLite via `RunRepo`). O contador **incrementa antes do spawn** e **decrementa quando o `SubagentRun` termina** (sucesso, falha, ou cancelamento — qualquer estado terminal).

A regra é verificada no momento do spawn (Etapa 4 da Fase 6): antes de criar o `SubagentRun`, o `SubagentRunner` consulta `parent.subagent_count + 1 <= 8`. Se exceder, o spawn é **rejeitado com erro estruturado** ("limite de subagentes atingido: 8") e o `RunExecutor` do pai recebe o erro como `RunError::SubagentLimitReached { current: u32, max: u32 }`. O `RunState` do pai **não transiciona** para `Failed` por isso (não é uma falha do pai, é uma decisão de design do orquestrador) — o erro volta pro modelo pai, que decide o que fazer.

**Por que 8:** é o suficiente para o caso de uso real do §9 (orquestrador delega para até 8 especialistas — revisor, pesquisador, validador, crítico, sumador, testador, arquiteto, executor). Mais que isso, nenhum fluxo desenhado pediu. **Subir depois é trivial** (constante). **Descer é quebra de comportamento** de quem já usava — princípio idêntico ao do ADR-0019 (Tesseract noturno: subir limite depois é fácil, descer quebra).

**Por que "todos os níveis somados"** (e não "8 por nível"): com 8 por nível e profundidade 2 (D2), o pior caso é 8 + 64 = 72. Difícil de raciocinar, difícil de testar, e abre porta para o usuário pagar 72 especialistas num único run. Um teto global força o orçamento a ser pensado como orçamento global, não por nível.

### D2 — Teto de profundidade = 2 (pai → filho; neto bloqueado)

`Run.depth: u32` (0 para o `Run` raiz do usuário; incrementa a cada delegação). O `SubagentRunner` rejeita spawn se `parent.depth + 1 > 2`, com `RunError::SubagentDepthExceeded { current: u32, max: u32 }`.

**Por que 2 e não 3:** o caso de uso do §9.1 ("orquestrador delega para especialistas") é pai → filho. Neto é "orquestrador que delega para um orquestrador que delega para especialistas" — comportamento que **nenhum fluxo do produto pediu ainda**, e que multiplica a dificuldade de:
- Depurar cancelamento hierárquico (3 níveis de `CancellationToken` para rastrear)
- Diagnosticar budget (3 níveis de desconto, cada um com fonte de verdade diferente)
- Renderizar UI do Modo Equipe (árvore de 3 níveis com especialista no fundo)

Subir o teto depois (constante) é trabalho de 1 linha + 1 teste. Descer é quebra.

### D3 — Budget herdado e **descontado**, não copiado

O `SubagentRunner.new(parent_budget: Budget, allocation: BudgetAllocation)` recebe **dois** argumentos:
- `parent_budget: &Budget` — referência ao Budget vivo do pai (não clone, não cópia). O pai continua sendo dono.
- `allocation: BudgetAllocation` — o **delta** que o pai libera para o filho: `BudgetAllocation { max_steps: u32, max_cost_microcents: u64, ... }`.

O `Budget` do filho é construído como **visão** sobre o pai + delta: `child_budget.effective = parent_budget.remaining ∩ allocation`. O filho não tem Budget próprio; tem **uma janela** dentro do Budget do pai.

A cada `cents` ou `step` que o filho gasta, o `SubagentRunner` **desconta** do `parent_budget.remaining` (atomicamente, dentro da mesma seção crítica que o `RunExecutor` usa para o loop). O desconto é unidirecional: pai → filho. Se o pai é cancelado, os filhos perdem o que tinham alocado (porque o `parent_budget.remaining` cai pra zero ou fica indisponível).

**Invariante testável (no caminho real, não só na estrutura):**

```text
Σ child.spent_microcents (todos os filhos vivos + terminais do run)
  ≤ parent.budget.remaining_inicial − parent.budget.remaining_atual
```

Ou seja: o que os filhos somaram jamais excede o que o pai gastou a mais desde o início do spawn. Testado em `crates/e2e/tests/e2e_subagent_e2e.rs::subagent_budget_sum_never_exceeds_parent` consumindo `build_chat_orchestrator`, não em `crates/agent-engine/tests/`.

**Por que desconto e não "allocation estática":** com allocation estática (filho recebe X cents, gasta à vontade até X), o pai pode ter 10 filhos com $1 cada = $10 potenciais, mesmo que o pai só tenha $5. O invariante de soma passa mas o total estoura. **Desconto unidirecional** garante que o gasto do filho aparece no saldo do pai em tempo real — o pai não pode mais gastar do que tem, e o teto de D1 (8 filhos) limita o paralelismo.

**Por que referência e não clone:** evita "dois Budgets que divergem" — o bug clássico de "child says I have $5, parent says I have $0, both pass their own assertion". O `&Budget` força a fonte única.

### D4 — Verificação no spawn, erro legível, nunca panic nem silent fail

O `SubagentRunner::try_spawn(parent, specialist_id, allocation) -> Result<SubagentHandle, SubagentError>`:
- Se D1 (8 global) exceder: `Err(SubagentError::GlobalLimitReached { current, max })`
- Se D2 (depth 2) exceder: `Err(SubagentError::DepthExceeded { current, max })`
- Se D3 (allocation > parent.remaining) exceder: `Err(SubagentError::AllocationExceedsParent { requested, available })`
- Se `specialist_id` não estiver no registry: `Err(SubagentError::UnknownSpecialist { requested, valid: Vec<SpecialistId> })` (§9.2 zero fallback silencioso — lista de válidos sempre no erro)

Nenhum panic. Nenhum "fallback para o pai executar a tarefa". Nenhum "spawn paralelo, vê se cabe, recusa depois". O erro volta pro modelo do pai, que decide:
- "Tentou gerar 9º filho" → modelo decide se cancela um filho existente, ou reformula sem o 9º, ou pede desculpa ao usuário.
- "Tentou gerar neto" → modelo reformula sem o neto, ou expande o trabalho do filho existente.

O texto do erro é **legível pelo modelo**: `"não foi possível criar o subagente 'revisor-final': limite global de 8 subagentes por run atingido (atual: 8). Subagentes ativos: ['pesquisador', 'arquiteto', 'testador', 'revisor-1', 'revisor-2', 'validador', 'sumador', 'crítico']. Cancele um subagente ativo ou reformule sem o 9º."` (exemplo ilustrativo).

### D5 — `BudgetAllocation` é a única superfície pública de alocação

`crates/execution-engine/src/budget_allocation.rs` (novo, 100% coberto por testes):
- `BudgetAllocation::try_from(parent: &Budget, requested: Budget) -> Result<Self, AllocationError>` — falha se `requested > parent.remaining` em qualquer eixo.
- `BudgetAllocation::split(self, parts: u32) -> Vec<BudgetAllocation>` — divide em N partes iguais (pra delegar em paralelo). Falha se `parts > parent.remaining.max_steps` (cada parte precisa de pelo menos 1 step).
- Testes de unidade: 12 cenários (over-cost, over-steps, over-time, split exato, split com resto, split impossível, etc.).

`BudgetAllocation` é o que o modelo do pai preenche; o `SubagentRunner.new` consome. **Nada mais aloca Budget** — o `SubagentRunner` é o portão.

### D6 — Teto de modelo: pendência nomeada, fora do escopo

Teto de "X modelos distintos em paralelo" (ex.: no máximo 4 modelos pagos num run) é **problema diferente** — limite de taxa do provedor, não descontrole recursivo. A anti-explosão desta ADR cuida do segundo. O primeiro vira pendência nomeada pra fase futura (provavelmente Fase 8 — Copiloto, ou um hotfix de provedor-engine).

Documentado em `subagent-architecture.md` §"Pendências" e em `process-architecture.md` quando aplicável.

## Consequências

- `crates/agent-engine/src/budget.rs` ganha `BudgetAllocation` (struct nova, 100% testada) + método `Budget::try_allocate(allocation) -> Result<Budget, AllocationError>`. Bump atômico.
- `crates/agent-engine/src/subagent_budget.rs` (novo, dentro do `agent-engine` pra manter a fronteira crítica do ADR-0025 D1) carrega o invariante: `Σ alocações vivas ≤ pai.remaining_inicial − pai.gasto_atual`. Função pura, testada.
- `crates/execution-engine/src/subagent_runner.rs` (novo) é o portão: consulta D1/D2/D3, devolve erro estruturado, chama o `ChatOrchestrator` do filho com `permissions ∩ parent_permissions` e `budget_allocation` (Etapa 4).
- `docs/modules/agent-engine.md` ganha §"Orçamento de subagentes" + bump no carimbo.
- `docs/modules/execution-engine.md` ganha §"SubagentRunner" + bump no carimbo.
- E2E em `crates/e2e/tests/e2e_subagent_e2e.rs` (criado na Etapa 4): 6 testes, todos consumindo `build_chat_orchestrator`:
  - `subagent_runs_with_reduced_permissions`
  - `subagent_inherits_cancellation_token`
  - `subagent_budget_discounted_from_parent_in_real_path` (o teste do invariante de soma, no caminho real)
  - `subagent_explosion_cap_8_rejects_ninth`
  - `subagent_depth_cap_2_rejects_grandchild`
  - `subagent_budget_sum_never_exceeds_parent`
- `docs/status.md` linha 33 (Fase 6) ganha referência a esta ADR na coluna "Pendências" da Etapa 4.
- Política de herança é **simétrica** à política de permissões (Fase 3 Etapa 3): assim como `perm(filho) ⊆ perm(pai)`, `spent(filhos somados) ≤ spent(pai incremental)`. Mesma família de invariante testável.

## Alternativas consideradas

1. **Teto por nível** (8 por nível × 3 níveis = 24 total). Rejeitado porque (a) o pior caso é difícil de raciocinar (8+64+512), (b) o teste de invariante teria que contar "filhos do nível 1" + "filhos do nível 2" + "filhos do nível 3" separados, (c) o teto de profundidade 2 (D2) já corta antes, tornando o teto de 3º nível decorativo. Um teto global, uma asserção, um teste.
2. **Profundidade 3** (pai → filho → neto → bisneto). Rejeitado porque (a) o caso de uso do §9.1 não passa de pai → filho, (b) subir depois é trivial (constante), (c) a UI do Modo Equipe fica ilegível com 3 níveis, (d) o teste de cancelamento hierárquico precisa de 3 níveis de `CancellationToken` rastreáveis.
3. **Allocation estática** (filho recebe X, gasta até X, não desconta do pai). Rejeitado porque (a) cria o bug "10 filhos × $1 = $10 potencial, pai tem $5, ambos passam各自", (b) o invariante de soma passa mas o total estoura, (c) o desconto unidirecional é a única forma de garantir que "filhos somados jamais excedem pai".
4. **Spawn paralelo, vê se cabe, recusa depois** (otimista). Rejeitado pelo mesmo motivo do `WorkerToolDispatcher::allowed_paths` (PR #25): mecanismo fail-open esconde o que nunca foi exercitado. Verificar no spawn-time é a única forma de o invariante ser **anterior** ao efeito, não posterior.
5. **Fallback silencioso** ("se o 9º filho não cabe, executa a tarefa no pai"). Rejeitado pelo §9.2 (zero fallback silencioso) — o modelo pai precisa saber que falhou, com a lista de filhos ativos, pra decidir conscientemente.
6. **Incluir teto de modelo nesta ADR** (ex.: "no máximo 4 modelos distintos em paralelo"). Rejeitado porque o teto de modelo resolve problema diferente (rate limit do provedor) e tem ciclo de vida próprio. Pendência nomeada em `subagent-architecture.md`.

## Pendências

- **Teto de modelo (Fase 8 ou hotfix)**: limite de taxa do provedor. Pendência nomeada, fora do escopo da Fase 6.
- **UI do erro de spawn**: o erro legível desta ADR é mostrado no log/CLI, mas a UI (modal de "não foi possível criar subagente, clique aqui para ver os ativos") é trabalho da Etapa 6. Por enquanto, o modelo pai recebe o texto e decide.
- **Política de timeout**: o timeout do subagente é **independente do pai** (cada um tem o seu, dos 10min default). A interação entre timeout do pai e timeout do filho (filho mais restritivo vence, ou o mais permissivo?) é trabalho de design que cabe na Etapa 4 da Fase 6. Por enquanto, **timeout é por run, não compartilhado** — fica documentado pra Etapa 4 revisar.
- **Migração SQLite** (Etapa 4): nova coluna `subagent_count` em `runs` + tabela `subagent_allocations`. Não é trabalho da Etapa 1.

## Histórico de revisão

- 2026-08-05 — versão inicial. Decisão da Etapa 1 da Fase 6 (plano de 2026-08-05, conversa de planejamento). Validação pelo user: profundidade 2, teto global 8, budget herdado e descontado, verificação no spawn com erro legível — teto de modelo como pendência nomeada. Cobertura de E2E apontada para `crates/e2e/tests/` (não `crates/agent-engine/tests/`), porque o teste do invariante de soma precisa provar no caminho real, não só na estrutura.
