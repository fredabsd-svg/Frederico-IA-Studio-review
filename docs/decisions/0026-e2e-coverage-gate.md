# 0026 — Gate de E2E por fase (mapa explícito de cobertura + distinção PR/noturno)

## Contexto

A Etapa 5 da Fase de Ligação (PR #24) fixou o `crates/e2e/tests/` como o ponto de E2E do produto e introduziu 5 testes que atravessam o caminho de produção **sem subir a casca Tauri**. A Etapa 5.X (PR #25) usou o `e2e_docs_generate_with_real_worker` (#[ignore], `verify-external.ps1` step 7) como teste de sanidade do "caminho do produto". A Etapa 3 (PR #27) introduziu `e2e_memory_pipeline_end_to_end_deterministic` (CI de PR) + `e2e_memory_real_embeddings` (#[ignore], `ci-nightly.yml` step novo).

O `status.md` §Tabela lista a fase, nome, estado, evidência e pendências. **Não lista quais testes E2E cobrem a fase, nem onde rodam.** A consequência é que uma fase pode ser marcada `concluída` sem que o reviewer consiga verificar, de relance, se o caminho do produto está exercitado. O caso concreto foi a própria Etapa 5 fechar com 412+ testes Rust + 11 E2E do `external_doc_worker` verde, mas o `docs.generate` (componentes da Etapa 3 Etapa 5) só teve cobertura "unit" do worker — o caminho do produto (`WorkerInvoker` real → Python) só foi exercitado quando a Etapa 5 da Fase de Ligação construiu a composição.

O gate que o projeto precisa ("fase só fecha com caminho de produto atravessado por E2E") tem que ser **mecânico** (não revisão humana), **explícito** (teste nomeado por fase), e **distinguir onde o teste roda** (CI de PR vs noturno), porque:

- **CI de PR (todo PR)**: precisa ser **grátis, determinístico, sem dependência externa** (sem rede, sem custo, sem cota). Senão um PR de documentação pode ficar vermelho porque a API deu 429 — o que o PR #27 do Frederico acabou de descobrir (helper `memory_real_providers_or_skip!` panic com `OPENROUTER_API_KEY ausente` no `verify-external.ps1` step 8 de toda PR).
- **CI noturno (1x/dia, com secret)**: cobre o que o determinístico não cobre — que o **adaptador real** funciona contra a API real. O gate não pode tratar "só noturno" como cobertura equivalente à cobertura de PR: uma PR pode quebrar o teste noturno e ser mesclada horas antes de alguém ver.

A regra tem que ser o oposto do que aconteceu com o `documents: None → Full` da Etapa 2.B da Fase 3 (relatado no PR #25): defaults permissivos escondem o que nunca foi exercitado. **Mecanismo que nunca roda no caminho real parece funcionar até o dia que precisa; quando precisa, é tarde.**

## Decisões

### D1 — Coluna "E2E de cobertura" no `status.md`

O `status.md` §Tabela ganha **2 colunas novas** ao lado de "Evidência":

- **E2E de cobertura** — formato `path::nome` (ex.: `crates/e2e/tests/e2e_files_read.rs::files_read_e2e_through_chat_orchestrator`). Múltiplos testes por fase = múltiplas linhas na mesma célula, separadas por vírgula. Fase que não tem E2E de caminho (ex.: Fase 0, documental) marca `—` (regra explicitamente não-aplicável, com nota na Pendência).
- **Passo CI** — formato `cargo test --workspace` (roda em todo PR, `verify-external.ps1#N`, `ci-nightly.yml#nome-step`). A coluna explicita **onde** cada teste roda — o gate confere que o passo existe e é exercitado.

**Por que `path::nome` e não só o path:** o caminho sozinho permite renomear o teste sem o gate perceber. O `path::nome` quebra o gate se o teste for renomeado (regressão) ou apagado (perda de cobertura). É o tipo de proteção que a Etapa 5.X da Fase de Ligação pediu para o `WorkerToolDispatcher::allowed_paths`.

**Por que `Passo CI` separado de `E2E de cobertura`:** o mesmo teste pode rodar em vários passos (ex.: `e2e_docs_generate_with_real_worker` roda em `verify-external.ps1#7` E em `ci-nightly.yml`). A coluna explicita **a lista de passos** — o gate confere que pelo menos um passo do CI exercita o teste. Múltiplos passos = separados por vírgula.

### D2 — "Só noturno" é cobertura mais fraca

O gate trata "só noturno" como **cobertura mais fraca por natureza**:

- **Fase com E2E só noturno**: o `status.md` marca a fase como "concluída" **somente se** o teste noturno passou no último run noturno disponível. Sem isso, a fase fica "em andamento" com a pendência "E2E noturno X não rodou verde".
- **Fase sem E2E noturno nem de PR**: a fase pode fechar **somente se** a coluna "E2E de cobertura" for `—` com a nota "regra não-aplicável" explícita na Pendência. Sem essa nota, o gate falha.

**A regra existe porque:** "só noturno" é **probatório no momento da execução, mas não protege contra regressão entre runs noturnos**. Se uma PR quebra `e2e_memory_real_embeddings`, o CI de PR **não detecta** (não roda o noturno); o PR merge acontece; o próximo noturno é o único que descobre — e pode ser 24h depois. A Etapa 6 (gate) institui a regra de que o CI de PR **tem que ter cobertura de regressão** (determinístico ou com skip) pra cada fase `concluída`.

### D3 — `check-e2e-gate.ps1` (script)

Novo script `scripts/check-e2e-gate.ps1`. Entrada: `BASE_SHA` (env var) + `HEAD_SHA` (env var). Comportamento:

1. Lê `docs/status.md` e parseia a tabela. Extrai pares `(fase, estado, e2e_de_cobertura, passo_ci)`.
2. Para cada fase `concluída`:
   - Se `E2E de cobertura = —`, confere que a `Pendência` tem a nota "regra não-aplicável". Sem nota, **gate falha**.
   - Se `E2E de cobertura` tem `path::nome`, confere que:
     - O path existe no repo (`Test-Path`).
     - O arquivo contém o `#[test]`/`#[tokio::test]` de nome `nome` (regex simples).
     - O `Passo CI` nomeado existe no `.github/workflows/ci.yml` (regex do step) **ou** no `.github/workflows/ci-nightly.yml` **ou** é `cargo test --workspace` (implícito no step "Tests" do `ci.yml`).
   - Se o teste é `#[ignore]` e o único Passo CI é o nightly, **gate exige** que o `e2e_memory_real_embeddings` (ou similar) tenha o deterministic twin em `cargo test --workspace`. Sem o twin, **gate falha** com a mensagem "E2E X é só noturno; sem twin determinístico, regressão pode ir pro main sem detecção no CI de PR".
3. Para cada fase `em andamento` ou `não iniciada`:
   - Ignora (a regra não se aplica).
4. Falha com mensagem PT-BR listando os problemas exatos. Exit != 0.

**Por que PowerShell e não Node/Rust:** o `verify-external.ps1` e os outros guards (`check-core-purity`, `check-fase-5-untouched`) são PowerShell. Consistência. E `pwsh` é nativo no runner `windows-latest` do GitHub Actions.

### D4 — Integração no `ci.yml`

Novo step no `ci.yml` depois do step "Tests":

```yaml
- name: E2E coverage gate (REGRAS §3)
  env:
    BASE_SHA: ${{ github.event.pull_request.base.sha }}
    HEAD_SHA: ${{ github.event.pull_request.head.sha }}
  shell: pwsh
  run: ./scripts/check-e2e-gate.ps1
```

O step roda em **todo PR** (igual os outros guards — `check-core-purity`, `check-doc-impact`). Se o gate falha, o job vermelho (mesma forma dos outros 6 steps de guard).

**Por que o step precisa de `BASE_SHA` e `HEAD_SHA`:** o script parseia a tabela **atual** do `status.md` (que reflete o HEAD). Se o reviewer promove uma fase pra `concluída` no PR, a tabela do HEAD já tem a fase como `concluída` — e o gate confere o E2E de cobertura dela. Não precisa diff.

### D5 — `REGRA 3` no `REGRAS-DO-PROJETO.md`

Nova regra (paralela à REGRA 2):

- **Princípio**: o `status.md` é a fonte de verdade do estado real por fase. A "real" da cobertura E2E vem do mapa explícito (D1) + do gate (D3).
- **Procedimento**:
  - Fase que adiciona caminho de produção novo: o PR que fecha a fase adiciona o E2E de cobertura no `status.md` no mesmo commit.
  - Fase que mexe no caminho: o PR atualiza o `status.md` (mesma regra do §1.3).
  - Renomear/apagar teste nomeado: **proibido** sem ADR (mesma lógica do `WorkerToolDispatcher::allowed_paths` do PR #25).
- **Válvula de escape**: **nenhuma** (mesmo princípio do §19.6 — auditoria estrutural sem interruptor, e do `path safety fail-closed` do PR #25). A fase que precisa de cobertura não tem o que precisa → **gate falha** → o PR não fecha. A negociação é: ou adiciona E2E de cobertura, ou não promove pra `concluída`.

**Por que sem válvula de escape:** o user sinalizou explicitamente que o analog do "label `no-e2e-needed`" (proposto na conversa inicial) seria usado na primeira sexta-feira apertada e nunca mais sairia. Mesmo princípio do `no skip` do `path safety` — **se o gate pode ser desligado por label, ele é desligado e ninguém percebe**.

### D6 — `testing-strategy.md` §Fronteira atualizado

O spec `docs/architecture/testing-strategy.md` §3 "Fronteira do que os E2E cobrem" ganha 2 parágrafos novos:

- A distinção **CI de PR vs nightly** (determinístico no PR, real no noturno, com a regra de que PR sem twin determinístico não cobre regressão).
- A regra de que o **mapa de cobertura** (D1) substitui a lista manual do §3 atual (que lista os 5 E2E da Etapa 5 + o real do PR #24; vira `path::nome` por fase com `Passo CI`).

## Consequências

- O `status.md` linha 5b (Fase de Ligação) promove pra `concluída` quando este PR merge — 6/6 etapas (1, 2A, 2B, 3, 4, 5 + Etapa 5.X patch-allowed-paths). Etapa 6 (gate) fecha a fase por construção: o gate existe e é exercitado.
- As Fases 0, 1, 2, 3, 4, 5 do plano mestre ganham `E2E de cobertura` + `Passo CI` retroativos. **Fase 1, 2, 3, 4**: a maioria coberta pelos 5 E2E da `crates/e2e/tests/` (PR de cobertura tem que nomear **exatamente** o que cada teste cobre — Fase 1 (Tauri+SQLite) coberta indiretamente, Fase 3 (motor) coberta por `e2e_files_read` + `e2e_degradation_declared`, Fase 4 (memória) coberta pelo `e2e_memory_pipeline_end_to_end_deterministic` do PR #27). **Fase 5 (documentos)**: coberta pelo `e2e_docs_generate_with_fake_worker` + `e2e_docs_generate_with_real_worker` (que é nightly). **Lacunas nomeadas** (cancelamento, recovery, approval, reload durante execução §32, provedor real com streaming) ficam na Pendência da fase — não como "—", como **lacunas explícitas** que viram trabalho da próxima fase (Fase 6 do plano mestre) ou da Etapa 6.X da Fase de Ligação.
- O `check-e2e-gate.ps1` precisa de **meta-teste próprio** (teste de unidade que simula cenários: teste renomeado, teste apagado, teste nomeado sem step, teste só-noturno sem twin determinístico, etc.) — sem meta-teste, o gate é código não-exercitado (mesma armadilha do PR #25).
- O CI noturno pode falhar com frequência até a secret `OPENROUTER_API_KEY` ser configurada (helper panic visível). Isso é **intencional** — alinha com a regra "mecanismo que nunca roda no caminho real parece funcionar até o dia que precisa". A configuração da secret é trabalho do user; enquanto não configurar, o noturno fica vermelho em todo run (visível).

## Alternativas consideradas

1. **Válvula de escape com label `no-e2e-needed`** (proposto na conversa inicial da Etapa 6). **Rejeitado** pelo user explicitamente: "label seria usado na primeira sexta-feira apertada e nunca mais sairia". Mesmo princípio do `no skip` do `path safety` do PR #25. A regra é: o gate é mecânico, sem negociação.
2. **"Só noturno" = mesma cobertura que CI de PR.** **Rejeitado** pela mesma razão: a regressão pode ir pro main sem detecção no CI de PR. O `e2e_memory_real_embeddings` é exemplo: sem `OPENROUTER_API_KEY` configurada, o noturno **falha** (helper panic) — mas enquanto o user não configurar, a Etapa 3 está provada no determinístico e **presumida** no real. "Só noturno" é cobertura fraca; o nome da fase continua "concluída" com a Pendência nomeada.
3. **Auto-criar twin determinístico a partir do teste real** (geração de código). **Rejeitado** por complexidade: o twin determinístico exige **decisão** sobre os vetores fixos (paráfrase vs distrator), o que é trabalho de design, não de geração. O `e2e_memory_pipeline_end_to_end_deterministic` foi escrito à mão com vetores escolhidos pra cosine ~0.99 entre paráfrase e ortogonal pro distrator — não dá pra gerar automaticamente.
4. **Mapa de cobertura no `docs/modules/e2e.md` (em vez de `status.md`)**. **Rejeitado** porque o `status.md` é a fonte de verdade do estado por fase (REGRAS §1.8). A cobertura E2E é parte do estado, não um documento à parte.

## Pendências

- **Configurar `OPENROUTER_API_KEY` no repo** (ação do user, não da IA) — o noturno passa de "sempre vermelho" pra "verdadeiro gate do adaptador real". Enquanto não configurar, o `e2e_memory_real_embeddings` é presumido, não provado.
- **Meta-teste do `check-e2e-gate.ps1`** — cobre os cenários: teste renomeado, teste apagado, teste nomeado sem step, teste só-noturno sem twin determinístico. Sem meta-teste, o gate é código não-exercitado.
- **Lacunas nomeadas** (cancelamento, recovery, approval, reload durante execução §32, provedor real com streaming, xlsx/pdf com Python real atravessando composição) — viram Pendência explícita na fase (não como `—`, como lacuna de cobertura a ser resolvida na próxima fase).

## Histórico de revisão

- 2026-08-04 — versão inicial. Convergência da Etapa 6 da Fase de Ligação (decisão do plano de 2026-08-04: "Etapa 6 antes da 3 e da 4"). Validação pelo user: a Etapa 3 fechou com PR #27 (CI verde); o PR 3 fecha a fase. Achado do PR #27: o teste real no `verify-external.ps1` step 8 fez CI depender de OpenRouter em toda PR — corrigido com split determinístico (PR) + nightly (real). Esse achado é a razão da D2 (cobertura fraca por natureza do só-noturno).
