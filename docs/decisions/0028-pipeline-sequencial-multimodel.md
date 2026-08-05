# 0028 — Pipeline Sequencial como único modo multimodelo da Fase 6

## Contexto

O spec `docs/architecture/multimodel-architecture.md` (criado na Fase 0, carimbado `especificado`) lista 4 modos distintos, cada um com semântica própria:

- **Comparação** — modelos respondem em paralelo, cartões do mesmo tamanho, autoria preservada, custo individual, síntese opcional.
- **Conselho** — modelos analisam o mesmo pedido, coordenador identifica consenso, mostra divergências e respostas originais, síntese separada, autoria preservada.
- **Debate** — rodadas limitadas, papéis definidos, orçamento, resumo entre rodadas, controle de crescimento do contexto, conclusão por coordenador.
- **Pipeline sequencial** (modo prioritário, `PROMPT MESTRE` §14.4) — cada modelo trabalha com o **artefato real** do anterior, registra versão de entrada/saída, hash, custo, ferramentas e validação. Persistência do pipeline entre etapas (sobreviver a fechamento do app, §14.5).

O `PROMPT MESTRE` §14.4 marca o Pipeline Sequencial como **modo prioritário** da Fase 6. Os outros 3 modos têm decisões em aberto no spec (algoritmo de consenso do Conselho, política de contexto do Debate, critério de "cartões do mesmo tamanho" da Comparação) — decisões essas que o próprio spec lista em §"Aprofundar antes da Fase 6".

