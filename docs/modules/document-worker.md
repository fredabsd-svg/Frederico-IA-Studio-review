# Módulo `document-worker` (worker sidecar)

> **Etapa 2B+Y fechada (2026-07-30):** Tesseract 5.4.0.20240606 (UB-Mannheim GitHub Releases) + `por`+`eng`+`osd` traineddata (`tessdata_fast` 4.1.0) + handler `ocr.run` (consumindo `pytesseract`) + `pdf.read` com fallback OCR transparente (`text` e `ocr_text` sempre separados, parâmetro `ocr: "auto"|"never"|"only"`, teto de 20 páginas com `ocr_truncated`) + 5 testes E2E novos (2 com Tesseract + 3 sem) + CI noturno isolado (`.github/workflows/ci-nightly.yml`) + bootstrap estendido com SHA-256 fixo + admin detection + verificação de portabilidade. **Mudança visível do `pdf.read`:** PDF 100% escaneado que antes retornava `ok: false, code: pdf_scanned_no_ocr` agora pode retornar `ok: true` com `text` do OCR (CHANGELOG registra). ADR-0019 documenta as 5 decisões. Estado: produção. Verificado contra o código em 2026-07-30.

## 1. O que este módulo faz

Worker sidecar Python que gera documentos profissionais (DOCX, XLSX, PDF), lê os 3 formatos e faz OCR de imagens e PDFs escaneados. Comunica com o app principal via **named pipes do Windows** sobre o **envelope IPC** do `frederico-process-architecture` (line-delimited JSON, 8 opcodes estáveis em snake_case com prefixo de direção: `worker.hello`, `app.ack`, `app.ping`, `worker.pong`, `app.shutdown`, `worker.error`, `tool.invoke`, `tool.result`).

7 handlers (primitivas de I/O, conforme ADR-0018 §Decisão 1):

| Capability   | Input                                                | Library          |
| ------------ | ---------------------------------------------------- | ---------------- |
| `docx.write` | `path`, `title`, `sections`                           | python-docx      |
| `docx.read`  | `path`                                               | python-docx      |
| `xlsx.write` | `path`, `sheets`                                      | openpyxl         |
| `xlsx.read`  | `path` (opcional `sheet`)                            | openpyxl         |
| `pdf.write`  | `path`, `title`, `sections`                           | reportlab + Adobe Source Sans 3 / Source Serif 4 (Tinta e Latao, ADR-0018 §Decisão 2b) |
| `pdf.read`   | `path`, `ocr: "auto"|"never"|"only"` (opcional)        | pdfplumber + pytesseract (fallback OCR) |
| `ocr.run`    | `path`, `lang: "por+eng"` (opcional)                  | pytesseract + Tesseract 5.4.0 |

## 2. O que ele expõe

**Não-público** (rodado como subprocess, contrato via IPC):

- **Manifesto** (`workers/document-worker/manifest.json`):
  - `worker_id: "document-worker"`, `version: "0.3.0"`.
  - `capabilities`: 7 capabilities declaradas.
  - `dependencies`: 9 entries com SHA-256 fixo pra reprodutibilidade (Python 3.12.7, pywin32, python-docx, openpyxl, reportlab, pdfplumber, pytesseract, Tesseract 5.4.0 com SHA-256 do instalador, `tessdata_fast` 4.1.0 com SHA-256 por arquivo `por`/`eng`/`osd`).
  - `compatibility.ocr_languages_default: "por+eng"`, `ocr_languages_available: ["por", "eng", "osd"]`.
  - `health: "unhealthy"` (vira `Ok` só depois do primeiro `pong`).

- **Handshake** (resumo do `document-worker.py::worker_main`):
  1. Carrega `manifest.json`, registra fontes T&L, configura pytesseract (`tesseract_cmd` + `TESSDATA_PREFIX` + `config_data_dir`), detecta versão do Tesseract.
  2. Cria `NamedPipeServer` (maxInstances=1) com nome `frederico-document-worker-<uuid12>`.
  3. Imprime `READY <pipe_name>` no stdout (handshake invertido, ADR-0017).
  4. Espera `ConnectNamedPipe` (60s timeout).
  5. Envia `worker.hello` com manifesto + extras (`font_status`, `ocr_available`, `tesseract_version`, `tesseract_status`).
  6. Loop: lê linhas JSON do pipe, dispatcha por `op`.

