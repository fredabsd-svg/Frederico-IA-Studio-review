<!--
Estado: implementado
Verificado contra o código em: 2026-07-31
Fase correspondente: 5 (Etapa 3 + Etapa 4)
-->

> Última verificação: 2026-07-31. Reflete a Etapa 3 + Etapa 4
> da Fase 5 — crate `frederico-document-kits` com `Kit` trait,
> `KitRegistry`, `DocumentFormat` (enum gerado a partir dos kits
> implementados, **atualmente `["docx", "xlsx"]`**, REGRAS §1.9),
> `WordProKit` v0.1 (DocumentSpec.blocks → payload do
> `docx.write`), `ExcelProKit` v0.1 (Spreadsheet → `.xlsx` com
> formatos numéricos brasileiros), `PdfProKit` como skeleton
> (`is_implemented = false`, **não** aparece no schema do
> `docs.generate`), `DocsGenerateTool` (tool único exposto ao
> modelo, roteador + validação) e `DocsInspectTool` (round-trip
> parcial `.docx`/`.xlsx` → `DocumentSpec` + `coverage`).
> Suíte do crate: **84/84 verde** (35 Etapa 3 + 49 Etapa 4:
> 13 `sheet_name` + 10 `excelpro` + 10 `inspect` + 1 E2E
> ExcelPro + 1 E2E inspect). ADR-0018 §Decisão 1 mantida:
> handler = primitiva, kit = renderer.

# `frederico-document-kits`

Kits WordPro / ExcelPro / PdfPro + `DocsGenerateTool` (Fase 5,
Etapa 3). Camada que **traduz** um `DocumentSpec` declarativo
para a chamada do handler correspondente no `document-worker`
Python. Os 7 handlers da v0.3.0 do `document-worker`
sobrevivem sem reescrita — esta camada é a ponte.

## 1. O que este módulo faz

É a fronteira entre o `ToolRegistry` (que o modelo enxerga) e
o `document-worker` (que faz I/O real). O `DocsGenerateTool`
é o **único** tool que o modelo vê: o schema `format` é um
enum `["docx"]` na Etapa 3, gerado a partir de
`KitRegistry::implemented_formats()`. Inventário não mente
(REGRAS §1.9).

