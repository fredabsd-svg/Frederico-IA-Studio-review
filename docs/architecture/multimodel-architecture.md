<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-05
Fase correspondente: 6
-->

# Arquitetura Multimodelo

> Aprofundado na Etapa 1 da Fase 6 (2026-08-05). Stub criado na Fase 0;
> o aprofundamento desta data foca o **Pipeline Sequencial** como único
> modo entregue pela Fase 6. Comparação, Conselho e Debate são
> documentados no §"Modos documentados e fora de escopo da Fase 6"
> abaixo e adiados para incremento posterior — ver
> [ADR-0028](../decisions/0028-pipeline-sequencial-multimodel.md) §D2.

## Escopo da Fase 6

A Fase 6 (Multimodelo e subagentes) fecha quando o **Pipeline Sequencial** está em produção. Os outros 3 modos (Comparação, Conselho, Debate) **não são critério de aceite** desta fase — cada um vira PR próprio com E2E próprio depois, plugando na infraestrutura do Pipeline.

A frase é literal e entra no `docs/status.md` linha 33 (Fase 6) quando a fase for promovida a `concluída` (Etapa 6 da Fase 6):

> "Comparação, Conselho e Debate são modos documentados no spec `multimodel-architecture.md` (v1, §Modo X) e adiados para incremento posterior. Não são critério de aceite da Fase 6."

Esta decisão é do ADR-0028 §D1 ("Fase 6 entrega apenas Pipeline Sequencial") e §D2 ("fora de escopo, não pendência envergonhada"). A motivação está no ADR-0028 §Contexto.

## Decisão tomada (Fase 0, Etapa 1 da Fase 6 aprofunda)

O `PROMPT MESTRE` §14 lista 4 modos distintos, cada um com semântica própria:

- **Comparação** — modelos respondem em paralelo, cartões do mesmo tamanho, autoria preservada, custo individual, síntese opcional.
- **Conselho** — modelos analisam o mesmo pedido, coordenador identifica consenso, mostra divergências e respostas originais, síntese separada, autoria preservada.
- **Debate** — rodadas limitadas, papéis definidos, orçamento, resumo entre rodadas, controle de crescimento do contexto, conclusão por coordenador.
- **Pipeline sequencial** (modo prioritário, `PROMPT MESTRE` §14.4) — cada modelo trabalha com o **artefato real** do anterior, registra versão de entrada/saída, hash, custo, ferramentas e validação. Persistência do pipeline entre etapas (sobreviver a fechamento do app, §14.5).

A Etapa 1 da Fase 6 fecha as 6 decisões em aberto do spec stub original (Fase 0) **para o escopo do Pipeline Sequencial**. As decisões dos outros 3 modos ficam adiadas — quem for atacar Comparação, Conselho ou Debate vai abrir o spec, ler o stub, e tomar suas próprias decisões (com ADR próprio).

## Pipeline Sequencial (modo da Fase 6)

### Contrato

```rust
enum MultimodelMode { Comparison | Council | Debate | Pipeline }    // só Pipeline na Fase 6

struct MultimodelRun {
    run_id: RunId,
    parent_run_id: Option<RunId>,
    mode: MultimodelMode,                                           // sempre Pipeline na Fase 6
    stages: Vec<MultimodelStage>,                                   // sequencial, sem paralelo
    final_artifact: Option<ArtifactId>,                             // output_artifact do último stage
    budget: Budget,                                                 // budget herdado do Run pai
    state: MultimodelState,                                         // Running | Completed | Failed | Cancelled
}

struct MultimodelStage {
    stage_id: StageId,
    run_id: RunId,                                                  // FK para MultimodelRun
    seq: u32,                                                       // ordem no pipeline (0-based)
    model_id: ModelId,                                              // resolve via model-catalog
    provider_id: ProviderId,
    state: RunState,                                                // reusa o do agent-engine
    input_artifact: Option<ArtifactId>,                             // None para o primeiro stage
    output_artifact: Option<ArtifactId>,                            // Some quando concluído
    input_hash: Option<String>,                                     // SHA-256 do input_artifact
    output_hash: Option<String>,                                    // SHA-256 do output_artifact
    started_at: Option<i64>,
    finished_at: Option<i64>,
    cost_microcents: u64,                                           // alimentado pelo provider-engine
    tools_used: Vec<ToolId>,                                        // do ToolRegistry
    validation: Option<ValidationResult>,                           // opcional, ver §"Validação por stage"
}

enum MultimodelState { Running | Completed | Failed | Cancelled }

struct ValidationResult {
    validator: ValidatorId,                                         // "json_parse", "pdf_pages_min", ...
    passed: bool,
    message: String,                                                // legível pelo próximo stage
}
```

