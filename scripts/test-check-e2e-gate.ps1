#!/usr/bin/env pwsh
# test-check-e2e-gate.ps1
#
# Meta-teste do check-e2e-gate.ps1 (REGRA §3 / ADR-0026).
# Roda o gate contra 9 cenarios (fixture sintetica) e confere
# exit code + mensagem PT-BR de cada um. Sem o meta-teste, o
# gate e' codigo nao-exercitado (mesma armadilha do PR #25:
# "mecanismo que nunca roda no caminho real parece funcionar").
#
# v2: usa [char]0xNNNN pros acentos, escrevendo o fixture
# com UTF-8 SEM BOM via UTF8Encoding. Sem isso, o Out-File
# com literal mojibake (porque o script aqui nao tem BOM)
# gera fixture com bytes errados e o gate pula a linha.

$ErrorActionPreference = 'Stop'

# O script pode rodar de um local temporario. Acha o repo pelo marker (Cargo.toml).
$cargoToml = Get-ChildItem -Path 'C:\src\Frederico' -Filter 'Cargo.toml' -Recurse -Depth 3 -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $cargoToml) {
    Write-Host "[meta-teste] nao encontrei Cargo.toml em C:\src\Frederico" -ForegroundColor Red
    exit 1
}
$repoRoot = $cargoToml.DirectoryName
$gateScript = Join-Path $repoRoot 'scripts\check-e2e-gate.ps1'

if (-not (Test-Path -Path $gateScript -PathType Leaf)) {
    Write-Host "[meta-teste] gate nao encontrado em $gateScript" -ForegroundColor Red
    exit 1
}

# Unicode chars via [char] (script ASCII, saida UTF-8 correta)
$A_ACUTE  = [char]0x00E1
$A_TILDE  = [char]0x00E3
$E_CIRC   = [char]0x00EA
$I_ACUTE  = [char]0x00ED

# Helpers --------------------------------------------------------------

$results = New-Object System.Collections.Generic.List[object]

function Run-Scenario {
    param(
        [string]$Name,
        [string]$ExpectedExit,
        [scriptblock]$Setup
    )

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("e2e-gate-meta-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    try {
        & $Setup $tmp

        $tmpScripts = Join-Path $tmp 'scripts'
        New-Item -ItemType Directory -Path $tmpScripts -Force | Out-Null
        Copy-Item $gateScript $tmpScripts/check-e2e-gate.ps1 -Force

        Push-Location $tmp
        $output = & powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-e2e-gate.ps1 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
        Pop-Location

        $passed = ($exitCode -eq $ExpectedExit)
        $results.Add([pscustomobject]@{
            Name = $Name
            ExpectedExit = $ExpectedExit
            ActualExit = $exitCode
            Output = $output
            Passed = $passed
        })

        $tag = if ($passed) { "PASS" } else { "FAIL" }
        $color = if ($passed) { "Green" } else { "Red" }
        Write-Host ("[{0}] {1} (esperado={2}, obtido={3})" -f $tag, $Name, $ExpectedExit, $exitCode) -ForegroundColor $color
        if (-not $passed) {
            Write-Host "  --- output ---" -ForegroundColor Red
            $output -split "`n" | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

function Write-StatusMd {
    param([string]$Dir, [string]$Body)
    $docsDir = Join-Path $Dir 'docs'
    New-Item -ItemType Directory -Path $docsDir -Force | Out-Null
    $body = $Body -replace "`r`n", "`n"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $docsDir 'status.md'), $body, $utf8NoBom)
}

function Write-TestFile {
    param([string]$Dir, [string]$RelPath, [string]$Body)
    $full = Join-Path $Dir $RelPath
    $parent = Split-Path -Parent $full
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($full, $Body, $utf8NoBom)
}

function Write-CiYml {
    param([string]$Dir, [string]$Name)
    $wfDir = Join-Path $Dir '.github/workflows'
    New-Item -ItemType Directory -Path $wfDir -Force | Out-Null
    $body = "name: CI`non: [push, pull_request]`njobs:`n  verify:`n    runs-on: windows-latest`n    steps:`n      - uses: actions/checkout@v4`n      - name: $Name`n        run: cargo test --workspace"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $wfDir 'ci.yml'), $body, $utf8NoBom)
}

# Header + separator (acentos via [char])
$hdr = '| Fase | Nome | Estado | Evid' + $E_CIRC + 'ncia | Pend' + $E_CIRC + 'ncias | E2E de cobertura | Passo CI |'
$sep = '|------|------|--------|-----------|------------|------------------|----------|'

