# 0032 — Fase 7 é só execução isolada; Git, GitHub, diff, projetos e checkpoints viram Fase 8

## Contexto

O `docs/architecture/development-roadmap.md` (criado na Fase 0, carimbado `especificado` na Etapa 1 da Fase 1) lista a Fase 7 com a frase atual:

> **Fase 7** | Modo desenvolvedor | Projetos, arquivos, diff, sandbox, runtimes, Git, GitHub, testes, checkpoints
> **Critério de "done" (resumo)**: Sandbox isola execução, Git portátil embutido, PR criado pelo app

A reunião de coisas nessa lista é, na origem, um agrupamento por "tudo que o usuário faz como desenvolvedor". É um agrupamento legítimo do ponto de vista do **usuário final** (o que o app precisa fazer pelo usuário que programa), mas é um agrupamento desastroso do ponto de vista de **engenharia de entrega**: reúne naturezas incompatíveis que pedem infraestruturas de teste, de revisão e de critério de aceite diferentes.

A análise (do prompt do user, 2026-08-08, que confirma a decisão):

1. **Execução isolada** (sandbox + runtimes portáteis + `exec.python`/`exec.node`/`exec.shell` + `files.write`/`edit`/`list`) — primitivas locais, testes determinísticos em `cargo test --workspace`, cobertura E2E de PR via `crates/e2e/tests/`. Sem rede, sem segredo, sem serviço externo.
2. **Integração com serviço externo autenticado** (Git local portátil + GitHub auth + `push` + `create_pr`) — token no DPAPI, operação destrutiva remota, E2E precisa de rede + secret + serviço GitHub real, **só noturno** (regra D2 do ADR-0026: "só noturno" é cobertura fraca por natureza).
3. **UI de projeto** (projetos, diff, checkpoints, run UI) — frontend React, navegação, persistência, polimento visual. Não é a mesma revisão que sandbox ou integração com GitHub.

Colar as três na Fase 7 tem três consequências:

- **Critério de done impossível de fechar honestamente**: "PR criado pelo app" exige o caminho de produção inteiro de GitHub, que só roda noturno. A fase fica `em andamento` por meses ou é marcada `concluída` por pressão, com cobertura fraca não-declarada. É o mesmo padrão que a Etapa 7 da Fase de Ligação caiu ("fase que depende de fase futura nunca fecha") — só que em escala maior.
- **Duas lentes de revisão no mesmo PR**: o reviewer do sandbox lê o ADR de primitivas do Windows; o reviewer de GitHub lê o ADR de OAuth + DPAPI. Misturar em um PR obriga a duas conversas no mesmo código, nenhuma das duas com a profundidade que merece.
- **Infraestrutura de CI diferente**: o sandbox precisa de Windows runner com primitivas; GitHub precisa de secret `GITHUB_TOKEN` no repo + serviço real. Tentar validar os dois no mesmo gate cria um gate que cobre mal os dois.

A Fase 6 da Multimodelo mostrou (no ADR-0028) que a regra "uma fase, um modo, um critério de aceite verificável" é o que destrava fechamento. O mesmo princípio se aplica aqui, em escala maior.

## Decisões

### D1 — Fase 7 = execução isolada, full stop

A Fase 7 fecha quando **todos** os critérios abaixo são verdadeiros simultaneamente:

1. Sandbox (Job Object + Restricted Token + env zeroed + Jail como barreira primária, ADR-0031) está implementado e exercitado em produção.
2. Runtimes portáteis (Node + Python, ADR-0037 planejado, escopo na Etapa 3) estão embutidos e localizados pelo executor.
3. `exec.python` e `exec.node` estão no `ToolRegistry` (Etapa 4), sob sandbox, com aprovação obrigatória por invocação (ADR-0034).
4. `files.write` / `files.edit` / `files.list` estão no `ToolRegistry` (Etapa 5), sob Jail, com semântica de sobrescrita explícita (ADR-0035).
5. `exec.shell` está no `ToolRegistry` (Etapa 6), sob sandbox, com `Denylist` de comandos destrutivos + aprovação obrigatória (ADR-0034).
6. Rede do sandbox passa por proxy local negando por padrão (ADR-0033).
7. Mapa de E2E por etapa nomeado em `docs/status.md` (D2 do ADR-0026), com a regra de "teste de negação" (do prompt do user, 2026-08-08) — cada etapa da 2 em diante entrega **pelo menos um teste que prova o que o sandbox bloqueia**, não só o que ele permite.
8. `docs/status.md` marca Fase 7 como `concluída` com a coluna `E2E de cobertura` preenchida com `path::fn_name` e `Passo CI` apontando o lugar onde roda.

**A frase que entra no `status.md` e nos specs é literal:** "Fase 7 entrega execução isolada. Git, GitHub, diff, projetos e checkpoints são adiados para Fase 8."

### D2 — Git, GitHub, diff, projetos e checkpoints vão para Fase 8