### Critério de "concluído" por stage

`stage.state == RunState::Completed AND stage.output_artifact.is_some() AND stage.output_hash.is_some()`. Stages com `validation = Some(_)` precisam de `validation.passed == true`. Stages com `validation = None` não exigem validação explícita (a aceitação é do próprio modelo chamador do próximo stage).

### Critério de "concluído" do pipeline

Todos os stages `Completed` + o último stage tem `output_artifact = Some(_)`. Quando isso vale, `MultimodelRun.state` transiciona para `MultimodelState::Completed` e o `final_artifact = last_stage.output_artifact`.

### Persistência entre etapas (PROMPT MESTRE §14.5)

`PipelineRepo` (novo, no `frederico-storage`) persiste o run após cada stage terminar (qualquer estado terminal). Tabelas:

- `multimodel_runs` (id, parent_run_id, mode, state, created_at, updated_at).
- `multimodel_stages` (id, run_id, seq, model_id, provider_id, state, input_artifact_id, output_artifact_id, input_hash, output_hash, cost_microcents, tools_used_json, validation_json, started_at, finished_at).
- `multimodel_artifacts` (id, run_id, stage_id, kind, content_ref, hash, size_bytes, created_at). `content_ref` aponta pro arquivo no workspace (validado pelo `Jail` da conversa).

Ao iniciar o app, runs em estado `Running`/`Streaming`/`WaitingToolCall` são carregados e a UI oferece "retomar pipeline interrompido". Runs em estado terminal não retomam — vão pro histórico.

### Reuso de stage quando input não mudou

Se o pipeline tem N stages e o usuário reabre o app sem ter mudado o input do stage K, os stages K+1..N podem ser pulados se o `output_artifact` do stage K é o mesmo (mesmo `output_hash`).

A regra é mecânica: ao iniciar (ou retomar) o pipeline, cada stage com `state == Completed AND output_hash == previous_output_hash` é pulado, e o próximo stage lê o `output_artifact` do storage em vez de chamar o modelo. **Stages pulados mantêm `state == Completed` e não incrementam `cost_microcents`** (o reuso é gratuito).

### Cancelamento hierárquico

Cancelar o `MultimodelRun` (botão "Parar" do usuário) cancela:

1. O `Run` em curso do stage atual (via `CancellationToken` herdado, ADR-0027).
2. Todos os `SubagentRun`s do stage atual (herança do CancellationToken).
3. Stages ainda não iniciados (marcados `Cancelled` direto, sem chamar o modelo).
4. O `PipelineRepo` grava o estado final.

Stages já **concluídos** mantêm `state == Completed` (não são revertidos — o trabalho feito não se desfaz). É o mesmo princípio do "rollback é caro, opt-in" do Write-Ahead Log do SQLite.

### Validação por stage

Um stage pode declarar um `validator` que roda após o modelo terminar, antes do `output_artifact` ser consumido pelo próximo stage. Sem validator declarado, o `output_artifact` segue direto. Com validator, o `MultimodelStage.validation` é `Some(ValidationResult)`, e o próximo stage só lê se `passed == true`. Se `passed == false`, o stage é marcado `Failed` com `validation.message` no `RunEvent` e o pipeline para (a menos que o usuário intervenha).

