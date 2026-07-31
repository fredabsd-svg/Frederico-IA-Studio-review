<!--
Estado: implementado
Verificado contra o código em: 2026-07-31
Fase correspondente: 5 (Etapa 3)
-->

> Última verificação: 2026-07-31. Reflete a Etapa 3 da Fase 5 —
> crate `frederico-document-kits` com `Kit` trait, `KitRegistry`,
> `DocumentFormat` (enum gerado a partir dos kits
> implementados), `WordProKit` (v0.1 — DocumentSpec.blocks
> → payload do `docx.write`), `ExcelProKit` e `PdfProKit`
> como skeletons (`is_implemented = false`, **não** aparecem
> no schema do `docs.generate`), e `DocsGenerateTool`
> (tool único exposto ao modelo, roteador + validação). Suíte
> do crate: 35/35 verde (34 unit + 1 E2E que atravessa
> kit → dispatcher → worker Python → arquivo real → reopen
> via `python-docx`). ADR-0018 §Decisão 1 mantida: handler =
> primitiva, kit = renderer.

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

**Etapa 3 (esta entrega):**

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

**O que está fora desta entrega (próximas etapas):**

- **Identidade visual "Tinta & Latão"** (Etapa 6). A v0.1
  do WordPro é deliberadamente "feia" em tipografia — o
  handler `docx.write` é primitivo, e o kit só traduz.
- **Tabela real no `.docx`** (Etapa 6). Hoje a tabela vira
  texto tab-separado (limitação do `docx.write` v0.3.0).
- **Round-trip `DocumentSpec` ← `.docx`** (Etapa 4 —
  `docs.inspect`). Hoje o `docs.generate` é one-way.
- **Auditoria bloqueante do PDFPro** (Etapa 5, §19.6).
  Sem ela, não é PDFPro.

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- `DocumentFormat` — enum `Docx` v0.1 (com `as_str`,
  `extension`, `mime_type`).
- `Kit` trait, `KitError`, `KitOutput`.
- `KitRegistry` — `new`, `register`, `get`, `all`,
  `implemented`, `implemented_formats`,
  `find_for_format`, `len`, `is_empty`.
- `WordProKit` (v0.1 implementado),
  `ExcelProKit`/`PdfProKit` (skeletons).
- `DocsGenerateTool` — `new(Arc<KitRegistry>,
  WorkerToolDispatcher)`, `manifest()`, async `execute()`.

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
  `DocsGenerateTool` no `AppState`. Wiring ainda não
  existe (Etapa 3 entrega só o kit; integração com a
  casca Tauri é Etapa 6 junto com a UI do modo
  documental).
- `crates/execution-engine` (Etapa 4) — o `RunExecutor`
  consome `Tool::execute` (agora async desde a Etapa 3
  da Fase 5). A `docs.generate` aparece no
  `effective_tools` quando o `ToolRegistry` a registra.

**Quem vai depender dele (próximas etapas):**

- Etapa 4: ExcelPro real (substitui o skeleton).
- Etapa 5: PdfPro real COM auditoria bloqueante
  (§19.6, sem interruptor).

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
# Suíte do crate (35 testes: 34 unit + 1 E2E)
cargo test -p frederico-document-kits

# Só unit (sem o E2E que precisa do Python runtime):
cargo test -p frederico-document-kits --lib

# Só o E2E (precisa do `runtime/python.exe` instalado):
cargo test -p frederico-document-kits --test e2e_docs_generate

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
- `generate.rs`: 5 testes — schema de `format`
  gerado a partir do registry, descrição avisa
  quando vazio, parse_format, rejeições de
  args inválidos.

## 6. O que ele **não** faz

- **Não conhece o `ToolRegistry`.** O
  `DocsGenerateTool` é construído com um
  `KitRegistry` separado. A integração com o
  `ToolRegistry` (registrar `DocsGenerateTool` como
  o tool `docs.generate`) é do caller (casca Tauri
  na Etapa 6).
- **Não chama o worker sem o `output_path` validado.**
  A validação acontece **no Rust**, antes do
  `kit.render`. O worker Python é a barreira
  mínima (rejeita `..`); o Rust é a forte
  (allowlist).
- **Não faz auditoria do PDF.** O `PdfProKit` é
  skeleton. A auditoria bloqueante do §19.6 entra
  com o kit real na Etapa 5.
- **Não renderiza tabela real no `.docx`.** vira
  texto tab-separado. Limitação do `docx.write`
  v0.3.0 do worker. Etapa 6 (estendida) traz a
  grade.
- **Não conhece UI.** A integração com a casca
  Tauri (modal de aprovação, painel de tools,
  preview) é Etapa 6.
- **Não persiste o `KitRegistry`.** A Etapa 4
  introduz migração numerada; a Etapa 3 entrega
  o registry como struct in-memory.
- **Não tem hot-reload de kit.** Adicionar
  `ExcelProKit` real é bump atômico do enum +
  flip do `is_implemented` + registro.
