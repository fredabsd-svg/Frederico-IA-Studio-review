# 0038 — A Etapa 1 (planejamento) não dispara a trava do §1.13

## Contexto

A REGRA §1.13 tem uma **trava do caminho inverso**:

> Nenhum documento permanece "especificado" depois que a fase dele começa. Se `docs/status.md` marcar a fase como "em andamento" ou "concluída" e o spec correspondente ainda estiver "especificado", é defeito.

A trava existe por um motivo bom: sem ela, um documento escaparia da §1.3 (documentação acompanha o código no mesmo commit) indefinidamente, bastando manter o cabeçalho em "especificado" para sempre. O `check-docs.mjs` a cobra mecanicamente.

Ela colide, porém, com a forma que o projeto adotou para abrir fase. O ADR-0032 §D5 institui que **a Etapa 1 de uma fase é planejamento puro** — ADRs e specs, sem código Rust — e que é ali que o escopo se corta, antes de haver o que defender. A Fase 6 (ADR-0028) e a Fase 7 (ADR-0032) fecharam a Etapa 1 assim, e o ADR-0032 §Consequências registra que a fase passa a `em andamento` **com a Etapa 1 fechada**.

O resultado é uma contradição estrutural, não um caso de borda:

1. A Etapa 1 fecha, e o `status.md` marca a fase como `em andamento`.
2. A trava passa a valer para todos os specs daquela fase.
3. Mas o código da fase ainda não existe — a Etapa 2 é que o traz.
4. Logo, todo spec criado na Etapa 1 é obrigado a declarar um estado (`parcialmente implementado`) que é falso no momento em que é escrito.