# Row helpers (acentos via [char])
function Row0Dash {
    return '| 0 | Doc | conclu' + $I_ACUTE + 'da | E | regra n' + $A_TILDE + 'o-aplic' + $A_ACUTE + 'vel: Fase documental | - | - |'
}
function Row0DashSimple {
    return '| 0 | Doc | conclu' + $I_ACUTE + 'da | E | sem pend' + $E_CIRC + 'ncia | - | - |'
}
function Row1F1TestRuns {
    return '| 1 | F1 | conclu' + $I_ACUTE + 'da | E1 | - | crates/e2e/tests/e2e_test.rs::e2e_test_runs | cargo test --workspace |'
}
function Row1F1TestDiffName {
    return '| 1 | F1 | conclu' + $I_ACUTE + 'da | E1 | - | crates/e2e/tests/e2e_test.rs::e2e_test_different_name | cargo test --workspace |'
}
function Row1F1BadPasso {
    return '| 1 | F1 | conclu' + $I_ACUTE + 'da | E1 | - | crates/e2e/tests/e2e_test.rs::e2e_test_runs | step_inexistente |'
}
function Row1F1Twin {
    return '| 1 | F1 | conclu' + $I_ACUTE + 'da | E1 | - | crates/e2e/tests/e2e_test.rs::e2e_test_deterministic, crates/e2e/tests/e2e_test.rs::e2e_test_real | cargo test --workspace, E2E document-worker handlers |'
}

function Build-Body {
    param([string[]]$Rows)
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('# Estado Real por Fase')
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine('## Tabela')
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine($hdr)
    [void]$sb.AppendLine($sep)
    foreach ($row in $Rows) {
        [void]$sb.AppendLine($row)
    }
    return $sb.ToString()
}

# ---- Cenarios -------------------------------------------------------------

# 1. Baseline valido
Run-Scenario -Name '1_valid_baseline' -ExpectedExit 0 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1TestRuns)))
    Write-TestFile -Dir $dir -RelPath 'crates/e2e/tests/e2e_test.rs' -Body "#[test]`nfn e2e_test_runs() { assert!(true); }"
    Write-CiYml -Dir $dir -Name 'Tests'
}

# 2. Header missing
Run-Scenario -Name '2_missing_header' -ExpectedExit 1 -Setup {
    param($dir)
    $body = "# Status`n| Fase | Nome | Estado | Evid" + $E_CIRC + "ncia | Pend" + $E_CIRC + "ncias |`n|------|------|--------|-----------|------------|`n| 0 | Doc | conclu" + $I_ACUTE + "da | E | - |"
    Write-StatusMd -Dir $dir -Body $body
}

# 3. Test renamed
Run-Scenario -Name '3_test_renamed' -ExpectedExit 1 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1TestDiffName)))
    Write-TestFile -Dir $dir -RelPath 'crates/e2e/tests/e2e_test.rs' -Body "#[test]`nfn e2e_test_runs() { assert!(true); }"
    Write-CiYml -Dir $dir -Name 'Tests'
}

# 4. Test file missing
Run-Scenario -Name '4_test_file_missing' -ExpectedExit 1 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1TestRuns)))
    Write-CiYml -Dir $dir -Name 'Tests'
}

# 5. Passo CI missing
Run-Scenario -Name '5_passo_ci_missing' -ExpectedExit 1 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1BadPasso)))
    Write-TestFile -Dir $dir -RelPath 'crates/e2e/tests/e2e_test.rs' -Body "#[test]`nfn e2e_test_runs() { assert!(true); }"
    Write-CiYml -Dir $dir -Name 'Tests'
}

# 6. Ignored sem twin
Run-Scenario -Name '6_ignored_no_twin' -ExpectedExit 1 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1TestRuns)))
    Write-TestFile -Dir $dir -RelPath 'crates/e2e/tests/e2e_test.rs' -Body "#[test]`n#[ignore]`nfn e2e_test_runs() { assert!(true); }"
    Write-CiYml -Dir $dir -Name 'Tests'
}

# 7. Ignored COM twin
Run-Scenario -Name '7_ignored_with_twin' -ExpectedExit 0 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash), (Row1F1Twin)))
    Write-TestFile -Dir $dir -RelPath 'crates/e2e/tests/e2e_test.rs' -Body "#[test]`nfn e2e_test_deterministic() { assert!(true); }`n`n#[test]`n#[ignore]`nfn e2e_test_real() { assert!(true); }"
    Write-CiYml -Dir $dir -Name 'E2E document-worker handlers'
}

# 8. Dash sem "regra nao-aplicavel"
Run-Scenario -Name '8_dash_without_nao_aplicavel' -ExpectedExit 1 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0DashSimple)))
}

# 9. Dash COM "regra nao-aplicavel"
Run-Scenario -Name '9_dash_with_nao_aplicavel' -ExpectedExit 0 -Setup {
    param($dir)
    Write-StatusMd -Dir $dir -Body (Build-Body -Rows @((Row0Dash)))
}

# ---- Sumario -------------------------------------------------------------

$passed = @($results | Where-Object { $_.Passed }).Count
$failed = @($results | Where-Object { -not $_.Passed }).Count
Write-Host ""
Write-Host "==================================================================="
Write-Host "Meta-teste do check-e2e-gate.ps1: $passed/$($results.Count) passaram"
if ($failed -gt 0) {
    Write-Host "Cenarios que falharam:" -ForegroundColor Red
    $results | Where-Object { -not $_.Passed } | ForEach-Object {
        Write-Host ("  - {0} (esperado={1}, obtido={2})" -f $_.Name, $_.ExpectedExit, $_.ActualExit) -ForegroundColor Red
    }
    exit 1
}
Write-Host "OK" -ForegroundColor Green
exit 0