- **Handshake auth**: `app.ack` carrega `WorkerAuth` (token de curta duração, UUID v4 ou pré-definido via `ExternalSpawnConfig::with_auth_token`). Validação em todo `tool.invoke` subsequente.

- **Extras no `worker.hello`** (campos além do `WorkerManifest`):
  - `font_status: {"TintaLataoSans": "loaded"|"fallback", "TintaLataoSerif": "loaded"|"fallback"}`.
  - `ocr_available: bool` — Tesseract + pytesseract prontos?
  - `tesseract_version: str|null` — versão detectada no startup via `tesseract.exe --version`.
  - `tesseract_status: {binary_present, pytesseract_imported, version, tessdata_dir}`.

- **Extras no `worker.pong`**:
  - `status: "ok"`, `env_received: {}`, `font_status` (memo do `worker.hello`).

- **Handler outputs** (cada um devolve um `dict` que vai pro `tool.result.payload`):
  - `docx.write`: `{ok, path, size_bytes, sections_written}`.
  - `docx.read`: `{ok, path, paragraphs, tables, n_paragraphs, n_tables}`.
  - `xlsx.write`: `{ok, path, size_bytes, sheets_written, total_rows}`.
  - `xlsx.read`: `{ok, path, sheets, n_sheets}`.
  - `pdf.write`: `{ok, path, size_bytes, pages_rendered, sections_written}`.
  - `pdf.read`: `{ok, path, text, ocr_text: {page: str}, page_count, scanned_pages, ocr_available, ocr_truncated, extraction: "text"|"ocr"|"mixed", tesseract_version}`. **Quebra de comportamento:** PDF 100% escaneado que antes retornava `ok: false, code: pdf_scanned_no_ocr` agora retorna `ok: true` com `text` do OCR + `extraction: "ocr"` (CHANGELOG registra).
  - `ocr.run`: `{ok, path, text, lang, conf, tesseract_version}`. Erros com code estruturado: `ocr_not_available`, `invalid_lang`, `tesseract_failed`, `ocr_timeout`, `image_not_found`.

- **Path safety** (ADR-0018 §Decisão 4): barreira mínima no handler Python.
  - Rejeita `..` como componente (`code: path_traversal`).
  - Path absoluto ou relativo ao `cwd`.
  - Write: diretório pai existe e gravável.
  - Read: arquivo existe e legível.
  - **Limitação:** sem allowlist de diretórios por tool. Allowlist forte entra na Etapa 3 com `ToolManifest::allowed_paths`; sandbox de OS (Fase 7) entra com `sandbox-runner`.

## 3. Do que depende e quem depende dele

**Quem depende:** o `apps/desktop` (casca Tauri) usa o `frederico-process-architecture` para abrir este worker. O `DocumentSpec` (Etapa 1 do `frederico-document-engine`) é mapeado para os 7 handlers via kit (`WordPro`/`ExcelPro`/`PdfPro`) na Etapa 3.

**Dependências externas (instaladas pelo `bootstrap.ps1` em `runtime/`, não via pip do sistema):**