Foi exatamente o que aconteceu na Fase 7. O commit `f7d1ab3` (PR #41) criou `runtimes-architecture.md` e `exec-tools-specification.md` já com `Estado: parcialmente implementado` e carimbo `2026-08-08`, no mesmo PR que o próprio ADR-0032 descreve como "sem código Rust". Não havia uma linha do `crates/runtimes` nesse commit — ele nasceu no PR #43, dois dias depois.

Ou seja: para satisfazer a trava do §1.13, o projeto violou a §1.1, que é a regra que todas as outras servem. Um cabeçalho afirmou implementação inexistente. E a §1.13 diz, na frase imediatamente anterior à trava, o oposto do que a trava forçou:

> A promoção de "especificado" para os demais estados acontece **no mesmo commit em que a primeira parte do código entra**, com link para o teste que prova.

Sem resolver isso, a Fase 8 repete a mentira: a Etapa 1 dela cria specs de Git, GitHub, projetos e checkpoints, nenhum deles com código, e todos seriam obrigados a se declarar parcialmente implementados.

## Decisão

**A Etapa 1 de planejamento não conta como início de fase para efeito da trava do §1.13.**

Concretamente:

### D1 — A trava passa a olhar a etapa, não só o estado da fase

Um spec pode permanecer `especificado` enquanto a fase dele estiver `em andamento` **e** a coluna "Evidência" do `status.md` registrar que apenas a Etapa 1 (planejamento) fechou. Assim que qualquer etapa de código fechar, a trava volta a valer integralmente, e o spec correspondente tem de ter sido promovido no commit em que o código entrou — como a §1.13 já mandava.

O sinal mecânico é uma marca literal na coluna "Evidência" da linha da fase: **`somente-planejamento`**. A frase é literal e conferida por substring, mesma forma da `regra não-aplicável` do §3.5 da REGRA 3. Enquanto ela estiver lá, a trava afrouxa para aquela fase; ao ser removida — o que o PR da Etapa 2 obrigatoriamente faz, porque traz código —, a trava aperta de volta.

### D2 — A marca é auto-expirável, não um interruptor

`somente-planejamento` não é um label que alguém liga e esquece. Ela vive na mesma célula que descreve as etapas fechadas da fase, e o primeiro PR de código da fase **precisa** editar essa célula para registrar a etapa que fechou. Remover a marca não é um passo extra que se possa pular: é o mesmo texto que o autor já está reescrevendo.

Isto responde à objeção que o §3.5 da REGRA 3 levanta contra válvulas de escape ("se o gate pode ser desligado por label, ele é desligado e ninguém percebe"). A diferença é que aqui a válvula não é desligável por conveniência — ela é desligada **pelo próprio ato** de fazer a coisa que a torna inválida.

### D3 — O estado honesto é `especificado`, e ele passa a ser obrigatório

Não é só permissão: durante a Etapa 1, spec novo da fase **deve** ser `especificado`. Declarar `parcialmente implementado` sem código é defeito, e passa a ser a violação que a §1.1 sempre disse que era. O carimbo "Verificado contra o código em" continua isento de prazo nesse estado, como a §1.13 já previa.

### D4 — A §1.13 é emendada, não contornada

O texto da REGRA passa a conter a exceção. Regra que o código do gate contradiz é pior que regra ausente: cria dois documentos normativos discordando, e quem lê a REGRA não descobre o comportamento real sem ler o script.

## Alternativas descartadas

1. **Seguir o precedente da Fase 7** — marcar os specs novos como `parcialmente implementado` já na Etapa 1. Rejeitado porque é precisamente o defeito que a §1.1 nomeia, e porque o projeto anterior morreu disso. Que já tenha acontecido uma vez é argumento para corrigir, não para repetir: o custo de normalizar "o cabeçalho mente um pouco no começo" é que a próxima leitura de qualquer cabeçalho passa a ser feita com desconto.

2. **Manter a fase `não iniciada` até a Etapa 2** — os specs entram `especificado` e o `status.md` só muda quando o primeiro código chega. Rejeitado porque faz o `status.md` mentir do outro lado: o PR de planejamento é trabalho real, com ADRs que já vinculam decisões, e uma sessão nova de IA lendo `não iniciada` concluiria que pode redecidir tudo. Troca uma mentira no cabeçalho do spec por uma mentira na fonte da verdade do estado — pior negócio, porque o `status.md` é o primeiro arquivo que qualquer sessão lê (§1.8).

3. **Criar um quarto estado, `em planejamento`** — entre `especificado` e `parcialmente implementado`. Rejeitado por não acrescentar informação: `especificado` já significa exatamente "plano, ainda não construído", e a §1.13 já o isenta da §1.3. O problema nunca foi a falta de um estado; foi a trava não distinguir "a fase começou" de "o código da fase começou".

4. **Remover a trava** — deixar a fiscalização de spec desatualizado para a revisão humana. Rejeitado: a trava existe porque revisão humana falha nisso de forma previsível, e sem ela um spec pode ficar `especificado` para sempre. O ajuste preserva a trava inteira para todo o resto do ciclo de vida da fase.

## Consequências

- **Fica mais fácil:** abrir fase com specs honestos. A Etapa 1 da Fase 8 cria os specs de Git, GitHub, projetos e checkpoints como `especificado`, que é o que eles são, e cada um é promovido no PR que traz o código dele — com link para o teste, como a §1.13 sempre pediu.
- **Fica mais difícil:** deixar um spec parado. A janela de folga é estreita e visível: dura exatamente enquanto a fase não tiver código, e a marca que a sustenta fica na linha mais lida do `status.md`.
- **Dívida herdada, não apagada:** `runtimes-architecture.md` e `exec-tools-specification.md` nasceram com estado falso na Fase 7. Hoje ambos são verdadeiros (a Fase 7 fechou, o código existe), então não há o que corrigir no conteúdo — mas o episódio fica registrado aqui, e não como nota de rodapé perdida no CHANGELOG, porque foi ele que motivou este ADR.
- **O `check-docs.mjs` ganha uma exceção a mais para manter.** Custo real, aceito: a alternativa era manter uma regra que o próprio projeto não conseguia cumprir sem mentir.
- **A REGRA §1.13 é emendada** com o parágrafo da exceção e a marca literal.

## Histórico de revisão

- 2026-08-16 — versão inicial. Decisão da Etapa 1 da Fase 8, tomada ao descobrir que a Fase 7 só conseguiu satisfazer a trava do §1.13 declarando implementação inexistente em `f7d1ab3`.