A Fase 8 absorve tudo que **não é execução isolada local** mas é parte do "Modo Desenvolvedor" do roadmap:

- **Git local portátil** (sem PATH global) — implementação em `crates/git-engine/`, com git embarcado (libgit2 ou similar), status, diff, branch, commit.
- **GitHub auth + push + PR** — `crates/github-engine/`, com token no DPAPI (mesma trilha do `WindowsCredentialStore` da Fase 2 Hardening 1), scopes por repositório/branch, e a matriz de autorização estruturada que o `agent/githubAccess.js` do projeto anterior mostrou ser necessária (decisão de Fase 4 da Fase 6, no ADR-0024 da Fase de Ligação, transposta).
- **Projetos** (workspace-as-project, com metadados, dono, configuração) — `crates/project-engine/`, com UI de "abrir projeto", "listar projetos", "projeto padrão do usuário".
- **Diff viewer** (visualização de mudanças staged/unstaged, render de patch, side-by-side) — feature de frontend React, consome API do `git-engine`.
- **Checkpoints** (estado nomeado do workspace, retorno nomeado) — extensão do `CheckpointRepo` da Fase 3 Etapa 4, com nome humano, listagem, restore.
- **Copiloto (Nino) e tarefas** — o `PROMPT MESTRE` §24.1 lista 6 itens de copiloto que dependem de Fase 3 + 4 + 6 + 7. Entra na Fase 8.

**A frase que entra no `development-roadmap.md` é literal:** "Fase 8 (Modo Desenvolvedor integrado): Git portátil, GitHub, diff, projetos, checkpoints, copiloto (Nino). Depende de Fase 3 + 4 + 6 + 7. GitHub E2E é `#[ignore]` (noturno) por natureza (precisa de secret + rede + serviço real)."

### D3 — Roadmap atualizado reflete a nova fronteira

`docs/architecture/development-roadmap.md` ganha:

- Fase 7 com o título e escopo novos, critério de done **sem** "PR criado pelo app".
- Fase 8 com o título e escopo novos, critério de done incluindo "PR criado pelo app" e "diff viewer funcional".
- Nota explícita de que a re-numeração **invalida** o número de Fase 8 anterior (que era "Copiloto, tarefas e refinamento" — esse conteúdo agora é subdivisão da nova Fase 8).

A numeração de fases no roadmap **é sequência histórica, não voto de qualidade**. A Etapa 1 da Fase 6 já moveu "Comparação, Conselho, Debate" para fora (D2 do ADR-0028) com a mesma lógica.

### D4 — "Fora de escopo" vence "pendência envergonhada"

A distinção (já fixada pelo D2 do ADR-0028) é:

- **"Pendência envergonhada"** = item em `Pendências` da fase que ninguém lê nem ataca. É o que a Etapa 7 da Fase de Ligação tentou ser (até ser removida por "fase que depende de fase futura nunca fecha"). A Fase 7 não repete o erro.
- **"Fora de escopo"** = decisão consciente de que a fase não vai entregar, e cada item vira **PR próprio com E2E próprio** quando a infra subjacente estiver pronta. Itens viram linha do `development-roadmap.md` (em "Fase 8"), não da `Pendências` da Fase 7.

A frase nos specs e no `status.md` é literal: **"Git, GitHub, diff, projetos e checkpoints são documentados no spec correspondente e adiados para Fase 8. Não são critério de aceite da Fase 7."**

### D5 — Precedente da Fase 6 (ADR-0028 D1-D2) explicitamente invocado

A Fase 6 reduziu escopo na Etapa 1 (Planejamento) com a mesma lógica — "Pipeline Sequencial é o único modo entregue. Comparação, Conselho e Debate são adiados." (D1 do ADR-0028). A regra é: **a Etapa 1 da fase é a hora de cortar escopo, antes do código**. Cortar na Etapa 5 é tarde (commit de reverter vira pendência envergonhada, com a pressão de manter a feature porque "o código já está escrito").

A Fase 7 Etapa 1 é essa hora. A re-numeração do roadmap é o custo de cortar agora, e o custo de não cortar agora é o mesmo que a Fase 6 pagou (D2 do ADR-0028: "Comparação, Conselho e Debate são modos documentados no spec multimodel-architecture.md e adiados para incremento posterior").

## Consequências

