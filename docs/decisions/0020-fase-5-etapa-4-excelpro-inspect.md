# 0020 — Fase 5 Etapa 4: `ExcelPro` real (Spreadsheet → `.xlsx`) + `docs.inspect` (round-trip parcial)

## Contexto

A Etapa 3 da Fase 5 fechou `docs.generate` (tool único exposto ao modelo) +
`WordPro` mínimo + `WorkerToolDispatcher` com path safety forte (PR #13
mergeado em `main` = `17f8e8a` em 2026-07-31). O `ExcelProKit` nasceu como
**skeleton** (`is_implemented = false`) na Etapa 3 — REGRAS §1.9 aplicada
literalmente: inventário não mente, `Xlsx` só entra no enum `DocumentFormat`
junto com o kit implementado. A Etapa 4 entrega:

1. **`ExcelProKit` v0.1** (real, substitui o skeleton) — renderiza
   `DocumentSpec` (`DocumentType::Spreadsheet`) em `.xlsx` real, cobrindo
   `Kpis` + `Table` + `Chart`.
2. **`docs.inspect`** (tool de round-trip) — abre um `.docx` ou `.xlsx` já
   gerado e devolve um `DocumentSpec` parcial + `coverage` do que foi
   preservado e do que se perdeu. Caso de uso: o modelo recebe um
   documento existente (anexo do usuário, por exemplo) e quer entender
   a estrutura sem "re-ler tudo do zero".
3. **Formatos numéricos brasileiros** (moeda R$, percentual, milhar) via
   `column_formats` no `xlsx.write` — antes do chart real no Excel
   (D2 do plano: prioridade invertida — formatos numéricos são mais
   úteis no cotidiano contábil do que chart nativo do Excel).
4. **Chart SEM aba `Charts_<n>`** (D1) — o `openpyxl` tem
   `add_chart(...)`, mas isso adiciona uma aba que não tem dados
   (só a figura). Decidido registrar o chart no **Painel** (cumulativa,
   primeira aba) e mandar os dados do chart para a próxima `Table`
   compatível em nº de linhas. Quando não houver, fica só como
   registro no Painel + **warning explícito** na resposta do
   `docs.generate`. Nunca silencioso.
5. **`docs.inspect` cobre `.xlsx` também** (D4) — modo resumo padrão
   (nomes, intervalo, header, contagens, amostra de 5 linhas, `has_total`,
   `currency_format` por coluna); `range` opcional (devolve intervalo
   pedido); `sheet` opcional (filtra 1 sheet). O `sheets: [{block_index,
   sheet_name}]` do `generate` fecha o ciclo: o `inspect` confirma o
   que o `generate` declarou.
6. **Definição de pronto do E2E do ExcelPro** (D5) — gerar via
   `docs.generate`, reabrir via `docs.inspect` (modo resumo), afirmar
   estrutura: Painel 1ª aba, 1 sheet por `Table`, linha de `TOTAL`
   presente, formato de moeda aplicado. **Round-trip pela mesma
   porta que o modelo usa** (não pelo handler direto).

## Decisão

### 1. Bump atômico do enum `DocumentFormat::Xlsx` (REGRAS §1.9)

`DocumentFormat::Xlsx` entrou no enum **junto** com `ExcelProKit`
implementado. O `KitRegistry::implemented_formats()` passou de
`["docx"]` para `["docx", "xlsx"]` no mesmo commit (commit 2 da
Etapa 4). Stubs (`PdfProKit`) continuam `is_implemented = false` e
ficam fora do schema — inventário não mente.

### 2. `KitOutput` ganha `sheets: Vec<SheetMapping>` e `warnings: Vec<String>`

Campos novos no `KitOutput` (commit 2):

```rust
pub struct KitOutput {
    pub output_path: PathBuf,
    pub sheets: Vec<SheetMapping>,   // [(block_index, sheet_name)] — .xlsx
    pub warnings: Vec<String>,        // ["chart_* renderizado apenas como registro..."]
}

pub struct SheetMapping {
    pub block_index: usize,
    pub sheet_name: String,
}
```

`KitOutput::simple(path)` helper para kits sem sheets (WordPro) —
encapsula `sheets: vec![]`, `warnings: vec![]` no caso comum.

### 3. Chart SEM aba `Charts_<n>` — D1 do plano

`openpyxl` tem `Workbook.create_sheet()` + `worksheet.add_chart(...)`,
mas o resultado seria uma aba vazia (só a figura do chart) ao lado
das abas com dados. Decidido **não** criar aba para chart.

