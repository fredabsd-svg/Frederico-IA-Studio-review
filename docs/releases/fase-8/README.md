# Fase 8 (Modo Desenvolvedor integrado): narrativas de release

<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 8 (Etapas 1 e 2b fechadas; Etapa 2 parcialmente entregue; Etapas 3, 4, 5, 5b, 6 e 7 não iniciadas)
-->

Índice das narrativas de processo da **Fase 8** — Git local portátil, GitHub, projetos, marcos e diff viewer. O escopo veio do [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md) §D2, quando a Fase 7 ficou restrita à execução isolada, e foi cortado pelo [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md).

**Não duplica o `CHANGELOG.md`**, que registra o efeito pro usuário (§1.7). Aqui mora a história técnica.

## Índice

| Etapa | Arquivo | Assunto |
|---|---|---|
| **1 — Planejamento** | este README + os 6 ADRs | 6 ADRs (0038-0043) + 3 specs novos + `status.md` + `CHANGELOG.md`. Sem código. |
| 2 — Noturno verde + credencial | (a ser escrito) | Consertar o `CI Nightly` e prová-lo verde; estender a trilha DPAPI para token de serviço. **Fechada em 2026-08-17:** credencial de serviço no DPAPI (PR #60) + noturno verde em `main` (run `32063550384`, PR #67). |
| **2b — Fechar o §D5 do ADR-0037** | [etapa-2b-narrativa.md](etapa-2b-narrativa.md) | **Fechada.** `exec.shell` de volta ao catálogo com resolução própria de programa (ADR-0044); Fase 7 reclosada. Herdada, não escolhida — ver abaixo. |
| 3 — `git-engine` | (a ser escrito) | Spike de biblioteca + crate local (status, diff, log, branch, commit). |
| 4 — Projetos e marcos | (a ser escrito) | `crates/project-engine/` + marcos nomeados sobre o `git-engine`. |
| 5 — `github-engine` | (a ser escrito) | Auth, push, `create_pr`, matriz de autorização. Noturno + twin. |
| 5b — Identidade visual e acessibilidade | (a ser escrito) | Tokens, estados de foco, WCAG 2.1 AA automatizável, sugestões estáticas. [ADR-0045](../../decisions/0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md). |
| 6 — Diff viewer + UI de projeto | (a ser escrito) | Frontend consumindo os dois crates. |
| 7 — Fechamento | (a ser escrito) | Pendências herdadas da Fase 7 + catálogo dinâmico (ADR-0043) + promoção. |

## Por que a fase começa consertando CI

A Etapa 2 é infraestrutura, não produto, e vir primeiro parece atraso. O motivo está no [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md) §D2.

O critério de done da fase, herdado do roadmap, é **"PR criado pelo app (E2E noturno — `#[ignore]`)"**. Em 2026-08-16 descobriu-se que o workflow `CI Nightly` acumulava **12 falhas consecutivas desde 2026-08-05**, todas determinísticas e pela mesma causa: o secret `OPENROUTER_API_KEY` não existe no repositório. O passo de pureza que roda depois dele nunca chegou a executar.

Ou seja: a cobertura que o [ADR-0026](../../decisions/0026-e2e-coverage-gate.md) §D2 já classificava como "mais fraca por natureza" era, na prática, **inexistente**. Construir o critério de aceite da fase sobre esse gate é assinar em branco.

A Fase 7 aprendeu a mesma lição duas vezes, do jeito caro: o wiring do proxy e o DNS intercept passaram por prontos até serem exercitados de verdade — um revelou 4 causas-raiz, o outro teve de ser removido inteiro. Aqui a ordem das etapas incorpora o aprendizado em vez de repeti-lo.

**E a ordem se pagou em 2026-08-17.** O secret foi criado, e o passo continuou vermelho — o diagnóstico de "falta configuração" cobria três defeitos, descobertos um de cada vez porque cada correção destravava o seguinte: a frase do teste era fato sobre terceiro e o classificador a recusava com razão; o teste fixava o escopo em `Profile` quando escopo é decisão do classificador; e, por baixo dos dois, um defeito de produção — o system prompt manda o modelo emitir `type` e o `NewMemory` exigia `type_`, então **o caminho real do classificador nunca funcionou ponta a ponta na história do repositório**, com os unitários verdes o tempo todo porque os fixtures deles foram escritos contra o nome Rust do campo, não contra o contrato com o modelo. O noturno ficou verde no run `32055019774` (ramo `fix/e2e-memoria-frase-e-escopo`) e, depois do merge do PR #67, no run `32063550384` sobre `main` — que é o citável pelo §D2. Se a Etapa 3 tivesse vindo primeiro, esse defeito só apareceria no fechamento da fase — ou não apareceria.

**A Etapa 2 fecha aqui, e com ela a última trava de promoção da fase.** A pré-condição 2 de 2 (§D6, Fase 7 de volta a `concluída`) caiu em 2026-08-16 com a Etapa 2b. O que separa a Fase 8 de `concluída` passa a ser só entrega: Etapas 3, 4, 5, 5b, 6 e 7.

## A fase abre com a anterior reaberta

Enquanto esta Etapa 1 era escrita, o ADR-0037 (PR #56) mediu o `exec.shell`, provou que a allowlist de comandos era contornável por qualquer separador do `cmd.exe`, tirou a ferramenta do catálogo e **devolveu a Fase 7 a `em andamento`**. O roadmap fixa `8 → 3 + 4 + 6 + 7`, e pular pré-requisito exige ADR — é o §D6 do [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md).

A resolução, em uma frase: **a Fase 8 abre, absorve a pendência como Etapa 2b, e não fecha antes da Fase 7 fechar.**

**Fechou no mesmo dia** ([ADR-0044](../../decisions/0044-exec-shell-com-resolucao-propria-de-programa.md), narrativa em [etapa-2b-narrativa.md](etapa-2b-narrativa.md)). `exec.shell` voltou ao catálogo com o `cmd.exe` fora do papel de resolvedor, e a Fase 7 voltou a `concluída` — cai a pré-condição 2 de 2. Fica a 1 de 2: o `CI Nightly` verde, que é a Etapa 2 — **fechada em 2026-08-17 com o run `32063550384` sobre `main`**. As duas travas caíram.

O item 2 do §D5 do ADR-0037 pedia uma resposta a um fato, e **o fato estava errado**: os comandos da allowlist antiga não morriam por incompatibilidade com o rótulo de integridade baixa; morriam porque o processo filho não recebe `PATH`. Cumprir aquele requisito ao pé da letra teria encolhido a allowlist para `echo` + `find` — e `find` também não funciona. A medição também achou um caminho de fuga que não estava em documento nenhum: o `cmd.exe` procura o programa no diretório corrente antes do `PATH`, e o diretório corrente é o workspace onde o `files.write` escreve.

O que sustenta abrir mesmo assim: nada no escopo desta fase — Git, GitHub, projetos, marcos, diff — toca `exec.shell`; a fundação que ela usa (sandbox, runtimes, file ops, `PermissionSet`, rede) continua entregue e intacta; e a Etapa 1 é planejamento, então não há código sendo construído sobre nada. Esperar não acelera o §D5 do ADR-0037, cujo item 2 pode reabrir o ADR-0031.

O que **não** foi feito: reescrever o critério de "done" da Fase 7 para não citar `exec.shell` e declarar a fase fechada. Seria mover a régua depois de medir — a alternativa 3 que o próprio ADR-0037 rejeitou.

## O que esta fase **NÃO** é

- **Não é o copiloto (Nino).** O ADR-0032 §D2 o previa aqui; o [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md) §D1 o tirou. É uma terceira natureza — produto e interação —, com critério de aceite qualitativo que não fecha por teste. Vai para o roadmap com PR e E2E próprios.
- **Não é cliente Git completo.** Sem rebase, sem amend, sem reescrita de histórico, sem force-push. O que a biblioteca não expõe, o produto não faz — e não cai para o `git` do sistema ([ADR-0040](../../decisions/0040-git-engine-biblioteca-e-fronteira.md) §D1).
- **Não fecha as lacunas de rede da Fase 7.** DNS exfiltration e bypass por socket raw exigem filtro no nível de processo (WFP/WDAC), de natureza diferente de tudo aqui. Colá-los nesta fase é o erro que o ADR-0032 documentou.
- **Não constrói o `CheckpointRepo`.** A tabela `checkpoints` existe desde a migração `0003` e nenhuma linha de Rust a usa. Nada a consome; construir por simetria é criar mais estrutura sem dono ([ADR-0042](../../decisions/0042-projetos-e-checkpoints-nomeados.md) §D1).

## As decisões desta Etapa 1

Seis ADRs, e três deles nasceram de erros encontrados no código durante o planejamento — não de escolhas de desenho:

1. **[ADR-0038](../../decisions/0038-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md)** — a trava do §1.13 obrigava spec novo a declarar implementação inexistente. A Fase 7 cedeu a ela no `f7d1ab3`. Corrigido antes de a Fase 8 repetir.
2. **[ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md)** — escopo e as 7 etapas. Copiloto sai; noturno verde vira pré-condição de fechamento.
3. **[ADR-0040](../../decisions/0040-git-engine-biblioteca-e-fronteira.md)** — Git por biblioteca, nunca `Command::new("git")`. A biblioteca é escolhida por spike, não por plausibilidade — precedente do ADR-0033, que cravou o DNS intercept sem experimento e teve de removê-lo.
4. **[ADR-0041](../../decisions/0041-github-auth-e-matriz-de-autorizacao.md)** — token no DPAPI, autorização como matriz, force-push ausente da API.
5. **[ADR-0042](../../decisions/0042-projetos-e-checkpoints-nomeados.md)** — corrige a premissa do ADR-0032 §D2: o `CheckpointRepo` que ele mandava estender nunca existiu.
6. **[ADR-0043](../../decisions/0043-catalogo-embutido-com-refresh-opcional.md)** — substitui parcialmente o ADR-0006. Catálogo embutido continua sendo a base offline; ganha refresh opcional e explícito.

Três correções de premissa em seis ADRs é o dado mais útil desta etapa, e a lição que ela deixa: **planejamento de fase parte do código, não do ADR anterior.** As três só apareceram porque a Etapa 1 varreu o repositório em vez de confiar no documento.

## Regras herdadas que continuam valendo

- **Teste de negação por etapa** (da Fase 7): cada etapa da 3 em diante entrega ao menos um teste do que a ferramenta **recusa**. Foi um teste de negação que expôs o escape de path do sandbox na Etapa 4 daquela fase.
- **PRs empilhadas:** PR aberta depois que a anterior entrou no main.
- **Capacidade incompleta é capacidade indisponível:** aplicada duas vezes na Fase 7 (`exec.*` fora do catálogo na Etapa 5+; DNS intercept removido inteiro na Etapa 7). Continua valendo.

## Referências

- [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md) — de onde veio o escopo
- [`development-roadmap.md`](../../architecture/development-roadmap.md)
- [`docs/releases/fase-7/README.md`](../fase-7/README.md) — a fase anterior e suas 7 lacunas nomeadas
- Specs desta fase: [`git-integration-architecture.md`](../../architecture/git-integration-architecture.md), [`github-integration-architecture.md`](../../architecture/github-integration-architecture.md), [`project-and-milestones-architecture.md`](../../architecture/project-and-milestones-architecture.md)

## Histórico de revisão

- 2026-08-16 — Etapa 1 (planejamento) fechada. 6 ADRs (0038-0043) + 3 specs novos + este README + `status.md` + `CHANGELOG.md`. Sem código Rust. Três dos seis ADRs corrigem premissas falsas encontradas no código durante o planejamento.