| Dependência         | Versão        | Origem                                                  | SHA-256 fixo                                                                                  |
| ------------------- | ------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Python embeddable   | 3.12.7        | python.org zip                                          | (n/a; vem com python312.dll)                                                                  |
| pywin32             | >=306         | pip (`pip install pywin32`)                             | (pip-managed)                                                                                 |
| python-docx         | >=1.1         | pip                                                     | (pip-managed)                                                                                 |
| openpyxl            | >=3.1         | pip                                                     | (pip-managed)                                                                                 |
| reportlab           | >=4.0         | pip                                                     | (pip-managed)                                                                                 |
| pdfplumber          | >=0.10        | pip                                                     | (pip-managed)                                                                                 |
| Pillow (transitivo) | >=10.0        | pip (vem com pdfplumber)                                | (pip-managed)                                                                                 |
| pytesseract         | >=0.3.10      | pip                                                     | (pip-managed)                                                                                 |
| **Tesseract**       | 5.4.0.20240606 | GitHub Releases UB-Mannheim (asset do release oficial) | `C885FFF6998E0608BA4BB8AB51436E1C6775C2BAFC2559A19B423E18678B60C9` (instalador)            |
| `por.traineddata`   | tessdata_fast 4.1.0 | raw.githubusercontent.com (SHA-256 ancorado)        | `C4932B937207A9514B7514D518B931A99938C02A28A5A5A553F8599ED58B7DEB`                          |
| `eng.traineddata`   | tessdata_fast 4.1.0 | raw.githubusercontent.com                              | `7D4322BD2A7749724879683FC3912CB542F19906C83BCC1A52132556427170B2`                          |
| `osd.traineddata`   | tessdata_fast 4.1.0 | raw.githubusercontent.com                              | `9CF5D576FCC47564F11265841E5CA839001E7E6F38FF7F7AACF46D15A96B00FF`                          |
| Source Sans 3 (TTF variable, 2 arquivos) | —    | `raw.githubusercontent.com/adobe-fonts/source-sans/release/VF/` | (sem hash; tamanho > 50 KB + magic TTF) |
| Source Serif 4 (TTF variable, 2 arquivos) | —   | `raw.githubusercontent.com/adobe-fonts/source-serif/release/VAR/` | (sem hash; tamanho > 50 KB + magic TTF) |

Tudo verificado por SHA-256 ou magic header antes de usar.

## 4. Decisões não óbvias e armadilhas conhecidas

