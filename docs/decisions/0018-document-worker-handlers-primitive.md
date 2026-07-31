# 0018 — `document-worker` v0.2.0: handlers como primitivas, bootstrap leve, OCR deferido

## Contexto

A Etapa 2B entregou o transporte real (ADR-0017) + o `WorkerManager::spawn_external` + o esqueleto Python do `document-worker` (PR #11). O esqueleto tem 7 capabilities declaradas no `manifest.json` (`docx.write`, `docx.read`, `xlsx.write`, `xlsx.read`, `pdf.write`, `pdf.read`, `ocr.run`), mas o handler de `tool.invoke` é um **stub único** que devolve `ok: false, code: "handler_stub"`.

A Etapa 2B+X precisa fechar duas pendências abertas no [`docs/modules/process-architecture.md`](../modules/process-architecture.md) §"Pendências para a próxima sessão":

1. **Tesseract + fontes "Tinta & Latão" no `bootstrap.ps1`** (pendência 1).
2. **Handlers reais** consumindo as bibliotecas da ADR-0004 (pendência 2).

O plano inicial tratava as duas juntas em um PR único, somando ~150 MB ao bootstrap (Python + libs + Tesseract UB Mannheim + por+eng traineddata + fontes Adobe). Em revisão com o usuário, três problemas apareceram:

- **OCR concentra o risco**: o `ocr.run` sozinho puxa Tesseract (~75 MB) + traineddata por+eng (~60 MB) — 90% do salto de tamanho da etapa. Uma capability de 15% do valor de uso puxando 90% do peso, com fonte third-party (UB Mannheim) e porção significativa da complexidade de "funciona na minha máquina". A separação faz sentido.
- **Testes gateados em CI não guardam nada**: marcar 7 E2E com `#[ignore]` + "skip limpo se o bootstrap não rodou" é o caminho mais curto para um test suite cheio de zombies. Regressão entra sem ninguém ver.
- **Risco de reescrita na Etapa 3**: o handler `docx.write` é uma primitiva de baixo nível (escreve um arquivo `.docx` com estrutura passada). A Etapa 3 introduz o `DocumentSpec` (§16.4 do PROMPT MESTRE) — formato declarativo de alto nível (Cover, Heading, Paragraph, Table, Kpis, ...) que mapeia para os formatos finais via **kits** (`WordPro`, `ExcelPro`, `PdfPro` — §16.5). Sem uma decisão explícita sobre o papel do handler (renderer do spec vs. primitiva sob o kit), a Etapa 3 pode reescrever os 6 handlers.

## Decisão

A Etapa 2B+X fecha **6 das 7 capabilities** declaradas no `manifest.json` original. **`ocr.run` é removido do manifesto e vai para a Etapa 2B+Y, sozinho** (Tesseract + por/eng traineddata + handler `ocr.run` + fallback OCR no `pdf.read`).

### 1. Handler = primitiva, não renderer de `DocumentSpec`

O `document-worker` v0.2.0 é uma **biblioteca de primitivas de I/O sobre formatos Office/PDF**. Cada handler é uma chamada de baixo nível sobre a biblioteca Python correspondente (`python-docx`, `openpyxl`, `reportlab`, `pdfplumber`):

- `docx.write(payload)` → escreve um `.docx` no path pedido, consumindo estrutura `{"title", "sections": [{"heading", "paragraphs": [...]}]}`. **Não** decide margem, fonte, cor, header/footer, numeração de página. Esses ficam para o **kit** (`WordPro`) da Etapa 3.
- `docx.read(path)` → extrai parágrafos e tabelas como JSON neutro. **Não** reconstrói o `DocumentSpec`. Quem faz isso é o kit (Etapa 3).
- `xlsx.write(payload)` / `xlsx.read(path)` — mesmo princípio, com `openpyxl`.
- `pdf.write(payload)` — `reportlab`, **embute** as fontes Source Sans 3 (corpo) e Source Serif 4 (títulos) por padrão, mas não faz paginação avançada, sem numeração de página, sem cabeçalho/rodapé. Quem faz isso é o `PdfPro` (Etapa 3).
- `pdf.read(path)` — `pdfplumber`, devolve texto por página + lista de `scanned_pages` (páginas sem camada de texto — imagens).

O `DocumentSpec` (definido em `crates/document-engine`, Etapa 1) **mapeia para** essas primitivas via um **kit** (Etapa 3). O kit recebe o `DocumentSpec` validado e traduz para o JSON de cada handler. O handler em si é burro — recebe estrutura, escreve arquivo, devolve `{ok, path, ...}`. **Nada de "smart formatting" no handler**: a v0.2.0 é deliberadamente feia em margens, quebras de página, etc. A beleza visual é trabalho do kit.

**Consequência prática:** os 6 handlers da v0.2.0 sobrevivem à Etapa 3 sem reescrita. A Etapa 3 adiciona um kit que **chama** esses handlers — não que os substitui. O contrato dos handlers (input/output shape) é estável a partir de agora.

### 2. Bootstrap estendido — sem Tesseract

O `bootstrap.ps1` ganha três blocos idempotentes depois do passo `pywin32` (já existente):

a) **Bibliotecas Python** (uma chamada `pip install`):

   - `python-docx>=1.1` (DOCX)
   - `openpyxl>=3.1` (XLSX)
   - `reportlab>=4.0` (PDF write)
   - `pdfplumber>=0.10` (PDF read)
   - `Pillow>=10.0` (transitivo, usado por `pdfplumber` pra extração de imagens)
   - `lxml>=5.0` (transitivo, requerido por `python-docx`)

   `pymupdf` (fitz) e `matplotlib` **ficam fora** da v0.2.0 — `pdfplumber` cobre o caso de leitura de texto em PDFs com camada; `matplotlib` só faz sentido com `DocumentSpec` integrado via `Chart` block, que entra na Etapa 3.

