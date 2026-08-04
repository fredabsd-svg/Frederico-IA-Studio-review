#!/usr/bin/env pwsh
# verify-external.ps1
#
# Verificacao dos testes E2E do `document-worker` Python (Fase 5,
# Etapa 2B+X). Roda em 3 passos:
#
#   1. Garante que o `workers/document-worker/runtime/` esta
#      instalado (chama `bootstrap.ps1` que e idempotente - pula
#      secoes ja instaladas).
#   2. Roda os 6 testes E2E em
#      `crates/process-architecture/tests/external_doc_worker.rs`
#      (NAO `#[ignore]` - sao obrigatorios).
#   3. Reporta tempo total + exit code.
#
# **Por que script separado?** O `verify.ps1` principal foca em
# gates mecanicos (fmt, clippy, cargo test, check-core-purity).
# O `verify-external` adiciona ~3s de overhead (Python cold-start)
# e requer que o `runtime/` esteja instalado - dependencias que
# o CI satisfaz com cache (`actions/cache@v4`) e o developer
# local satisfaz com `pwsh -File bootstrap.ps1` uma unica vez.
#
# Falha no primeiro erro. Cada bloco loga tempo e exit code.

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path "$PSScriptRoot/.."
Set-Location $repoRoot

# Garante que cargo e rustc estao no PATH.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

# Detecta o executavel do PowerShell. No GitHub Actions
# windows-latest, `pwsh` (PowerShell 7) esta disponivel. Em
# maquinas locais com so Windows PowerShell 5.1, so `powershell`
# existe. Aceita os dois.
$PsExe = if (Get-Command pwsh -ErrorAction SilentlyContinue) { 'pwsh' } else { 'powershell' }

function Invoke-Step {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [scriptblock] $Block
    )
    Write-Host ""
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Block
    $code = $LASTEXITCODE
    $sw.Stop()
    if ($code -ne 0) {
        Write-Host "[$Name] FALHOU em $($sw.Elapsed.ToString('mm\:ss')) (exit $code)" -ForegroundColor Red
        exit $code
    }
    Write-Host "[$Name] ok em $($sw.Elapsed.ToString('mm\:ss'))" -ForegroundColor Green
}

# Step 1 - Bootstrap do document-worker (idempotente: pula secoes
# ja instaladas). Em CI, o cache `actions/cache@v4` no
# `.github/workflows/ci.yml` faz o `runtime/` ja chegar pre-instalado
# - o bootstrap detecta e pula tudo em < 100ms.
Invoke-Step "document-worker bootstrap" {
    & $PsExe -NoProfile -ExecutionPolicy Bypass -File workers/document-worker/bootstrap.ps1
    if ($LASTEXITCODE -ne 0) { throw "bootstrap.ps1 falhou" }
}