- `docs/architecture/development-roadmap.md` é atualizado com a nova linha de Fase 7 e Fase 8. O cabeçalho do arquivo mantém `Fase correspondente: 1-9` (isenção de escopo global do §1.13).
- `docs/architecture/multimodel-architecture.md` (Fase 6, atualmente `implementado`) **não muda** — a Fase 6 já fechou.
- `docs/architecture/subagent-architecture.md` (Fase 6, atualmente `implementado`) **não muda**.
- O `docs/status.md` linha 34 (Fase 7) muda de "não iniciada" para "em andamento" com a Etapa 1 fechada. A coluna `E2E de cobertura` ganha o **plano por etapa** (mapa nomeado de testes previstos, com a regra de que cada etapa entrega o seu antes de fechar). A coluna `Passo CI` aponta `cargo test --workspace` (CI de PR) para os testes determinísticos, e `ci-nightly.yml` para os `#[ignore]` noturnos.
- O `docs/architecture/windows-sandbox-design.md` sai do estado `especificado` para `parcialmente implementado` quando a Etapa 2 entrar — ainda no escopo desta fase, agora aprofundado pelo ADR-0031 + este ADR + ADR-0033.
- A Etapa 1 (Planejamento) desta fase **fecha** com a entrega deste PR de docs — o mesmo formato do `pr-fase-ligacao-etapa-1.md` (5 commits encadeados + CHANGELOG). A Etapa 2 começa só depois que esta PR entra no main.

## Alternativas consideradas

1. **Manter o escopo original do roadmap** (Fase 7 = tudo: sandbox + Git + GitHub + diff + projetos + checkpoints + runtimes). Rejeitado pelo custo de fechamento (mesma análise do D2 do ADR-0028, aplicada em escala maior): uma fase que promete tudo, entrega nada. "PR criado pelo app" como critério de done com cobertura só-noturna é cobertura fraca disfarçada de critério forte.
2. **Fase 7 = sandbox + runtimes + file ops + exec (sem exec.shell)** — corte intermediário, mantendo Git/GitHub. Rejeitado pelo mesmo motivo: a fronteira "execução isolada local" é o que faz sentido; "execução isolada local + Git" mistura duas naturezas (uma é primitiva, outra é integração com serviço externo).
3. **Adiar execução isolada também** (Fase 7 = só Git/GitHub; sandbox vira Fase 8). Rejeitado porque o salto de risco da Fase 7 (escrita em arquivo do usuário, execução de processo arbitrário) é o que **exige** sandbox. Git/GitHub não destrava nada de risco novo (o agente pode chamar `bash` com `git push` de qualquer jeito, com ou sem ferramenta de Git — a ferramenta formaliza o que já era possível). A execução isolada é a fundação, não o adorno.
4. **Criar Fase 7.5 entre as duas** (Fase 7 = sandbox, Fase 7.5 = runtimes, Fase 8 = exec, Fase 9 = Git/GitHub). Rejeitado por fragmentação: o roadmap ganha granularidade que o projeto não precisa. As 3 naturezas se reduzem a 2 (execução isolada vs integração externa), e cada uma vira uma fase com critério de done verificável.
5. **Mover execução isolada para Fase 8 e manter Fase 7 = Git/GitHub/diff/projetos** (inversão). Rejeitado porque a execução isolada é pré-requisito para o Modo Desenvolvedor "honesto" (o usuário que programa quer rodar script, não só dar commit). Sem sandbox, o Modo Desenvolvedor vira "interface para GitHub" — feature, mas não o que o nome promete.

## Pendências

- **Renumeração de Fase 8** ("Copiloto, tarefas e refinamento" → "Modo Desenvolvedor integrado: Git, GitHub, diff, projetos, checkpoints, copiloto") quebra referências externas. Especificamente: o `docs/architecture/subagent-architecture.md` (Fase 6, atual) e o `docs/architecture/agent-state-machine.md` (Fase 6) podem ter referências à Fase 8 antiga. A Etapa 2 da Fase 7 varre e atualiza (REGRA 1.3, §1.11).
- **Frontmatter do `development-roadmap.md`** (Estado: especificado, Verificado contra o código em: —, Fase correspondente: 1-9) **não muda** — o roadmap é isento de escopo global pelo §1.13, e a re-numeração de Fase 7/8 é uma mudança do **planejado**, não do **real**. A linha 23 da tabela ganha a nota de re-numeração; o cabeçalho continua como está.
- **PR de Etapa 1 desta fase** é o primeiro PR da Fase 7, e a primeira entrada de CHANGELOG da fase. Segue o formato do `pr-fase-ligacao-etapa-1.md` (5+ commits encadeados: ADRs primeiro, specs depois, status/CHANGELOG por último).
- **"PR criado pelo app" sai do critério de done** — é uma mudança de plano que merece entrada própria no `CHANGELOG.md` da fase, para que o histórico registre a re-numeração (não vira "ah, sempre foi assim").

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7. Validação pelo user (via `ask_user`): "C — porque o corte que importa não é o número de etapas, é a fronteira da fase. Antes de decidir entre 9 e 5, tire Git/GitHub da Fase 7. Ele é de outra natureza: serviço externo autenticado, token no DPAPI, operação destrutiva remota (push, PR), E2E que precisa de rede e segredo — ou seja, noturno, com toda a discussão de cobertura fraca que vocês já tiveram. Colar isso na fase do sandbox mistura duas lentes de revisão e, pior, faz a fase depender de infraestrutura de CI diferente para poder fechar." A precedência é a mesma do ADR-0028 (Fase 6, D1-D2): cortar na Etapa 1 é o que destrava a fase.
