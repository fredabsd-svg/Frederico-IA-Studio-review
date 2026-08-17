# 0045 — Etapa 5b da Fase 8: identidade visual do app, acessibilidade e sugestões estáticas

> **Estende o [ADR-0039](0039-fase-8-escopo-e-etapas.md) §D3** — acrescenta uma etapa à tabela da fase, sem alterar as demais.

## Contexto

O `development-roadmap.md` não tem fase para interface. A verificação de 2026-08-17 sobre o `main` (`c36377c`) confirma que a lacuna é estrutural, não um esquecimento de agenda:

- **24 specs em `docs/architecture/`, nenhum sobre interface.** Existem specs para máquina de estados, memória, sandbox, PDF, Excel, subagentes e modelo de ameaças. O frontend inteiro é governado por um `apps/desktop/src/styles.css` de 1.037 linhas sem documento que o descreva.
- **A Fase 9 não é o lugar.** O escopo declarado dela é "testes completos, segurança, assinatura, instalador, atualização, documentação, máquina limpa, versão estável", com critério de aceite nos 49 itens do `PROMPT MESTRE` §32. É endurecimento para release, natureza diferente.
- **"Acessibilidade" perdeu o dono documental.** O roadmap registra que o conteúdo retirado da Fase 8 era "Copiloto, tarefas, refinamento (Nino + sugestões + **acessibilidade**)". O item que chegou à tabela "Itens com fase própria a definir" chama-se apenas "Copiloto (Nino) e tarefas". A acessibilidade saiu junto, sem nome próprio e sem decisão que a tirasse.

### O que a medição mostrou

A leitura de `styles.css` e o cálculo de contraste (WCAG 2.1, fórmula de luminância relativa) em 2026-08-17:

| Medida | Valor no `main` |
|---|---|
| Tokens CSS existentes | 6 (`--bg`, `--bg-elev`, `--fg`, `--fg-dim`, `--accent`, `--border`) |
| Literais de cor no arquivo | 35, dos quais ~29 fora do `:root` |
| Tokens de espaçamento, tipografia, raio, sombra | 0 |
| Tamanhos de fonte distintos, ad-hoc | 10 (`0.75`/`0.8`/`0.85`/`0.9`/`0.95rem`, `0.95em`, `1`/`1.1`/`1.5rem`, `16px`) |
| Raios de borda distintos | 4 (3px, 4px, 6px, 8px) |
| **Ocorrências de `focus` em 1.037 linhas** | **0** |
| Regra de estilo para `<a>` | nenhuma — links usam o violeta padrão do navegador |

Contraste medido do tema escuro, todos contra `--bg` `#1a1a1a`:

| Par | Razão | AA (4.5:1) |
|---|---|---|
| `--fg` `#e8e8e8` | 14,20:1 | passa |
| `--fg-dim` `#9a9a9a` | 6,19:1 | passa |
| `--accent` `#d4a05a` | 7,44:1 | passa |
| erro `#c66` | 4,69:1 | passa |
| sucesso `#6c6` | 8,61:1 | passa |

Contraste do tema claro (bloco `prefers-color-scheme: light`):

| Par | Razão | AA (4.5:1) |
|---|---|---|
| `--accent` `#d4a05a` sobre `#fafafa` | **2,24:1** | **falha** |

### O diagnóstico que a medição impõe

**O problema não é a paleta.** O tema escuro passa em AA em todos os pares testados, com folga. A cor de destaque `#d4a05a` é um tom de latão — a identidade "Tinta & Latão" já está no app, sem nome e sem desenvolvimento. A impressão de pobreza vem de outro lugar:

1. **Ausência de estrutura.** Dez tamanhos de fonte e quatro raios escolhidos caso a caso não formam escala; formam ruído. Consistência é o que faz uma interface parecer projetada.
2. **Ausência de estados.** Zero `focus` em 1.037 linhas significa que quem navega por teclado não vê onde está. Isso é falha direta do critério 2.4.7 do WCAG 2.1 AA, não questão de gosto.
3. **Controles nativos não estilizados.** Os `<select>` renderizam com a cromagem clara do Windows dentro de um app escuro; é a descontinuidade mais visível do screenshot que originou esta decisão.
4. **Tema claro meio-feito.** O bloco `prefers-color-scheme: light` sobrescreve 5 dos 6 tokens e esquece `--accent`. Quem usa o Windows em modo claro recebe uma cor de destaque a 2,24:1.
5. **Pilha de fontes com entradas mortas.** `-apple-system`, `BlinkMacSystemFont`, `Roboto`, `Oxygen`, `Ubuntu` num produto declaradamente Windows-only.

## Decisões