A Fase 6 (Multimodelo e subagentes) já carrega, no mínimo, dois trabalhos estruturais pesados: o **portão único de transição** (pendência do ADR-0025, herdada do PR #26) e a **máquina de subagentes com anti-"explosão"** (ADR-0027). Adicionar a essa lista 3 modos com semânticas próprias, cada um com decisões de design em aberto, é o caminho para a fase ficar **meses aberta** sem entregar.

A regra do projeto (Fase 3, lições de PR #22 e #25) é: **uma fase que promete tudo, entrega nada**. Os 4 modos no escopo da Fase 6 vira 1 modo pronto (Pipeline) + 3 modos parcialmente desenhados, sem critério de aceite verificável. Cada modo novo vira, depois, **um PR pequeno com E2E próprio** — desde que a infraestrutura esteja pronta.

## Decisões

### D1 — Fase 6 entrega apenas Pipeline Sequencial

A Fase 6 (Multimodelo e subagentes) fecha quando:
1. `MultimodelRun` + `MultimodelStage` + `PipelineRepo` (persistência entre etapas) estão em produção.
2. `input_artifact` hash + cost por stage + reuso quando hash não muda estão em produção.
3. O E2E de `crates/e2e/tests/e2e_pipeline_sequencial_e2e.rs` está verde, consumindo `build_chat_orchestrator` (não mock no `provider-engine`).
4. A UI do Modo Equipe mostra o pipeline (entrada, saída, hash, custo, status por stage).
5. O `docs/status.md` linha 33 marca `concluída` e o gate `check-e2e-gate.ps1` confere consistência.

**Os outros 3 modos (Comparação, Conselho, Debate) NÃO entram no critério de conclusão da Fase 6.** Esta é a única entrega de multimodelo desta fase.

### D2 — Comparação, Conselho e Debate ficam **fora de escopo** (não como "pendência envergonhada")

A distinção é explícita e importante:

- **"Pendência envergonhada"** = lista em `Pendências` da fase que ninguém lê nem ataca. É o que a Etapa 7 da Fase de Ligação tentou ser (até ser removida por "fase que depende de fase futura nunca fecha"). A Fase 6 não repete o erro.
- **"Fora de escopo"** = decisão consciente de que esta fase não vai entregar, e cada um vira **PR próprio com E2E próprio** quando a infra do Pipeline estiver pronta. O modo vira linha do `docs/development-roadmap.md` (sob "Fases Futuras" ou "Incrementos pós-Fase 6"), não da `Pendências` da Fase 6.

A frase que entra nos specs e no `status.md` é literal: **"Comparação, Conselho e Debate são modos documentados no spec `multimodel-architecture.md` (v1, §Modo X) e adiados para incremento posterior. Não são critério de aceite da Fase 6."**

### D3 — Cada modo novo vira PR próprio com E2E próprio

A infraestrutura do Pipeline (Etapa 5 da Fase 6) é o que destrava os outros 3 modos. Com ela em produção, **adicionar Comparação** (provavelmente o mais barato) é:
- 1 `MultimodelMode::Comparison` no enum
- 1 `MultimodelStage` paralelo em vez de sequencial
- 1 `e2e_comparacao_e2e.rs` (3 testes: `parallel_cards_same_size`, `cost_per_model_individual`, `synthesis_optional_preserves_authorship`)
- 1 entrada no `status.md` linha 33 (mudança de escopo: a fase continua `concluída`, o E2E só complementa)

O mesmo padrão serve para Conselho e Debate. **Nenhum dos 3 entra em PRs que mexam no código do Pipeline já mergeado** — eles plugam na infra existente. Esta é a regra de "com a infraestrutura do pipeline pronta, cada modo novo vira um PR pequeno" que motivou D1.

### D4 — Critério de "concluído" por estágio do Pipeline

Para fechar a Etapa 5 da Fase 6, cada `MultimodelStage` precisa ter:
- `state: RunState` (reusa o do `agent-engine`).
- `output_artifact: Option<ArtifactId>` — referência ao artefato produzido (não o conteúdo inline).
- `input_hash: Option<String>` — SHA-256 do artefato de entrada (pra pular stage se não mudou).
- `output_hash: Option<String>` — SHA-256 do artefato de saída.
- `cost_microcents: u64` — custo do stage, alimentado pelo `provider-engine` via `descriptor.cost_microcents(p, c)`.
- `tools_used: Vec<ToolId>` — ferramentas chamadas pelo stage (do `ToolRegistry`).
- `validation: Option<ValidationResult>` — opcional, definido quando o stage declara um validador (ex.: "esse JSON deve parsear", "esse PDF deve ter N páginas"). Sem validador declarado, `validation = None`.

**Critério de stage "concluído"** = `state == RunState::Completed AND output_artifact.is_some() AND output_hash.is_some()`. Stages com `validation = Some(_)` precisam de `validation.is_ok()`. Stages com `validation = None` não exigem validação explícita (a aceitação é do próprio modelo chamador do próximo stage).

**Critério de pipeline "concluído"** = todos os stages `concluídos` + último stage tem `output_artifact` que é o `final_artifact` do pipeline.

### D5 — Persistência entre etapas (PROMPT MESTRE §14.5)

`PipelineRepo` (novo, no `frederico-storage`):
- Tabela `multimodel_runs` (id, parent_run_id, mode, state, created_at, updated_at).
- Tabela `multimodel_stages` (id, run_id, seq, model_id, provider_id, state, input_artifact_id, output_artifact_id, input_hash, output_hash, cost_microcents, tools_used_json, validation_json, started_at, finished_at).
- Tabela `multimodel_artifacts` (id, run_id, stage_id, kind, content_ref, hash, size_bytes, created_at). `content_ref` aponta pro arquivo (workspace-relative, validado pelo `Jail` da conversa).

Após cada stage terminar (qualquer estado terminal), o `PipelineRepo` grava o estado completo do run + stages + artifacts. Ao iniciar o app, o `frederico-app::build_chat_orchestrator` carrega runs em estado `Running`/`Streaming`/`WaitingToolCall` e oferece continuação (botão "retomar pipeline interrompido" no Modo Equipe). Runs em estado terminal não retomam — vão pro histórico.

E2E de "sobrevive a restart" está em `crates/e2e/tests/e2e_pipeline_sequencial_e2e.rs::pipeline_survives_app_restart` — abre o storage, fecha, abre de novo, afirma que o pipeline retomou do último stage completo (não recomeçou do zero).

### D6 — Reuso de stage quando input não mudou

Se um pipeline tem 5 stages e o usuário edita o input do stage 1, os stages 2-5 normalmente precisariam re-roda. Mas se o usuário só edita metadata do stage 3 (não o artefato), os stages 4-5 podem pular se o `output_artifact` do stage 3 é o mesmo (mesmo `output_hash`).

A regra é: ao iniciar (ou retomar) o pipeline, cada stage com `state == Completed AND output_hash == previous_output_hash` é pulado, e o próximo stage lê o `output_artifact` do storage em vez de chamar o modelo. **Stages pulados mantêm `state == Completed` e não incrementam `cost_microcents`** (o reuso é gratuito, como esperado).

E2E em `::pipeline_skips_stage_when_input_artifact_unchanged` — pipeline roda, fecha, reabra com mesmo input, afirma que stages 2-N não rodaram (cost total não incrementou, log mostra "stage X pulado, output_hash unchanged").

### D7 — Cancelamento hierárquico do Pipeline

Cancelar o `MultimodelRun` (botão "Parar" do usuário) cancela:
1. O `Run` em curso do stage atual (via `CancellationToken`).
2. Todos os `SubagentRun`s do stage atual (herança D2 do ADR-0027).
3. Stages ainda não iniciados (marcados `Cancelled` direto, sem chamar o modelo).
4. O `PipelineRepo` grava o estado final.

Stages já **concluídos** mantêm `state == Completed` (não são revertidos — o trabalho feito não se desfaz; é o mesmo princípio do "rollback é caro, opt-in" do `Write-Ahead Log` do SQLite do projeto).

E2E em `::pipeline_cancel_propagates_to_current_stage_and_skips_remaining` — lança pipeline, cancela no meio do stage 2, afirma: stage 1 `Completed` (preservado), stage 2 `Cancelled`, stages 3-N `Cancelled` (nunca iniciados), `MultimodelRun.state == Cancelled`.

## Consequências

- `docs/architecture/multimodel-architecture.md` é aprofundado: §"Escopo da Fase 6" abre com "Pipeline Sequencial é o único modo entregue. Comparação, Conselho e Debate são adiados."
- `docs/architecture/development-roadmap.md` ganha §"Incrementos pós-Fase 6" listando Comparação, Conselho e Debate com referência a este ADR.
- `docs/status.md` linha 33 (Fase 6) marca `concluída` com a frase acima na Pendência.
- `crates/storage/migrations/0028_multimodel_runs.sql` (Etapa 5): 3 tabelas novas.
- `crates/execution-engine/src/multimodel/` (Etapa 5): `pipeline.rs`, `stage.rs`, `artifact.rs`, `validation.rs`. ~600 linhas estimadas.
- `crates/e2e/tests/e2e_pipeline_sequencial_e2e.rs` (Etapa 5): 5 testes, todos consumindo `build_chat_orchestrator`.
- `crates/app/src/composition.rs` ganha `build_multimodel_orchestrator` (Etapa 5).
- UI do Modo Equipe (Etapa 6) ganha o componente `PipelineView` que renderiza o grafo de stages.
- Os 3 modos adiadados viram **um PR cada** depois, com E2E próprio, plugando na infra do Pipeline. Nenhum dos 3 é prometido no `CHANGELOG.md` da Fase 6.

## Alternativas consideradas

1. **Os 4 modos no escopo da Fase 6** (proposta inicial do plano de 2026-08-05 antes da Etapa 1). Rejeitado porque (a) Fase 6 já carrega portão único + subagentes + anti-explosão, (b) Conselho e Debate multiplicam chamadas de API por rodada (o que a anti-explosão está sendo criada pra controlar), (c) cada modo tem decisões de design em aberto que podem conflitar (consenso do Conselho vs contexto do Debate vs "mesmo tamanho" da Comparação), (d) o histórico do projeto (PR #22, saga de rebase; Etapa 7 da Fase de Ligação, "fase que depende de fase futura nunca fecha") mostra que promessas amplas viram entrega zero.
2. **Pipeline + Comparação** (sugestão intermediária do plano de 2026-08-05). Rejeitado pela mesma razão: Comparação é barato **justamente porque** a infra do Pipeline está pronta. Forçar Comparação na Fase 6 obriga a projetar 2 modos em paralelo, e a UI do Modo Equipe precisa suportar os 2 layouts antes de qualquer um estar validado.
3. **Fechar a Fase 6 com Pipeline + adiamento de Comparação/Conselho/Debate como pendência envergonhada** (`Pendências` da linha 33). Rejeitado pela regra do PR #25 (mecanismo que nunca roda no caminho real parece funcionar até o dia que precisa) e pela regra do Etapa 7 da Fase de Ligação (fase que depende de fase futura nunca fecha). Pendência envergonhada = feature que não vai aterrissar. **Fora de escopo explícito** = feature que aterrissa como PR pequeno com E2E, depois.
4. **Adiar Pipeline para fase futura também** (só fechar subagentes na Fase 6). Rejeitado porque Pipeline é o modo prioritário do §14.4 e o que tem critério de aceite mais claro (§32.49). Sem Pipeline, a Fase 6 não tem entrega de multimodelo.

## Pendências

- **Comparação** (incremento pós-Fase 6): estimativa 1 PR, E2E de 3 testes. Sem ADR próprio ainda — quando entrar, ganha ADR `00XX-multimodel-comparison.md`.
- **Conselho** (incremento pós-Fase 6): estimativa 2 PRs (algoritmo de consenso + UI). Decisão de design "como caracterizar divergência sem alucinar relação entre respostas independentes" ainda em aberto.
- **Debate** (incremento pós-Fase 6): estimativa 2 PRs (política de contexto + UI). Decisão de design "quanto manter entre rodadas, como resumir, quando parar" ainda em aberto.
- **Critério de "cartões do mesmo tamanho"** da Comparação: definição de tamanho, métrica, e o que fazer quando uma resposta estoura — ainda em aberto no spec, vira trabalho do PR de Comparação.
- **Visualização na UI** (`PROMPT MESTRE` §23.3) dos 3 modos: grade consistente, linha do tempo, autoria, versões, comparação, resposta consolidada separada — design visual fica pra fase de UI (Etapa 6 da Fase 6 fecha o esqueleto; os 3 modos plugam depois).

## Histórico de revisão

- 2026-08-05 — versão inicial. Decisão da Etapa 1 da Fase 6. Validação pelo user: "Fase 6 mínima fecha; modos entram como incrementos com E2E próprio." A regra de "Pipeline é o único modo" é o que destrava a fase: sem isso, a Fase 6 repetiria o erro do Etapa 7 da Fase de Ligação.
