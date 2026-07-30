# `document-worker`

Worker sidecar do Frederico IA Studio que gera documentos profissionais
(DOCX, XLSX, PDF) e le os tres formatos. Python embutido (ADR-0004),
comunica com o app via **named pipes** do Windows.

## Estado atual (Etapa 2B+X, 2026-07-30)

**6 handlers reais** (o esqueleto de protocolo + transporte da Etapa
2B continuação agora consome bibliotecas de verdade):

| Capability   | Input                                          | Library         |
| ------------ | ---------------------------------------------- | --------------- |
| `docx.write` | `path`, `title`, `sections`                    | python-docx     |
| `docx.read`  | `path`                                         | python-docx     |
| `xlsx.write` | `path`, `sheets`                               | openpyxl        |
| `xlsx.read`  | `path` (opcional `sheet`)                      | openpyxl        |
| `pdf.write`  | `path`, `title`, `sections`                    | reportlab       |
| `pdf.read`   | `path`                                         | pdfplumber      |

**`ocr.run` foi REMOVIDO do manifesto nesta versão.** Vai pra
**Etapa 2B+Y** (Tesseract + por/eng traineddata) sozinho —
ADR-0018 §Decisao 2d justifica a separação (1 capability de 7
puxava 90% do peso, do risco de install e da variabilidade
"funciona na minha maquina"). Versao bump 0.1.0 -> 0.2.0;
quando 2B+Y entrar, vira 0.3.0 com `ocr.run` re-adicionado.

**Path safety minima (ADR-0018 §Decisao 4):** handlers rejeitam
`..` como componente, exigem path absoluto ou relativo ao `cwd`,
e validam gravabilidade do diretorio pai (write) ou legibilidade
do arquivo (read). Allowlist mais forte (por `ToolManifest`) entra
na Etapa 3; sandbox de OS (sandbox-runner) entra na Fase 7.

**`pdf.read` limitacao conhecida:** PDFs 100% escaneados (imagens
sem camada de texto) devolvem `code: pdf_scanned_no_ocr` no
payload. `scanned_pages: [n, m, ...]` lista paginas sem texto
quando ha mistura (texto + imagem). OCR de verdade vem na 2B+Y
com Tesseract + fallback automatico. Registrado no CHANGELOG.

**Fontes "Tinta e Latao" (ADR-0018 §Decisao 2b):** Adobe Source Sans 3
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
