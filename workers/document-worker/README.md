# `document-worker`

Worker sidecar do Frederico IA Studio que gera documentos profissionais
(DOCX, XLSX, PDF) e le os tres formatos. Python embutido (ADR-0004),
comunica com o app via **named pipes** do Windows.

## Estado atual (Etapa 2B+Y, 2026-07-30)

**7 handlers reais** (Etapa 2B+X entregou 6; o 7º, `ocr.run`, entra
nesta etapa com Tesseract 5.4.0 + por/eng/osd traineddata):

| Capability   | Input                                                  | Library                          |
| ------------ | ------------------------------------------------------ | -------------------------------- |
| `docx.write` | `path`, `title`, `sections`                            | python-docx                      |
| `docx.read`  | `path`                                                 | python-docx                      |
| `xlsx.write` | `path`, `sheets`                                       | openpyxl                         |
| `xlsx.read`  | `path` (opcional `sheet`)                              | openpyxl                         |
| `pdf.write`  | `path`, `title`, `sections`                            | reportlab                        |
| `pdf.read`   | `path`, `ocr: "auto"|"never"|"only"` (opcional)         | pdfplumber + pytesseract (fallback OCR) |
| `ocr.run`    | `path`, `lang: "por+eng"` (opcional)                   | pytesseract + Tesseract 5.4.0     |

**`pdf.read` com fallback OCR transparente (Etapa 2B+Y, ADR-0019):**

- `text` e `ocr_text` são **sempre separados** (procedência,
  mesma disciplina de `origin`/`external_content` da memória).
  OCR troca 8 por B, 0 por O, 1 por l — e é exatamente em
  CNPJ/competência/valor que o erro cai. Misturar apagaria a
  procedência.
- Parâmetro `ocr: "auto"` (default): se há páginas escaneadas
  E Tesseract disponível, faz OCR delas e popula `ocr_text`.
  `ocr: "never"`: rápido, só checa camada de texto. `ocr: "only"`:
  força OCR de TODAS as páginas.
- `ocr_truncated: true` quando o teto de páginas/timeout foi
  atingido (`MAX_OCR_PAGES_PDF = 20`, `OCR_TIMEOUT_S_PER_PAGE = 30`).
- `tesseract_version` no retorno = reprodutibilidade (3 meses
  depois: "qual versão do Tesseract produziu esse texto?").

**MUDANÇA VISÍVEL DO `pdf.read` (breaking change):** PDF 100%
escaneado que antes (v0.2.0) retornava `ok: false, code:
pdf_scanned_no_ocr` agora pode retornar `ok: true` com `text` do
OCR + `extraction: "ocr"`. Caller que dependia do code antigo
precisa migrar pra checar `extraction == "ocr"` ou `ocr_text`
não-vazio. CHANGELOG registra.

**`ocr.run` (Etapa 2B+Y):** OCR de uma imagem (PNG/JPG/TIFF/BMP)
via Tesseract. `lang` validado com regex estrita
(`^[a-z]{3}(+[a-z]{3})*$`) e contra os traineddata realmente
instalados — erro estruturado (`code: "invalid_lang"`) com lista
de disponíveis, em vez da mensagem críptica do Tesseract quando
o idioma não existe. Códigos: `ocr_not_available`, `invalid_lang`,
`tesseract_failed`, `ocr_timeout`, `image_not_found`.

**Path safety minima (ADR-0018 §Decisão 4):** handlers rejeitam
`..` como componente, exigem path absoluto ou relativo ao `cwd`,
e validam gravabilidade do diretorio pai (write) ou legibilidade
do arquivo (read). Allowlist mais forte (por `ToolManifest`) entra
na Etapa 3; sandbox de OS (sandbox-runner) entra na Fase 7.

**Fontes "Tinta e Latao" (ADR-0018 §Decisão 2b):** Adobe Source Sans 3
(corpo) + Source Serif 4 (titulos), TTF variable, instaladas pelo
`bootstrap.ps1` em `runtime/fonts/`. `pdf.write` registra no
reportlab e embarca no PDF. O `worker.hello` e `worker.pong`
incluem `font_status: {"TintaLataoSans": "loaded", "TintaLataoSerif":
"loaded"}` pra que o caller saiba se as TTFs foram encontradas
ou se cai no fallback (Helvetica/Times-Roman built-in do
reportlab). **TTF e nao OTF** porque o Source Serif 4 release
notes avisa: Windows 10/11 tem bug com CFF2 variable OTF
(texto corrompido).

