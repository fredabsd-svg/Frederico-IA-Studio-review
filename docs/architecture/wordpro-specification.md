<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-31
Fase correspondente: 5
-->

> Última verificação: 2026-07-31. Reflete a Etapa 3 da Fase 5 —
> `WordProKit` v0.1 implementado no crate
> `frederico-document-kits` (35/35 testes verde: 34 unit + 1
> E2E full vertical com `python-docx` round-trip). Tradução
> completa de `DocumentSpec.blocks` → payload do handler
> `docx.write` da v0.3.0 do `document-worker`. Cobertura: todos
> os 20 blocos do spec (alguns com fallback textual — ver
> limitações abaixo). Identidade visual "Tinta & Latão" (§17.4)
> e modo Sóbrio (§16.6) **ainda não** aplicados no `.docx` —
> o `docx.write` da v0.3.0 é deliberadamente feio, e a
> tipografia fina entra na Etapa 6.

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

- D-WP1 (Etapa 3): **fallback textual para blocos sem cobertura
  direta no `docx.write` v0.3.0** — Table vira texto tab-separado
  no parágrafo, Image vira caption (sem embed de binário), Chart
  vira placeholder textual. Caller que precisa da versão rica
  usa o `xlsx.write` (Table real) ou o `pdf.write` (imagem real).
  Extensão do `python-docx` para Table real fica para a Etapa 6
  (junto com identidade visual Word).

- D-WP2 (Etapa 4): **quebra de contrato do `docx.read` para
  carregar `style`** — `paragraphs` mudou de `[str]` para
  `[{text, style}]` (Etapa 4 da Fase 5, ADR-0020 §7). O
  `docs.inspect` usa o style real do `python-docx` pra
  reconstruir heading (antes era heurística de string match em
  "Heading 1 " que falhava 100% das vezes — `python-docx` não
  prefixa o style no texto). Caller que dependia de `[str]`
  precisa migrar: extrair `paragraphs[i].text` ao invés de
  `paragraphs[i]` direto. **CHANGELOG registra como breaking
  change visível** (afeta 1 teste de integração no
  `process-architecture` que foi atualizado no mesmo commit).

## Limitações registradas (v0.1)

1. **Tabela vira texto tab-separado** no `.docx` (limitação do
   `docx.write` v0.3.0). O `docs.inspect` .docx não tem como
   distinguir tabela real de texto tab-separado — `coverage.preserved`
   não inclui `table` quando o spec original tinha Table, e a
   table não vai pra `coverage.lost` (o inspect sabe ler tabela
   real; é o gerador que não escreve). Extensão fica pra Etapa 6.

2. **Imagem vira caption** (sem embed de binário no `docx.write`
   v0.3.0).

3. **Chart vira placeholder textual** (sem chart visual).

4. **Sem identidade visual "Tinta & Latão"** no `.docx` (handler
   primitivo, v0.1 deliberadamente "feia" em tipografia — Etapa 6
   traz o estilo via `python-docx` estendido).

5. **Sem modo Sóbrio** para registráveis (§16.6) — Etapa 6.

## Referências

- `PROMPT MESTRE` §16 (suíte), §16.5 (regra zero de diagramação), §16.6 (modo Sóbrio), §17 (WordPro)
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`pdfpro-specification.md`](./pdfpro-specification.md) (fidelidade Word → PDF)
- `docs/development-roadmap.md` (Fase 5)
- [ADR-0020](../decisions/0020-fase-5-etapa-4-excelpro-inspect.md) — D-WP2 (quebra de contrato do `docx.read`).
