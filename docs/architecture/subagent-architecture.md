<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-06
Fase correspondente: 6 (Etapa 1 + Etapa 4 PR 1 fechadas; Etapa 4 PR 2 pendente)
-->

# Arquitetura de Subagentes

> Aprofundado na Etapa 1 da Fase 6 (2026-08-05). Stub criado na Fase 0;
> o aprofundamento desta data foca o **registro explícito de
> especialistas**, o **grafo de execução** (decidido como tarefa única
> com pré-requisitos), o **cancelamento hierárquico**, e a
> **política anti-"explosão"** (ver
> [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md)).
> O [`PermissionSet::is_subset_of`](../modules/tool-registry.md) (Fase 3
> Etapa 3) e a `SpecialistDefinition` registry (Etapa 3 da Fase 6, ver
> [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md))
> fecham o invariante "subagente ⊆ pai" no caminho de produção.

## Decisão tomada (Fase 0, Etapa 1 da Fase 6 aprofunda)

- **Registro explícito de especialistas** (`PROMPT MESTRE` §9.1): o modelo principal só pode delegar para IDs existentes; nunca para nomes inventados. Lista dos 8 default em [`SpecialistDefinition`](#contrato) §"Bundled default".
- **Zero fallback silencioso** quando o especialista não existe (`PROMPT MESTRE` §9.2) — erro estruturado, lista de válidos, sem substituição. Implementação em [`SubagentRunner`](#subagentrunner) §"Erros estruturados".
- **Grafo de execução** com dependências entre sub-tarefas (`PROMPT MESTRE` §9.3); paralelização só quando independente (revisão nunca antes da criação, validação nunca antes da geração, etc.). Decisão da Etapa 1: **tarefa única com lista de pré-requisitos** (mais simples que DAG completo para a Fase 6).
- **Cancelamento hierárquico** via `CancellationToken` (`PROMPT MESTRE` §9.4) — o "Parar" do usuário cancela agente principal, subagentes, ferramentas, workers, processos filhos, downloads, processamento de documentos. Implementação em [`CancellationToken`](#cancellationtoken) abaixo.
- **Subagente nunca tem mais permissões que o agente pai** — `PermissionSet::is_subset_of` (Fase 3 Etapa 3) provado em teste; a Etapa 3 da Fase 6 carrega o `PermissionSet` real do `assistant`/`project`/`user` (ADR-0030 §D3).
- **Anti-"explosão"** (decisão crítica da Etapa 1, ADR-0027): subagente é o primeiro recurso que gasta dinheiro de forma recursiva; sem política, um run gera dezenas de chamadas em minutos. Tetos: **8 subagentes global por run**, **profundidade 2**, **budget herdado e descontado** (não copiado), **verificação no spawn com erro legível**. Detalhes em [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md).
- **Interface do Modo Equipe** mostra agente principal, especialistas, modelo de cada um, objetivo, dependências, ferramentas, progresso, arquivos, custo, resultado, erros (`PROMPT MESTRE` §9.5). A UI completa é a Etapa 6 da Fase 6.

## Contrato

```rust
struct SpecialistDefinition {
    id: SpecialistId,                    // ex.: "revisor", "pesquisador", "testador"
    name: String,                        // "Revisor de Código"
    description: String,                 // "Revisa o diff e aponta problemas..."
    purpose: String,                     // "Revisão de PR antes de merge"

    default_model: ModelId,              // resolve via model-catalog
    allowed_model_capabilities: Vec<String>,  // ["code", "long-context", "tools"]

    allowed_tools: Vec<ToolId>,
    denied_tools: Vec<ToolId>,           // tem precedência sobre allowed_tools

    max_steps: u32,                      // sub-budget de steps
    timeout_ms: u32,                     // timeout do subagente (não compartilhado com pai)
    token_budget: Option<u64>,
    cost_budget: Option<Budget>,         // alocação (ADR-0027 D5)
}

struct SubagentRun {
    subagent_run_id: Uuid,
    parent_run_id: RunId,                // FK para o Run pai
    specialist_id: SpecialistId,         // resolve via SpecialistRegistry
    state: RunState,                     // Running | Streaming | WaitingToolCall | Completed | Failed | Cancelled
    depth: u32,                          // 0 para o run raiz; 1 para filho direto; 2 bloqueado
    allocation: BudgetAllocation,        // ADR-0027 D5: delta dentro do Budget do pai
    effective_permissions: PermissionSet,// assistant ∩ project ∩ user ∩ parent_permissions ∩ allowed_tools − denied_tools
    dependencies: Vec<TaskId>,           // pré-requisitos (tarefa única com lista, ver §"Grafo")
    output: Option<SubagentOutput>,      // Some quando concluído
    cost_microcents: u64,                // gasto efetivo (descontado do pai, ADR-0027 D3)
    started_at: i64,
    finished_at: Option<i64>,
}

struct SubagentOutput {
    artifact_id: Option<ArtifactId>,     // artefato produzido (resolve via ArtifactId)
    summary: String,                     // resumo legível pelo pai
    tools_used: Vec<ToolId>,             // subset do que allowed_tools autorizou
}
```

### Bundled default

8 especialistas bundled em `crates/model-catalog/frederico://specialists/default.toml` (criado na Etapa 3 da Fase 6, ADR-0030 §D1):

- `revisor` — lê código, emite diff de revisão
- `pesquisador` — busca em memória + web_browse
- `testador` — roda testes via terminal sandboxes
- `validador` — checa invariantes declaradas pelo pai
- `sumador` — sumariza o output de outros especialistas
- `arquiteto` — projeta estrutura antes de implementar
- `crítico` — aponta fraquezas no plano
- `executor` — implementa o plano

O usuário pode desabilitar/reescrever via `~/.config/frederico/specialists.toml` (override). Pode **adicionar** novos com IDs próprios, mas **não pode invadir** os 8 bundled nem IDs que não existem — o `SpecialistRegistry::get(id)` retorna erro com a lista de válidos (§9.2).

## Grafo de execução (decisão: tarefa única com pré-requisitos)

A decisão da Etapa 1 é **tarefa única com lista de pré-requisitos** em vez de DAG completo. Razões:

- DAG completo (com paralelização automática por dependência) é trabalho de fase futura (Fase 8 — Copiloto). A v1 não precisa: o usuário declara a ordem de execução via `dependencies: Vec<TaskId>`, e o `SubagentRunner` executa em ordem topológica simples (BFS).
- Paralelização automática introduz non-determinismo na ordem de eventos do `RunEvent` journal, o que complica o teste E2E `run_event_seq_monotonic_through_orchestrator` (ADR-0029).
- O caso de uso do §9.3 ("revisão nunca antes da criação, validação nunca antes da geração") é ordem parcial, não paralelismo. `dependencies` cobre.

**Invariante:** o `SubagentRunner` não inicia o subagente enquanto todos os `dependencies` não estão em estado terminal `Completed`. Se uma `dependency` está `Failed` ou `Cancelled`, o subagente dependente herda esse estado (sem chamar o modelo).

Exemplo (ilustrativo):

```text
executor depende de [arquiteto, validador]
  → executor não inicia enquanto arquiteto AND validador não estão Completed
sumador depende de [executor]
  → sumador inicia após executor Completed
crítico depende de [arquiteto]
  → crítico pode rodar em paralelo com executor (independentes)
```

**Limitação conhecida:** esta versão não paraleliza subagentes independentes. Eles executam sequencialmente. Paralelização é trabalho de fase futura (decisão de ADR próprio).

## CancellationToken

`crates/execution-engine/src/cancellation.rs` (existente desde a Fase 3 Etapa 4.x):

- `CancellationToken::new() -> (CancellationToken, CancellationTrigger)` — producer/consumer.
- `CancellationToken::is_cancelled() -> bool` — não-bloqueante, lê estado atômico.
- `CancellationTrigger::cancel() -> ()` — propaga pra todos os tokens filhos.

A propagação é **por herança**: quando o `SubagentRunner` cria o filho, ele passa **clone** do `CancellationToken` do pai. O filho pode passar clone pros netos, mas netos estão bloqueados pela profundidade 2 (ADR-0027 D2).

E2E em `crates/e2e/tests/e2e_subagent_e2e.rs::subagent_inherits_cancellation_token`:
- Lança 3 subagentes.
- Cancela o pai.
- Assert: `is_cancelled() == true` em todos os 3 filhos, sem precisar cancelar cada um.

## SubagentRunner

`crates/execution-engine/src/subagent_runner.rs` (novo, ADR-0027 + ADR-0030):

```rust
impl SubagentRunner {
    pub fn new(
        parent: &Run,
        specialist_registry: Arc<dyn SpecialistRegistry>,
        permission_loader: Arc<dyn PermissionLoader>,
    ) -> Self { ... }

    pub fn try_spawn(
        &self,
        parent: &mut Run,
        specialist_id: &str,
        requested_allocation: BudgetAllocation,
    ) -> Result<SubagentHandle, SubagentError> { ... }
}
```

A função `try_spawn` é o portão: consulta D1 (8 global), D2 (depth 2), D3 (allocation ≤ parent.remaining), §9.2 (specialist existe), e devolve erro estruturado em qualquer falha. **Nunca panic, nunca silent fail.**

### Erros estruturados

```rust
pub enum SubagentError {
    GlobalLimitReached { current: u32, max: u32 },
    DepthExceeded { current: u32, max: u32 },
    AllocationExceedsParent { requested: BudgetAllocation, available: Budget },
    Registry(RegistryError),         // UnknownSpecialist { requested, valid } | ...
    PermissionDenied { required: PermissionSet, available: PermissionSet },
    InternalError(String),
}
```

O texto do erro é **legível pelo modelo**:

> "não foi possível criar o subagente 'revisor-final': limite global de 8 subagentes por run atingido (atual: 8). Subagentes ativos: ['pesquisador', 'arquiteto', 'testador', 'revisor-1', 'revisor-2', 'validador', 'sumador', 'crítico']. Cancele um subagente ativo ou reformule sem o 9º."

## Política de timeout

**Decisão da Etapa 1:** timeout do subagente é **independente do pai**, com interação documentada:

- `SubagentRun.timeout_ms` (default 10min, configurável por `SpecialistDefinition.timeout_ms`).
- Pai tem o próprio `timeout_ms` (default 10min do `Budget`).
- Se o pai estoura o timeout antes do filho, o filho é **cancelado** junto (cancelamento hierárquico, mesmo `CancellationToken`).
- Se o filho estoura o timeout antes do pai, o filho é marcado `Failed`, o pai **continua** (filho independente do pai em timeout, mas não em budget).

A interação com budget é o que limita o gasto real: se o budget do pai esgota antes do timeout de qualquer filho, todos os filhos perdem o `parent_budget.remaining` e param (desconto unidirecional, ADR-0027 D3).

**Pendência:** a política de "timeout do filho é mais restritivo vence, ou o mais permissivo?" é trabalho de design da Etapa 4 (cabe lá, quando o `SubagentRunner` for implementado). Por enquanto, **timeout é por run, não compartilhado** — fica documentado pra Etapa 4 revisar.

## Anti-"explosão" (decisão crítica, ADR-0027)

Resumo das 4 regras que protegem contra gasto recursivo descontrolado:

1. **Teto global de 8 subagentes por run** (todos os níveis somados). Verificação no spawn. Erro: `GlobalLimitReached { current, max }`.
2. **Teto de profundidade 2** (pai → filho; neto bloqueado). Verificação no spawn. Erro: `DepthExceeded { current, max }`.
3. **Budget herdado e descontado** (não copiado). `Σ alocações vivas + Σ gastos ≤ pai.remaining_inicial`. Invariante testado em `subagent_budget_sum_never_exceeds_parent` no caminho real.
4. **Verificação no spawn, erro legível, nunca panic nem silent fail**. Texto do erro devolvido pro modelo do pai, que decide.

Por que **profundidade 2 e não 3**: o caso de uso do §9.1 é pai → filho (orquestrador delega a especialistas). Neto é "orquestrador delegando a quem delega" — comportamento que nenhum fluxo do produto pediu ainda. Subir depois é trivial (constante). Descer é quebra.

Por que **teto global 8 e não por nível**: tetos por nível (8 × 3 = 24) são difíceis de raciocinar e de testar. Um teto global, uma asserção, um teste.

Detalhamento completo em [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md).

## Não-objetivos (v1)

- Subagentes que o modelo "inventa" dinamicamente fora do registro.
- Auto-modificação de `allowed_tools` em runtime (mudança exige reload do app).
- Subagentes como plugin de terceiros na v1.
- Comunicação direta entre subagentes sem passar pelo agente pai (canal lateral = vazamento de estado).
- Paralelização automática de subagentes independentes (tarefa única com pré-requisitos, ver §"Grafo").
- Auto-tunar o `dependencies` ou o `allocation` baseado em histórico.

## Decisões

- [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md) — herança e desconto de Budget, tetos anti-"explosão", verificação no spawn.
- [ADR-0029](../decisions/0029-run-event-journal-replaces-message-event.md) — `RunEvent` é a fonte de verdade do journal de transições (subagentes também transicionam por aí).
- [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md) — `SpecialistRegistry` carrega do `model-catalog`; `PermissionSet` real do `assistant`/`project`/`user` antes do `validate_tool_call`.
- Nenhuma outra nova nesta versão.

## Pendências

- **Paralelização automática de subagentes independentes** (fase futura, depois da Fase 6). A v1 executa sequencialmente respeitando `dependencies`.
- **UI completa do Modo Equipe** (Etapa 6 da Fase 6): sidebar com especialista, modelo, objetivo, dependências, ferramentas, progresso, custo, resultado, erros. Esta Etapa 1 fecha o contrato; a Etapa 6 fecha a UI.
- **Política de timeout do subagente** (interação com timeout do pai): decidir "mais restritivo vence" vs "mais permissivo vence" na Etapa 4.
- **Customização de `allowed_tools` por chamada** (override do `SpecialistDefinition` por spawn): trabalho de Fase 8.
- **Teto de modelo** (limite de taxa do provedor, não descontrole recursivo): pendência nomeada, fora do escopo da Fase 6. Citado em [`multimodel-architecture.md`](./multimodel-architecture.md) §"Não-objetivos".
- **Auto-modificação de `SpecialistDefinition` em runtime**: proibido na v1. Mudança exige reload do app.
- **Retomada de árvore de subagentes interrompida pelo watchdog**: a Etapa 4 (cancelamento hierárquico) cobre o caso comum; a retomada após watchdog (process kill externo) é trabalho de fase futura.

## E2E de cobertura planejado por etapa

Mesmo formato do `multimodel-architecture.md` §"E2E de cobertura planejado por etapa". Alvo declarado na Etapa 1, atualizado por etapa conforme cada PR mergea. **6 testes na Etapa 4** (a mais densa), todos consumindo `build_chat_orchestrator` — não teste de crate. Justificativa: o teste do invariante de soma do Budget precisa provar no caminho real, não só na estrutura (lição do PR #26, ADR-0025 §Fato).

| Etapa | E2E de cobertura (alvo) | Passo CI |
|-------|--------------------------|----------|
| 1 | — (sem código) | — |
| 4 | `crates/e2e/tests/e2e_subagent_e2e.rs::subagent_runs_with_reduced_permissions`, `::subagent_inherits_cancellation_token`, `::subagent_budget_discounted_from_parent_in_real_path`, `::subagent_explosion_cap_8_rejects_ninth`, `::subagent_depth_cap_2_rejects_grandchild`, `::subagent_budget_sum_never_exceeds_parent` | `cargo test --workspace` |

(E2E das Etapas 2, 3, 5, 6 listados no spec `multimodel-architecture.md` e no `status.md`.)

## Referências

- `PROMPT MESTRE` §9 (subagentes e modo equipe), §9.1 (registro explícito), §9.2 (zero fallback silencioso), §9.3 (grafo de execução), §9.4 (cancelamento hierárquico), §9.5 (UI do Modo Equipe)
- [`agent-state-machine.md`](./agent-state-machine.md) (estados do `Run` reusados pelo `SubagentRun.state`)
- [`tool-permission-model.md`](./tool-permission-model.md) (invariante "subagente ≤ pai" via `PermissionSet::is_subset_of`)
- [`chat-and-providers.md`](./chat-and-providers.md) (catálogo de modelos e provedores)
- [`testing-strategy.md`](./testing-strategy.md) (cobertura E2E do caminho de produção)
- [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md) — anti-"explosão"
- [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md) — registry de especialistas