## Como rodar localmente

```pwsh
# 1. Instala Python 3.12.7 embeddable + pywin32 + python-docx +
#    openpyxl + reportlab + pdfplumber + 4 TTFs em runtime/
pwsh -NoProfile -ExecutionPolicy Bypass -File .\bootstrap.ps1
```

`bootstrap.ps1` e **idempotente**: cada bloco checa a presenca
do artefato final antes de baixar/instalar. Re-executar e O(1)
(~100ms de checks). Pra reinstalar do zero: apague `runtime/`
e rode de novo.

```pwsh
# 2. Roda o worker standalone (sem o app)
.\runtime\python.exe .\document-worker.py
# imprime `READY <name>` no stdout. Sem cliente, fica bloqueado
# em ConnectNamedPipe ate matar.

# 3. (Producao) O WorkerManager::spawn_external no Rust abre
#    esse mesmo python.exe com cwd = este diretorio. Cuidado:
#    o app define o env por allowlist (ADR-0017 §Invariantes) -
#    o `PATH` do pai e injetado automaticamente, mas o resto do
#    env e construido via `ExternalSpawnConfig.env`.
```

## Validacao E2E

Os 6 testes em
`crates/process-architecture/tests/external_doc_worker.rs`
rodam em CI via `scripts/verify-external.ps1` (NAO `#[ignore]`).
Se o `runtime/` nao estiver instalado, o test panic com
mensagem clara apontando pro bootstrap (REGRAS §2.6 - teste
pulado e regressao silenciosa).

## Layout

```text
document-worker/
|-- README.md            <- este arquivo
|-- manifest.json        <- manifesto versionado (worker.hello payload)
|-- pyproject.toml       <- deps (pywin32 + 4 libs)
|-- document-worker.py   <- entry point - protocolo + loop + 6 handlers
|-- bootstrap.ps1        <- instala runtime completo (Python + libs + TTFs)
|-- smoke.ps1            <- smoke local: sobe worker, mata depois
|-- smoke_handler.py     <- smoke local dos 6 handlers (sem pipe)
|-- manual_client.py     <- cliente PowerShell-like pra debug do pipe
|-- tests/               <- pytest roundtrip (NAO roda no CI - pytest
|                          nao esta no stack do Frederico; entra
|                          quando DocumentSpec integrar via Etapa 3)
|-- .gitignore           <- ignora runtime/ e __pycache__/
\-- runtime/             <- criado pelo bootstrap; nao versionado
    |-- python.exe
    |-- python312.dll
    |-- ...
    |-- Lib/site-packages/
    |   |-- win32/...
    |   |-- docx/...
    |   |-- openpyxl/...
    |   |-- reportlab/...
    |   \-- pdfplumber/...
    \-- fonts/
        |-- SourceSans3VF-Upright.ttf
        |-- SourceSans3VF-Italic.ttf
        |-- SourceSerif4Variable-Roman.ttf
        \-- SourceSerif4Variable-Italic.ttf
```

## Integração com o Rust

Quando o `WorkerManager::spawn_external` (Fase 5, Etapa 2B
continuacao, ja implementado) quiser abrir o `document-worker`, a
casca Tauri monta:

```rust
ExternalSpawnConfig::new("workers/document-worker/runtime/python.exe")
    .with_args(vec!["workers/document-worker/document-worker.py".into()])
    .with_cwd("workers/document-worker")
    .with_env(&[
        ("PYTHONIOENCODING", "utf-8"),
        ("PYTHONUNBUFFERED", "1"),  // critico: print() flush imediato
    ])
```

A Etapa 3 (integracao com o `ToolRegistry`) faz esse wire-up
completo - adiciona `ToolManifest` por capability, allowlist de
diretórios, e o mapeamento `DocumentSpec -> handler primitives`.
Ver ADR-0018 §Decisao 1: **handler = primitiva, kit = renderer
do DocumentSpec**. Os 6 handlers da v0.2.0 sobrevivem a Etapa 3
sem reescrita.