### D1 — A etapa entra como **5b**, entre a Etapa 5 e a Etapa 6

As Etapas 3 (`git-engine`), 4 (`project-engine`) e 5 (`github-engine`) são crates de backend. **A Etapa 6 — "Diff viewer + UI de projeto" — é a única etapa de frontend restante na fase.** Um sistema de design que chegue depois dela obriga a construir aquelas telas duas vezes: uma sobre estilo ad-hoc, outra sobre os tokens.

Portanto 5b é o último momento em que o ganho de ordem ainda é de graça. Não é preferência de agenda: é a diferença entre a Etapa 6 consumir tokens desde a primeira linha ou gerar retrabalho conhecido de antemão.

O sufixo em letra segue o precedente da **Etapa 2b**, e é deliberadamente preferido à renumeração de 6 e 7 — renumerar quebraria referências no ADR-0039, no `development-roadmap.md` e no índice de `docs/releases/fase-8/README.md`, trocando um problema real por deriva documental silenciosa.

### D2 — O critério de aceite é mecânico. "Ficar bonito" não fecha etapa

Esta é a decisão que sustenta as outras. O ADR-0039 §D1 tirou o Copiloto (Nino) da Fase 8 com uma razão explícita: **critério de aceite qualitativo não fecha por teste.** Uma etapa chamada "melhorar o layout" reimporta exatamente esse defeito, com outro nome.

A etapa 5b fecha contra quatro portas verificáveis, todas no CI:

1. **Contraste.** Todo par token-texto sobre token-fundo usado na interface atinge 4,5:1 (3:1 para texto ≥ 24px ou ≥ 19px negrito), **nos dois temas**. Script versionado, roda no `check-docs.mjs` ou irmão dele.
2. **Foco visível.** Todo elemento interativo tem `:focus-visible` com indicador de no mínimo 3:1 contra o fundo adjacente. Zero violações do conjunto automatizável do WCAG 2.1 AA (eixo `axe-core` ou equivalente) nas telas existentes.
3. **Zero literal visual fora do `:root`.** Guarda por varredura, do mesmo formato da guarda de versão literal que o PR #58 criou — cor, tamanho de fonte, raio e espaçamento passam a existir só como token.
4. **Travessia por teclado.** Um E2E percorre a jornada principal — criar conversa, enviar mensagem, aprovar ferramenta — sem mouse, e falha se algum passo for inalcançável.

**O que explicitamente não é critério:** parecer bonito. A avaliação estética é do revisor humano, acontece na revisão do PR e **não bloqueia**. As quatro portas bloqueiam. Sem essa separação, a etapa não teria como ser declarada concluída sem opinião, e opinião como régua é o que o ADR-0039 §D1 recusou.

### D3 — A identidade não é inventada: é nomeada e completada

`--accent: #d4a05a` já é latão. A direção visual do app adota o vocabulário **Tinta & Latão** que já existe nos documentos gerados, em vez de criar uma segunda identidade para o mesmo produto.

**Mas os tokens não são compartilhados com os `document-kits`.** Tela e impressão têm requisitos diferentes — contraste em RGB emissivo, densidade, tamanho mínimo legível — e um arquivo comum faria uma mudança feita por razão de impressão mover a interface em silêncio. Mesma família, arquivos separados, decisão registrada aqui para que ninguém "unifique" isso depois achando que corrige duplicação.

Personalidade declarada, derivada do público (usuário único, técnico, sessões longas, alta densidade de informação): **técnico e institucional** — neutros dominantes, uma cor de destaque só, sans compacta com dígitos tabulares, cantos mínimos, respiro consistente. Não é "moderno/tech" com gradiente e vidro.

### D4 — Sugestões são estáticas e sem motor. O Nino continua fora da fase

O escopo de "sugestões" desta etapa é **afordância de interface**: estado vazio com pontos de partida, próximo passo visível após uma ação, mensagem de erro com ação sugerida — este último já existe e vira padrão documentado.

A fronteira, em uma regra: **se precisa de chamada a modelo ou de inferência sobre o contexto da conversa, é Copiloto, e está fora.** Sugestão desta etapa vem de lista fixa ligada ao estado da tela — logo é testável por igualdade, e as portas do §D2 continuam mecânicas.

O item "Copiloto (Nino) e tarefas" permanece na tabela de itens sem fase, intocado. Esta etapa não o antecipa nem o prepara.

### D5 — Escopo fechado nas telas que já existem

Entram: chat, configurações, memórias, modo equipe, sobre, e os componentes compartilhados (modal de aprovação, painel de memória, formulários). **Não entram telas novas** — o diff viewer e a UI de projeto são a Etapa 6, que consome o sistema em vez de ser redesenhada dentro da 5b.