b) **Fontes "Tinta & Latão"** — Adobe Source Sans 3 + Source Serif 4 (variable fonts, ~3 MB total em 4 arquivos: Roman + Italic de cada). Fontes SIL Open Font License 1.1 (compatível com embedding em PDFs comerciais — REGRAS do `pdfpro-specification.md`).

   - **Fonte primária:** repositórios oficiais `adobe-fonts/source-sans` (branch `release`) e `adobe-fonts/source-serif` (branch `release`) no GitHub. Os arquivos variáveis estão em `VF/` (Source Sans 3) e `VAR/` (Source Serif 4) — diretórios versionados pela Adobe com cada release taggeada. Download direto via `raw.githubusercontent.com` (estável, apontando pro SHA do último release).
   - **Por que Adobe:** são as fontes nomeadas no `PROMPT MESTRE` §16.3 e na ADR-0004 ("fontes da identidade visual 'Tinta & Latão'"). Substituta não é decisão trivial — REGRAS §1.1 + REGRAS §1.11 diz que decisão de identidade visual vira ADR ou entra num já existente; este é o ADR.
   - **Por que variable font:** um único `.ttf` cobre todos os pesos (Light → Black) e italics. PDF fica menor (4 arquivos em vez de 8+), e o `reportlab` registra com `TTFont("Source Sans 3", "...")` igual.
   - **Por que TTF e não OTF:** a release `4.005R` do Source Serif 4 traz uma nota explícita — *"Windows 10 and 11 currently have a major bug handling CFF2 variable fonts that could result in text corruption; we recommend using the TTF files on Windows machines."* O Frederico roda em Windows (PROMPT MESTRE §3), então OTF variable está descartado para o Source Serif 4. Por consistência (mesma justificativa, mesma mitigação), Source Sans 3 também usa TTF. O TTF variable é o que o `reportlab` registra com `TTFont` direto.
   - **Instalação:** `runtime/fonts/SourceSans3VF-Upright.ttf`, `SourceSans3VF-Italic.ttf`, `SourceSerif4Variable-Roman.ttf`, `SourceSerif4Variable-Italic.ttf`. O `pdf.write` faz auto-load procurando primeiro em `runtime/fonts/`, depois em `%LOCALAPPDATA%\Microsoft\Windows\Fonts\` (caso o usuário tenha a fonte instalada no sistema — fallback).

c) **Skip markers** — cada bloco verifica a presença do artefato final (`runtime/fonts/SourceSans3-VF.ttf` existe, `runtime/Lib/site-packages/docx` existe, etc.) e pula. Bootstrap vira O(1) na reexecução (~100ms de checks).

**Tamanho final do `runtime/`**: ~70-80 MB (Python 3.12.7 ~30 MB + pywin32 ~10 MB + 5 libs + transitivos ~25 MB + fontes ~1 MB + caches pip ~5 MB). **Cabe** num job de CI Windows com cache (`Swatinem/rust-cache@v2` já é usado — estender o cache para `workers/document-worker/runtime/`).

### 3. `pdf.read` com detecção de páginas escaneadas (limitação registrada)

`pdfplumber` extrai texto de PDFs com camada de texto. Para PDFs escaneados (imagens sem OCR), o texto extraído de uma página é vazio. O `pdf.read` da v0.2.0 detecta isso e devolve um payload com `scanned_pages`:

```json
{
  "text": "página 1 tem texto aqui\n",
  "page_count": 5,
  "scanned_pages": [3, 4],
  "ocr_available": false
}
```

Se **todas** as páginas são escaneadas (texto vazio + todas em `scanned_pages`), o worker devolve `tool.result { ok: false, code: "pdf_scanned_no_ocr", message: "PDF escaneado detectado (página(s) N, M, ...); OCR não disponível até 2B+Y" }`. Honesto sobre o que não dá pra fazer.

Esse modo degradado é **registrado no `CHANGELOG.md`** como limitação conhecida, com link para a pendência 4 do `docs/modules/process-architecture.md` (a 2B+Y). O caller (kit ou app) pode tratar como aviso e seguir, ou como erro e abortar — o payload deixa claro.

### 4. Path safety — barreira mínima

Worker sidecar ainda **não** tem sandbox de OS (esse é o `sandbox-runner` da Fase 7). Para a v0.2.0, a barreira mínima é:

- Toda path de saída (`docx.write`/`xlsx.write`/`pdf.write` `payload.path`) é validada: não pode conter `..` como componente, deve ser absoluta ou relativa ao `cwd` do worker, e o **diretório pai** deve existir e ser gravável.
- Toda path de leitura (`docx.read`/`xlsx.read`/`pdf.read` `payload.path`) é validada: não pode conter `..`, deve ser absoluta ou relativa ao `cwd`, e o **arquivo** deve existir e ser legível.
- A Etapa 3 (ToolRegistry) vai adicionar uma camada de **allowlist de diretórios** por tool — o `ToolManifest` carrega `allowed_paths: [PathBuf]` e o manager valida antes do `invoke`. Isso é **explicitamente fora** do escopo da v0.2.0 (registrado como pendência — o worker só sabe o seu próprio `cwd`).

Essas regras estão no **handler Python** (não no manager Rust) porque o manager é genérico. A validação é um único helper `validate_path(p: str, kind: "read" | "write") -> Path` que todos os handlers chamam.

### 5. CI roda os 6 testes E2E

Os 6 testes E2E (`tests/external_doc_worker.rs`) **não** são `#[ignore]`. Eles rodam no `windows-latest` do GitHub Actions em **todos os PRs**. O CI ganha um step novo:

