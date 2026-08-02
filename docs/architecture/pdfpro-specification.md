<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-01
Fase correspondente: 5 (Etapa 5 — PR 3 do PDFPro: auditoria estrutural bloqueante do §19.4)
-->

> Última verificação: 2026-08-01. Reflete a Etapa 5 da Fase 5
> até o **PR 3** (auditoria estrutural bloqueante do §19.4 — D-PDF5 + D-PDF6 do ADR-0021):
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
> - **PR 3 (esta entrega) — auditoria estrutural (§19.4):**
>   capability `pdf.audit` no `document-worker` (kind
>   "structural", expansivel pra "visual" no PR 4); `pikepdf`
>   valida abertura, n_pages, DocInfo populado, fontes
>   embutidas, sem cifragem, sem referencias externas;
>   **PDF/A-2B opt-in** (D-PDF5) valida adicionalmente
>   OutputIntent com ICC sRGB (D-PDF6), XMP pdfaid batendo
>   com DocInfo, sem JavaScript. **Tagged PDF NAO e
>   verificado** - A-2B (básico) nao exige; v1 declara
>   apenas nível B (A fora de escopo). Falha = `KitError::
>   AuditFailed` com `code: "pdf_audit_structural_failed"` +
>   lista legivel de checks que falharam. Cache key (D-AUDIT-1)
>   = `sha256(pdf_bytes) + ":" + AUDIT_RULES_VERSION` (v0.1.0).
>   19 testes Python em `tests/test_pdf_audit.py` (5+ negativos
>   injetando falha). `KitError::AuditFailed` novo variant
>   no `kit.rs` (D-PDF5 do ADR-0021). `SpecVersion` 0.2.0 →
>   0.3.0 (novo campo opcional `DocumentMetadata.pdfa`).
>   **D-PDF6** novo no ADR-0021 registra a escolha do sRGB ICC
>   v2 + SHA-256 (`C4188E5C...D74B64`); gerado localmente pelo
>   `tools/generate_srgb_icc.py` (color.org nao tem URL estavel
>   desde 2026; bootstrap.ps1 valida o SHA-256 com hard-fail).
>   Pendencia 5.x registrada: `n_pages` real do `pdf.write`
>   (limitacao do reportlab - v0.4.0 chuta 1).
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
> PR 3: a v0.1 do `render` (PR 2) + a auditoria estrutural
> do §19.4 (PR 3) estão entregues, mas a **auditoria visual
> do §19.3** (pypdfium2 rasteriza + checa grade, sobreposição,
> página vazia, alinhamento) entra no PR 4 e os 3 pacotes do
> ExcelPro (chart nativo, identidade visual, tabela estilizada)
> entram no PR 5. As 4 lacunas restantes da v0.1 do PDFPro
> (Tagged, fidelidade Word/Excel→PDF, sumário em duas passadas,
> `docs.inspect` cobrindo .pdf) continuam registradas; Tagged
> especificamente é declarada como **fora de escopo** (pertence
> ao PDF/A-2A, fora do nível B reivindicado).

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
- **Validação estrutural**: abertura, quantidade de páginas, metadados, fontes, texto, imagens, links, bookmarks, tamanho, corrupção (`PROMPT MESTRE` §19.4). **Tagged PDF NÃO é requisito** do PDF/A-2B (nível B / basic) — é o que separa o nível B (basic) do nível A (accessible). v1 declara apenas nível B; Tagged fica fora de escopo (ver "Não-objetivos" abaixo).
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
- **PDF/A-2A** (nível A / accessible, com Tagged PDF / `StructTree` completo) — fora do escopo da v1. v1 reivindica **apenas nível B (básico)**. Tagged PDF é o que separa o nível A do B; reivindicar A-2A exige escrever `StructTree` XML manualmente no `reportlab`, o que o v1 não faz. Se um dia reivindicar nível A, vira pendência 5.x com bump de schema.
- **Tagged PDF automático fora do contexto PDF/A-2A** — não-objetivo da v1. A v1 nunca reivindica Tagged, mesmo quando o usuário pede explicitamente. (Reivindicar Tagged sem ser nível A é o que sistemas de protocolo (Junta, órgãos públicos) rejeitam na entrada.)
- **Subida de Tagged para "PDF sério"** — não-objetivo. PDF 1.7 com fontes embutidas e grade auditada já é o piso de "PDF sério" do Frederico. Tagged é para conformidade arquivística (PDF/A-2A) ou acessibilidade (PDF/UA), não para "ficar mais sério".

