# 0001 — Especificação versus as-built em `docs/architecture/`

## Contexto

O `PROMPT MESTRE` §33 exige 18 documentos de especificação **antes** do código ser escrito. Ao mesmo tempo, `REGRAS-DO-PROJETO.md` §1.1 proíbe terminantemente documentação que descreva intenção em vez do que o código faz hoje ("o projeto anterior morreu em parte por isso"). Sem uma regra explícita de como esses dois requisitos coexistem, qualquer IA trabalhando no repo tende a cair em um de dois vícios: ou escreve specs detalhados que apodrecem antes de virar código, ou pula o planejamento e descobre tarde demais que está refazendo decisões.

## Decisão

Documentos em `docs/architecture/` podem existir em três estados, declarados no cabeçalho:

- **especificado** — descreve o que se pretende construir; **é explicitamente um plano** e está isento da regra de sincronia de `REGRAS §1.3`.
- **parcialmente implementado** — código correspondente existe e a parte do doc que ele ancora é fiel à realidade; passa a obedecer `§1.3`.
- **implementado** — todo o conteúdo do doc está refletido em código testado; obedece `§1.3` integralmente.

A transição entre estados é feita **no mesmo commit em que a parte do código entra**, com referência ao teste que prova. A regra de sincronia não vale retroativamente para "especificado" — mas a transição para os outros estados é obrigatória e **unidirecional** (não há caminho de volta para "especificado" sem um ADR que declare o motivo e atualize o `status.md`).

A trava do caminho inverso: se `docs/status.md` marcar a fase correspondente como "em andamento" ou "concluída" e o spec dela ainda estiver "especificado", é defeito detectável em CI.

## Alternativas descartadas

- **Não escrever specs até ter código.** Descartada: perde a função do §33 de forçar a equipe a pensar a arquitetura antes de mergulhar em implementação, e empurra decisões caras (formato de contrato, layout de monorepo) para o momento em que já há código para reverter.
- **Specs detalhados sem regra de transição.** Descartada: recria exatamente o problema que matou a documentação do projeto anterior — texto que diverge do código sem ninguém perceber.
- **Dois documentos separados (spec + as-built).** Descartada: a duplicação inevitavelmente diverge, e a regra `1.9` ("gerado vence manual") mostra que manter duas fontes da verdade para a mesma coisa é defeito sistemático.

## Consequências

**Mais fácil:**
- A IA pode começar um projeto grande planejando com profundidade sem violar `§1.1`.
- O `status.md` funciona como índice de "quão prontos estamos" sem ambiguidade.
- A revisão de PR pode cobrar a transição de estado junto com a transição de código.

**Mais difícil:**
- Toda vez que a primeira parte do código de uma área entra, o autor precisa lembrar de promover o spec correspondente e referenciar o teste.
- O `status.md` precisa ser mantido disciplinadamente, senão a trava do caminho inverso dispara falsos positivos (ou falsos negativos).
- A regra do CI para a trava (§1.13) precisa ser implementada cedo na Fase 0/Fase 1 para fazer sentido.
