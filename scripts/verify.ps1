#!/usr/bin/env pwsh
# verify.ps1
#
# Verificação completa da Fase 1 (e ponto de partida para as próximas).
# Roda em sequência:
#   1. `cargo fmt --check`           — formatação.
#   2. `cargo clippy --workspace`    — lints.
#   3. `cargo test --workspace`      — suíte unit + integration.
#   4. `npm ci && npm run build`     — frontend.
#   5. `scripts/check-core-purity.ps1` — guarda do ADR-0003.
#
# Falha no primeiro erro. Cada bloco loga tempo e exit code.

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path "$PSScriptRoot/.."
Set-Location $repoRoot

# Garante que cargo e rustc estão no PATH.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

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

# Step 1
Invoke-Step "cargo fmt --check" {
    cargo fmt --all -- --check
}

# Step 2
Invoke-Step "cargo clippy" {
    cargo clippy --workspace --all-targets -- -D warnings
}

# Step 3
Invoke-Step "cargo test" {
    cargo test --workspace
}

# Step 4 — frontend
Invoke-Step "frontend build" {
    Push-Location "apps/desktop"
    try {
        if (Test-Path "package-lock.json") {
            npm ci
        } else {
            npm install
        }
        npm run build
    } finally {
        Pop-Location
    }
}

# Step 5
Invoke-Step "check-core-purity" {
    & "$PSScriptRoot/check-core-purity.ps1"
}

Write-Host ""
Write-Host "TUDO VERDE — Fase 1 pronta para revisão." -ForegroundColor Green