```yaml
- name: Cache document-worker runtime
  uses: actions/cache@v4
  with:
    path: workers/document-worker/runtime
    key: document-worker-runtime-${{ hashFiles('workers/document-worker/pyproject.toml', 'workers/document-worker/bootstrap.ps1') }}

- name: Bootstrap document-worker (idempotente)
  shell: pwsh
  run: pwsh -NoProfile -ExecutionPolicy Bypass -File workers/document-worker/bootstrap.ps1

- name: E2E document-worker handlers
  shell: pwsh
  run: pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/verify-external.ps1
```

`verify-external.ps1` é um script fino: roda `cargo test -p frederico-process-architecture --test external_doc_worker` e nada mais. Idempotente. Se o bootstrap não rodou, ele falha com mensagem clara apontando para o bootstrap. O cache é por hash de `pyproject.toml` + `bootstrap.ps1` — bump de dependência ou do script invalida automaticamente.

A Etapa 2B+Y ganha um **job noturno** separado (`schedule: cron` em `.github/workflows/ci-nightly.yml`) que adiciona o Tesseract e roda os testes de OCR. Justificativa: Tesseract no Windows tem edge cases de install (UB Mannheim installer, `PATH` injection, locale) que podem produzir testes flaky. Job noturno isola a flakiness potencial do PR gate.