### D6 — O tema claro é completado, não removido

Duas saídas honestas para o `#d4a05a` a 2,24:1: completar o bloco claro com um latão escurecido, ou remover o suporte a tema claro e assumir o app como escuro-only. **Decide-se completar**, porque remover significa ignorar uma preferência que o sistema operacional expõe e que o app hoje diz respeitar.

Fica proibida a terceira saída, que é a de hoje: um tema claro pela metade que falha em AA sem que nada avise. A porta 1 do §D2 passa a cobrir os dois temas exatamente para isso.

### D7 — A spec entra com o primeiro código, não agora

`docs/architecture/ui-design-system.md` **não** é criado neste PR. A trava do caminho inverso da REGRA §1.13 diz que nenhum spec permanece "especificado" depois que a fase dele começa, e a exceção do ADR-0038 já caiu — a Fase 8 tem código mesclado desde a Etapa 2b, então a marca `somente-planejamento` não está mais na célula de evidência.

Criar o spec agora com `Estado: especificado` e `Fase correspondente: 8` **quebra o `check-docs.mjs` no primeiro CI**. Criá-lo como "parcialmente implementado" sem código seria mentir para satisfazer a trava — precisamente o defeito do `f7d1ab3` que o ADR-0038 documentou.

Logo: a decisão vive aqui (§1.6, decisão precede código); o spec nasce promovido, no mesmo commit da primeira parte da implementação, com link para o teste que prova.

## Alternativas descartadas

1. **Deixar interface para a Fase 9.** Rejeitado pelo §D1: a Etapa 6 já teria sido construída sobre estilo ad-hoc, e a natureza da Fase 9 é outra.
2. **Criar uma fase própria de UI.** Rejeitado: o trabalho tem tamanho de etapa. Uma fase exigiria pré-requisitos próprios no roadmap e um critério de promoção separado, estrutura maior que o problema.
3. **Etapa sem porta mecânica ("deixar bonito").** Rejeitado pelo §D2 — é o defeito que tirou o Nino da fase.
4. **Adotar framework de UI pronto (Tailwind, shadcn, MUI).** Rejeitado por três razões independentes: acrescenta dependência e peso de bundle a um app desktop offline de usuário único; importa identidade genérica, que é o oposto de Tinta & Latão; e reabre revisão de licença, com o precedente da AGPL barrada antes de contaminar o `.exe`. 1.037 linhas de CSS com tokens não é dívida que justifique framework.
5. **Compartilhar o arquivo de tokens com os `document-kits`.** Rejeitado pelo §D3.
6. **Renumerar as Etapas 6 e 7 para abrir espaço.** Rejeitado pelo §D1.

## Consequências

- **Fica mais fácil:** construir tela nova. A Etapa 6 recebe tokens, estados e componentes prontos. E provar acessibilidade deixa de depender de inspeção manual.
- **Fica mais difícil:** introduzir uma cor ou um tamanho ad-hoc. A guarda do §D2.3 recusa. É o efeito pretendido, e vai incomodar na primeira vez.
- **Superfície nova:** um gate de CI que pode ficar vermelho por razão adjacente à estética. Mitigado por construção — o gate mede contraste e literais, nunca gosto.
- **Custo declarado:** as ~29 cores hoje soltas no `styles.css` precisam virar token de uma vez. É refatoração de arquivo único, sem mudança de comportamento, e deve entrar em commit próprio para que o diff de aparência não se misture ao de estrutura.
- **A acessibilidade recupera dono nomeado.** A linha "Copiloto (Nino) e tarefas" na tabela de itens sem fase ganha nota registrando que a acessibilidade saiu dela e passou a viver aqui.
- **O `development-roadmap.md`, o `status.md`, o ADR-0039 §D3 e o índice de `docs/releases/fase-8/README.md` ganham a linha da 5b** — no mesmo PR deste ADR, pela regra de commit atômico.

## Histórico de revisão

- 2026-08-17 — versão inicial. Criada fora da Etapa 1 de planejamento, a partir da medição do `styles.css` no `main` (`c36377c`).
- 2026-08-17 — medições reconferidas contra o `main` em `b0760a4`, três merges à frente. O `styles.css` está byte a byte idêntico ao de `c36377c`, então tudo que deriva dele se manteve: 1.037 linhas, 6 tokens, 35 literais de cor com 29 fora do `:root`, 10 tamanhos de fonte, 4 raios, zero `focus`, nenhuma regra para `<a>`, e as seis razões de contraste conferem na segunda casa decimal. **Uma correção:** a contagem de specs em `docs/architecture/` era 24, não 23 — o texto acima foi corrigido.