Em vez disso, o `ExcelProKit`:
- Para cada `DocumentBlock::Chart` no spec:
  1. Procura a próxima `Table` compatível (mesmo nº de linhas que
     `labels`). Se encontrar, embute os dados do chart na próxima
     linha vazia da `Table` (com prefixo `chart_<title>: <kind>`).
  2. Se **não** encontrar `Table` compatível, registra no Painel
     (cumulativa, primeira aba — `Kpis` table) com
     `kind=<kind>`, `title=<title>`, `ref=<Table bloqueada se houver>`.
  3. **Sempre** adiciona um warning no `KitOutput.warnings`:
     `"chart_<title> renderizado apenas como registro no Painel;
     chart nativo previsto para a Etapa 5/6"`.

A premissa: o usuário contábil brasileiro abre o `.xlsx` no Excel
e **vê a estrutura tabular** (dados). O chart visual é nice-to-have,
não bloqueia o uso real.

**Limitação registrada no `excelpro-specification.md`:** chart nativo
do Excel (com `openpyxl.chart.BarChart` / `LineChart` / `PieChart`
real) entra na Etapa 5/6, junto com a identidade visual "Tinta &
Latão" para Excel (cores, fills, borders, freeze panes).

### 4. Formatos numéricos via `column_formats` — D2 do plano

Estendido o handler `xlsx.write` no `document-worker.py` (commit 1)
para aceitar **opcionalmente** `column_formats: {col_idx: format_str}`
no payload. Sem `column_formats`, comportamento idêntico ao v0.2.0
— **backward-compat** (todos os testes antigos continuam verde).

Aliases (case-insensitive):

| Alias        | openpyxl `cell.number_format`            | Exemplo                 |
| ------------ | ---------------------------------------- | ----------------------- |
| `BRL`        | `"$#,##0.00"`                            | `R$ 1.234,56` (visual)  |
| `PCT`        | `"0.00%"`                                | `12,50%`                |
| `THOUSANDS`  | `"#,##0"`                                | `1.234.567`             |
| `INT`        | `"0"`                                    | `42`                    |
| (string raw) | passada direto pra `cell.number_format`  | `"0.0000"`              |

Heurística no `ExcelProKit` (Rust) para `column_formats`:
- `Table.currency = "BRL"` → aplica `"BRL"` em todas as colunas de
  dados (pula 1ª coluna se houver > 1 coluna, pra não formatar o
  label "Mês" como moeda).
- `Table.percent = true` → aplica `"PCT"` em todas as colunas de dados.
- `Table.thousands = true` → aplica `"THOUSANDS"` em todas as colunas
  de dados.
- `Kpis.format = "BRL"` → aplica `"BRL"` no valor numérico de cada card.

**Limitação registrada:** o `openpyxl` aplica o formato na exibição da
célula, mas o `value` numérico subjacente é `int`/`float` Python
(1.23456), não uma string formatada. Excel abre e mostra
`R$ 1.234,56` mas internamente o número é 1.23456. Isso é o
comportamento esperado — o formato é **visual**, não transforma
o tipo. Caller que precisa de string formatada (ex: PDF/A) usa
`docs.inspect` ou um formatador explícito.

**Pendente pra Etapa 5:** identidade visual Excel (cores dos cards
KPI, fill do header, borders, freeze panes na primeira linha,
largura automática de coluna). Ficou fora da Etapa 4 pelo orçamento
— D2 já era "formatos numéricos brasileiros no `xlsx.write` (dentro
do orçamento)".

### 5. Sanitização de sheet name em Rust (não no Python)

`xlsx.write` recebe `sheet_name` já sanitizado pelo `ExcelProKit`.
Função pura `sanitize_sheet_name(proposed: &str, block_index: usize,
used: &mut HashSet<String>) -> String` (em `sheet_name.rs`):

- **Max 31 chars** (regra do Excel — silenciosamente trunca além disso
  e pode colidir com sheets já existentes).
- **Remove** `\ / ? * [ ] :` (caracteres ilegais no Excel — `xlsx.write`
  daria erro genérico do openpyxl).
- **Strip** whitespace no início e no fim.
- **UTF-8 safe** — `chars().take(31)` (não `bytes().take(31)` que
  quebraria no meio de um char multibyte).
- **Fallback** `Table_<i>` se ficar vazio após sanitização.
- **Colisão** resolvida com sufixo `_2`, `_3`, ..., `_999` (testado
  com `Table_<longo>` colidindo contra `Table_<longo>` após corte).