- **Handler = primitiva, não renderer de `DocumentSpec`** (ADR-0018 §Decisão 1). Os 7 handlers são burros: recebem estrutura, escrevem arquivo, devolvem `{ok, ...}`. Não decidem margem, fonte, numeração de página, etc. — isso é trabalho do **kit** (`WordPro`/`ExcelPro`/`PdfPro`) da Etapa 3. A v0.3.0 é deliberadamente feia em tipografia; a beleza visual é o kit.
- **Bootstrap é best-effort em contexto non-elevated** (ADR-0019 §Decisão 1). O instalador NSIS do Tesseract tem `requireAdministrator` no manifesto PE; PowerShell 5.1 non-admin bloqueia o `Start-Process` antes de executar. Em dev local não-admin, o bootstrap pula o bloco com warning + instruções. CI (`windows-latest`) roda como admin → silent install funciona. Usuário final recebe Tesseract via instalador NSIS do Tauri (Fase 9).
- **Tesseract silent install ignora `/D=<path>`** (bug do instalador UB-Mannheim confirmado pela issue tesseract-ocr/tesseract#4360 — o NSIS macro `MULTIUSER_INSTALLMODE_INSTDIR` sobrescreve o command line). O instalador SEMPRE instala em `C:\Program Files\Tesseract-OCR\` (w64-setup), mesmo passando `/D=...` corretamente. Estratégia do bootstrap: silent install com `/S` no path default + `Copy-Item -Recurse` pra `runtime/tesseract/`. Resultado: self-contained no `runtime/`, reproduzível, e o SHA-256 ainda protege contra MITM.
- **`pytesseract` opcional**: a falta dele **não** impede o worker de subir. Os handlers `docx`/`xlsx`/`pdf.write` continuam funcionando. Só `ocr.run` e fallback OCR do `pdf.read` ficam indisponíveis. O `worker.hello` carrega `ocr_available: false` e o `pdf.read` retorna `code: "ocr_not_available"` quando alguém chama com `ocr: "auto"`. Caller (kit) decide se trata como erro ou segue.
- **`text` e `ocr_text` sempre separados no `pdf.read`** (ADR-0019 §Decisão 3). Procedência clara, mesma disciplina de `origin`/`external_content` da memória (Etapa 4). OCR troca 8 por B, 0 por O, 1 por l — exatamente em CNPJ/valor/competência. Misturar apagaria a procedência.
- **Validação rigorosa do `lang`** (ADR-0019 §Decisão 2.5): regex `^[a-z]{3}(+[a-z]{3})*$` + checagem contra `INSTALLED_OCR_LANGS = {"por", "eng", "osd"}`. Defesa contra injeção de argumento de linha de comando. Erro estruturado (`code: "invalid_lang"`) com lista de disponíveis.
- **PDF fallback OCR automático usa `por` sozinho** (não `por+eng`): contexto brasileiro, sem lixo de outro idioma. Por que: em texto 100% português, `por+eng` costuma sair ligeiramente pior que `por` sozinho (léxicos competem).
- **Teto de páginas OCR no `pdf.read`** (`MAX_OCR_PAGES_PDF = 20`, `OCR_TIMEOUT_S_PER_PAGE = 30`): PDF escaneado de 200 páginas trava o worker por minutos. Quando bate o teto, devolve parcial com `ocr_truncated: true`.
- **Tesseract + tessdata com SHA-256 fixo e tag do release** (não `raw/main`): `raw/main` muda sem aviso; dois downloads, dois traineddata, dois resultados. Com tag `4.1.0` + SHA-256 por arquivo, o bootstrap é reproduzível.
- **Inversão do handshake** (ADR-0017 §Decisão 2): worker cria o `NamedPipeServer` e anuncia o nome via stdout `READY <pipe_name>`; app se conecta como `NamedPipeClient`. Resolve herança de handle (Tokio `Command` herda stdin/stdout/stderr automaticamente no Windows).
- **A v0.3.0 do `document-worker` foi commitada SEM o instalador Tesseract pré-instalado** (diferente da v0.2.0 que já vinha com fontes). CI cacheia o `runtime/` por hash de `pyproject.toml` + `bootstrap.ps1` — bump de qualquer um refaz a instalação.
- **Comparação OCR no E2E é normalizada** (lowercase + colapso de espaços), não literal: Tesseract troca caracteres (`1` por `l`, `0` por `O`, `5` por `S`). Comparação exata quebra o teste em 100% dos casos reais.
- **Testes OCR (E2E) chamam `tesseract_or_panic()` no início**: panic claro apontando pro bootstrap se Tesseract não estiver. **Não** são `#[ignore]`. CI gate + CI noturno têm Tesseract (via bootstrap), então passam. Em dev local sem Tesseract, panic instrui o dev a rodar como Admin.
- **CI noturno isolado** (`.github/workflows/ci-nightly.yml`, cron `0 4 * * *` UTC = 01:00 BRT): detecta flakiness do Tesseract sem bloquear merge. Gate principal (`ci.yml`) continua intocado.
- **Verificação de portabilidade do Tesseract** (rodada na primeira execução do bootstrap): copia `runtime/tesseract/` pra outro path, seta `TESSDATA_PREFIX`, roda `--list-langs`. Se a árvore depender de registro/HKLM/variável global, o bootstrap loga warning e marca como não-portátil (plano alternativo entra — extração NSIS via dep externa, registrada no ADR-0019 §"Plano alternativo").

## 5. Como testá-lo isoladamente

```pwsh
# 1. Instala runtime completo (Python + libs + Tesseract + tessdata + fontes)
pwsh -NoProfile -ExecutionPolicy Bypass -File .\bootstrap.ps1

# 2. Roda o worker standalone (sem o app)
.\runtime\python.exe .\document-worker.py
# imprime `READY <name>` no stdout. Sem cliente, fica bloqueado em ConnectNamedPipe.

# 3. (Produção) O WorkerManager::spawn_external no Rust abre esse mesmo python.exe
#    com cwd = este diretório, env_allowlist explícito + PATH do pai.
```

**Suíte E2E (Rust):**

```pwsh
# Roda o `verify-external.ps1` (11 testes: 9 sem Tesseract + 2 com).
# - 6 da Etapa 2B+X: docx.write/read, xlsx.write/read, pdf.write/read,
#   path_safety, pdf_read_reports_ocr_unavailable, unknown_capability.
# - 5 da Etapa 2B+Y: ocr_run_with_real_image (Tesseract),
#   ocr_run_with_invalid_lang (sem Tesseract), ocr_run_without_tesseract,
#   pdf_read_with_ocr_param_never (sem Tesseract),
#   pdf_read_with_ocr_fallback_on_scanned (Tesseract).
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-external.ps1
```

Se os 2 testes com Tesseract panics com mensagem "tesseract.exe não encontrado em X", rode o bootstrap como Admin:

```pwsh
# Abre PowerShell como Admin e roda o bootstrap:
pwsh -NoProfile -ExecutionPolicy Bypass -File .\bootstrap.ps1
```

## 6. O que ele **não** faz (limites explícitos)

- **Não decide layout, tipografia, cores, numeração de página, header/footer.** Isso é trabalho do kit (`WordPro`/`ExcelPro`/`PdfPro`) da Etapa 3. A v0.3.0 é deliberadamente feia em tipografia.
- **Não aplica `DocumentSpec` declarativo** (20 blocos da Etapa 1 do `frederico-document-engine`). O kit da Etapa 3 faz o mapeamento.
- **Não tem sandbox de OS.** `validate_path` rejeita `..` e exige path absoluto/relativo-ao-cwd, mas é barreira mínima. Allowlist forte (por `ToolManifest::allowed_paths`) entra na Etapa 3. Sandbox de OS (Fase 7 com `sandbox-runner`).
- **Não compila Tesseract do source.** Usa o binário pré-compilado do UB-Mannheim (5.4.0.20240606). Compilar do source precisa de Visual Studio Build Tools (~6 GB) que o ambiente não tem.
- **Não suporta OpenCV, Poppler, pdf2image como dependência explícita.** Pillow + pdfplumber cobrem o necessário (render de PDF → imagem). Tesseract fala direto com a imagem.
- **Não expõe API HTTP.** Sem `localhost`, sem TCP (regra do `process-architecture.md` §Invariantes). Só named pipes locais.
- **Não tem hot-reload de capabilities.** Tabela `HANDLERS` é estática no `document-worker.py`. Adicionar capability = bump de versão + alterar manifesto + restart do worker.
- **Não tem retry de OCR.** Falha do Tesseract (`tesseract_failed`, `ocr_timeout`) é reportada ao caller, que decide retry.
- **Não é determinístico.** OCR é probabilístico. Comparação de E2E é por similaridade (token-level, normalizado), não exata.
- **Não roda em Linux/macOS.** Gateado em Windows (named pipes do Windows). CI em outras plataformas compila o `lib.rs` do `process-architecture` sem o módulo.
- **PDF escaneado > 20 páginas** cai no teto de OCR (`MAX_OCR_PAGES_PDF`) e devolve parcial com `ocr_truncated: true`. Caller decide retry/abort. Sem teto, o timeout do worker estoura.
- **Idiomas fora de `{por, eng, osd}` não funcionam.** Adicionar novo idioma = download de 1 arquivo + bump de SHA-256 + bump da tag tessdata. Sem alteração de código Python.

## Próxima etapa

- **Etapa 3 (ToolRegistry + kits DocumentSpec):** `ToolManifest::allowed_paths` para path safety forte. Os 7 handlers da v0.3.0 sobrevivem à Etapa 3 sem reescrita (handler = primitiva, kit = renderer do DocumentSpec, conforme ADR-0018 §Decisão 1).
- **Fase 9 (Produção):** instalador NSIS do Frederico (Tauri) pré-instala Tesseract em `runtime/tesseract/`, em contexto elevado. O bootstrap detecta Tesseract já presente e pula o bloco (idempotente).
- **Capacities dinâmicas:** quando usuário final desinstala Tesseract, o `ToolRegistry` deve refletir `ocr_available: false` na UI (não mostrar "abrir PDF escaneado"). Acoplamento melhor é trabalho do `ToolRegistry` (Etapa 3).

## Referências

- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — Python embeddable + libs base.
- [ADR-0017](../decisions/0017-process-architecture-windows-pipes.md) — transporte sobre named pipes.
- [ADR-0018](../decisions/0018-document-worker-handlers-primitive.md) — handler como primitiva; `ocr.run` deferido para 2B+Y.
- [ADR-0019](../decisions/0019-document-worker-ocr-tesseract.md) — Tesseract source, lang, fallback pdf, CI noturno.
- [`docs/architecture/process-architecture.md`](../architecture/process-architecture.md) — invariantes (env allowlist, sem TCP, worker autenticado).
- [`docs/architecture/document-engine-architecture.md`](../architecture/document-engine-architecture.md) — `DocumentSpec` v0.1 (20 blocos, Etapa 1 fechada).
- `PROMPT MESTRE` §5.3, §7.3, §16.3-§16.6, §22.5
