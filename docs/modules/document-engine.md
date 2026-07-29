# Módulo `frederico-document-engine`

> Etapa 1 da Fase 5. Estado: parcialmente implementado. Verificado contra o código em 2026-07-28.

## 1. O que este módulo faz

Define o `DocumentSpec` — o **contrato declarativo** que o modelo
emite para os três kits (Word, Excel, PDF). O `DocumentSpec` é a
fonte da verdade do formato de documento: tipo, estilo, idioma, lista
de blocos (20 variantes) e metadados. O módulo valida o spec em
duas camadas (JSON Schema + regras semânticas) e gera o prompt do
modo documental a partir do catálogo de blocos.

É o **núcleo** do Document Engine; os kits (Word, Excel, PDF)
consomem o spec. A renderização em si acontece no
`document-worker` (Etapa 2) — este crate é puro e não tem I/O.

## 2. O que ele expõe

**Público (re-exportado em `lib.rs`):**

- `DocumentSpec` (struct) — raiz do contrato.
- `DocumentBlock` (enum) — 20 variantes, `#[serde(tag = "type")]`.
- `DocumentType` (enum) — 12 tipos (`report`, `memo`, `contract`,
  `spreadsheet`, `proposal`, `technical_opinion`,
  `power_of_attorney`, `official_letter`, `announcement`, `manual`,
  `presentation`, `generic`).
- `DocumentStyle` (enum) — `tinta_e_latao` (default) e `sobrio`.
- `DocumentMetadata` (struct) — título, autor, organização,
  palavras-chave, descrição.
- `ConfidentialityMark`, `ConfidentialityLevel`.
- 12 sub-tipos de bloco (`Cover`, `ListItem`, `Table`'s `TotalSpec`,
  `KpiCard`, `CalloutKind`, `Quote`, `Step`, `ChartKind`,
  `ChartSeries`, `ImageBlock`, `CodeBlock`, `SignaturePair`,
  `ContactInfo`).
- `DocumentError` (enum) — `Parse`, `Schema`, `Semantic`. Tem
  `.code()` (`document_parse_error` / `document_schema_invalid` /
  `document_semantic_invalid`) e `.path()` (JSON pointer).
- `validate_against_schema(value) -> Result<(), DocumentError>` —
  valida um `serde_json::Value` contra o JSON Schema gerado em
  runtime.
- `validate_semantic(spec) -> Result<(), DocumentError>` — aplica
  as 7 regras semânticas (v0.1).
- `prompt::document_mode_prompt() -> String` — gera o system prompt
  do modo documental a partir do catálogo de blocos (REGRAS §1.9).

**Não-público (interno):**

- `compiled_schema()` — cache thread-safe (`OnceLock`) do
  `JSONSchema` compilado a partir de `schema_for!(DocumentSpec)`.
- O enum `blocks::block_kind()` — usado em mensagens de erro.

## 3. Do que depende e quem depende dele

**Dependências (`Cargo.toml`):**

- `serde` + `serde_json` — serialização do `DocumentSpec`.
- `schemars 0.8` — geração do JSON Schema via `schema_for!`
  (default features, inclui a derive `JsonSchema`).
- `jsonschema 0.18` (feature `draft202012`) — validação em
  runtime. Mesmo crate que o `tool-registry` usa.
- `thiserror` — `DocumentError`.
- `tracing` — reservado para logs futuros (Etapa 3+).

**Quem depende dele:**

- Nenhum crate do workspace ainda. A Etapa 3 adiciona o
  `docs.generate` no `ToolRegistry` (consome `DocumentSpec` para
  serializar como `input_schema`).
- A casca Tauri (Etapa 3) consome via IPC do frontend.
- O `document-worker` (Etapa 2) consome via named pipe.

## 4. Decisões não óbvias e armadilhas conhecidas

- **Schema gerado em runtime, não em `build.rs`.** O `build.rs`
  padrão do Rust que importaria o próprio crate causaria ciclo de
  dependência. Solução: `schemars::schema_for!` roda na primeira
  chamada de `validate_against_schema`, com cache em
  `std::sync::OnceLock`. O custo é pago **uma vez por processo**.
  REGRAS §1.9 continua sendo atendida (não há schema mantido à
  mão), e o teste `schema_generation_is_idempotent` prova
  determinismo.

- **`Spreadsheet` aceita apenas `Kpis`/`Table`/`Chart`.** Regra
  semântica (não estrutural) — um `DocumentSpec` cobre um
  documento, não um workbook multi-aba. Workbook completo
  multi-aba vira uma **lista** de specs (formato decidido na
  Etapa 4 com o ExcelPro).

- **`f32` não implementa `Eq`.** Por isso `ImageBlock` e `Spacer`
  (que carregam `f32`) implementam `PartialEq` apenas, sem `Eq`.
  Compilação fica `#[allow(clippy::derive_partial_eq_without_eq)]`
  explícito no variant `Spacer`. Sem isso, `clippy -D warnings`
  falha.