- **Acentos preservados** (Excel aceita "Receitas por Mês" — `é` é
  char válido, não cai na lista de ilegais).

13 testes em `sheet_name.rs`:
- vazio → `Table_<i>`
- só forbidden chars → `Table_<i>`
- 80 chars → cortado pra 31
- 31 chars exato → passa
- 32 chars → cortado pra 31
- acento (`é`, `ã`, `ç`) → preservado
- barra → removida
- dois pontos → removido
- whitespace nas pontas → stripado
- colisão `Table_R` (cabe em 31) + `Table_R` (cabe em 31) → 1º passa,
  2º vira `Table_R_2`
- colisão após corte: `Table_<60 chars>` ×2 → 1º vira `Table_<30>`,
  2º vira `Table_<30>_2`
- `Table_R` (cabe) vs `Table_R_with_extra_text` (não cabe) → 1º fica
  `Table_R`, 2º corta pra 31 e... se ficar diferente, sem colisão
  (testado: `"A".repeat(31)` vs `"A".repeat(31) + "B"` → 1º
  `A×31`, 2º vira `A×30` + colisão detectada → `A×30_2`)

### 6. Mapeamento de blocos → sheets — D3 do plano

`ExcelProKit::render` mapeia:

| Bloco                | Sheet                              |
| -------------------- | ---------------------------------- |
| `Kpis`               | `Painel` (sempre 1ª aba, cumulativa — KPIs vão se acumulando se houver mais de um `Kpis` no spec) |
| `Table` (com title)  | `<title>` sanitizado               |
| `Table` (sem title)  | `Table_<i>` (i = posição do bloco no spec) |
| `Chart` (com Table compatível) | Próxima `Table` (dados embutidos) |
| `Chart` (sem Table compatível) | Só registro no Painel + warning |

**`Painel` é a 1ª aba e é cumulativa**: se o spec tem 2 `Kpis`
separados (ex: "KPI de vendas" + "KPI de estoque"), o `Painel`
consolida os 2 em uma única tabela de KPIs (com separador "—" ou
linha em branco entre os 2 grupos) e registra **um warning**
informando o caller.

**`Painel` também lista os "Gráficos previstos"** (D3.2): linhas
extras no Painel com `label = "Gráfico previsto: <kind> <title> —
ref: <Table>"` quando há chart sem Table compatível.

Output do `generate` (no `KitOutput.sheets`):

```json
"sheets": [
  {"block_index": 0, "sheet_name": "Painel"},
  {"block_index": 2, "sheet_name": "Receitas por Mes"},
  {"block_index": 3, "sheet_name": "Crescimento Mensal"}
],
"warnings": [
  "chart_previsto_bar_Tendencia renderizado apenas como registro no Painel; chart nativo previsto para a Etapa 5/6"
]
```

### 7. `docs.inspect` cobre `.docx` E `.xlsx` — D4 do plano

`DocsInspectTool` (em `crates/document-kits/src/inspect.rs`):

- **Input**: `path` (obrigatório), `format` (opcional, inferido pela
  extensão), `sheet` (só `.xlsx`, filtra 1 sheet), `sample_rows`
  (default 5, max 20), `range` (opcional, **v0.1 = flag apenas** —
  não aplica o range, sempre devolve o summary completo).
- **Output**:
  - `spec`: `DocumentSpec` parcial reconstruído.
  - `coverage`: `{ preserved: [str], lost: [str] }` — `preserved` é
    o que o inspect extraiu; `lost` é o **catálogo hardcoded** do
    que o inspect v0.1 não sabe ler (cover, kpis, callout, quote,
    steps, chart, image, code, footer, signatures, backcover, toc,
    keyvalue, list).
  - `sheets`: array de `SheetSummary` (só `.xlsx`); para `.docx`,
    fica vazio.

**`SheetSummary`** (apenas `.xlsx`):
- `name`, `used_range`, `headers`, `n_rows`, `n_cols`
- `first_rows: Vec<Vec<String>>` (amostra, `sample_rows` linhas)
- `has_total: bool` (heurística: última linha começa com "Total",
  case-insensitive)
- `column_formats: {col_idx: alias}` (mapa reverso do `xlsx.read`
  Python — devolve o alias, não o `number_format` cru)

