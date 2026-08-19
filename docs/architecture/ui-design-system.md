<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-19
Fase correspondente: 8 (Etapa 5b)
-->

# Sistema de design da interface (Tinta & Latão)

**Nasce promovido, junto com o código.** O [ADR-0045](../decisions/0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md) §D7 decidiu que este documento não podia existir antes: criá-lo como `especificado` quebraria o `check-docs.mjs` — a Fase 8 tem código mesclado desde a Etapa 2b — e criá-lo como `parcialmente implementado` sem código seria mentir para satisfazer a trava, que é o defeito que o [ADR-0038](../decisions/0038-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md) documentou.

Decisão que governa: [ADR-0045](../decisions/0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md).

## A identidade não foi inventada

O `--accent` `#d4a05a` já era um tom de latão no app desde a Fase 1, sem nome. **Tinta & Latão** nomeia o que existia, em vez de criar uma segunda identidade para o mesmo produto.

Personalidade declarada, derivada do público (usuário único, técnico, sessões longas, alta densidade de informação): **técnico e institucional** — neutros dominantes, uma cor de destaque só, sans compacta com dígitos tabulares, cantos mínimos, respiro consistente. Não é "moderno/tech" com gradiente e vidro.

**Estes tokens não são compartilhados com os `document-kits`** (§D3). Tela e impressão têm requisitos diferentes — contraste em RGB emissivo, densidade, tamanho mínimo legível —, e um arquivo comum faria uma mudança feita por razão de impressão mover a interface em silêncio. Mesma família, arquivos separados; quem for "corrigir a duplicação" depois deve ler isto antes.

## Os tokens

Vivem no topo de `apps/desktop/src/styles.css`, entre o começo do arquivo e a linha `* { box-sizing: border-box; }`. **Nada visual existe fora dali.**

| Eixo | Degraus | Substituiu |
|---|---|---|
| Cor | tokens semânticos (`--bg`, `--fg`, `--erro`, `--foco`, …) | 40 literais |
| Tipografia | 6 (`--fonte-xs` … `--fonte-xl`) | 10 tamanhos ad-hoc |
| Espaçamento | 6 (`--esp-1` … `--esp-6`) | 12 valores |
| Raio | 3 (`--raio-sm/md/lg`) | 4 valores |

Consistência é o que faz uma interface parecer projetada. Dez tamanhos de fonte escolhidos caso a caso não formam escala; formam ruído.

## As quatro portas

O critério de aceite é **mecânico** (§D2). Uma etapa chamada "melhorar o layout" reimportaria o defeito que tirou o Copiloto da fase — critério qualitativo não fecha por teste.

| Porta | Script | Estado |
|---|---|---|
| 1 — Contraste, nos dois temas | `scripts/check-ui-contrast.mjs` | **entregue** (entra no `ci.yml` em PR separada) |
| 2 — Foco visível + AA automatizável | — | CSS entregue; o gate `axe` precisa de runner |
| 3 — Zero literal visual fora do `:root` | `scripts/check-ui-tokens.mjs` | **entregue** (entra no `ci.yml` em PR separada) |
| 4 — Travessia por teclado | — | precisa de runner |

**O que explicitamente não é critério:** parecer bonito. A avaliação estética é do revisor humano, acontece na revisão do PR e **não bloqueia**. As portas bloqueiam.

**Por que os scripts não entram no CI neste mesmo PR:** alterar arquivo em `.github/workflows/` impede o `ci.yml` de rodar na PR que o altera, neste repositório — medido em 2026-08-17, e foi o que obrigou a separar o conserto do noturno na Etapa 2. Ligá-los aqui deixaria este PR sem nenhuma verificação.

As portas 2 e 4 exigem `vitest` + `jsdom` + `axe-core`, que o `apps/desktop` não tem — hoje ele só tem `vite` e `tsc`. Acrescentar runner de teste ao frontend é decisão com peso próprio e entra em PR separado, não como linha solta no meio deste.

### O que a porta 1 encontrou

A medição do próprio ADR-0045 declarou que o tema escuro passava em AA "em todos os pares testados". Ela testou os pares contra `--bg` e **não** contra `--bg-elev`. O `--erro` `#c66` dá 4,69:1 no primeiro e **4,18:1 no segundo** — e é no painel elevado que a mensagem de erro aparece. Passou a `#d17878` (4,93:1 no painel).

É o motivo de a porta existir: a medição manual cobriu o que quem media lembrou de medir.

## O tema claro deixou de estar pela metade

O bloco `prefers-color-scheme: light` sobrescrevia 5 dos 6 tokens e esquecia o `--accent`. O latão `#d4a05a` sobre `#fafafa` dava **2,24:1**, abaixo do mínimo de 4,5:1, e nada avisava.

O §D6 tinha duas saídas honestas — completar o bloco ou assumir o app como escuro-only — e escolheu completar, porque remover significaria ignorar uma preferência que o sistema operacional expõe e que o app diz respeitar. O latão escurecido do tema claro (`#8a5d18`) mede 5,50:1.

A terceira saída, que era a de então, fica proibida por construção: a porta 1 cobre os dois temas.

## Estados

Antes desta etapa havia **zero** ocorrências de `focus` em 1.037 linhas — quem navegava por teclado não via onde estava, o que é falha direta do critério 2.4.7 do WCAG 2.1 AA.

- **`:focus-visible`, não `:focus`.** O indicador aparece para quem navega por teclado e não para quem clica. Um anel em todo botão clicado provocaria a reação previsível de alguém removê-lo — trocando incômodo por falha de acessibilidade.
- **Controles nativos** herdam `color-scheme`, para o sistema desenhar a seta e a lista suspensa do `<select>` na versão escura. Reimplementar isso em CSS não alcança a lista suspensa.
- **Links** ganharam regra: usavam o violeta padrão do navegador, sem par com a paleta e sem medição contra fundo nenhum.
- **Dígitos tabulares** em custo e contagem de token, que mudam a cada quadro do streaming — sem isso a linha dança a cada dígito que troca de largura.
- **`prefers-reduced-motion`** respeitado (critério 2.3.3).

## O que este documento não cobre

- **Telas novas.** O diff viewer e a UI de projeto são a Etapa 6, que consome este sistema em vez de ser redesenhada aqui (§D5).
- **Sugestões com motor.** O escopo de sugestão aqui é afordância estática — estado vazio com pontos de partida, próximo passo visível. **Se precisa de chamada a modelo ou de inferência sobre o contexto, é Copiloto, e está fora da fase** (§D4).
- **Framework de UI.** Rejeitado por três razões independentes no §Alternativas do ADR: peso num app desktop offline, identidade genérica onde se quer Tinta & Latão, e revisão de licença — com o precedente da AGPL barrada antes de contaminar o `.exe`.

## Referências

- [ADR-0045](../decisions/0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md) — a decisão e a medição que a originou
- [ADR-0039](../decisions/0039-fase-8-escopo-e-etapas.md) §D1 — por que critério qualitativo não fecha etapa
- WCAG 2.1 AA — critérios 1.4.3 (contraste), 1.4.11 (não-textual), 2.4.7 (foco visível), 2.3.3 (movimento)
