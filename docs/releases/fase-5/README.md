# Fase 5 — Documentos: narrativas de release

<!--
Estado: implementado
Verificado contra o código em: 2026-08-01
Fase correspondente: 5
-->

Índice das narrativas de processo (descrições de PR, lições de
execução) associadas à Fase 5. **Não duplicam o `CHANGELOG.md`**,
que registra só o efeito pro usuário (§1.7 do `REGRAS-DO-PROJETO.md`).
O que mora aqui é a história técnica — o que aconteceu em cada PR,
quais decisões foram tomadas no caminho, e o que se aprendeu.

O leitor típico de um release fechado não precisa abrir estes
arquivos. São referência pra quem está entrando num trabalho
relacionado e quer entender **por que** a Fase 5 ficou como ficou.

## Índice

| PR | Arquivo | Assunto |
|----|---------|---------|
| #17 | [`pr-17-pdfpro-v01-real.md`](./pr-17-pdfpro-v01-real.md) | PDFPro v0.1 real (render + glifo-check + watermark) + bump atômico do `DocumentFormat::Pdf`. |
| #19 | [`pr-19-auditoria-estrutural.md`](./pr-19-auditoria-estrutural.md) | Auditoria estrutural bloqueante do §19.4 — versão original (PR #16) que abriu com base empilhada. |
| #19 | [`pr-19-correcao-base.md`](./pr-19-correcao-base.md) | Correção da base do PR 3 (PR #18) — leva a auditoria para `main` depois do PR #17 ter entrado. Saga e a regra "só abra a próxima PR depois que a anterior entrou no main" (segunda ocorrência, vira regra). |
| #20 | [`pr-20-promocao-fase5-concluida.md`](./pr-20-promocao-fase5-concluida.md) | Promoção formal da Fase 5 para `concluída` no `docs/status.md`. |

## Como estes arquivos nasceram

Os 4 arquivos eram `docs/pr*-description.md` soltos na raiz,
descrições rascunhadas durante a abertura dos PRs. Como já
estavam mergeados no `main` e o conteúdo é de processo (não de
spec), foram movidos para cá em vez de apagados. Decisão tomada
na Etapa 1 da Fase de Ligação (PR único `fase-ligacao/conectar-motor-a-casca`).

Fase futuras podem usar o mesmo padrão: `docs/releases/fase-N/README.md`
indexando as narrativas dos PRs daquela fase.
