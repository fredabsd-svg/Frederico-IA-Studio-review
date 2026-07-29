<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-28
Fase correspondente: 5
-->

> Última verificação: 2026-07-28. Reflete a Etapa 1 da Fase 5 — o
> catálogo de blocos do `DocumentSpec` (20 blocos) já está definido
> e validado no `frederico-document-engine`; o WordPro **em si**
> (renderização via `python-docx`, estilos "Tinta & Latão" e modo
> "Sóbrio") entra na Etapa 3. Estilo centralizado (`PROMPT MESTRE`
> §17.4) e modo Sóbrio (`§16.6`) já estão previstos no enum
> `DocumentStyle`.

# Especificação do WordPro Kit (stub)

> Stub criado na Fase 0. Aprofundado na Etapa 1 da Fase 5 (catálogo
> de blocos); renderização via `python-docx` entra na Etapa 3.

## Decisão tomada

- Geração de `.docx` profissionais a partir de `DocumentSpec` (`PROMPT MESTRE` §17).
- **Estilos centralizados** — formatação manual parágrafo a parágrafo é proibida quando um estilo reutilizável serve (`PROMPT MESTRE` §17.4).
- **Mesma identidade visual "Tinta & Latão"** dos outros kits, com **modo Sóbrio** para registráveis (`PROMPT MESTRE` §16.6).
- **Revisão por outro modelo** sobre o arquivo real: abre `.docx`, extrai estrutura, analisa, altera, preserva estilos, produz nova versão, apresenta diferenças (`PROMPT MESTRE` §17.5).
- **Mesma renderização preservada quando convertido em PDF** — fidelidade Word → PDF via PDFPro (`PROMPT MESTRE` §19.5).

## Contrato previsto

O WordPro consome `DocumentSpec` (ver [`document-engine-architecture.md`](./document-engine-architecture.md)) e produz um arquivo `.docx` real no disco. Abertura, tamanho, número de páginas, presença de seções e estilos são **validados** antes de o artefato ser marcado como `valid` (ver invariante em [`testing-strategy.md`](./testing-strategy.md)).

## Recursos mínimos (`PROMPT MESTRE` §17.1)

Capas, cabeçalhos, rodapés, numeração de páginas, sumário, títulos hierárquicos, tabelas, notas, avisos, caixas de destaque, listas, imagens, legendas, quebras de página, estilos, margens, orientação, seções, assinaturas, anexos, referências.

## Modelos previstos (`PROMPT MESTRE` §17.2)

Relatório executivo, parecer técnico, relatório contábil, relatório fiscal, análise de débitos, proposta, contrato, procuração, ofício, comunicado, memorando, manual, documentação de sistema.

## Não-objetivos

- Editor visual completo de Word dentro do app.
- Macros VBA ou automação COM.
- Conversão de PDF para Word com fidelidade pixel-a-pixel.
- Templates personalizados pelo usuário na v1 (apenas os modelos do §17.2).

## Aprofundar antes da Fase 5

- Schema do estilo "Tinta & Latão" e do modo "Sóbrio" no `python-docx` (styles.xml).
- Catálogo de estilos internos (Heading 1-3, Body, Callout, etc.) com métricas de qualidade (§17.3: linhas órfãs, tabelas cortadas, cabeçalhos repetidos, quebras incorretas, campos vazios, imagens ausentes, texto fora de página, numeração, sumário).
- Política de fallback quando `python-docx` não consegue renderizar um bloco do spec (erro estruturado, ver [`document-engine-architecture.md`](./document-engine-architecture.md)).
- Contrato da API de revisão multimodelo (`PROMPT MESTRE` §17.5) — o que o segundo modelo recebe, em que formato, como devolve alterações.
- Procedimento de teste: `.docx` gerado é aberto por `python-docx` em modo round-trip e os elementos críticos são validados.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado.

## Referências

- `PROMPT MESTRE` §16 (suíte), §16.5 (regra zero de diagramação), §16.6 (modo Sóbrio), §17 (WordPro)
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`pdfpro-specification.md`](./pdfpro-specification.md) (fidelidade Word → PDF)
- `docs/development-roadmap.md` (Fase 5)
