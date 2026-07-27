#!/usr/bin/env pwsh
# check-core-purity.ps1
#
# Verifica as invariantes de pureza do núcleo (ADR-0003 + ADR-0007):
#
# 1) Nenhum crate em `crates/` (exceto `frederico-security`) importa
#    `tauri`, `tauri-runtime` ou usa `windows`/`winapi`/`winrt` no
#    código. O `frederico-security` é a exceção controlada (DPAPI
#    via `windows-rs`, gateado por `#[cfg(windows)]`).
# 2) `crates/provider-engine/` não lê env vars, não importa `dotenv`
#    nem parseia config de provedor de arquivo. Credenciais vêm
#    apenas do trait `CredentialStore` do `frederico-security`
#    (ADR-0007).
#
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

# Crates onde `tauri`/`windows`/etc. são permitidos. Cada entrada
# é o NOME DA PASTA (igual ao `Cargo.toml` `name` exceto que
# `frederico-` é stripado, ficando só `security`).
$allowedPlatformCrates = @('security')

$violations = New-Object System.Collections.Generic.List[string]

# ---- Regra 1: deps proibidas em Cargo.toml --------------------------------
$forbiddenDeps = @('\btauri\b', '\btauri-runtime\b', '\btauri_runtime\b', '\btauri_runtime_wry\b', '\bwinapi\b', '\bwinrt\b')
# `windows` é proibido em qualquer crate, exceto frederico-security.
$forbiddenWindowsDeps = @('\bwindows\b', '\bwindows-rs\b', '\bwindows-sys\b')

$tomlFiles = Get-ChildItem -Path $cratesDir -Recurse -Filter "Cargo.toml" -File

foreach ($toml in $tomlFiles) {
    $content = Get-Content -LiteralPath $toml.FullName -Raw
    $crateName = $null
    if ($content -match '(?m)^\s*name\s*=\s*"([^"]+)"') {
        $crateName = $Matches[1]
    }
    foreach ($pattern in $forbiddenDeps) {
        if ($content -match $pattern) {
            $violations.Add("$($toml.FullName) contém dep proibida: $pattern")
        }
    }
    foreach ($pattern in $forbiddenWindowsDeps) {
        if ($content -match $pattern) {
            # $crateName é o nome do crate (ex.: `frederico-security`),
            # $allowedPlatformCrates contém o nome da pasta (ex.: `security`).
            # Comparamos o nome da pasta.
            $crateFolder = $crateName -replace '^frederico-', ''
            if ($crateFolder -notin $allowedPlatformCrates) {
                $violations.Add("$($toml.FullName) contém dep 'windows' proibida (permitida só em pastas $allowedPlatformCrates): $pattern")
            }
        }
    }
}

# ---- Regra 1 (parte 2): imports proibidos em src/*.rs --------------------
$rsFiles = Get-ChildItem -Path $cratesDir -Recurse -Filter "*.rs" -File

foreach ($rs in $rsFiles) {
    $content = Get-Content -LiteralPath $rs.FullName -Raw
    # Caminho relativo para a mensagem.
    $rel = $rs.FullName.Substring($repoRoot.Path.Length + 1)
    # Detecta a qual crate o arquivo pertence, para liberar o
    # `frederico-security/src/windows.rs` e os integration tests em
    # `frederico-security/tests/` (que falam direto com a Win32 para
    # validar DPAPI). Esses são os únicos pontos onde o crate
    # `windows` pode aparecer — em qualquer outro lugar do núcleo
    # é violação.
    $inAllowed = $false
    foreach ($c in $allowedPlatformCrates) {
        # `-like` no PowerShell usa `*` como wildcard de path; `*`
        # casa com `\` (separador) também, então
        # `crates\security\src\windows*` casa tanto
        # `crates\security\src\windows.rs` quanto arquivos dentro
        # de `crates\security\src\windows/`.
        if ($rel -like "crates\$c\src\windows*") {
            $inAllowed = $true
            break
        }
        if ($rel -like "crates\$c\tests*") {
            $inAllowed = $true
            break
        }
    }

    if ($content -match '(?m)^\s*use\s+(tauri|tauri_runtime|tauri_runtime_wry|windows|winapi|winrt)') {
        if (-not $inAllowed) {
            $violations.Add("$rel usa crate proibido no núcleo")
        }
    }
    if ($content -match '(?m)^\s*extern\s+crate\s+(tauri|tauri_runtime|tauri_runtime_wry|windows|winapi|winrt)') {
        if (-not $inAllowed) {
            $violations.Add("$rel declara crate proibido no núcleo")
        }
    }
}

# ---- Regra 2: provider-engine não lê env, dotenv ou config em arquivo ----
$providerEngineDir = Join-Path $cratesDir "provider-engine"
if (Test-Path $providerEngineDir) {
    $peFiles = Get-ChildItem -Path $providerEngineDir -Recurse -Filter "*.rs" -File
    foreach ($rs in $peFiles) {
        $content = Get-Content -LiteralPath $rs.FullName -Raw
        $rel = $rs.FullName.Substring($repoRoot.Path.Length + 1)
        # Rejeita qualquer leitura de env var.
        if ($content -match '(?m)\bstd::env::var\b') {
            $violations.Add("$rel lê env var (proibido em provider-engine; use CredentialStore)")
        }
        if ($content -match '(?m)\benv::var_os\b') {
            $violations.Add("$rel lê env::var_os (proibido em provider-engine; use CredentialStore)")
        }
        # Rejeita dotenv / dotenvy.
        if ($content -match '(?m)\bextern\s+crate\s+dotenv') {
            $violations.Add("$rel importa dotenv (proibido em provider-engine)")
        }
        if ($content -match '(?m)\bextern\s+crate\s+dotenvy') {
            $violations.Add("$rel importa dotenvy (proibido em provider-engine)")
        }
        if ($content -match '(?m)\buse\s+dotenv[;:]') {
            $violations.Add("$rel usa dotenv (proibido em provider-engine)")
        }
        if ($content -match '(?m)\buse\s+dotenvy[;:]') {
            $violations.Add("$rel usa dotenvy (proibido em provider-engine)")
        }
    }
}

# ---- Resultado ------------------------------------------------------------
if ($violations.Count -gt 0) {
    Write-Host "[check-core-purity] $($violations.Count) violação(ões) encontrada(s):" -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "[check-core-purity] OK - pureza do núcleo preservada." -ForegroundColor Green
exit 0
