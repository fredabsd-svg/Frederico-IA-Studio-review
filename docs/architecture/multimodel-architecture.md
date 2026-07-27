<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 6
-->

# Arquitetura Multimodelo (stub)

> Stub criado na Fase 0. Será aprofundado antes do início da Fase 6 (Multimodelo e subagentes).

## Decisão tomada

Quatro modos distintos, cada um com semântica própria (`PROMPT MESTRE` §14):

- **Comparação** — modelos respondem em paralelo, cartões do mesmo tamanho, autoria preservada, custo individual, síntese opcional.
- **Conselho** — modelos analisam o mesmo pedido, coordenador identifica consenso, mostra divergências e respostas originais, síntese separada, autoria preservada.
- **Debate** — rodadas limitadas, papéis definidos, orçamento, resumo entre rodadas, controle de crescimento do contexto, conclusão por coordenador.
- **Pipeline sequencial** (modo prioritário, `PROMPT MESTRE` §14.4) — cada modelo trabalha com o **artefato real** do anterior, registra versão de entrada/saída, hash, custo, ferramentas e validação. Persistência do pipeline entre etapas (sobreviver a fechamento do app, §14.5).

## Contrato previsto

```rust
enum MultimodelMode { Comparison | Council | Debate | Pipeline }

struct MultimodelRun {
    run_id: RunId,
    parent_run_id: Option<RunId>,
    mode: MultimodelMode,
    stages: Vec<MultimodelStage>,
    budget: Budget,
    artifact_refs: Vec<ArtifactId>,   // entrada e saída
    state: MultimodelState,
}

struct MultimodelStage {
    stage_id: StageId,
    model_id: ModelId,
    provider_id: ProviderId,
    input_artifact: Option<ArtifactId>,
    output_artifact: Option<ArtifactId>,
    input_hash: Option<String>,       // hash do artefato de entrada
    output_hash: Option<String>,
    state: RunState,
    cost_usd: Decimal,
    tools_used: Vec<ToolId>,
    validation: Option<ValidationResult>,
}
```

## Não-objetivos

- Votação de modelos como mecanismo de decisão (consenso ≠ verdade).
- Modelo que avalia modelo fora de um pipeline declarado (a UI não esconde origem da síntese).
- Multimodelo com mais de 4 modelos em paralelo na v1 (limite de orçamento e UI).
- Persistência cross-device de pipelines multimodelo.

## Aprofundar antes da Fase 6

- Critério de "cartões do mesmo tamanho" na comparação — definição de tamanho, métrica, e o que fazer quando uma resposta estoura.
- Algoritmo de consenso do conselho: como caracterizar "divergência" e "consenso" sem alucinar relação entre respostas independentes.
- Política de "debate" quanto a contexto: quanto manter entre rodadas, como resumir, quando parar.
- Regras de `input_artifact` no pipeline: quando pular etapa se o artefato anterior não mudou.
- Critério de "concluído" em cada modo — quem decide, e como a UI mostra a autoria de cada trecho.
- Visualização na UI (`PROMPT MESTRE` §23.3): grade consistente, linha do tempo, autoria, versões, comparação, resposta consolidada separada.
- Como cancelar `MultimodelRun` no meio (cancelamento hierárquico do `PROMPT MESTRE` §9.4) sem corromper artefatos já produzidos.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado.

## Referências

- `PROMPT MESTRE` §14 (multimodelo)
- [`agent-state-machine.md`](./agent-state-machine.md) (estados do `Run`)
- [`subagent-architecture.md`](./subagent-architecture.md) (subagentes, intimamente ligado)
- [`testing-strategy.md`](./testing-strategy.md)