# Step 2 - Testes E2E do document-worker. Os 6 tests em
# `external_doc_worker.rs` (NAO `#[ignore]` - rodam em todo PR).
# Se o bootstrap nao foi feito antes, o helper `doc_worker_config`
# no teste faz `panic!` com mensagem clara apontando pro bootstrap.
Invoke-Step "E2E document-worker handlers" {
    # `--skip ocr --skip pdf_read_with_ocr` é no-op em CI
    # (Tesseract instalado via bootstrap) e desliga os 2
    # testes que exigem Tesseract em dev local non-elevated.
    # Os 9 testes restantes (docx/xlsx/pdf write+read +
    # adaptativos) rodam em todo PR.
    cargo test -p frederico-process-architecture --test external_doc_worker -- --skip ocr --skip pdf_read_with_ocr
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 3 - Teste E2E do `docs.generate` (Etapa 3 da Fase 5).
# Valida o full vertical: DocumentSpec → kit → dispatcher →
# worker → .docx → reopen via python-docx → hierarquia +
# linhas da tabela.
Invoke-Step "E2E docs.generate full vertical" {
    cargo test -p frederico-document-kits --test e2e_docs_generate
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 4 - Teste E2E do `docs.generate` para `.xlsx` (Etapa 4
# da Fase 5). Valida o full vertical do ExcelPro: DocumentSpec
# (Spreadsheet com Kpis + Table + Table + Chart) → kit →
# dispatcher → worker → .xlsx → reopen via openpyxl → Painel
# 1ª aba + 3 sheets + has_total + has_brl_format +
# has_pct_format + charts_sheet_count=0. Inclui também
# validação do `sheets: [{block_index, sheet_name}]` no
# output do `generate` e `warnings` com chart.
Invoke-Step "E2E docs.generate xlsx" {
    cargo test -p frederico-document-kits --test e2e_docs_generate_xlsx
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 5 - Teste E2E do `docs.inspect` (Etapa 4 da Fase 5).
# Valida o round-trip: spec com Cover + 2 Headings + Paragraph
# + Table → generate .docx → inspect no mesmo arquivo →
# `coverage.preserved` tem heading+paragraph (NÃO table — é
# limitação do WordPro v0.1), `coverage.lost` inclui cover
# (NÃO table — inspect sabe ler tabela real), 2 headings
# preservados com os textos certos, 0 tables e 0 covers
# em `spec.blocks`. Round-trip pela mesma porta que o
# modelo usa (não pelo handler direto).
Invoke-Step "E2E docs.inspect" {
    cargo test -p frederico-document-kits --test e2e_docs_inspect
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 6 - Teste E2E do `docs.generate` para PDF (Etapa 5 PR 2).
# Cobre o bump atomico do enum `DocumentFormat::Pdf` + o
# `PdfProKit` real: render com fontes Tinta & Latao
# embutidas + glifo-check via fontTools (D-GLYPH-1) +
# watermark opt-in (D-PDF2). Reabre o .pdf via `pdfplumber`
# em subprocess do Python do worker e valida n_pages,
# titulo, heading, paragrafo, tabela, chart placeholder e
# callout. 3 testes: full vertical, missing_glyph (D-GLYPH-1),
# watermark (D-PDF2).
Invoke-Step "E2E docs.generate pdf" {
    cargo test -p frederico-document-kits --test e2e_docs_generate_pdf
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 7 - Teste E2E do `docs.generate` com `DocumentWorkerLauncher`
# real (Fase de Ligação, Etapa 5, 2026-08-04). Gera um .docx
# via Python e reabre validando a hierarquia. **Esse é o teste
# que prova "a Fase de Ligação fechou"** — sem ele, a Etapa 5
# fecha com `cargo test --workspace` verde mas sem nunca ter
# gerado um documento pelo caminho do produto. Marcado
# `#[ignore]` no source; `--include-ignored` ativa.
#
# Ver `crates/e2e/tests/e2e_docs_generate_with_real_worker.rs`
# e `docs/architecture/testing-strategy.md` §3 "Fronteira do
# que os E2E cobrem".
Invoke-Step "E2E docs.generate (caminho do produto, frederico-e2e)" {
    cargo test -p frederico-e2e --test e2e_docs_generate_with_real_worker -- --include-ignored
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

# Step 8 - Teste E2E de memória real (Fase de Ligação, Etapa 3,
# 2026-08-04). Classificador LLM + embedding real via OpenRouter;
# salva um facto, recupera por paráfrase com `HybridRetriever`.
# **Esse é o teste que prova "memória real funciona ponta a
# ponta"** — sem ele, a Etapa 3 fecha com `cargo test
# --workspace` verde mas o classificador fica em `Noop` e o
# retrieval lexical-only (mesma armadilha que a Etapa 5.X
# da Fase de Ligação documentou no PR #25: mecanismo que
# nunca roda no caminho real parece funcionar até o dia
# que precisa). Marcado `#[ignore]` no source;
# `--include-ignored` ativa.
#
# Requer `OPENROUTER_API_KEY` em runtime — o helper
# `memory_real_providers_or_skip!` faz panic com mensagem
# clara se faltar. Em CI, a env é setada como secret do
# repositório (ver `.github/workflows/ci.yml` `env:` block).
#
# Ver `crates/e2e/tests/e2e_memory_real_embeddings.rs` e
# `docs/architecture/testing-strategy.md` §3 "Fronteira do
# que os E2E cobrem".
Invoke-Step "E2E memory real (classificador LLM + embedding, caminho do produto)" {
    cargo test -p frederico-e2e --test e2e_memory_real_embeddings -- --include-ignored
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

Write-Host ""
Write-Host "E2E document-worker handlers + docs.generate (docx + xlsx + pdf) + docs.inspect + memory real (LLM + embedding) - TODOS OS TESTES PASSARAM" -ForegroundColor Green