Exemplos de validator (especificação, não implementação):

- `"json_parse"` — o output_artifact deve ser JSON válido.
- `"pdf_pages_min:N"` — o output_artifact (PDF) deve ter N+ páginas.
- `"schema:name"` — o output_artifact deve casar com schema TOML name.
- `"tool_required:tool_id"` — o stage deve ter usado o tool_id pelo menos uma vez.

A lista de validators é fixa no início da Fase 6 (não extensível pelo usuário na v1).

## Modos documentados e fora de escopo da Fase 6

Esta seção documenta **o que cada modo é** (pra quem for atacá-lo depois ter uma âncora), mas **não decide** os detalhes — eles ficam pra quando o PR do modo entrar. A Fase 6 não os entrega.

### Comparação (adiado)

Modelos respondem em paralelo (até 4 na v1, fora do teto de 8 subagentes do ADR-0027 porque não são subagentes — são cards paralelos do run raiz). Cartões do mesmo tamanho (métrica TBD: tokens de output, caracteres, parágrafos? — decisão do PR). Autoria preservada por cartão. Custo individual por cartão. Síntese opcional (um 5º modelo consome os 4 e emite resumo). O usuário pode aceitar a síntese ou ignorar.

### Conselho (adiado)

Modelos analisam o mesmo pedido. Coordenador identifica consenso (definição TBD — provavelmente interseção de afirmações factuais, com lista explícita de divergências). Mostra respostas originais + consenso + divergências + síntese separada. Autoria preservada.

### Debate (adiado)

Rodadas limitadas (N=3 default). Papéis definidos (proponente, crítico, sintetizador). Orçamento por rodada. Resumo entre rodadas (cada rodada vê o resumo das anteriores, não o histórico completo — política de contexto TBD). Conclusão por coordenador (papel fixo, modelo configurável).

## Não-objetivos (v1)

- Votação de modelos como mecanismo de decisão (consenso ≠ verdade — ADR-0028 §Contexto).
- Modelo que avalia modelo fora de um pipeline declarado (a UI não esconde origem da síntese).
- Multimodelo com mais de 4 modelos em paralelo na v1 (limite de orçamento e UI). **Nota:** o teto de 8 subagentes do ADR-0027 é **separado** deste limite — Comparação tem até 4 cards paralelos do run raiz, mais 8 subagentes herdados por eles (potencialmente).
- Persistência cross-device de pipelines multimodelo (mesma razão do `PROMPT MESTRE` §15).
- Auto-tunar o pipeline (escolher quantos stages, quais modelos, baseado em histórico). O usuário declara o pipeline; o Frederico executa.
- Stages com retry automático. Falha de stage = `Failed`; o pipeline para. Retry é decisão do usuário.
- Modificar o `MultimodelRun` durante execução (adicionar stage, remover stage, trocar modelo no meio). Mutação só antes de iniciar ou após `Cancelled`/`Failed`.

## Decisões

- [ADR-0028](../decisions/0028-pipeline-sequencial-multimodel.md) — Pipeline Sequencial é o único modo da Fase 6; Comparação/Conselho/Debate ficam explicitamente fora do escopo.
- [ADR-0029](../decisions/0029-run-event-journal-replaces-message-event.md) — `RunEvent` com `seq` monotonicamente crescente é a fonte de verdade do journal de transições do `MultimodelRun` (não `MessageEvent`).
- Nenhuma outra nova nesta versão. Modos adiados (Comparação, Conselho, Debate) ganham ADRs próprios quando forem atacados.

## Pendências