**`build_docx_spec` reescrito** (commit 4, Etapa 4) para usar
**style real do python-docx** (campo `style` no dict de cada
parágrafo) ao invés de heurística de string match em "Heading 1 "
que nunca batia (python-docx não prefixa o style no texto).
- Quebra de contrato intencional: `docx.read` agora devolve
  `paragraphs: [{text, style}]` ao invés de `[str]`. Caller que
  dependia de `[str]` precisa migrar (CHANGELOG registra).
- `e2e_docx_write_and_read` em `process-architecture` atualizado
  pra extrair o campo `text` do dict.

**Limitação documentada do inspect .docx:** Table vira texto
tab-separado no `.docx` (limitação do **gerador** WordPro v0.1,
Etapa 3 — `docx.write` v0.3.0 não tem cobertura direta pra
tabela). O inspect .docx não tem como distinguir tabela real de
texto tab-separado. **Resultado:** `coverage.preserved` no inspect
.docx não tem `table` (a tabela nunca existiu como tabela no
`.docx`); `coverage.lost` é o catálogo hardcoded (que **não**
inclui `table` — o inspect sabe ler tabela real, é o gerador
que não escreve). O inspect .xlsx preserva `Table` corretamente.

### 8. E2E do ExcelPro + E2E do inspect

Dois E2Es novos em `crates/document-kits/tests/`:

**`e2e_docs_generate_xlsx.rs`** (1 teste, full vertical):
- Spec com `Kpis` (2 cartões) + `Table` (com `total`, `currency="BRL"`)
  + `Table` (com `percent=true`) + `Chart` (sem Table compatível).
- Gera `.xlsx` via `docs.generate` (cobre `WorkerToolDispatcher` +
  `ExcelProKit` + `xlsx.write`).
- Python subprocess com `openpyxl` reabre o `.xlsx` e imprime:
  - `CHECK first_sheet=Painel`
  - `CHECK n_sheets=3`
  - `CHECK sheet_names=Painel,Receitas por Mes,Crescimento Mensal`
  - `CHECK has_total=True`
  - `CHECK has_brl_format=True` (tem célula com `number_format == "$#,##0.00"`)
  - `CHECK has_pct_format=True` (tem célula com `number_format == "0.00%"`)
  - `CHECK charts_sheet_count=0` (chart SEM aba `Charts_<n>`)
- Rust parseia as linhas `CHECK` e assertiona.
- Valida o `output.sheets` do `generate`: contém `Painel` +
  `Receitas por Mes` + `Crescimento Mensal`, na ordem dos blocos.
- Valida o `output.warnings`: tem 1 warning de chart.

**`e2e_docs_inspect.rs`** (1 teste, full vertical):
- Spec com `Cover` + 2 `Heading` (Heading 1 + Heading 2) + 1
  `Paragraph` + 1 `Table` (vira texto tab-separado no `.docx`).
- Gera `.docx` via `docs.generate`, depois roda `docs.inspect` no
  mesmo arquivo.
- Valida:
  - `coverage.lost` inclui `cover` (Cover não é reconstruído).
  - `coverage.lost` **NÃO** inclui `table` (inspect sabe ler; a
    limitação é do gerador).
  - `coverage.preserved` inclui `heading` e `paragraph`.
  - `coverage.preserved` **NÃO** inclui `table` (WordPro v0.1
    não gera tabela real).
  - `spec.blocks` tem 2 headings (Visao geral, Detalhe).
  - 0 tables em `spec.blocks` (WordPro v0.1 vira Table em texto).
  - 0 covers em `spec.blocks` (Cover lost).
  - `sheets` vazio (inspect de `.docx`).

Ambos `#[cfg(windows)]` com `python_exe_or_panic()` (REGRAS §2.6).
Adicionados ao `scripts/verify-external.ps1` como 2 steps novos
("E2E docs.generate xlsx" + "E2E docs.inspect"). Rodam no
`windows-latest` em todo PR.

## Travas de CI

- `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace`, `scripts/check-core-purity.ps1`
  — todos continuam.
- **`scripts/verify-external.ps1` ganhou 2 steps novos**:
  - "E2E docs.generate xlsx" (roda `e2e_docs_generate_xlsx_full_vertical`)
  - "E2E docs.inspect" (roda `e2e_docs_inspect_docx_roundtrip`)
- Suíte workspace **514 passed / 0 failed / 4 ignored** (era 478
  antes da Etapa 4; +36 testes — 13 `sheet_name` + 10 `excelpro`
  + 10 `inspect` + 1 E2E ExcelPro + 1 E2E inspect + 1 atual
  `e2e_xlsx_write_with_column_formats` no document-worker).
