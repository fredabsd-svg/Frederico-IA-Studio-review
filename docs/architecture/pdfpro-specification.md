<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-01
Fase correspondente: 5 (Etapa 5 — PR 2 do PDFPro)
-->

> Última verificação: 2026-08-01. Reflete a Etapa 5 da Fase 5
> até o **PR 2** (PDFPro v0.1 entregue):
>
> - **PR 1 (commit `d518226`) — fundação:** ADR-0021
>   (D-PDF1 engine, D-PDF2 marca d'água, D-PDF3
>   compressão, D-PDF4 proteção, D-PDF5 PDF/A-2B, D-CONF-1
>   correção §7.1, D-CHART-1 chart nativo, D-AUDIT-1 cache
>   key, D-GLYPH-1 glifo, D-FAIL-1 hard-fail, D-INSPECT-1
>   inspect PDF); `DocumentMetadata.watermark` (D-PDF2) +
>   `SpecVersion` 0.1.0 → 0.2.0 (MINOR: novo campo
>   opcional, backward-compat); `validate_semantic` regra 8
>   (Sobrio + watermark rejeitados); `bootstrap.ps1` +
>   `pyproject.toml` + `manifest.json` ganham `pikepdf`,
>   `pypdfium2`, `fonttools` com hard-fail (D-FAIL-1). A
>   tentativa original de flipar `is_implemented` + bump do
>   enum sem o `render` real foi pega em review e revertida
>   no commit `5c39bac` — precedente do ADR-0020 §3 D3.
> - **PR 2 (esta entrega) — `render` real + bump atômico:**
>   `DocumentFormat::Pdf` no enum + `PdfProKit` real
>   (`is_implemented() == true`, `target_format() == Pdf`)
>   no **mesmo commit**; `render` via `reportlab` Platypus
>   com fontes Tinta & Latão embutidas (sem fallback,
>   D-FAIL-1), identidade visual "Tinta & Latão" + modo
>   Sóbrio (registráveis, monocromático, margens maiores),
>   20 blocos cobertos (Cover, Toc, Heading, Paragraph,
>   List, Table, KeyValue, Kpis, Callout, Quote, Steps,
>   Chart-placeholder, Image, Code, Divider, Spacer,
>   PageBreak, Footer, Signatures, BackCover); glifo-check
>   via `fontTools` ANTES do `doc.build()` (D-GLYPH-1) —
>   falha estruturada com `code: "missing_glyph"`; marca
>   d'água opt-in (D-PDF2) via `onPage` callback; **bug
>   fix do `bootstrap.ps1`**: a `LibSentinels` original
>   só checava 4 libs e pulava o `pip install` de
>   pikepdf/pypdfium2/fonttools se elas já estivessem
>   presentes — **violava D-FAIL-1**. Corrigido: sentinels
>   cobrem as 7 libs, hard-fail real.
> - **PR 3 — auditoria estrutural (§19.4):** `pikepdf`
>   valida n_pages, metadados, fontes embutidas, Tagged PDF,
>   PDF/A-2B só se opt-in. 5+ testes negativos.
> - **PR 4 — auditoria visual (§19.3):** `pypdfium2`
>   rasteriza → checagem de grade, sobreposição, página
>   vazia, alinhamento. 10+ testes negativos. Cache key
>   = hash + versão (D-AUDIT-1).
> - **PR 5 — chart nativo Excel + identidade visual Excel +
>   tabela estilizada + warning sai:** fecha lacunas 1, 2 e
>   3 do `excelpro-specification.md`. Spec ExcelPro
>   atualizado.
> - **PR 6 — fechamento:** E2E completo (audit visual +
>   estrutural ambos passando), bump `status.md` Etapa 5 →
>   concluída, specs `pdfpro` e `excelpro` → `implementado`
>   (3 lacunas do ExcelPro fechadas), CHANGELOG v0.5.0,
>   `verify-external.ps1` cobre os E2E novos, ADR-0021
>   mergeado.
>
> **Estado continua `parcialmente implementado`** após o
> PR 2: a v0.1 do `render` (fontes, identidade, modo
> Sóbrio, 20 blocos, glifo-check, marca d'água) está
> entregue, mas a **auditoria bloqueante do §19.6**
> (visual + estrutural) entra nos PRs 3 e 4. **Tagged PDF**
> continua como lacuna registrada (PDF/A-2B exige Tagged;
> v0.1 do PDFPro entrega PDF 1.7 com fontes embutidas e
> grade auditada, com Tagged marcado como pendência 5.x).

# Especificação do PDFPro Kit

> Especificação criada na Fase 0 (stub), aprofundada na Etapa 1
> da Fase 5 (catálogo de blocos), fechada em política na Etapa 5
> (ADR-0021 — engine + 4 políticas). A v0.1 do `render` entra no
> PR 2 da Etapa 5.

## Decisão tomada

- Geração, revisão, combinação e validação de PDFs a partir de `DocumentSpec` (`PROMPT MESTRE` §19).
- **Fontes embutidas no PDF final** — nenhum documento pode depender de fonte instalada na máquina do usuário (`PROMPT MESTRE` §5.3 final).
- **Identidade "Tinta & Latão"** + **modo Sóbrio** para registráveis, idênticos aos outros kits.
- **Validação visual** das páginas via renderização para imagens temporárias: conteúdo cortado, tabela ultrapassando margem, sobreposição, página vazia, fonte ausente, caractere quebrado, espaçamento, resolução, cabeçalho, rodapé, alinhamento (`PROMPT MESTRE` §19.3).
- **Validação estrutural**: abertura, quantidade de páginas, metadados, fontes, texto, imagens, links, bookmarks, tamanho, corrupção, **Tagged PDF** (quando o opt-in `pdfa: PdfA2b` for usado) (`PROMPT MESTRE` §19.4).
- **Auditoria bloqueante** (`PROMPT MESTRE` §19.6): as duas validações executam dentro do salvamento do artefato; reprovação deixa em `invalid` e impede a entrega. **Sem interruptor** para desligar.
- **Fidelidade ao criar PDF a partir de Word/Excel**: hierarquia, títulos, tabelas, gráficos, paginação preservados, arquivo de origem registrado (`PROMPT MESTRE` §19.5). **Etapa 6.**
- **Engine** (D-PDF1 do ADR-0021): `reportlab` (BSD-3) para render; `pikepdf` (MPL-2.0) para auditoria estrutural e manipulação; `pypdfium2` (Apache-2.0/BSD-3-Clause, binding PDFium) para rasterizar na auditoria visual; `fontTools` (MIT) para checagem de glifo antes de renderizar. **AGPL descartada** (PyMuPDF/borb contaminariam o app .exe; cláusula de rede da AGPL atinge o caminho 2 do §5.5 do PROMPT MESTRE).
- **Marca d'água** (D-PDF2): opt-in via `DocumentMetadata.watermark: Option<WatermarkSpec>` (text + position + opacity + font_size). **Validador rejeita `style == Sobrio && watermark.is_some()`** (modo Sóbrio é para registráveis; tarja visual atravessando instrumento da Junta é erro).
- **Compressão** (D-PDF3): sempre JPEG q=80 por padrão, configurável.
- **Proteção** (D-PDF4): opt-in (AES-128, `open_password` configurável).
- **PDF/A-2B** (D-PDF5): opt-in. Quando ligado, a auditoria estrutural ganha passo de conformidade e falha bloqueando (como qualquer outro item do §19.6). `veraPDF` roda no `ci-nightly.yml` (job noturno) validando PDFs gerados com opt-in.

## Contrato previsto

O PDFPro consome `DocumentSpec` (com `DocumentType::Pdf` ou `Report` etc.) ou um `.docx`/`.xlsx` já gerado (no caso de fidelidade, Etapa 6), e produz um `.pdf` real no disco. O `.pdf` passa pelas duas validações (§19.3 e §19.4) **antes** de o artefato ser marcado como `valid`. A auditoria é parte do salvamento, não uma etapa opcional depois.

## Recursos mínimos (`PROMPT MESTRE` §19.1)

Relatórios, demonstrações, documentos para apresentação, capas, sumários, tabelas, gráficos, imagens, cabeçalhos, rodapés, paginação, marca d'água (opt-in), anexos, metadados, bookmarks, divisão, união, compressão, proteção (opt-in), PDF/A-2B (opt-in).

## Não-objetivos

- Editor de PDF dentro do app (anotação leve pode ser considerada depois).
- Assinatura digital de PDF com certificado A1/A3 na v1.
- OCR de PDF escaneado de entrada (a entrada do app é o `DocumentSpec`; OCR é para anexos do usuário, `PROMPT MESTRE` §15).
- Geração de PDF "impressa" de uma página web (a não ser via `DocumentSpec`).
- **PDF/A-1B** (só PDF/A-2B na v1).
- **PDF/UA** (acessibilidade completa — fora do escopo da v1).
- **Tagged PDF** automático sem opt-in de PDF/A-2B (lacuna registrada, ver "Lacunas da v0.1" abaixo).

## Lacunas da v0.1 (que impedem "implementado")

A Etapa 5 entrega a v0.1 do PDFPro com `reportlab` (fontes Tinta & Latão embutidas, identidade visual, modo Sóbrio, 20 blocos) + auditoria bloqueante do §19.6 (visual + estrutural). **Não** entrega o pacote completo que o spec promete:

1. **Tagged PDF** (D-FAILS-1) — `reportlab` tem suporte fraco a `StructTree`
   (Tagged PDF). A v0.1 entrega PDF 1.7 com fontes embutidas e
   grade auditada, mas **sem** Tagged PDF automático. PDF/A-2B
   (que exige Tagged) é opt-in e a auditoria vai reportar a
   lacuna. **Subida pra Tagged automático** é pre-requisito pra
   PDF/A-2B virar padrão, não só opt-in.
2. **Fidelidade Word → PDF e Excel → PDF** (`PROMPT MESTRE` §19.5) —
   Etapa 6. A v0.1 do PDFPro consome `DocumentSpec`; converter um
   `.docx` ou `.xlsx` existente em PDF é trabalho da Etapa 6.
3. **Sumário automático em duas passadas** (`PROMPT MESTRE` §16.4) —
   Etapa 5.x. Requer `multiBuild` do `reportlab` + integração com
   o bloco `Toc` do `DocumentSpec`. A v0.1 renderiza o `Toc` como
   placeholder textual.
4. **`docs.inspect` cobrindo `.pdf`** — Etapa 5.x. O handler
   `pdf.read` da v0.3.0 devolve `text` + `ocr_text` + `page_count`
   + `scanned_pages`, mas **não** reconstrói `DocumentSpec`. O
   round-trip de `.pdf` para `DocumentSpec` é não-trivial e
   registrado como pendência.
5. **PDF/A-1B / PDF/UA** — fora do escopo da v1. PDF/A-2B é o
   piso de "PDF sério" hoje; PDF/UA (acessibilidade completa)
   entra em versão futura.

A promoção pra `implementado` exige que todos os 5 itens acima
estejam fechados. **A Etapa 6 não é pré-requisito pra isso** — o
trabalho de Tagged PDF e sumário em duas passadas pode entrar em
qualquer ordem entre 5.x e 6.

## Decisões

- [ADR-0021](../decisions/0021-fase-5-etapa-5-pdfpro-excelpro.md) —
  decisão completa da Etapa 5 (D-PDF1 a D-PDF5 + D-CONF-1 +
  D-CHART-1 + D-AUDIT-1 + D-GLYPH-1 + D-FAIL-1 + D-INSPECT-1 +
  alternativas descartadas + pendências).

## Referências

- `PROMPT MESTRE` §5.3 (fontes embutidas), §5.5 (modo servidor),
  §7.1 (confidencialidade — corrigido por D-CONF-1 do ADR-0021),
  §16.4 (sumário), §16.5 (regra zero), §19 (PDFPro), §19.3-§19.6
  (auditoria).
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`wordpro-specification.md`](./wordpro-specification.md) (fidelidade Word → PDF)
- [`excelpro-specification.md`](./excelpro-specification.md) (fidelidade Excel → PDF)
- [ADR-0021](../decisions/0021-fase-5-etapa-5-pdfpro-excelpro.md) — D-PDF1 a D-PDF5.
- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — Python embeddable + libs base.
- [ADR-0018](../decisions/0018-document-worker-handlers-primitive.md) — handler como primitiva.
- [ADR-0020](../decisions/0020-fase-5-etapa-4-excelpro-inspect.md) — Etapa 4 ExcelPro v0.1 (referência pra chart nativo + identidade visual Excel).
- `docs/development-roadmap.md` (Fase 5).
