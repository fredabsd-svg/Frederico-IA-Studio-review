#!/usr/bin/env pwsh
# run-contract-tests.ps1
#
# Roda os contract tests off-PR (`tests/contract/` em
# `frederico-provider-engine`). Esses tests falam com provedores
# reais — precisam de env vars com chaves de API.
#
# Como rodar:
#   pwsh scripts/run-contract-tests.ps1
#
# Variáveis reconhecidas (todas opcionais; tests sem env pulam):
#   OPENAI_API_KEY      — test do OpenAI direto
#   OPENROUTER_API_KEY  — test do OpenRouter
#   ANTHROPIC_API_KEY   — test do Anthropic
#
# Os tests pulam limpo (exit 0) quando a env var não está
# configurada. Em CI noturno, configure os secrets via
# `actions/secrets` e rode este script.

$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path "$PSScriptRoot/.."
Set-Location $repoRoot

$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
if (-not (Test-Path $cargo)) {
    Write-Host "[contract] cargo não encontrado em $cargo" -ForegroundColor Red
    exit 1
}

$envVars = @(
    'OPENAI_API_KEY',
    'OPENROUTER_API_KEY',
    'ANTHROPIC_API_KEY',
)

Write-Host "[contract] env vars configuradas:" -ForegroundColor Cyan
$anySet = $false
foreach ($v in $envVars) {
    $val = (Get-Item env:$v -ErrorAction SilentlyContinue).Value
    if ($null -ne $val -and $val -ne '') {
        $masked = if ($val.Length -gt 8) { $val.Substring(0, 4) + '…' + $val.Substring($val.Length - 4) } else { '****' }
        Write-Host "  - $v = $masked"
        $anySet = $true
    } else {
        Write-Host "  - $v = (vazio) — test vai pular" -ForegroundColor DarkGray
    }
}
if (-not $anySet) {
    Write-Host ""
    Write-Host "[contract] NENHUMA chave configurada. Os tests vão pular todos." -ForegroundColor Yellow
    Write-Host "[contract] Para rodar de verdade, defina pelo menos uma das env vars acima." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "[contract] rodando: cargo test -p frederico-provider-engine --test contract -- --nocapture" -ForegroundColor Cyan
Write-Host ""

& $cargo test -p frederico-provider-engine --test contract -- --nocapture
$code = $LASTEXITCODE

if ($code -eq 0) {
    Write-Host ""
    Write-Host "[contract] OK — suíte verde (com ou sem skips)." -ForegroundColor Green
} else {
    Write-Host ""
    Write-Host "[contract] FALHOU — exit $code. Veja os eventos acima." -ForegroundColor Red
}

exit $code