- **Sem quebras de contrato** além da declarada em §7 (`docx.read`
  agora devolve `paragraphs: [{text, style}]` ao invés de `[str]`,
  atualizada em `e2e_docx_write_and_read` no mesmo commit).

## Alternativas descartadas

- **Chart nativo com `openpyxl.chart.BarChart` real na Etapa 4.**
  Descartada por orçamento da etapa (D2 do plano prioriza formatos
  numéricos brasileiros antes do chart real). Chart real fica pra
  Etapa 5/6, junto com identidade visual Excel (cores, fills,
  borders).
- **Chart na própria aba Charts_<n>.** Descartada por D1 do plano —
  sheet vazia (só com a figura) polui o `.xlsx` e força o usuário
  a alternar entre abas. Decidido embutir na `Table` compatível
  ou registrar no Painel + warning.
- **Sanitização de sheet name no Python (`xlsx.write`).** Descartada
  — o Python só receberia string e bateriaia erro genérico do
  openpyxl. Sanitização em Rust é pura (`fn sanitize_sheet_name`),
  testável sem worker (13 testes cobrindo acento, barra, 80 chars,
  colisão, forbidden, vazio).
- **`docs.inspect` só pra `.docx` na Etapa 4.** Descartada por D4
  do plano — o `sheets: [{block_index, sheet_name}]` do `generate`
  fecha o ciclo com o inspect: o `inspect` confirma o que o
  `generate` declarou. Sem inspect .xlsx, o caller não tem como
  ler de volta a estrutura do `.xlsx` que ele mesmo gerou.
- **`xlsx.write` vira parte do `ExcelProKit` (Rust faz openpyxl
  via subprocess).** Descartada — o `xlsx.write` Python já existe
  (handler da Etapa 2B+X) e é 200 linhas testadas. A Etapa 4
  estende com `column_formats` opcional (backward-compat). Reescrever
  em Rust seria jogar fora ~6 meses de hardening.
- **Formatadores numéricos no Python com formatação manual (em vez
  de `cell.number_format`).** Descartada — o openpyxl já dá o
  suporte nativo (`cell.number_format`); reinventar a formatação
  manual no Python (string com `R$` antes, vírgula, etc.) quebra
  se o usuário abrir no Excel (ele re-formata no padrão do locale
  dele). O `cell.number_format` é o jeito certo.
- **PDFPro de carona na Etapa 4.** Descartada explicitamente
  (morto na Etapa 3): PDFPro sem auditoria bloqueante do §19.6
  (PDF/A-2B, tagged PDF, fontes embutidas) seria precedente ruim.
  Fica pra Etapa 5 nascer completo, com ADR próprio.
- **`docs.inspect` despeja planilha inteira.** Descartada — uma
  planilha de 5000 linhas no contexto do modelo estoura o
  `max_tokens`. Default `sample_rows=5` (max 20); `range` opcional
  pra pegar mais se o caller quiser.

## Consequências

**Mais fácil:**

- A Etapa 4 fecha o ciclo do `docs.generate`: gera `.docx` E
  `.xlsx` reais, com formatos numéricos brasileiros (moeda R$,
  percentual, milhar) — o caso de uso contábil principal.
- `docs.inspect` permite ao modelo entender a estrutura de um
  documento existente sem "re-ler do zero" — pré-requisito pra
  edições incrementais em iteração futura (Etapa 5/6).
- O `sheets: [{block_index, sheet_name}]` do `generate` +
  `coverage: { preserved, lost }` do `inspect` dá um **contrato
  completo de ida-e-volta** entre DocumentSpec e arquivo real.
  O modelo consegue "eu gerei um .xlsx, agora deixa eu inspecionar
  o que realmente ficou" sem precisar de uma lib Python própria.
- Sanitização em Rust (`sheet_name.rs`) é pura, testável, e elimina
  toda uma classe de bugs "abro no Excel e dá erro de sheet
  inválido".
- Chart-como-registro-no-Painel é honesto: o usuário vê que existe
  um chart previsto, vê os dados, e sabe que a versão visual vem
  depois.

**Mais difícil:**

- **Quebra de contrato do `docx.read`** (commit 4): `paragraphs`
  mudou de `[str]` para `[{text, style}]`. Caller externo que
  dependia de `[str]` precisa migrar. CHANGELOG registra.
