<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 6
-->

# Arquitetura de Subagentes (stub)

> Stub criado na Fase 0. Será aprofundado antes do início da Fase 6 (Multimodelo e subagentes).

## Decisão tomada

- **Registro explícito de especialistas** (`PROMPT MESTRE` §9.1): o modelo principal só pode delegar para IDs existentes; nunca para nomes inventados.
- **Zero fallback silencioso** quando o especialista não existe (`PROMPT MESTRE` §9.2) — erro estruturado, lista de válidos, sem substituição.
- **Grafo de execução** com dependências entre sub-tarefas (`PROMPT MESTRE` §9.3); paralelização só quando independente (revisão nunca antes da criação, validação nunca antes da geração, etc.).
- **Cancelamento hierárquico** via `CancellationToken` (`PROMPT MESTRE` §9.4) — o "Parar" do usuário cancela agente principal, subagentes, ferramentas, workers, processos filhos, downloads, processamento de documentos.
- **Subagente nunca tem mais permissões que o agente pai** (ver [`tool-permission-model.md`](./tool-permission-model.md)).
- **Interface do Modo Equipe** mostra agente principal, especialistas, modelo de cada um, objetivo, dependências, ferramentas, progresso, arquivos, custo, resultado, erros (`PROMPT MESTRE` §9.5).

## Contrato previsto

```rust
struct SpecialistDefinition {
    id: SpecialistId,
    name: String,
    description: String,
    purpose: String,

    default_model: Option<ModelId>,
    allowed_model_capabilities: Vec<String>,

    allowed_tools: Vec<ToolId>,
    denied_tools: Vec<ToolId>,

    max_steps: u32,
    timeout_ms: u32,
    token_budget: Option<u64>,
    cost_budget: Option<Decimal>,
}

struct SubagentRun {
    subagent_run_id: Uuid,
    parent_run_id: RunId,
    specialist_id: SpecialistId,
    state: RunState,
    dependencies: Vec<TaskId>,         // grafo de execução
    output: Option<SubagentOutput>,
}
```

## Não-objetivos

- Subagentes que o modelo "inventa" dinamicamente fora do registro.
- Auto-modificação de `allowed_tools` em runtime (mudança exige reload do app).
- Subagentes como plugin de terceiros na v1.
- Comunicação direta entre subagentes sem passar pelo agente pai (canal lateral = vazamento de estado).

## Aprofundar antes da Fase 6

- Formato exato do grafo de execução (DAG? tarefa única com pré-requisitos?).
- Política de timeout: timeout do subagente é absoluto ou compartilhado com o pai?
- Como evitar "explosão de subagentes" (modelo que delega em loop para inflar custo).
- Visualização do Modo Equipe na UI (sidebar, modal, drawer — `PROMPT MESTRE` §9.5 + §23).
- Como retomar uma árvore de subagentes interrompida pelo watchdog.
- Estratégia de teste: simulação determinística de subagentes com fixtures em `tests/fixtures/subagent/`.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado.

## Referências

- `PROMPT MESTRE` §9 (subagentes e modo equipe)
- [`agent-state-machine.md`](./agent-state-machine.md)
- [`tool-permission-model.md`](./tool-permission-model.md) (invariante "subagente ≤ pai")
- [`testing-strategy.md`](./testing-strategy.md)