**Etapa 3 (fechada em 2026-07-31, PR #13):**

- `Kit` trait — contrato dos kits. `id`, `target_format`,
  `is_implemented`, `manifest`, `async render(spec, output_path)`.
- `DocumentFormat` enum — `Docx` na v0.1. Adicionar
  `Xlsx`/`Pdf` é bump atômico **junto** com a implementação
  do kit.
- `KitRegistry` — registro thread-safe.
  `implemented_formats()` é o que alimenta o enum do
  schema. Skeletons ficam na registry (provam a forma do
  trait) mas **não** aparecem no inventário do modelo.
- `WordProKit` v0.1 — DocumentSpec.blocks → `docx.write`
  payload. Cobertura: Cover, Heading, Paragraph, List,
  Table (texto tab-separado — limitação do
  `docx.write` v0.3.0), Kpis, Callout, Quote, Steps,
  Code, Divider, Spacer, PageBreak, Signatures,
  BackCover, Footer, Toc, KeyValue, Chart (placeholder).
- `ExcelProKit`, `PdfProKit` — skeletons (`is_implemented
  = false`). Provam a forma do trait. Quando Etapa 4/5
  implementarem, basta trocar `is_implemented` por
  `true` e o `render` por uma implementação real.
- `DocsGenerateTool` — `impl Tool` async. Roteador: valida
  `output_path` contra a allowlist, re-valida o
  DocumentSpec, roteia pro kit certo, devolve `ToolResult`.

**Etapa 4 (fechada em 2026-07-31, branch `fase-5/etapa-4-excelpro-inspect`):**

- **`ExcelProKit` v0.1** — substitui o skeleton.
  Renderiza `DocumentSpec` (`DocumentType::Spreadsheet`) em
  `.xlsx` real (cobre `Kpis` + `Table` + `Chart`). Mapeamento:
  `Kpis` → sheet `Painel` (cumulativa, **sempre 1ª aba**);
  `Table` (com `title`) → sheet `<title>` sanitizado; `Table`
  (sem `title`) → `Table_<i>`; `Chart` (com Table compatível)
  embute os dados na Table; `Chart` (sem Table compatível) vira
  registro no Painel + **warning explícito** (nunca silencioso).
  Formatos numéricos brasileiros (`column_formats` opcional
  no `xlsx.write`): `BRL` (moeda), `PCT` (percentual),
  `THOUSANDS` (milhar), `INT`. Sanitização de sheet name em
  Rust (`sheet_name.rs`, 13 testes): max 31 chars, remove
  `\ / ? * [ ] :`, strip whitespace, UTF-8 safe, acentos
  preservados, fallback `Table_<i>`, sufixo `_2.._999` em
  colisão. Bump atômico do enum `DocumentFormat::Xlsx` no
  mesmo commit (REGRAS §1.9). **Chart SEM aba `Charts_<n>`**
  (registro no Painel + warning) — chart visual nativo
  (BarChart/LineChart/PieChart) fica pra Etapa 5/6.
- **`DocsInspectTool`** — round-trip parcial de `.docx`/`.xlsx`
  para `DocumentSpec` + `coverage`. Modo resumo padrão (não
  despeja planilha de 5000 linhas). Cobre `.xlsx` também
  (SheetsSummary com `name`/`used_range`/`headers`/`n_rows`/
  `n_cols`/`first_rows`/`has_total`/`column_formats`). Mesma
  barreira de path do `docs.generate` (allowed_paths).
  **Quebra de contrato do `docx.read`** (Etapa 4): agora
  devolve `paragraphs: [{text, style}]` ao invés de `[str]`
  — o inspect usa o style real do `python-docx` pra
  reconstruir heading (antes era heurística de string match
  em "Heading 1 " que falhava 100% das vezes). Caller que
  dependia de `[str]` precisa migrar (CHANGELOG registra).
- `KitOutput` ganha `sheets: Vec<SheetMapping>` e
  `warnings: Vec<String>` (campos novos, Etapa 4) +
  `KitOutput::simple(path)` helper para kits sem sheets
  (WordPro).

**O que está fora desta entrega (próximas etapas):**

- **Identidade visual "Tinta & Latão"** (Etapa 6). A v0.1
  do WordPro é deliberadamente "feia" em tipografia — o
  handler `docx.write` é primitivo, e o kit só traduz.
  Mesmo vale pro Excel: cores dos cards KPI, fill do
  header, borders, freeze panes, largura automática de
  coluna — Etapa 5/6.
- **Tabela real no `.docx`** (Etapa 6). Hoje a tabela vira
  texto tab-separado (limitação do `docx.write` v0.3.0).
  O `docs.inspect` .docx reflete isso: 0 tables em
  `spec.blocks` quando o spec original tinha Table.
- **Chart visual nativo no `.xlsx`** (Etapa 5/6). v0.1
  registra o chart no Painel e (quando possível) embute
  os dados na próxima Table compatível. `openpyxl.chart`
  real (BarChart/LineChart/PieChart) entra depois.
- **Auditoria bloqueante do PDFPro** (Etapa 5, §19.6).
  Sem ela, não é PDFPro. `PdfProKit` continua skeleton.

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- `DocumentFormat` — enum `Docx` + `Xlsx` v0.1 (com
  `as_str`, `extension`, `mime_type`). `Pdf` é bump
  atômico junto com `PdfProKit` real (Etapa 5).
- `Kit` trait, `KitError`, `KitOutput` (ganha
  `sheets: Vec<SheetMapping>` e `warnings: Vec<String>`
  na Etapa 4).
- `KitRegistry` — `new`, `register`, `get`, `all`,
  `implemented`, `implemented_formats` (atualmente
  retorna `["docx", "xlsx"]`),
  `find_for_format`, `len`, `is_empty`.
- `WordProKit` v0.1 (implementado, Etapa 3),
  `ExcelProKit` v0.1 (implementado, Etapa 4),
  `PdfProKit` (skeleton, Etapa 5).
- `DocsGenerateTool` — `new(Arc<KitRegistry>,
  WorkerToolDispatcher)`, `manifest()`, async `execute()`.
- `DocsInspectTool` (Etapa 4) — `new(WorkerToolDispatcher)`,
  `manifest()`, async `execute()`. Cobre `.docx`/`.xlsx`.

**Não-público (interno):**

- `translate_spec_to_docx_payload` em `wordpro.rs` —
  função **pura** que faz a tradução. `WordProKit::translate`
  é wrapper fino em torno desta.
- `KitError::NotImplemented { id, format, etapa }` —
  erro dos skeletons; `etapa` é `"4"` ou `"5"`.

## 3. De quem depende e quem depende dele

**Dependências (`Cargo.toml`):**

- `frederico-document-engine` — `DocumentSpec`,
  `DocumentError`, validação.
- `frederico-process-architecture` — `WorkerHandle`.
- `frederico-tool-registry` — `Tool`, `ToolResult`,
  `ToolManifest`, `ToolCategory::Docs`, `RiskLevel`,
  `JsonSchema`, `WorkerToolDispatcher`, `DispatchError`.
- `frederico-test-support` — `with_test_timeout_at` (E2E).
- `frederico-core` — `ToolId`.
- `async-trait` (kit execute), `tokio` (rt, macros, sync),
  `serde`, `serde_json`, `schemars`, `thiserror`,
  `tracing`, `uuid`.

**Quem depende dele (hoje):**

- `apps/desktop/src-tauri` (Etapa 6) — registra o
  `DocsGenerateTool` e o `DocsInspectTool` no `AppState`.
  Wiring ainda não existe (Etapa 4 entrega só os kits;
  integração com a casca Tauri é Etapa 6 junto com a
  UI do modo documental).
- `crates/execution-engine` (Etapa 4) — o `RunExecutor`
  consome `Tool::execute` (async desde a Etapa 3 da
  Fase 5). A `docs.generate` aparece no `effective_tools`
  quando o `ToolRegistry` a registra. `docs.inspect`
  também.

**Quem vai depender dele (próximas etapas):**

- Etapa 5: PdfPro real COM auditoria bloqueante
  (§19.6, sem interruptor). Fecha o ciclo dos 3 kits
  DocumentSpec. Inclui chart nativo (`openpyxl.chart`)
  e identidade visual Excel (cores/fills/borders/freeze
  panes).
- Etapa 6: identidade visual Word ("Tinta & Latão"
  no `.docx` via `python-docx` estendido) + tabela
  real no `.docx` (com grade e células formatadas).

## 4. Decisões não óbvias e armadilhas conhecidas

- **Inventário gerado, não mantido à mão.** O enum
  `format` no schema do `docs.generate` vem de
  `KitRegistry::implemented_formats()`. Adicionar
  `DocumentFormat::Xlsx` exige:
  1. Variante no enum.
  2. `ExcelProKit` com `is_implemented() == true`.
  3. Registro no `KitRegistry`.

  Sem os 3, o modelo não sabe que `.xlsx` existe. É
  a **única** forma de evitar o defeito "ferramenta
  anunciada que não funciona" que derrubou o app
  anterior.

- **Skeletons ficam na registry mas não no schema.** O
  `ExcelProKit` e `PdfProKit` da Etapa 3 **existem**
  no código (provam a forma do trait, podem ser
  inspecionados em testes), mas `is_implemented()
  == false` os filtra do `implemented_formats()`.
  Substituir por `true` é o gate de promoção.

- **PDFPro sem auditoria bloqueante = não é PDFPro.**
  O skeleton do `PdfProKit` é honesto sobre isso: o
  `is_implemented() == false` **permanece** até a
  Etapa 5 entregar a auditoria do §19.6 junto.
  Entregar um `pdf.write` sem a auditoria (mesmo
  que funcione) é o precedente ruim que a Etapa 3
  evita.

- **Tool::execute é async (Etapa 3 da Fase 5).** A
  mudança no `Tool` trait (era sync, virou
  `async_trait::async_trait`) elimina a ponte
  sync→async que esta etapa precisaria. O
  `RunExecutor` (Etapa 4 da Fase 3) já é async — o
  casamento é natural. Sem `block_in_place`, sem
  `flavor = "multi_thread"` obrigatório em testes
  do kit. Os testes do `FilesReadTool` viraram
  `#[tokio::test]` (default current_thread é
  suficiente — file I/O é bloqueante mas curto).

- **`DocumentFormat` é finito e versionado.** O enum
  é o contrato com o modelo. Adicionar variante
  sem ter o kit pronto quebra o schema. Adicionar
  o kit sem adicionar a variante também. Bumps
  atômicos.

- **Tabela no `.docx` é fallback textual (v0.1).** O
  handler `docx.write` da v0.3.0 só tem `heading` e
  `paragraphs` por seção. O kit vira tabela em
  texto tab-separado (1 paragraph com a tabela).
  Tabela real no `.docx` (com grade, células
  formatadas) é trabalho da Etapa 6 (extensão do
  `python-docx`). Documentado no
  `wordpro-specification.md` e no CHANGELOG.

- **Chart SEM aba `Charts_<n>` no `.xlsx` v0.1
  (D1 da Etapa 4).** O `ExcelProKit` não cria aba
  vazia pra chart (decisão registrada em ADR-0020
  §3). Em vez disso, embute os dados do chart na
  próxima `Table` compatível, ou (se não houver)
  registra no `Painel` + warning explícito. Chart
  visual nativo (`openpyxl.chart.BarChart` etc.)
  fica pra Etapa 5/6.

- **Formatos numéricos brasileiros via `column_formats`
  (D2 da Etapa 4).** O `xlsx.write` Python foi
  estendido com `column_formats: {col_idx: format_str}`
  opcional — `BRL`/`PCT`/`THOUSANDS`/`INT` são
  aliases para o `cell.number_format` do openpyxl.
  Backward-compat: sem `column_formats`, o
  comportamento é idêntico ao v0.2.0. Heurística no
  `ExcelProKit`: aplica conforme `Table.currency`,
  `Table.percent`, `Table.thousands`, `Kpis.format`.

- **Sanitização de sheet name em Rust
  (D3 da Etapa 4).** Função pura
  `sanitize_sheet_name(proposed, block_index, used)`
  com 13 testes: max 31 chars, remove forbidden,
  strip whitespace, UTF-8 safe, acentos preservados,
  fallback `Table_<i>`, sufixo `_2.._999` em colisão.
  Razão: o `xlsx.write` Python receberia string
  inválida e daria erro genérico do openpyxl. Em
  Rust é pura, testável, e elimina toda uma classe
  de bugs "abro no Excel e dá erro de sheet inválido".

- **Quebra de contrato do `docx.read` (D-WP2 da
  Etapa 4).** `paragraphs` mudou de `[str]` para
  `[{text, style}]` (Etapa 4 da Fase 5, ADR-0020 §7).
  Razão: o `python-docx` não prefixa o style no
  texto, então a heurística de string match em
  "Heading 1 " falhava 100% das vezes no `docs.inspect`.
  O style real (`p.style.name`) é a fonte da verdade.
  Caller que dependia de `[str]` precisa migrar.
  CHANGELOG registra.

- **`docs.inspect` cobre `.xlsx` também (D4 da
  Etapa 4).** Modo resumo padrão (não despeja
  planilha de 5000 linhas no contexto do modelo).
  `range` opcional (v0.1 = flag apenas, não aplica
  o range). `sheet` opcional (filtra 1 sheet).
  `SheetSummary` inclui `has_total` (heurística:
  última linha começa com "Total") e `column_formats`
  (mapa reverso do `xlsx.read` Python — devolve o
  alias, não o `number_format` cru).

- **Path safety forte via `ToolManifest::allowed_paths`.**
  O `DocsGenerateTool::execute` valida `output_path`
  contra a allowlist **antes** de chamar o kit. O
  `WorkerToolDispatcher` (do `tool-registry`) é a
  peça que valida. Defesa em profundidade: o worker
  Python também valida (rejeita `..`), mas a barreira
  forte é no Rust — o worker é externo, não confiamos
  pra revalidar.

- **Re-validação do DocumentSpec no `execute` é
  defensiva.** O `validate_tool_call` da Etapa 2
  confere o `input_schema` (que aceita `spec:
  object` genérico), mas o spec tem schema próprio.
  O `DocsGenerateTool::execute` re-valida via
  `validate_against_schema` + `validate_semantic` —
  custo de ~1ms, defesa em profundidade.

- **Sem `unsafe`.** `unsafe_code = "forbid"` no
  crate. Não depende de `tauri`/`windows`. Pureza
  verificada por `scripts/check-core-purity.ps1`.

## 5. Como testá-lo isoladamente

```pwsh
# Suíte do crate (84 testes: 82 unit + 2 E2E)
cargo test -p frederico-document-kits

# Só unit (sem os E2Es que precisam do Python runtime):
cargo test -p frederico-document-kits --lib

# E2E do WordPro (precisa do `runtime/python.exe` instalado):
cargo test -p frederico-document-kits --test e2e_docs_generate

# E2E do ExcelPro (Etapa 4):
cargo test -p frederico-document-kits --test e2e_docs_generate_xlsx

# E2E do inspect (Etapa 4):
cargo test -p frederico-document-kits --test e2e_docs_inspect

# Verificação completa (clippy + workspace + purity + E2E):
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-external.ps1
```

**Cobertura por área:**

- `format.rs`: `as_str_matches_serde`, `extension_includes_dot`.
- `registry.rs`: 7 testes — empty, register, implemented
  filter, no-duplicate formats, find_for_format (com
  skeleton = None), sorted by id.
- `wordpro.rs`: 22 testes de tradução — smoke, title
  priority, cada bloco (Heading, PageBreak, List,
  Callout, Quote, Steps, Code, Table, Kpis, Chart
  placeholder, Image fallback, Toc placeholder,
  Signatures, BackCover, KeyValue, Cover subtitle) +
  full flow (Cover + 2 Headings + Paragraph + Table).
- `excelpro.rs` (Etapa 4): 10 testes — `sheets` no
  `KitOutput`, mapeamento Painel 1ª aba, sanitização
  via `sheet_name`, `column_formats` heurística,
  chart-como-registro, warning explícito.
- `sheet_name.rs` (Etapa 4): 13 testes — vazio,
  forbidden chars, 80 chars → 31, acento, barra,
  whitespace, colisão `_2.._999`, UTF-8 safe,
  fallback `Table_<i>`.
- `inspect.rs` (Etapa 4): 10 testes — `infer_format`
  (docx/xlsx/erro), `parse_args` (range, sheet,
  sample_rows), `build_docx_spec` (style real do
  python-docx, headings preservados, table lost),
  `build_sheet_summaries` (headers, n_rows,
  has_total, column_formats).
- `generate.rs`: 5 testes — schema de `format`
  gerado a partir do registry, descrição avisa
  quando vazio, parse_format, rejeições de
  args inválidos.

## 6. O que ele **não** faz

- **Não conhece o `ToolRegistry`.** O
  `DocsGenerateTool` e o `DocsInspectTool` são
  construídos com `KitRegistry` / `WorkerToolDispatcher`
  separados. A integração com o `ToolRegistry`
  (registrar como os tools `docs.generate` /
  `docs.inspect`) é do caller (casca Tauri
  na Etapa 6).
- **Não chama o worker sem o `output_path` validado.**
  A validação acontece **no Rust**, antes do
  `kit.render` ou do `inspect.invoke_read`. O worker
  Python é a barreira mínima (rejeita `..`); o
  Rust é a forte (allowlist).
- **Não faz auditoria do PDF.** O `PdfProKit` é
  skeleton. A auditoria bloqueante do §19.6 entra
  com o kit real na Etapa 5.
- **Não renderiza tabela real no `.docx`.** vira
  texto tab-separado. Limitação do `docx.write`
  v0.3.0 do worker. Etapa 6 (estendida) traz a
  grade.
- **Não renderiza chart visual nativo no `.xlsx`.**
  v0.1 registra o chart no Painel + warning
  explícito. `openpyxl.chart.BarChart`/
  `LineChart`/`PieChart` real entra na Etapa 5/6.
- **Não aplica identidade visual "Tinta & Latão"**
  no `.xlsx` (cores dos cards KPI, fill do header,
  borders, freeze panes, largura automática). v0.1
  é "funcional mas sem graça" — Etapa 5/6 traz o
  estilo.
- **Não conhece UI.** A integração com a casca
  Tauri (modal de aprovação, painel de tools,
  preview do `docs.inspect`) é Etapa 6.
- **Não persiste o `KitRegistry`.** A Etapa 4
  introduz migração numerada; a Etapa 3 entrega
  o registry como struct in-memory.
- **Não tem hot-reload de kit.** Adicionar
  `PdfProKit` real é bump atômico do enum +
  flip do `is_implemented` + registro.
- **`range` no `docs.inspect` é só flag (v0.1).**
  Não aplica o range — sempre devolve o summary
  completo. Caller que precisa de range real filtra
  localmente. Extensão fica pra Etapa 5.x.
- **`docs.inspect` .docx não distingue tabela real
  de texto tab-separado.** Limitação herdada do
  WordPro v0.1 (Table vira texto). Extensão do
  `python-docx` para tabela real entra na Etapa 6.