- **Limitação do WordPro v0.1 carrega pro inspect .docx**: Table
  vira texto tab-separado no `.docx` (limitação do `docx.write`
  v0.3.0, registrada na Etapa 3). O inspect .docx não tem como
  distinguir tabela real de texto tab-separado. Extensão do
  `python-docx` para tabela real fica pra Etapa 6 (junto com
  identidade visual Word).
- **Chart real** (visual) só na Etapa 5/6. Caller que precisa
  de chart visual hoje tem que abrir no Excel e adicionar manualmente
  — ou usar `xlsx.write` direto sem passar pelo kit.
- **Identidade visual Excel** (cores dos cards, fill do header,
  borders, freeze panes, largura automática) só na Etapa 5/6.
  O .xlsx v0.1 é "funcional mas sem graça" — mesma lógica do
  WordPro v0.1 ser "feio em tipografia" (Etapa 3 decisão).
- **`range` no inspect** é só flag na v0.1 (não aplica o range).
  Caller que precisa de range real hoje recebe o summary completo
  e filtra localmente. Extensão fica pra Etapa 5.x.

## Pendências para a próxima sessão

1. **Etapa 5 (PDFPro completo)** — nasce com auditoria bloqueante
   do §19.6 (PDF/A-2B, tagged PDF, fontes embutidas, tabela como
   tabela real, chart real). A Etapa 5 fecha o ciclo dos 3 kits
   DocumentSpec. Inclui chart nativo (`openpyxl.chart.BarChart`)
   e identidade visual Excel (cores, fills, borders, freeze panes).
2. **Identidade visual Word** (Etapa 6) — `docx.write` estendido
   para tabela real, cores, headings estilizados, "Tinta & Latão"
   no `.docx` (hoje o `.docx` v0.1 é deliberadamente feio).
3. **`range` real no inspect** (Etapa 5.x) — hoje é só flag, sempre
   devolve summary completo. Extensão: `range=A1:D10` filtra o
   `first_rows` e o `n_rows` no output.
4. **Formatos numéricos de data brasileira** (Etapa 5.x) — o
   `xlsx.write` aceita `BRL`/`PCT`/`THOUSANDS`/`INT`; falta
   `DATE_BR` (`dd/mm/yyyy`) que é o caso de uso óbvio de planilha
   de recebimentos/pagamentos.
5. **Auto-detecção de formato no `docs.inspect`** (Etapa 5.x) —
   hoje a extensão do path define o formato. Caminho sem extensão
   ou com extensão exótica dá erro. Adicionar detecção por magic
   bytes (XLSX é ZIP, DOCX é ZIP, mas os bytes internos diferem
   — `xl/` vs `word/` no Content_Types.xml).
6. **Chart de verdade no Excel** (Etapa 5/6) — `openpyxl.chart`
   real (BarChart, LineChart, PieChart) com chart visual na aba
   de dados (não `Charts_<n>` separada, mas o chart embutido na
   própria aba de dados via `worksheet.add_chart(anchor="G2")`).

## Referências

- [ADR-0004](0004-document-worker-em-python-embutido.md) — Python
  embeddable + libs base.
- [ADR-0017](0017-process-architecture-windows-pipes.md) —
  transporte sobre named pipes.
- [ADR-0018](0018-document-worker-handlers-primitive.md) — handler
  como primitiva, kit como renderer. Os 7 handlers da v0.3.0 do
  `document-worker` sobrevivem à Etapa 4 sem reescrita.
- [ADR-0019](0019-document-worker-ocr-tesseract.md) — `ocr.run`
  + fallback OCR no `pdf.read`. Pendência 1 do ADR-0018
  ("Etapa 3 com `ToolManifest::allowed_paths`") sai no PR #13.
- [`docs/architecture/document-engine-architecture.md`](../architecture/document-engine-architecture.md)
  — `DocumentSpec` v0.1 (20 blocos).
- [`docs/architecture/excelpro-specification.md`](../architecture/excelpro-specification.md)
  — atualizado de "parcialmente implementado (Etapa 1)" para
  "implementado v0.1 (Etapa 4)" + limitações registradas.
- [`docs/architecture/wordpro-specification.md`](../architecture/wordpro-specification.md)
  — registra que `docx.read` agora devolve `paragraphs: [{text, style}]`
  (quebra de contrato intencional).
- [`docs/modules/document-kits.md`](../modules/document-kits.md) —
  atualizado com `ExcelProKit` v0.1 + `docs.inspect`.
- `PROMPT MESTRE` §17 (Word), §18 (Excel), §19 (PDF).
