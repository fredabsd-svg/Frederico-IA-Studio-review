# 0039 — Escopo e etapas da Fase 8 (Modo Desenvolvedor integrado)

> **Errata de fato, 2026-08-17 (Etapa 2).** O §Contexto e o §D2 afirmam que "a cobertura noturna deste repositório **nunca** funcionou". Medido: o `CI Nightly` rodou **verde** em 2026-08-03 (run `30794191640`) e 2026-08-04 (`30884415872`). A primeira falha é de 2026-08-05, no commit `d41b182` (PR #27, Fase de Ligação Etapa 3), que acrescentou o passo `E2E memory real` exigindo o secret `OPENROUTER_API_KEY` — secret que nunca foi criado. **A decisão do §D2 não muda** (o noturno continua precisando de um run verde citável antes de a fase fechar), e a severidade tampouco: um passo que não podia passar ficou 12 dias sem ninguém notar. O que muda é o diagnóstico — o problema não é um pipeline que nunca funcionou, é uma regressão datada, com autor conhecido e conserto de uma linha de configuração.

## Contexto

O ADR-0032 §D2 definiu o que a Fase 8 absorve quando a Fase 7 ficou restrita à execução isolada:

> Git local portátil, GitHub auth + push + PR, projetos, diff viewer, checkpoints, copiloto (Nino) e tarefas.

A Fase 7 fechou em 2026-08-14 e o pré-requisito `8 → 3 + 4 + 6 + 7` está satisfeito. Falta o que o ADR-0032 §D5 chama de "a hora de cortar escopo": a Etapa 1, antes do código.

E há escopo demais. A lista do §D2 tem seis blocos de natureza diferente, e repetir na Fase 8 o erro que a Fase 7 corrigiu — juntar naturezas incompatíveis sob um critério de done único — é o risco óbvio. Além disso, a Fase 7 deixou pendências nomeadas que caem naturalmente aqui (cache de aprovação por escopo, `ExecDeps` per-run), e a revisão de 2026-08-16 acrescentou duas descobertas que também pedem lugar: o catálogo de modelos estático (ADR-0006) e o CI noturno que nunca rodou verde.

Um dado pesa sobre todo o resto. O critério de done da Fase 8 no roadmap é **"PR criado pelo app (E2E noturno — `#[ignore]`)"**, e a cobertura noturna deste repositório **nunca funcionou**: o workflow `CI Nightly` acumula 12 falhas consecutivas desde 2026-08-05, todas por secret ausente. O ADR-0026 §D2 já classificava cobertura noturna como "mais fraca por natureza"; na prática ela era inexistente. Fechar a Fase 8 sobre essa base é assinar em branco.

## Decisões

### D1 — A Fase 8 entrega Git, GitHub e projetos/checkpoints. Copiloto (Nino) sai.

O corte segue a mesma lógica do ADR-0028 §D1 (Fase 6) e do ADR-0032 §D1 (Fase 7): uma fase, um critério de aceite verificável.

**Entra:**

1. **Git local portátil** — `crates/git-engine/`, sem depender de `git` no PATH. Status, diff, branch, commit, log.
2. **GitHub** — `crates/github-engine/`, token no DPAPI (mesma trilha do `WindowsCredentialStore` da Fase 2), push e criação de PR, com matriz de autorização por repositório/branch.
3. **Projetos e checkpoints** — `crates/project-engine/`, workspace-as-project com metadados, mais checkpoints nomeados estendendo o `CheckpointRepo` da Fase 3 Etapa 4.
4. **Diff viewer** — frontend React consumindo o `git-engine`.

**Sai — vai para a Fase 9 ou posterior:** o **copiloto (Nino) e tarefas** (`PROMPT MESTRE` §24.1). É a terceira natureza da lista: não é primitiva local nem integração externa, é produto — sugestão, acessibilidade, refinamento de interação. Tem critério de aceite qualitativo ("cumpre os 6 itens do §24.1") que não fecha por teste, e o ADR-0032 §D4 é explícito sobre o destino de item assim colado numa fase: vira pendência envergonhada. Fica como linha do roadmap com PR e E2E próprios.

**A frase literal para os specs e o `status.md`:** "A Fase 8 entrega Git, GitHub, projetos, checkpoints e diff. O copiloto (Nino) e tarefas são documentados no roadmap e adiados."

### D2 — "PR criado pelo app" só vale como critério de done com o noturno provado verde

O critério de done herdado do roadmap exige um E2E que crie um PR de verdade no GitHub — rede, secret e serviço externo, portanto `#[ignore]` noturno por natureza. A REGRA §3.3 já obriga twin determinístico para cobertura só-noturna. Este ADR acrescenta uma pré-condição:

**A Fase 8 não é promovida a `concluída` enquanto o workflow `CI Nightly` não tiver ao menos um run verde registrado no `status.md`, com número do run.**

Não é burocracia acrescentada: é a REGRA §2.2 aplicada ao workflow que a fase depende. Um E2E noturno num pipeline que nunca completa não é cobertura fraca — é cobertura nenhuma com aparência de cobertura. A Etapa 2 desta fase começa por consertar isso, e é por isso que ela vem antes do Git.

### D3 — Etapas da fase

| Etapa | Foco | Bloqueia |
|---|---|---|
| **1 — Planejamento** | Este ADR + 0038 + 0040-0043, specs novos, `releases/fase-8/README.md`, `status.md`, `CHANGELOG.md`. Sem código. | — |
| **2 — Noturno verde + fundação de credencial** | Consertar o `CI Nightly` (secret ausente) e provar verde. Estender a trilha DPAPI para token de serviço. | Etapa 5 |
| **2b — Fechar o §D5 do ADR-0037 e reclosar a Fase 7** | `exec.shell` volta ao catálogo (ou é aposentado por ADR novo), e a Fase 7 volta a `concluída`. Ver §D6. | Promoção da Fase 8 |
| **3 — `git-engine`** | Crate novo, puro, `unsafe_code = "forbid"`. Status, diff, log, branch, commit sobre repositório local. | Etapa 4, 6 |
| **4 — Projetos e checkpoints** | `crates/project-engine/` + checkpoints nomeados sobre o `CheckpointRepo` existente. | — |
| **5 — `github-engine`** | Auth, push, `create_pr`, matriz de autorização. E2E noturno + twin determinístico. | — |
| **5b — Identidade visual, acessibilidade e sugestões estáticas** | Sistema de design com tokens, estados de foco, WCAG 2.1 AA automatizável, sugestões estáticas de estado vazio. Sem tela nova. Ver [ADR-0045](0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md). | Etapa 6 |
| **6 — Diff viewer + UI de projeto** | Frontend consumindo `git-engine` e `project-engine`. | — |
| **7 — Fechamento** | Pendências herdadas da Fase 7 (D4), catálogo dinâmico (ADR-0043), promoção da fase. | — |

Cada etapa da 3 em diante entrega **pelo menos um teste de negação** — o que a ferramenta recusa, não só o que ela faz. Regra herdada da Fase 7, que a validou: foi um teste de negação que expôs o escape de path do sandbox na Etapa 4 daquela fase.

A Etapa 5b foi acrescentada em 2026-08-17, fora da Etapa 1 de planejamento, pelo [ADR-0045](0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md). Ela precede a Etapa 6 porque a 6 é a única etapa de frontend restante da fase — sistema de design que chegasse depois obrigaria a construir o diff viewer e a UI de projeto duas vezes.

### D4 — Pendências herdadas da Fase 7 entram nomeadas, não por osmose

Três pendências da Fase 7 apontam explicitamente para a Fase 8 e são adotadas aqui, cada uma com dono:

- **Cache de aprovação por escopo** (`OneTurn`/`OneSession`) — não existe em código nenhum; toda tool com `requires_user_approval` pede aprovação a cada invocação. Etapa 7.
- **`ExecDeps` per-run** — hoje é process-wide, o que impede o layer de assistant de entrar na interseção da allowlist de rede. Etapa 7.
- **Filtro de rede no nível de processo (WFP/WDAC)** — fecharia DNS exfiltration e o bypass por socket raw. **Não entra na Fase 8**: é trabalho de kernel/política do Windows, de natureza diferente de tudo aqui, e colá-lo nesta fase é o erro que o ADR-0032 documentou. Vai para o roadmap com ADR próprio.

### D5 — Catálogo de modelos dinâmico entra como decisão da fase, não como implementação dela

A revisão de 2026-08-16 confirmou que o catálogo é estático por decisão (ADR-0006), não por esquecimento: 13 modelos embutidos no binário, zero HTTP no crate, `/models` inexistente no código. O ADR-0043 revisita essa decisão. A implementação fica na Etapa 7, e o ADR-0043 é quem diz se ela acontece — porque a decisão precede o código (§1.6).

### D6 — A Fase 8 abre com a Fase 7 reaberta, e o pré-requisito é resolvido dentro dela

Enquanto esta Etapa 1 era escrita, o ADR-0037 (`docs/decisions/0037-exec-shell-fora-do-catalogo.md`, PR #56) tirou `exec.shell` do catálogo e **devolveu a Fase 7 a `em andamento`**. O `development-roadmap.md` fixa `8 → 3 + 4 + 6 + 7` e diz que pular pré-requisito exige ADR. Este é o ADR.

**O que a Fase 8 não pode fazer:** ignorar a pendência e seguir. Seria a régua sendo movida por conveniência, que é exatamente o que o ADR-0037 §"Alternativas" 3 rejeitou.

**O que ela faz:** absorve a pendência como etapa própria (2b, acima) — que é o que o próprio ADR-0037 sugere em §Consequências ("é razoável que D5 seja retomada como etapa dela"). A Fase 8 **não é promovida a `concluída`** enquanto a Fase 7 não voltar a `concluída`.

A justificativa para abrir mesmo assim, em vez de esperar:

1. **A pendência não bloqueia o trabalho desta fase.** Git, GitHub, projetos, marcos e diff não dependem de `exec.shell` em nenhum ponto. O pré-requisito existe para impedir construir sobre fundação inexistente — e a fundação que a Fase 8 usa (sandbox, runtimes, file ops, `PermissionSet`, rede) está entregue e não foi tocada pelo ADR-0037.
2. **A Etapa 1 é planejamento.** Não há código a construir sobre nada. O custo de estar errado aqui é reescrever documento, não desfazer implementação.
3. **Esperar não acelera o §D5.** O item 2 dele — a incompatibilidade dos binários MSYS2 com o rótulo de integridade baixa — pode reabrir o ADR-0031, e é trabalho de dias ou semanas. Congelar todo o Modo Desenvolvedor até lá troca uma pendência nomeada por uma fase parada.

**A trava permanece explícita e mecânica:** a promoção da Fase 8 tem agora duas pré-condições registradas — um run verde citável do `CI Nightly` (§D2) e a Fase 7 de volta a `concluída` (este §D6). As duas ficam na coluna "Pendências" da Fase 8 no `status.md`.

## Alternativas descartadas

1. **Manter o copiloto (Nino) na Fase 8**, como o ADR-0032 §D2 previa. Rejeitado: acrescenta uma natureza (produto/interação) a uma fase que já tem duas (primitiva local e integração externa autenticada), com critério de aceite que não fecha por teste. É o padrão que o ADR-0032 desmontou, reintroduzido.
2. **Começar pelo `git-engine`**, deixando o CI noturno para o fim. Rejeitado: o critério de done da fase depende do noturno, e descobrir na Etapa 7 que ele não funciona é descobrir tarde. A Fase 7 aprendeu isso duas vezes — o wiring do proxy e o DNS intercept passaram por prontos até serem exercitados de verdade.
3. **Usar `git` do PATH em vez de embutir** — mais simples, sem crate novo. Rejeitado pelo mesmo princípio dos runtimes portáteis da Fase 7 (ADR-0031): depender do ambiente da máquina do usuário torna o comportamento não reprodutível e o erro indiagnosticável. Detalhe no ADR-0040.
4. **Fase 8 só com Git local, GitHub em fase separada.** Rejeitado por fragmentação: Git local sem push é metade de uma ferramenta, e o ADR-0032 já rejeitou o mesmo tipo de corte (alternativa 4 daquele ADR). As duas partilham a mesma lente de revisão — integração com sistema de versionamento.

## Consequências

- **Fica mais fácil:** fechar a fase. O critério de done tem uma pré-condição verificável (noturno verde com run citável) em vez de depender de um pipeline que ninguém olhava.
- **Fica mais difícil:** entregar o copiloto na v1. É o preço declarado do corte, e o roadmap passa a dizê-lo em vez de a fase carregá-lo sem entregar.
- **A Etapa 2 é infraestrutura, não produto.** Uma fase que começa consertando CI parece atraso; começar por qualquer outra coisa é construir sobre um gate que não existe.
- **O `development-roadmap.md` muda** — Fase 8 sem copiloto, e o copiloto aparece como item próprio.
- **Sete etapas é mais do que a Fase 7 planejou e menos do que ela executou** (a Fase 7 planejou 7 e executou 9, com a 5+ e a 6+1 intercaladas). A expectativa registrada aqui: etapa intercalada não é falha de planejamento, é o plano encontrando o código.

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 1 da Fase 8.
