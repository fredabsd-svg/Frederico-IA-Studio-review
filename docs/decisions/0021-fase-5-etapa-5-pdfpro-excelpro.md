# 0021 — Fase 5 Etapa 5: `PdfPro` completo (auditoria bloqueante §19.6) + chart visual nativo Excel + identidade visual Excel

> **Revisão 1 (2026-08-01, Etapa 5 PR 3):** duas correções aplicadas
> sem mudar a numeração. (a) Erro factual em D-PDF5 e em "Mais
> difícil" / "Pendências #8": Tagged PDF **não** é requisito do
> PDF/A-2B (nível B / basic). É requisito do **nível A**
> (accessible). A v1 do PDFPro reivindica **apenas nível B**;
> Tagged vira "não-objetivo" da v1, não lacuna. (b) Adicionado
> **D-PDF6** (escolha do sRGB ICC + SHA-256 fixado) porque o PR 3
> entrega PDF/A-2B opt-in e o OutputIntent precisa de ICC embedded.
> color.org não tem mais URL estável, então geramos localmente
> (espaço sRGB = IEC standard público, formato ICC v2 = ISO 15076-1
> público). Detalhes em D-PDF6.

## Contexto

A Etapa 4 da Fase 5 fechou o `ExcelPro` v0.1 (PR #14, SHA `5460a0f` em
2026-07-31) e está registrada como `parcialmente implementado` no
[`excelpro-specification.md`](../architecture/excelpro-specification.md) com
**6 lacunas nomeadas** que mantêm o spec fora de `implementado`:

1. **Chart visual nativo** (`openpyxl.chart.BarChart` / `LineChart` /
   `PieChart`).
2. **Identidade visual Excel** (cores dos cards KPI, fill do header,
   borders, freeze panes na 1ª linha, largura automática de coluna).
3. **Tabela visual estilizada** (zebrado, header com fill, bordas
   entre células).
4. **Fórmulas Excel** como 1ª classe (campo `formula` no `Table`).
5. **Memória de cálculo** como aba oculta (`PROMPT MESTRE` §18.6).
6. **Filtros / tabelas estruturadas / validação de dados** (`PROMPT
   MESTRE` §18.1).