## Travas de CI

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock`, `cargo test --workspace`, `scripts/check-core-purity.ps1` (REGRAS §2, ADR-0015, ADR-0003) — todos continuam.
- **Novo:** step "E2E document-worker handlers" no `windows-latest` job. Falha aqui = PR não entra.
- O cache do `runtime/` é invalidado por hash de `pyproject.toml` + `bootstrap.ps1` — bump de `python-docx` rebuilda, mas bump do handler Python **não** (handlers não estão no `runtime/`).
- O `WorkerManifest::health` continua reportando `Unhealthy` no boot (só vira `Ok` depois do primeiro `pong`) — sem regressão na invariant.

## Alternativas descartadas

- **Tesseract no 2B+X.** Descartada pelo princípio de Pareto — 1 capability de 7 puxa 90% do peso, do risco de install, e da variabilidade "funciona na minha máquina". A 2B+Y sozinha (com job noturno) isola essa complexidade.
- **Handlers como renderer de `DocumentSpec` no worker.** Descartada pelo risco de reescrita na Etapa 3. O `DocumentSpec` é declarativo (alto nível); os handlers são imperativos (baixo nível). Quem faz a tradução é o **kit** (Etapa 3, Rust). O worker vira "biblioteca de I/O" — burro de propósito, sobrevive a mudanças no spec.
- **PyMuPDF (`fitz`) em vez de `pdfplumber`.** Descartada: `pdfplumber` é built on top de `pdfminer.six` e cobre o caso de leitura de texto com mais clareza. `fitz` é melhor para extração de imagens e manipulação de PDF binário, que **não** é requisito da v0.2.0 (Kpis/Chart blocks do `DocumentSpec` ficam para a Etapa 3 com `matplotlib`).
- **Testes `#[ignore]` com mensagem "rode o bootstrap".** Descartada pelo §2.6 do REGRAS — teste instável é defeito bloqueante, mas teste "pulado pra sempre" é pior: é regressão silenciosa. Com o bootstrap leve (~70 MB) cabe no CI com cache.
- **Fontes do Google Fonts (mirror) em vez de adobe-fonts GitHub.** Descartada: Google Fonts redistribui as mesmas fontes, mas a fonte de verdade (release oficial com tag, changelog, assinatura) é o repo Adobe. Mirror é mais um hop de risco. A Adobe mantém as releases ativas.
- **Bootstrap do Tesseract via `pip install` (wrapper).** Descartada: `pip install pytesseract` instala só o wrapper Python; o binário Tesseract precisa ser baixado separado. Tesseract no Windows é distribuído como instalador `.exe` (UB Mannheim) ou zip manual. O `bootstrap.ps1` da 2B+Y baixa o zip e extrai.
- **Path safety via `chroot`/jail de OS.** Descartada: fora do escopo da v0.2.0. É o trabalho do `sandbox-runner` (Fase 7). Para a v0.2.0, o app principal confia no worker (mesma confiança que tem no `pywin32` que ele já usa).

## Consequências

**Mais fácil:**

- A v0.2.0 do `document-worker` é genuinamente útil: gera DOCX, XLSX, PDF e lê os três formatos. O `docs.generate` da Etapa 3 pode ser wireado já com cobertura razoável.
- CI pega regressão em todos os 6 handlers a cada PR — fail-fast.
- O ADR-0018 é a **âncora** da Etapa 3: quando alguém for escrever o kit, a regra "handler é primitiva, kit é renderer" já está documentada e justificada.
- Bootstrap cabe em CI com cache — mesmo pipeline, sem job extra.

**Mais difícil:**

- O `pdf.read` da v0.2.0 **não faz OCR** — caller precisa saber disso e tratar como limitação. A interface (`scanned_pages` no payload, `pdf_scanned_no_ocr` no erro) é clara, mas é mais um campo que o caller precisa conhecer. Documentado no `CHANGELOG.md`.
- O cache de `runtime/` no CI adiciona ~100-500 KB por bump de `pyproject.toml` (o hash muda, cache é repopulado). Aceitável.
- A separação 2B+X / 2B+Y adiciona **uma sessão** de trabalho no roadmap da Fase 5. Compensado pelo ganho de CI fechado e ADR-0018 travando o handler=primitiva.

## Pendências para a próxima sessão

1. **Etapa 2B+Y (separada desta):** Tesseract bootstrap, `ocr.run` handler, `pdf.read` ganha fallback OCR para `scanned_pages`. Job noturno no CI. Re-adiciona `ocr.run` no `manifest.json` (versão bump pra 0.3.0). Pode ser feita no mesmo PR ou separada — Tesseract no Windows é grande.
2. **Allowlist de paths no `ToolManifest` (Etapa 3).** A barreira de path atual é por handler Python (rejeita `..`). A camada mais forte é no manager Rust, consumindo `ToolManifest::allowed_paths`. DocumentSpec → ToolManifest acontece na Etapa 3; a allowlist entra junto.
3. **Revogação de token** (pendência herdada da Etapa 2B original). Continua fora de escopo.
4. **Tabela de capabilities dinâmica** (pendência herdada da Etapa 2B original). Continua fora de escopo.

## Referências

- [ADR-0004](0004-document-worker-em-python-embutido.md) — Python embeddable + libs base.
- [ADR-0017](0017-process-architecture-windows-pipes.md) — transporte sobre named pipes.
- [`docs/architecture/process-architecture.md`](../architecture/process-architecture.md) — invariantes (env allowlist, sem TCP, worker autenticado).
- [`docs/architecture/document-engine-architecture.md`](../architecture/document-engine-architecture.md) — `DocumentSpec` v0.1 (20 blocos, Etapa 1 fechada).
- `PROMPT MESTRE` §5.3, §7.3, §16.3-§16.6, §22.5