- **Critério de "cartões do mesmo tamanho"** da Comparação: definição de tamanho, métrica, e o que fazer quando uma resposta estoura. Trabalho do PR de Comparação.
- **Algoritmo de consenso do Conselho**: como caracterizar "divergência" e "consenso" sem alucinar relação entre respostas independentes. Trabalho do PR de Conselho.
- **Política de contexto do Debate**: quanto manter entre rodadas, como resumir, quando parar. Trabalho do PR de Debate.
- **UI dos 3 modos adiados**: linha do tempo, autoria, versões, comparação, resposta consolidada separada — desenho visual fica pra fase de UI (Etapa 6 da Fase 6 fecha o esqueleto do Modo Equipe; os 3 modos plugam depois).
- **Lista fechada de validators** da §"Validação por stage": decisão de produto sobre quais validators entram na v1.

## E2E de cobertura planejado por etapa

Conforme o gate de E2E (ADR-0026) e a regra de "cada etapa nomeia seu E2E desde a Etapa 1", os testes abaixo **serão entregues** pelas etapas correspondentes. Eles ainda não existem; a Etapa 1 é o que declara o alvo.

| Etapa | E2E de cobertura (alvo) | Passo CI |
|-------|--------------------------|----------|
| 1 | — (sem código) | — |
| 2 | `crates/e2e/tests/e2e_portao_transicao_e2e.rs::run_executor_rejects_invalid_transition_through_orchestrator`, `::run_event_seq_monotonic_through_orchestrator`, `::valid_transition_persists_in_run_event_journal`, `::recovery_loads_state_from_run_event_journal` | `cargo test --workspace` |
| 3 | `crates/e2e/tests/e2e_specialist_registry_e2e.rs::registry_loads_specialists_from_catalog`, `::permission_set_inherited_from_assistant_project_user`, `::specialist_unknown_id_returns_structured_error`, `::effective_permission_set_is_subset_of_parent` | `cargo test --workspace` |
| 4 | `crates/e2e/tests/e2e_subagent_e2e.rs::subagent_runs_with_reduced_permissions`, `::subagent_inherits_cancellation_token`, `::subagent_budget_discounted_from_parent_in_real_path`, `::subagent_explosion_cap_8_rejects_ninth`, `::subagent_depth_cap_2_rejects_grandchild`, `::subagent_budget_sum_never_exceeds_parent` | `cargo test --workspace` |
| 5 | `crates/e2e/tests/e2e_pipeline_sequencial_e2e.rs::pipeline_two_stages_passes_artifact`, `::pipeline_survives_app_restart`, `::pipeline_skips_stage_when_input_artifact_unchanged`, `::pipeline_stage_cost_tracked`, `::pipeline_cancel_propagates_to_current_stage_and_skips_remaining` | `cargo test --workspace` |
| 6 | `crates/e2e/tests/e2e_team_mode_ui_e2e.rs::team_mode_sidebar_renders_specialists`, `::memory_control_in_ui` | `cargo test --workspace` |

A tabela acima é **alvo declarado na Etapa 1**. Conforme cada etapa mergea, a coluna "E2E de cobertura" e "Passo CI" do `docs/status.md` linha 33 (Fase 6) é atualizada no mesmo commit (REGRA §3.4 do `REGRAS-DO-PROJETO.md`).

## Referências

- `PROMPT MESTRE` §14 (multimodelo), §14.4 (Pipeline Sequencial como modo prioritário), §14.5 (persistência entre etapas), §32.49 (critério de aceite)
- [`agent-state-machine.md`](./agent-state-machine.md) (estados do `Run` reusados pelo `MultimodelStage.state`)
- [`subagent-architecture.md`](./subagent-architecture.md) (subagentes usados pelos stages; herança de Budget, CancellationToken, permissões)
- [`chat-and-providers.md`](./chat-and-providers.md) (catálogo de modelos e provedores)
- [`testing-strategy.md`](./testing-strategy.md) (cobertura E2E do caminho de produção)
- [ADR-0028](../decisions/0028-pipeline-sequencial-multimodel.md) — Pipeline é o único modo da Fase 6
- [ADR-0029](../decisions/0029-run-event-journal-replaces-message-event.md) — `RunEvent` substitui `MessageEvent` como journal de transições