A Etapa 3 fechou o `WordPro` v0.1 (PR #13, SHA `17f8e8a`). O
`PdfProKit` permanece como **skeleton** (`is_implemented = false`,
`target_format() == Docx` por honestidade — `crates/document-kits/src/pdfpro.rs`)
e o enum `DocumentFormat` em
[`crates/document-kits/src/format.rs`](../../crates/document-kits/src/format.rs)
tem apenas `Docx` e `Xlsx`. O `KitRegistry::implemented_formats()`
devolve `["docx", "xlsx"]` (REGRAS §1.9 — inventário não mente).

A Etapa 5 fecha o ciclo dos 3 kits `DocumentSpec`. Pelo
[`pdfpro-specification.md`](../architecture/pdfpro-specification.md) §"Decisão
tomada" e "Aprofundar antes da Fase 5", o `PDFPro` precisa entregar:

- Auditoria bloqueante do `PROMPT MESTRE` §19.6 (visual §19.3 +
  estrutural §19.4) **dentro do salvamento do artefato**, sem
  interruptor. Reprova = artefato **não** é entregue.
- Fontes embutidas no PDF final (nenhum documento pode depender de
  fonte instalada na máquina — `PROMPT MESTRE` §5.3 final).
- Identidade "Tinta & Latão" + modo "Sóbrio" para registráveis.
- Fidelidade Word → PDF e Excel → PDF (`PROMPT MESTRE` §19.5).

E o spec do `PDFPro` lista 5 decisões adiadas da Fase 0 que precisam
ser tomadas na Fase 5: engine (`reportlab` vs `PyMuPDF`), política de
marca d'água, política de compressão, política de proteção (senha), e
política de acessibilidade (`PDF/A`).

A Etapa 5 entrega em conjunto com a parte Excel (3 das 6 lacunas
fechadas) para não deixar o chart "voando" sem o resto do pacote
visual — chart sem identidade visual ainda é funcional mas feio;
identidade visual sem chart também. A escolha de fechar as 3 primeiras
juntas é registrada em `excelpro-specification.md` §"Lacunas do v0.1".

## Decisão

### D-PDF1 — Engine de renderização e auditoria

`reportlab` (BSD-3) para render; `pikepdf` (MPL-2.0) para
auditoria estrutural e manipulação; `pypdfium2` (Apache-2.0 /
BSD-3-Clause, binding PDFium) para auditoria visual (rasterização);
`fontTools` (MIT) para checagem de glifo antes da renderização.

**PyMuPDF e `borb` foram eliminados por licença.** PyMuPDF é
AGPL-3.0 com licença comercial da Artifex; `borb` é AGPL-3.0. A
AGPL-3.0 com cláusula de rede (§13) atinge o Frederico IA Studio
quando o app é distribuído como `.exe` (work as a whole contamina
produto) e quando o modo servidor do `PROMPT MESTRE` §5.5 (caminho
2) entra em vigor (cláusula de rede dispara). Independente do mérito
técnico, a licença fecha essas duas alternativas.

**Mérito técnico também aponta para `reportlab`.** O `reportlab`
tem motor de layout de verdade (Platypus, com fluxo de elementos,
quebra de página automática, `Table` com estilos, e `multiBuild` para
renderização em duas passadas do `sumario()` sem argumentos do
`PROMPT MESTRE` §7.4). O `PyMuPDF` é excelente para ler e
manipular PDF, mas escrever com ele é de baixo nível: o caller
implementaria quebra de linha, paginação e layout de tabela — o oposto
exato da regra zero do `PROMPT MESTRE` §16.5, onde é o **kit** que
decide margem e espaçamento, não código improvisado. O `borb` é
PDF/A-first mas menos maduro que `reportlab + pikepdf`.

**`pikepdf` (binding `qpdf`) no Windows**: as wheels geralmente
trazem `qpdf` embutido, mas o bootstrap vai validar com ambiente
limpo antes de assumir — roda um `pikepdf.Pdf.open(...)` smoke após
o install e aborta se falhar.

**`pypdfium2` para rasterizar (auditoria visual §19.3)**: PDFium é
o motor de rendering do Chromium (mantido pelo Google), BSD-style.
Mais leve e menos litigioso que PyMuPDF.

**`fontTools` para checagem de glifo (`D-GLYPH-1`)**: lê o `cmap`
das Source Sans 3 / Source Serif 4 e verifica se todo caractere do
`DocumentSpec` existe na fonte. Determinístico, rápido (sem
renderização), aponta exatamente qual caractere e qual bloco é o
culpado. A rasterização via `pypdfium2` fica só para checagem de
grade, página irregular, sobreposição e alinhamento.

### D-PDF2 — Marca d'água: opt-in, com validador rejeitando Sóbrio + marca

Marca d'água é um **overlay visual** que sobrepõe o conteúdo. O
spec atual tem `DocumentSpec.confidentiality: Option<ConfidentialityMark>`
que vai como metadado (cabeçalho destacado em Tinta & Latão, nota de
rodapé em Sóbrio). A marca d'água **visual** é uma coisa separada,
controlada por **novo campo** no `DocumentMetadata`:

```rust
pub struct DocumentMetadata {
    // ... campos existentes (title, author, organization, keywords, description)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<WatermarkSpec>,
}

pub struct WatermarkSpec {
    /// Texto da marca (ex: "CONFIDENCIAL", "USO INTERNO").
    pub text: String,
    /// Posição na página.
    pub position: WatermarkPosition,  // Center | BottomRight | TopRight | Diagonal
    /// Opacidade 0.0-1.0. Default = 0.15 (visível mas não obstrutivo).
    #[serde(default = "default_watermark_opacity")]
    pub opacity: f32,
    /// Tamanho da fonte em pontos. Default depende da posição (72pt
    /// para Center/Diagonal, 14pt para Corner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}
```

**Validador (`validate_semantic` ganha regra 8):** `style == Sobrio
&& metadata.watermark.is_some() → DocumentError::Semantic`. Modo
Sóbrio é para registráveis (ata, contrato, alteração contratual) que
vão para Junta Comercial. Tarja "CONFIDENCIAL" atravessando
instrumento registrável é erro — o usuário menos vai perceber antes
de enviar, porque confia que o modo Sóbrio é limpo por definição. A
combinação é rejeitada pelo validador, não silenciosamente obedecida.

**Por que opt-in (não opt-out):** a maioria da saída do app vai para
clientes (parecer, proposta, relatório). Marca sobrando obriga a
regerar e passa vergonha; marca faltando é irrelevante (o documento
é do próprio usuário).

### D-PDF3 — Compressão: sempre JPEG q=80, configurável

PDF profissional sem compressão estoura 5 MB fácil. Padrão JPEG
quality 80 é "comprimido mas legível" para a maioria dos casos.
Configurável por DocumentSpec.metadata.compression (`None` = sem
compressão, `Some("jpeg-q80")` = padrão, `Some("jpeg-q95")` = alta
qualidade, `Some("lossless")` = flate). Default = `jpeg-q80`.

### D-PDF4 — Proteção (senha): opt-in, AES-128

Campo `DocumentSpec.metadata.protection: Option<ProtectionSpec>` com
`open_password: Option<String>`, `owner_password: Option<String>`,
`allow_print: bool` (default true), `allow_copy: bool` (default
false), `allow_modify: bool` (default false). Implementação via
`pikepdf.Pdf.save(..., encryption=pikepdf.Encryption(...))` — AES-128
é o padrão de `pikepdf` para encryption level 1.

Default sem senha (opt-in) — fricção zero para o caso comum.
Caso de uso "documento sigiloso" existe mas é minoria.

### D-PDF5 — PDF/A-2B: opt-in, com auditoria de conformidade bloqueando

PDF/A-2B é o piso de "PDF sério" hoje (fontes embutidas + XMP
+ OutputIntent com perfil ICC). **Tagged PDF NÃO é requisito do
nível B (basic) — é o que separa o nível B (basic) do nível A
(accessible).** A v1 do PDFPro declara conformidade **apenas
com o nível B**; nível A (Tagged, acessibilidade completa) está
fora de escopo. Ver `pdfpro-specification.md` §"Não-objetivos".

O que o nível B exige (audit do §19.4 verifica quando o opt-in
`pdfa: PdfA2b` é usado):
- Fontes embutidas (mesma regra do PDF comum, mas auditada)
- XMP `pdfaid:part=2` e `pdfaid:conformance=B` batendo com DocInfo
- OutputIntent com ICC profile RGB embedded
- Sem cifragem, sem JavaScript, sem `/OpenAction`
- Sem referências externas

O `reportlab` permite escrever o metadado `pdfaid:part=2` sem
que o arquivo de fato satisfaça os requisitos. Declarar
conformidade sem verificar é sistemicamente pior que um PDF
comum — sistemas de protocolo (Junta Comercial, órgãos públicos)
validam na entrada e rejeitam, ou pior, aceitam e o problema
aparece meses depois.

**Política: opt-in.** Campo `DocumentSpec.metadata.pdfa: Option<PdfaSpec>`
com `flavor: PdfaFlavor` (`PdfA2b` na v1). Quando ligado, a
auditoria estrutural do §19.4 ganha um **passo adicional de
conformidade** que falha bloqueando (como qualquer outro item do
§19.6). Sem o passo, o opt-in vira só metadado otimista.

**Validação no caminho quente**: `pikepdf` valida a estrutura do
PDF/A-2B (presença de XMP, OutputIntent, fontes embutidas, sem
JavaScript, sem ações de abertura). Não é tão rigoroso quanto
`veraPDF` (que é o validador de referência da indústria), mas pega
os casos óbvios (XMP faltando, OutputIntent faltando, etc.).

**Validação rigorosa no job noturno** (`.github/workflows/ci-nightly.yml`):
`veraPDF` roda contra os PDFs gerados com opt-in. Se um PDF passa
no `pikepdf` mas falha no `veraPDF`, o CI noturno marca como
pendência (não bloqueia PR) e gera ADR com a correção.

**Combinação de marca d'água + PDF/A-2**: PDF/A-2 aceita marca com
transparência. PDF/A-1 não aceitaria. Como PDF/A-1 não está na v1,
a combinação funciona — mas o teste cobre essa combinação para
garantir que o comportamento não regride.

### D-PDF6 — sRGB ICC: gerado localmente, SHA-256 fixado no bootstrap

O D-PDF5 exige OutputIntent com perfil ICC embedded pra
PDF/A-2B. O sRGB IEC 61966-2.1 é o perfil canônico pro
Frederico ("Tinta e Latao" = tinta/escuro sobre branco).

**Por que não usar `sRGB2014.icc` do ICC reference:** o site
`color.org` mudou de estrutura em 2026 e a URL direta
(`/srgb2014.icc`, `/profiles/srgb2014.icc`, etc.) está 404
(verificado em 2026-08-01). É o mesmo problema do `raw/main`
do `tessdata` que o ADR-0019 §Decisão 1 já pagou — URL
versionada + SHA-256 do arquivo é o antídoto. Como o ICC não
tem URL versionada estável, a alternativa é gerar localmente.

**Por que não usar `Pillow.ImageCms.createProfile("sRGB")`:** a
API não expõe `tobytes()` de forma estável entre versões. O
hash do output muda quando Pillow é bumpado. Bump de Pillow =
bump do ICC = dependência escondida que o `pip install`
resolveria silenciosamente. Ruim pra reprodutibilidade.

**Por que não vendorar o ICC no repo:** a decisão foi não
commitar binários que têm alternativa determinística (a fonte
TTF e o `tessdata` também entram via bootstrap, mesmo princípio).

**Decisão:** gerar o ICC v2 programaticamente a partir dos
parâmetros sRGB D50-adaptados (IEC 61966-2-1:1999, Anexo A,
padrão internacional público) e do formato ICC v2 (ISO 15076-1,
padrão internacional público). Script
`workers/document-worker/tools/generate_srgb_icc.py` produz
bytes determinísticos. SHA-256 do output fixado em
`workers/document-worker/bootstrap.ps1`:

```text
sRGB.icc (526 bytes): C4188E5C06585DDAD4F8781B8F8791BB4563874AC462C934D1420EA133D74B64
```

**Bump do hash = bump do `AUDIT_RULES_VERSION` (D-AUDIT-1).**
Mudança no gerador (parâmetros, formato, encoding) exige
atualizar o hash no mesmo commit. `bootstrap.ps1` falha
estruturado com mensagem clara se o SHA não bater.

**Sobre `veraPDF` (job noturno `ci-nightly.yml`):** ele vai
validar a conformidade rigorosa (incluindo TRC exato do sRGB).
Nosso ICC v2 usa gamma 2.2 (aproximação "sRGB compatível");
o EOTF exato do sRGB é piecewise. O delta é visualmente
imperceptível e o `veraPDF` no noturno vai dizer se o
aproximação passa ou se precisamos de TRC paramétrico (v4).
PR 3 só fecha a auditoria estrutural sem Java; rigor do
`veraPDF` é problema do nightly, não deste PR.

**Licença:** o espaço sRGB é IEC standard público; o ICC
profile gerado a partir dele é public domain na prática (os
parâmetros numéricos do padrão não têm copyright). O gerador
segue a licença do projeto.

### D-CONF-1 — Correção do §7.1 do `PROMPT MESTRE`

O `PROMPT MESTRE` §7.1 sugeria `confidencial=True` como padrão para
`DocumentSpec.confidentiality`. A experiência real mostra que isso
está errado. Ata, contrato e alteração contratual vão para Junta
Comercial sem marca; a maioria da saída (parecer, proposta,
relatório) vai para clientes sem necessidade de tarja. O default
correto é **`None` (público)**, e o código atual já está nesse
caminho — `confidentiality: Option<ConfidentialityMark>` com
`#[serde(default, skip_serializing_if = "Option::is_none")]`. Esta
ADR formaliza o que o código já pratica e impede que uma
"correção" lendo o spec antigo reverta a política.

### D-CHART-1 — Chart visual nativo Excel + remoção do warning de degradação

`ExcelProKit` ganha render de `openpyxl.chart.BarChart` /
`LineChart` / `PieChart` real, com cores da identidade "Tinta &
Latão". Fechamento da lacuna 1 do ExcelPro.

**Aviso de degradação "chart virou tabela" sai no mesmo PR.** O
aviso foi combinado para a v0.1 (Etapa 4, ADR-0020 §3) com a
intenção de tirar quando o chart entrasse de verdade. Se ficar,
passa a mentir na direção oposta — vai dizer que o gráfico virou
tabela quando não virou mais. `excelpro-specification.md` §"Lacunas
do v0.1" é atualizado junto.

**Formatos numéricos BRL/PCT/THOUSANDS/INT** (Etapa 4) continuam
fortes — chart nativo não pode introduzir regressão na coluna de
valor. Os testes E2E (Etapa 4) que validam `has_brl_format` e
`has_pct_format` permanecem e ficam ainda mais críticos.

### D-AUDIT-1 — Cache da auditoria visual: hash + versão das regras

A chave do cache da auditoria visual (`pypdfium2` rasteriza →
checa grade, sobreposição, página vazia) tem que incluir a
**versão das regras de auditoria**, não só o hash do `.pdf`. Caso
contrário, ao apertar uma regra, arquivos antigos continuam
passando pelo cache antigo (defeito silencioso). Formato:

```text
cache_key = sha256(pdf_bytes) + ":" + AUDIT_RULES_VERSION
```

Onde `AUDIT_RULES_VERSION` é uma constante bumpada a cada mudança
nas regras de auditoria. `AUDIO_RULES_VERSION = "0.1.0"` no PR 1;
bumps em PRs futuros que mudam regras.

### D-GLYPH-1 — Checagem de glifo via `fontTools` ANTES de renderizar

Detectar tofu (caixa vazia no lugar do caractere) por pixel é caro
e frágil. Com `fontTools.ttLib.TTFont` (MIT), lê-se o `cmap` da
fonte e verifica-se se todo caractere do `DocumentSpec` existe na
fonte. O check é determinístico, rápido, e aponta exatamente
**qual caractere** e **qual bloco** é o culpado.

A rasterização via `pypdfium2` fica só para checagens de **grade
e página irregular** (não detecção de glifo). Falha no glifo-check
= `KitError::AuditFailed` com `code: "pdf_glyph_missing"`.

### D-FAIL-1 — Hard-fail de dependências de auditoria

Se `pikepdf` ou `pypdfium2` ou `fontTools` faltarem, o
`bootstrap.ps1` **falha com exit 1**. Sem "plano B" silencioso.

Justificativa: §19.6 diz que a auditoria não tem interruptor. Um
"plano B" que roda auditoria parcialmente e devolve verde é o
mesmo problema do interruptor com nome diferente. As 3
dependências são o **mínimo** para a Etapa 5 entregar o que
promete (auditoria estrutural + visual + glifo-check).

Alternativa documentada para caso degradado conhecido (não
encobertado): o `KitOutput` da auditoria pode reportar
`coverage: "partial"` com lista explícita do que **não** foi
checado. Esse modo é opt-in do caller (teste, debug), não do
runtime — o caminho quente sempre roda cobertura completa.

### D-INSPECT-1 — `docs.inspect` cobrindo `.pdf`: registrado como pendência 5.x

O handler `pdf.read` da v0.3.0 devolve `text` (camada de texto) +
`ocr_text` (páginas escaneadas via Tesseract) + `page_count` +
`scanned_pages`. Não reconstrói `DocumentSpec` estrutural — não
tem como saber "este Heading 1 corresponde ao `block_index=2`" ou
"esta KpiCard deveria ter 4 cartões mas só tem 3". O round-trip
de PDF para `DocumentSpec` é não-trivial e registrado como
pendência 5.x.

Pendência 5.x #1 da lista abaixo.

### Bumps atômicos do PR 1 (REGRAS §1.9, REGRAS §1.13)

- `DocumentFormat::Pdf` adicionado ao enum. Bump atômico: o
  `KitRegistry::implemented_formats()` volta de `["docx", "xlsx"]`
  para `["docx", "xlsx", "pdf"]` no mesmo commit. O `PdfProKit`
  ganha `is_implemented() == true` e `target_format() ==
  DocumentFormat::Pdf` no mesmo commit. Inventário não mente.
- `SpecVersion` 0.1.0 → 0.2.0. **MINOR** porque adiciona campo
  opcional `watermark: Option<WatermarkSpec>` no `DocumentMetadata`
  com `#[serde(default, skip_serializing_if = "Option::is_none")]` —
  backward-compat: spec 0.1.0 desserializa em 0.2.0 com watermark =
  None. Default da `SpecVersion` em `spec.rs` muda de `"0.1.0"` para
  `"0.2.0"`.
- `DocumentMetadata` ganha campo `watermark: Option<WatermarkSpec>`
  + struct `WatermarkSpec` + enum `WatermarkPosition`. Tudo
  opcional.
- `validate_semantic` ganha regra 8 (Sobrio + watermark).
- `pyproject.toml` do `document-worker` ganha
  `pikepdf>=9.0`, `pypdfium2>=4.0`, `fonttools>=4.50`.
- `manifest.json` do `document-worker` ganha as 3 dependências.
- `bootstrap.ps1` instala as 3 libs no mesmo `pip install` que já
  tem `python-docx openpyxl reportlab pdfplumber` + hard-fail se
  faltar. A verificação final (`# 2. Bibliotecas`) é estendida para
  incluir as 3 novas.

## Sequência de PRs

A Etapa 5 entrega em 5-6 PRs, todos com CI verde antes de merge:

- **PR 1 (fundação)** — este ADR + bump atômico do enum + bump da
  `SpecVersion` 0.1.0 → 0.2.0 + `DocumentMetadata.watermark` +
  `validate_semantic` regra 8 + bootstrap + manifest + pyproject +
  `PdfProKit` skeleton com `is_implemented() == true` mas `render`
  retornando `KitError::NotImplemented { etapa: "5.v0.1" }`. Suíte
  do `document-kits` verde. ADR-0021 mergeado em paralelo.
- **PR 2 (PDFPro v0.1)** — `render` real do `DocumentSpec` em
  `.pdf` com fontes Tinta & Latão embutidas (sem fallback
  Helvetica/Times), identidade visual "Tinta & Latão" + modo Sóbrio,
  cobertura dos 20 blocos (Cover, Toc, Heading, Paragraph, List,
  Table, KeyValue, Kpis, Callout, Quote, Steps, Chart via
  `reportlab.graphics`, Image, Code, Divider, Spacer, PageBreak,
  Footer, Signatures, BackCover), glifo-check via `fontTools` antes
  de render. Suíte unit + integration do `document-kits` verde.
  E2E do PDF (gera, abre, confere `n_pages` + textos-chave) verde.
- **PR 3 (auditoria estrutural §19.4)** — `pikepdf` valida
  abertura, `n_pages`, metadados, fontes embutidas, sem cifragem,
  sem referências externas. **PDF/A-2B só quando o opt-in for
  usado** (D-PDF5) — verifica OutputIntent com ICC sRGB (D-PDF6),
  XMP `pdfaid` batendo com DocInfo, sem JavaScript. **Tagged
  PDF não é verificado** — A-2B não exige; v1 não reivindica
  nível A. Falha = `KitError::AuditFailed` com
  `code: "pdf_audit_structural_failed"` + motivo legível.
  10+ testes negativos injetando falha. Cache key = hash do
  `.pdf` + `AUDIT_RULES_VERSION` (`D-AUDIT-1`).
- **PR 4 (auditoria visual §19.3)** — `pypdfium2` rasteriza página
  → checagem de grade, sobreposição, página vazia, alinhamento,
  cabeçalho/rodapé. **glifo-check via `fontTools` já feito no PR 2**
  (não duplica). Falha = `KitError::AuditFailed` com
  `code: "pdf_audit_visual_failed"`. 10+ testes negativos. Cache
  key = hash + versão.
- **PR 5 (chart nativo Excel + identidade visual Excel + tabela
  estilizada + warning sai)** — `openpyxl.chart` real + cores
  Tinta & Latão nos cards KPI + fill do header + borders + freeze
  panes + largura automática + tabela com zebrado, header com fill
  e bordas. Fecha 3 das 6 lacunas do ExcelPro. Spec
  `excelpro-specification.md` atualizado: lacunas 1, 2, 3 fechadas;
  lacunas 4, 5, 6 vão pra Etapa 5.x. **Warning de degradação
  removido no mesmo PR.**
- **PR 6 (fechamento)** — E2E completo do PDFPro (gera, auditoria
  passa, reabre e confere estrutura), bump do `status.md` Etapa 5
  → concluída, specs `pdfpro` → `implementado` e `excelpro` →
  `implementado` (3 lacunas fechadas, 3 nomeadas como 5.x),
  CHANGELOG v0.4.0, `verify-external.ps1` cobre o E2E do PDF.

## Alternativas descartadas

- **PyMuPDF (`fitz`) como engine.** Descartada por licença:
  AGPL-3.0 com cláusula de rede (§13) atinge o produto
  distribuído como `.exe` e o modo servidor do `PROMPT MESTRE`
  §5.5. Independente do mérito técnico, fecha a alternativa.
- **`borb` como engine.** Descartada pela mesma razão (AGPL-3.0)
  e por maturidade — `borb` é PDF/A-first mas menos battle-tested
  que `reportlab + pikepdf`.
- **Plano B silencioso para auditoria** ("se pikepdf faltar,
  auditoria cobre só o que `reportlab + pdfplumber` leem, devolve
  verde"). Descartada por violar §19.6 — auditoria silenciosamente
  reduzida devolvendo verde é o mesmo problema do interruptor com
  outro nome. `D-FAIL-1` exige hard-fail.
- **PDF/A-2B como padrão** (todos os PDFs saem afirmando ser
  arquiváveis). Descartada — `reportlab` escreve o metadado sem
  satisfazer os requisitos; PDF que afirma conformidade e não
  tem é pior que PDF comum (rejeitado em protocolo de órgão
  público, ou aceito e o problema aparece meses depois).
  `D-PDF5` exige opt-in + auditoria obrigatória quando ligado.
- **Marca d'água padrão-ligada** (sempre, exceto se desabilitar
  explicitamente). Descartada — quebra o caso mais sensível (modo
  Sóbrio para registráveis). `D-PDF2` exige opt-in + validador
  rejeita Sobrio + marca.
- **Tofu-detection por pixel** (rasterizar e procurar caixas
  vazias). Descartada por caro e frágil. `D-GLYPH-1` usa
  `fontTools` cmap — determinístico, rápido, aponta caractere e
  bloco.
- **`docs.inspect` cobrindo `.pdf`** como escopo da Etapa 5.
  Descartada — handler `pdf.read` só dá texto+ocr, não reconstrói
  `DocumentSpec`. Não-trivial. Vai pra Etapa 5.x.
- **Chart nativo sem identidade visual** (só chart, sem cores,
  sem freeze panes). Descartada — chart "voando" sem o resto do
  pacote visual é funcional mas feio; identidade visual sem chart
  também. `D-CHART-1` fecha as 3 primeiras lacunas juntas.
- **Fórmulas Excel como 1ª classe** (campo `formula` no `Table`).
  Descartada por escopo da Etapa 5 — fórmulas Excel arrastam
  recálculo, e a regra do `PROMPT MESTRE` §7.3 diz que `salvar()`
  falha se alguma fórmula der erro. Isso é etapa própria (5.x),
  não apêndice da Etapa 5.
- **Filtros / validação de dados no Excel** (lacuna 6).
  Descartada por escopo da Etapa 5 — vai com fórmulas e memória
  de cálculo no pacote da Etapa 5.x.

## Consequências

**Mais fácil:**

- A Etapa 5 fecha o ciclo dos 3 kits `DocumentSpec` (Word, Excel,
  PDF). O modelo pode emitir um spec e o `docs.generate` produz
  o formato pedido sem que o caller saiba qual engine está
  embaixo. Inventário que reflete a realidade (REGRAS §1.9).
- Auditoria bloqueante (§19.6) garante que PDF "seriinho" só
  sai se a auditoria passou. Caso de uso "documento para
  cliente" / "registrável para Junta" sai com fontes embutidas
  e grade auditada, sem precisar de pós-processamento manual.
- Licenças livres (BSD-3, MPL-2.0, Apache-2.0, MIT) não
  contaminam o app `.exe` nem o modo servidor (§5.5). Defensável
  contra auditoria de IP.
- Glifo-check determinístico (`fontTools`) pega o caso "fonte não
  tem o caractere" antes de render — sem desperdiçar renderização
  e sem dar mensagem genérica de "o .pdf não abriu".
- Cache da auditoria com versão de regras (`D-AUDIT-1`) evita o
  defeito "regra apertada, arquivo antigo passa pelo cache".
- Chart nativo Excel fecha a maior lacuna visual do ExcelPro v0.1
  (degradação que virava "chart virou tabela" sai). Identity
  visual Excel vira o .xlsx profissional, não o "funcional mas
  sem graça" da v0.1.
- Correção do §7.1 (`D-CONF-1`) documenta formalmente o que o
  código já pratica (default = público) e impede regressão.

**Mais difícil:**

- **Tagged PDF no `reportlab` é fraco, mas o PDF/A-2B não
  exige Tagged** (é requisito do nível A, não do B). A v1 do
  PDFPro reivindica conformidade apenas com o nível B;
  promoção pra nível A (Tagged, acessibilidade) exige escrever
  `StructTree` XML manualmente. Mitigação: a v0.1 do `PDFPro`
  entrega **PDF 1.7 com fontes embutidas e grade auditada**;
  `Tagged PDF` registrado como **não-objetivo explícito da v1**
  (pertence ao PDF/A-2A, fora de escopo do B). Se um dia
  reivindicar nível A, vira pendência 5.x.
- **Auditoria visual é cara** (render de N páginas = subprocess
  `pypdfium2` + checagem pixel a pixel). Mitigação: cache
  (`hash + AUDIT_RULES_VERSION`); render em `tokio::task::spawn_blocking`
  pra não bloquear o runtime; imagens temporárias em
  `tempfile::TempDir` (limpas ao final).
- **`pikepdf` no Windows pode ter problema com wheels** (embora
  as wheels geralmente tragam `qpdf` embutido). Mitigação: o
  `bootstrap.ps1` valida com ambiente limpo após install
  (`pikepdf.Pdf.new()` smoke). Se falhar, exit 1.
- **`pypdfium2` exige PDFium no Windows** (geralmente bundled).
  Mesma mitigação do `pikepdf` — smoke test no bootstrap.
- **`fontTools` para checar cmap é trabalho manual** (precisa
  abrir cada TTF, ler cmap, normalizar texto do spec para o
  formato que o cmap espera). Mitigação: função pura em
  `crates/document-kits/src/pdfpro/glyphs.rs` com testes cobrindo
  acentos (`é`, `ã`, `ç`), caracteres especiais (`R$`, `%`,
  `/`), e emoji rejeitado (a v1 do spec é pt-BR; emoji não
  deveria entrar).
- **Bumps cascata** (`SpecVersion` 0.1.0 → 0.2.0, novo campo
  `watermark`, regra semântica nova) são alterações de contrato
  que o modelo precisa enxergar. Mitigação: backward-compat
  garantido por `#[serde(default, skip_serializing_if = ...)]` no
  novo campo, e a regra nova é um erro estruturado claro
  (`DocumentError::Semantic` com mensagem "watermark não pode
  ser usado com DocumentStyle::Sobrio").
- **3 das 6 lacunas do ExcelPro ficam pra 5.x** (fórmulas,
  memória de cálculo, filtros). O spec do `excelpro-specification.md`
  continua `parcialmente implementado` mesmo depois da Etapa 5
  fechada — porque ainda há 3 lacunas honestas. Promoção pra
  `implementado` fica pra Etapa 5.x quando essas 3 fecharem.

## Pendências para a próxima sessão

**Etapa 5.x (registradas no `excelpro-specification.md` e
`pdfpro-specification.md`, não silenciadas):**

1. `docs.inspect` cobrindo `.pdf` (round-trip com `coverage`,
   tipo o que já existe pra `.docx`/`.xlsx`).
2. `range` real no `docs.inspect` (`range=A1:D10` filtra
   `first_rows` e `n_rows` no output). Hoje é só flag.
3. `DATE_BR` formato numérico (alias pro `cell.number_format`
   `"dd/mm/yyyy"`).
4. Auto-detecção de formato no `docs.inspect` por magic bytes
   (sem depender da extensão do path).
5. **Fórmulas Excel** como 1ª classe (campo `formula` no
   `Table`).
6. **Memória de cálculo** como aba oculta (`PROMPT MESTRE` §18.6).
7. **Filtros / tabelas estruturadas / validação de dados**
   (`PROMPT MESTRE` §18.1).
8. **Tagged PDF** no `reportlab` (escrita manual de `StructTree`
   XML) — pre-requisito pra **PDF/A-2A (accessible)** entrar
   como conformidade declarada. **Não** bloqueia PDF/A-2B
   (básico), que é o que o PR 3 desta Etapa entrega opt-in.
   O `reportlab` tem suporte fraco a StructTree, e a v1
   reivindica apenas nível B.
9. **Fidelidade Word → PDF e Excel → PDF** (`PROMPT MESTRE` §19.5).
10. **Sumário automático em duas passadas** (`PROMPT MESTRE`
    §16.4) — requer `multiBuild` do `reportlab` + integração com
    o `Toc` block do `DocumentSpec`.

**Etapa 6 (registradas, mas Etapa 6 é outra conversa):**

- Identidade visual Word no `.docx` ("Tinta & Latão" via
  `python-docx` estendido).
- Tabela real no `.docx` (com grade, células formatadas — hoje
  vira texto tab-separado).
- UI do modo documental na casca Tauri.

**Pendência eterna (fora do escopo da v1):**

- `PROMPT MESTRE` §7.1 dizia `confidencial=True` como padrão.
  `D-CONF-1` corrige o código (default = público) e documenta.
  Se um dia o spec for revisto, o caminho de volta precisa de
  ADR explicando o motivo (registrável em modo Sóbrio, etc.).

## Referências

- [ADR-0004](0004-document-worker-em-python-embutido.md) —
  Python embeddable + libs base.
- [ADR-0017](0017-process-architecture-windows-pipes.md) —
  transporte sobre named pipes.
- [ADR-0018](0018-document-worker-handlers-primitive.md) —
  handler como primitiva, kit como renderer. Os 7 handlers da
  v0.3.0 do `document-worker` sobrevivem à Etapa 5 sem
  reescrita.
- [ADR-0019](0019-document-worker-ocr-tesseract.md) — Tesseract
  bootstrap + `ocr.run` + fallback OCR no `pdf.read`.
- [ADR-0020](0020-fase-5-etapa-4-excelpro-inspect.md) — Etapa 4
  ExcelPro v0.1 + `docs.inspect` + 6 lacunas nomeadas.
- [`docs/architecture/document-engine-architecture.md`](../architecture/document-engine-architecture.md)
  — `DocumentSpec` v0.1 (20 blocos).
- [`docs/architecture/excelpro-specification.md`](../architecture/excelpro-specification.md)
  — atualizado com D-CHART-1 (lacunas 1-3 fechadas, 4-6
  nomeadas como 5.x).
- [`docs/architecture/pdfpro-specification.md`](../architecture/pdfpro-specification.md)
  — atualizado com D-PDF1 a D-PDF5 (engine + 4 políticas).
- [`docs/architecture/development-roadmap.md`](../architecture/development-roadmap.md)
  — Fase 5, segundo fluxo vertical do `PROMPT MESTRE` §33.
- `PROMPT MESTRE` §5.3 (fontes embutidas), §5.5 (modo servidor),
  §7.1 (confidencialidade — corrigido por D-CONF-1), §7.3
  (fórmulas), §7.4 (sumário), §16.4-§16.6 (suíte profissional),
  §17-§19 (Word/Excel/PDF), §19.3-§19.6 (auditoria PDF),
  §22.5 (env allowlist), §33 (fluxos verticais).
