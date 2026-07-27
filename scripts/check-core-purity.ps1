#!/usr/bin/env pwsh
# check-core-purity.ps1
#
# Verifica que NENHUM crate em `crates/` importa `tauri`, `tauri-runtime`
# ou `windows` (ver ADR-0003 — núcleo desacoplado da casca).
# Falha com exit 1 se encontrar.
#
# Execução:
#   pwsh scripts/check-core-purity.ps1
# ou via Cargo (definido como alias `check-core`).

$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path "$PSScriptRoot/.."
$cratesDir = Join-Path $repoRoot "crates"

if (-not (Test-Path $cratesDir)) {
    Write-Host "[check-core-purity] pasta 'crates' não encontrada em $repoRoot" -ForegroundColor Red
    exit 1
}

# Padrões proibidos: dependências diretas ou imports em crates/*.
$forbiddenDeps = @('\btauri\b', '\btauri-runtime\b', '\bwindows\b', '\bwinapi\b', '\bwinrt\b')

# 1) Procura em Cargo.toml dos crates.
$tomlFiles = Get-ChildItem -Path $cratesDir -Recurse -Filter "Cargo.toml" -File

$violations = New-Object System.Collections.Generic.List[string]

foreach ($toml in $tomlFiles) {
    $content = Get-Content -LiteralPath $toml.FullName -Raw
    foreach ($pattern in $forbiddenDeps) {
        if ($content -match $pattern) {
            $violations.Add("$($toml.FullName) contém padrão proibido: $pattern")
        }
    }
}

# 2) Procura em código fonte de crates/* (src/**.rs).
$rsFiles = Get-ChildItem -Path $cratesDir -Recurse -Filter "*.rs" -File

foreach ($rs in $rsFiles) {
    $content = Get-Content -LiteralPath $rs.FullName -Raw
    if ($content -match '(?m)^\s*use\s+(tauri|tauri_runtime|tauri_runtime_wry|windows|winapi|winrt)') {
        $violations.Add("$($rs.FullName) usa crate proibido no núcleo")
    }
    if ($content -match '(?m)^\s*extern\s+crate\s+(tauri|tauri_runtime|tauri_runtime_wry|windows|winapi|winrt)') {
        $violations.Add("$($rs.FullName) declara crate proibido no núcleo")
    }
}

if ($violations.Count -gt 0) {
    Write-Host "[check-core-purity] $($violations.Count) violação(ões) encontrada(s):" -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "[check-core-purity] OK - nenhum crate em crates/ depende de tauri/windows." -ForegroundColor Green
exit 0
