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
    cargo test -p frederico-process-architecture --test external_doc_worker
    if ($LASTEXITCODE -ne 0) { throw "cargo test falhou" }
}

Write-Host ""
Write-Host "E2E document-worker handlers - TODOS OS 6 TESTES PASSARAM" -ForegroundColor Green
