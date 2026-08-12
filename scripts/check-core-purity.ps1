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
# `process-architecture` é a Etapa 2B da Fase 5 — o `WindowsPipe`
# (gateado em `#[cfg(windows)]`) usa o crate `windows` para named
# pipes. Ver ADR-0007 §Implementação Windows (mesma filosofia do
# `frederico-security`).
$allowedPlatformCrates = @('security', 'process-architecture')

# Crates de teste (não-núcleo). O check-core-purity é sobre
# **produção** (crates que vão pro binário distribuído);
# crates com `publish = false` e só `[dev-dependencies]` não
# entram no binário. `e2e` é o exemplo canônico (Fase de
# Ligação, Etapa 5) — crate dedicado pros E2E que consomem
# `frederico-app` sem ser núcleo. **Não** adicionar crates
# "úteis em produção" aqui — o gate perde sentido.
$testOnlyCrates = @('e2e')

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
    $crateFolder = if ($crateName) { $crateName -replace '^frederico-', '' } else { '' }
    # Crates só de teste (publish = false, só [dev-dependencies])
    # ficam fora do check de produção.
    $isTestOnly = $crateFolder -in $testOnlyCrates
    if ($isTestOnly) { continue }
    foreach ($pattern in $forbiddenDeps) {
        # Casa só linhas de dep (`tauri = { ... }`, `tauri-runtime = ...`),
        # não palavras em comentários/descrições (ex.: "casca Tauri" no
        # description do `frederico-e2e`).
        if ($content -match "(?m)^\s*tauri[\w-]*\s*=") {
            $violations.Add("$($toml.FullName) contém dep proibida: $pattern")
        }
    }
    foreach ($pattern in $forbiddenWindowsDeps) {
        if ($content -match "(?m)^\s*$($pattern -replace '\\b', '')\s*=") {
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

    # Detecta arquivos que são parte da ponte Win32 via marker
    # `#![allow(unsafe_code)]` no topo (o `Cargo.toml` do crate tem
    # `unsafe_code = "deny"`, então `#![allow(unsafe_code)]` é
    # explícito e intencional). Esses arquivos são a extensão
    # do módulo `windows/` (que já é allowlisted por path) —
    # tratar como ponte Win32 também. Usado por `jail.rs` (o
    # orquestrador) e `raw_child.rs` (wrapper sobre handles
    # Win32, Etapa 5+ da Fase 7).
    $hasUnsafeAllow = $content -match '(?m)^\s*#!\[allow\(unsafe_code\)\]'
    if ($hasUnsafeAllow) {
        $inAllowed = $true
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