- **JSON Schema é gerado como Draft 2020-12** (padrão do
  `schemars` 0.8). A validação usa `jsonschema 0.18` com a feature
  `draft202012` habilitada (workspace dep). Sem a feature, o
  `with_draft(Draft::Draft202012)` não compila.

- **Tagged enum com `rename_all = "snake_case"`.** O JSON do
  `DocumentBlock` é `{"type": "callout", "kind": "info", ...}` —
  cada bloco é auto-descritivo. O modelo emite isso sem precisar
  de um discriminador externo.

- **`prompt::document_mode_prompt` é função pura determinística.**
  Não há timestamp, ID de build, ou outra fonte de não-determinismo
  — `execution-engine` (Etapa 3) pode cachear por `spec_version`
  sem invalidar.

## 5. Como testá-lo isoladamente

```powershell
cd C:\src\Frederico
$env:PATH = $env:PATH + ";C:\Users\conta\.cargo\bin"
cargo test -p frederico-document-engine
```

**22/22 verde** (2 unit em `prompt.rs` + 20 integration em
`tests/spec_roundtrip.rs`).

Cobertura por regra semântica (Etapa 1):

| Regra | Teste |
|---|---|
| 1. `blocks` não vazio | `validate_semantic_rejects_empty_blocks` |
| 2. `spec_version` MAJOR.MINOR.PATCH | `validate_semantic_rejects_bad_spec_version` |
| 3. `Kpis` 2-4 | `validate_semantic_rejects_kpis_with_one_card`, `..._with_five_cards`, `..._accepts_kpis_with_two_three_or_four_cards` |
| 4. `Steps` ≥ 1 | `validate_semantic_rejects_empty_steps` |
| 5. `Table` colunas consistentes | `validate_semantic_rejects_table_with_mismatched_columns`, `..._rejects_table_without_headers` |
| 6. `Spreadsheet` apenas Kpis/Table/Chart | `validate_semantic_rejects_spreadsheet_with_cover`, `..._accepts_spreadsheet_with_table_kpis_chart` |
| 7. `language` minúsculas | `validate_semantic_rejects_uppercase_language` |

Cobertura do schema (JSON Schema):

| Caso | Teste |
|---|---|
| Spec válido | `validate_against_schema_accepts_valid_spec` |
| Tipo errado | `validate_against_schema_rejects_wrong_type` |
| Campo obrigatório faltando | `validate_against_schema_rejects_missing_required_field` |
| Idempotência do schema gerado | `schema_generation_is_idempotent` |

Cobertura do prompt:

| Caso | Teste |
|---|---|
| Lista os 20 blocos | `prompt_lists_every_block_kind_in_catalog` |
| Menciona as 7 regras | `prompt_mentions_all_semantic_rules` |
| Estável e não-vazio | `prompt_is_stable_and_nonempty` |

## 6. O que ele **não** faz

- **Não renderiza documentos.** A renderização em `.docx`/`.xlsx`/`.pdf`
  é trabalho do `document-worker` (Python, Etapas 3-5) e do
  `process-architecture` (manager de workers, Etapa 2).
- **Não persiste nada.** Não há I/O de banco ou de arquivo. O
  `MemoryRepo`/`DocumentRepo` (quando existir) será em outro
  crate — este é puro.
- **Não fala IPC.** O envelope `IpcRequest`/`IpcResponse` (definido
  em `packages/shared-contracts`) consome `serde_json::Value`, não
  `DocumentSpec` direto — a fronteira é por JSON, não por tipo.
- **Não tem fallback de "spec parcial".** Spec inválido é
  rejeitado com path; não há modo "aceita e corrige" (decisão
  explícita: o modelo recebe o erro, reescreve o spec, reenvia).
- **Não conhece o `DocumentPermission` do `tool-registry`.** A
  integração entre `documents: DocumentPermission` e o
  `DocumentSpec` é trabalho da Etapa 3 (registro do `docs.generate`
  com `requires_user_approval: true` etc.).
- **Não tem macros `#[derive(JsonSchema)]` em tudo.** O
  `DocumentMetadata` e o `SpecVersion` derivam, mas alguns tipos
  que `JsonSchema` não consegue derivar diretamente (ex:
  `Vec<(String, String)>` no `KeyValue`) também — `schemars` lida
  com tuplas nativamente.

## Pendências conhecidas (próximas etapas)

- **Etapa 2:** `process-architecture` + `document-worker` vão
  consumir o `DocumentSpec` via IPC.
- **Etapa 3:** `docs.generate` no `ToolRegistry` (com
  `input_schema = schema_for!(DocumentSpec)` e
  `requires_user_approval: true` para `risk_level: Moderate` por
  default).
- **Etapa 4:** batch de specs para workbook multi-aba; cache
  chaveado por hash do spec.
- **Etapa 5:** `output_schema` do `docs.generate` (info do
  artefato gerado, caminho, bytes, validações passadas).