## Escopo declarado do PDF/A-2B (D-PDF5 do ADR-0021)

A v1 do PDFPro **reivindica apenas o nível B (basic) do PDF/A-2**. O que isso significa em termos de auditoria:

**O que o nível B verifica (D-PDF5, D-PDF6 — auditoria do §19.4 quando `pdfa: Some(PdfA2b)` é usado):**

- Fontes embutidas (mesma regra do PDF comum, mas auditada via pikepdf)
- XMP `pdfaid:part=2` e `pdfaid:conformance=B` batendo com DocInfo
- OutputIntent com perfil ICC RGB embedded (sRGB IEC 61966-2.1, gerado pelo `tools/generate_srgb_icc.py` e validado por SHA-256 no `bootstrap.ps1`)
- Sem cifragem, sem JavaScript (`/OpenAction`, `/AA`, `/Names/JavaScript` ausentes)
- Sem referências externas (URL em `/A /URI`, `/EmbeddedFiles` com `/F`)

**O que o nível B NÃO verifica (escopo declarado):**

- **Tagged PDF / `StructTree`** — Nível B não exige. A v1 não verifica e não promete verificação no futuro para A-2B.
- **PDF/A-2A (accessible)** — fora do escopo da v1. Reivindicar A-2A exige Tagged completo + outras restrições, e o `reportlab` tem suporte fraco a `StructTree`.
- **PDF/A-1B, PDF/UA, PDF/A-3, PDF/A-4** — fora do escopo da v1.
- **Validação rigorosa de TRC do sRGB, primaries, white point** — o `veraPDF` no job noturno `ci-nightly.yml` faz essa checagem. O PR 3 só fecha auditoria estrutural sem Java; rigor do `veraPDF` é problema do nightly, não deste PR.

**Consequência prática:** se um sistema externo (Junta Comercial, órgão público, portal de protocolo) rejeitar PDFs A-2B por ausência de Tagged, é feature, não bug — o Frederico não reivindica A-2A. Para A-2A, o usuário precisa de outro pipeline (provavelmente com `borb` ou Ghostscript, com licença compatível — fora do escopo da v1).

## Lacunas da v0.1 (que impedem "implementado")

A Etapa 5 entrega a v0.1 do PDFPro com `reportlab` (fontes Tinta & Latão embutidas, identidade visual, modo Sóbrio, 20 blocos) + auditoria bloqueante do §19.4 estrutural. **Não** entrega o pacote completo que o spec promete:

1. **Auditoria visual do §19.3** — `pypdfium2` rasteriza → checagem de grade, sobreposição, página vazia, alinhamento. Entra no PR 4 (D-AUDIT-1 já define o cache key).
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
5. **PDF/A-1B / PDF/UA / PDF/A-2A** — fora do escopo da v1.
   PDF/A-2B (nível B) é o piso de "PDF sério" hoje; PDF/A-2A
   (Tagged, accessible) e PDF/UA (acessibilidade completa)
   entram em versão futura.

A promoção pra `implementado` exige que todos os 5 itens acima
estejam fechados. **A Etapa 6 não é pré-requisito pra isso** — o
trabalho de sumário em duas passadas pode entrar em qualquer
ordem entre 5.x e 6. (Tagged PDF foi reclassificado: agora é
**não-objetivo da v1**, não lacuna — ver "Não-objetivos" e
"Escopo declarado do PDF/A-2B" acima. Lacuna é "ainda vou
fazer"; não-objetivo é "decidi não fazer nesta versão".)

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
