#!/usr/bin/env pwsh
# check-e2e-gate.ps1
#
# Guarda: REGRAS-DO-PROJETO §3 (gate de E2E por fase,
# ADR-0026). Cada fase `concluida` em `docs/status.md`
# precisa ter a coluna "E2E de cobertura" (formato
# `path::fn_name`, varios separados por virgula) e a
# coluna "Passo CI" (mesma quantidade de itens, onde o
# teste roda) preenchidas. O gate confere que:
#
#   1. O teste nomeado existe (arquivo presente no repo +
#      funcao com `fn`/`async fn` casa o nome).
#   2. O Passo CI existe em `.github/workflows/ci.yml` ou
#      `ci-nightly.yml` (match por `name:` do step YAML) ou
#      em `scripts/<file>.ps1` (referencia `verify-external.ps1#N`)
#      ou e literalmente `cargo test --workspace` (que
#      roda no step "Tests" do ci.yml).
#   3. Teste `#[ignore]` (so noturno ou so verify-external)
#      exige twin deterministico no PR gate - a regra D2 do
#      ADR-0026: "so noturno" e cobertura mais fraca por
#      natureza; regressao pode ir pro main sem deteccao
#      no CI de PR. O twin e qualquer outro teste NAO-
#      `#[ignore]` da mesma fase.
#   4. Fase com `E2E de cobertura = -` exige Pendencia
#      declarando "regra nao-aplicavel" (frase literal,
#      case-insensitive).
#
# Por que script e nao revisao manual: mesma logica do
# `WorkerToolDispatcher::allowed_paths` (PR #25) e do
# `check-fase-5-untouched.ps1` - o mecanismo que nunca
# roda no caminho real parece funcionar ate o dia que
# precisa. Em script, o gate roda em todo CI de PR e
# quebra o build se o mapa de cobertura ficar
# inconsistente com o codigo.
#
# Sem valvula de escape: o gate falha se a tabela
# estiver inconsistente. O mesmo principio do `no skip`
# do path safety (PR #25) e do `no interruptor` da
# auditoria estrutural do PDF (PROMPT MESTRE §19.6).
#
# Execucao:
#   pwsh scripts/check-e2e-gate.ps1
#
# Saida:
#   - exit 0: todas as fases `concluidas` estao consistentes
#   - exit 1: pelo menos uma violacao (lista em PT-BR no stdout)

# UTF-8 obrigatorio: status.md tem acentos (REGRAS §1.11).
$PSDefaultParameterValues['Out-File:Encoding'] = 'utf8'
$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path "$PSScriptRoot/.."
Set-Location $repoRoot

$statusPath = Join-Path $repoRoot 'docs/status.md'
if (-not (Test-Path -Path $statusPath -PathType Leaf)) {
    Write-Host "[check-e2e-gate] docs/status.md nao encontrado em $repoRoot/docs" -ForegroundColor Red
    exit 1
}

# ---- Parsing do cabecalho da tabela --------------------------------------

$lines = Get-Content -LiteralPath $statusPath -Encoding UTF8
$tableLines = @($lines | Where-Object { $_.TrimStart().StartsWith('|') })

# Colunas obrigatorias: Fase, Estado, E2E de cobertura,
# Passo CI, Pendencias. O cabecalho e a primeira linha da
# tabela que contem TODAS essas 5.
$requiredHeaders = @('Fase', 'Estado', 'E2E de cobertura', 'Passo CI', 'Pendências')
$headerCells = $null
$headerIdx = -1
for ($i = 0; $i -lt $tableLines.Count; $i++) {
    $cells = @($tableLines[$i] -split '\|' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
    $hitCount = 0
    foreach ($h in $requiredHeaders) { if ($cells -contains $h) { $hitCount++ } }
    if ($hitCount -eq $requiredHeaders.Count) {
        $headerCells = $cells
        $headerIdx = $i
        break
    }
}
if ($null -eq $headerCells) {
    Write-Host "[check-e2e-gate] cabecalho da tabela em docs/status.md nao encontrado." -ForegroundColor Red
    Write-Host "  Esperado: Fase, Nome, Estado, Evidencia, Pendencias, E2E de cobertura, Passo CI." -ForegroundColor Red
    Write-Host "  A coluna 'E2E de cobertura' e o Passo CI sao obrigatorios desde o PR #28 (Etapa 6 da Fase de Ligacao)." -ForegroundColor Red
    exit 1
}

$idxFase      = [Array]::IndexOf($headerCells, 'Fase')
$idxEstado    = [Array]::IndexOf($headerCells, 'Estado')
$idxCobertura = [Array]::IndexOf($headerCells, 'E2E de cobertura')
$idxPasso     = [Array]::IndexOf($headerCells, 'Passo CI')
$idxPendencia = [Array]::IndexOf($headerCells, 'Pendências')

if ($idxFase -lt 0 -or $idxEstado -lt 0 -or $idxCobertura -lt 0 -or $idxPasso -lt 0 -or $idxPendencia -lt 0) {
    Write-Host "[check-e2e-gate] colunas obrigatorias ausentes no cabecalho de docs/status.md" -ForegroundColor Red
    exit 1
}

# Dados = linhas apos o cabecalho + a linha de separador (`|---|`).
$dataLines = @($tableLines | Select-Object -Skip ($headerIdx + 2))

# ---- Helpers --------------------------------------------------------------

# Testa se o arquivo `path` existe E se contem `fn nome(` ou
# `async fn nome(`. Retorna o status e a flag `#[ignore]`
# (procura nos 5 linhas anteriores a declaracao da funcao -
# cobre o caso `#[tokio::test]` seguido de `#[ignore = "..."]`
# seguido de `fn`).
function Get-TestStatus {
    param(
        [string]$Path,
        [string]$FnName
    )
    $full = Join-Path $repoRoot $Path
    if (-not (Test-Path -Path $full -PathType Leaf)) {
        return @{ Ok = $false; Reason = "arquivo nao existe: $Path"; Ignored = $false }
    }
    $content = Get-Content -LiteralPath $full -Raw -Encoding UTF8
    $fnPattern = '(?m)^\s*(?:async\s+)?fn\s+' + [Regex]::Escape($FnName) + '\s*\('
    if ($content -notmatch $fnPattern) {
        return @{ Ok = $false; Reason = "funcao '$FnName' nao encontrada em $Path"; Ignored = $false }
    }
    $ignored = $false
    $fileLines = $content -split "`n"
    for ($j = 0; $j -lt $fileLines.Count; $j++) {
        $linePattern = '^\s*(?:async\s+)?fn\s+' + [Regex]::Escape($FnName) + '\s*\('
        if ($fileLines[$j] -notmatch $linePattern) { continue }
        $start = [Math]::Max(0, $j - 5)
        for ($k = $start; $k -lt $j; $k++) {
            if ($fileLines[$k] -match '#\s*\[\s*ignore(\s*=.*)?\s*\]') { $ignored = $true; break }
        }
        break
    }
    return @{ Ok = $true; Reason = $null; Ignored = $ignored }
}

# Confere se o `passo` e uma das 3 formas validas:
#   1. `cargo test --workspace` (literal, implicito no step "Tests" do ci.yml)
#   2. `name:` de um step em `.github/workflows/ci.yml` ou `ci-nightly.yml`
#   3. `<script>.ps1#<N>` ou `<script>.ps1` em `scripts/` (verify-external.ps1#7)
function Test-PassoCi {
    param([string]$Passo)
    $p = $Passo.Trim()
    if ($p -eq 'cargo test --workspace') { return $true }
    foreach ($yml in @('ci.yml', 'ci-nightly.yml')) {
        $full = Join-Path $repoRoot ".github/workflows/$yml"
        if (-not (Test-Path -Path $full -PathType Leaf)) { continue }
        $c = Get-Content -LiteralPath $full -Raw -Encoding UTF8
        if ($c -match ('(?m)^\s*-?\s*name:\s+' + [Regex]::Escape($p) + '\s*$')) { return $true }
    }
    if ($p -match '^(.+\.ps1)(?:::#?\d+)?$') {
        $script = Join-Path $repoRoot 'scripts/' + $Matches[1]
        if (Test-Path -Path $script -PathType Leaf) { return $true }
    }
    return $false
}

# ---- Loop principal -------------------------------------------------------

$violations = New-Object System.Collections.Generic.List[string]
$concluidasCount = 0

foreach ($row in $dataLines) {
    $rawCells = @($row -split '\|' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
    if ($rawCells.Count -lt $headerCells.Count) { continue } # linha malformada ou continuacao

    # A Markdown table pode ter `|` literal dentro de celulas (ex.: codigo
    # tipo `O_CREAT|O_EXCL` no Evidencia). Quando o split gera MAIS celulas
    # que o schema (7 com as 2 colunas novas, 5 sem), parseia pela DIREITA:
    # os 2 ultimos sao sempre E2E + Passo (sem `|` interno), o anterior e
    # a Pendencia, e tudo entre cells[3] e cells[count-4] vira parte da
    # Evidencia (juntado com `|`). Os 3 primeiros (Fase/Nome/Estado) sao
    # fixos.
    $count = $rawCells.Count
    if ($count -eq $headerCells.Count) {
        $fase      = $rawCells[$idxFase]
        $estado    = $rawCells[$idxEstado]
        $evidencia = $null  # nao usado no gate
        $pendencia = $rawCells[$idxPendencia]
        $cobertura = $rawCells[$idxCobertura]
        $passo     = $rawCells[$idxPasso]
    } elseif ($count -gt $headerCells.Count) {
        $fase      = $rawCells[0]
        $estado    = $rawCells[2]
        $evParts   = $rawCells[3..($count - 4)]
        $evidencia = $evParts -join ' | '
        $pendencia = $rawCells[$count - 3]
        $cobertura = $rawCells[$count - 2]
        $passo     = $rawCells[$count - 1]
    } else {
        # Menos celulas que o schema - pulamos (linha de cabecalho ou
        # continuacao). O `if ($rawCells.Count -lt $headerCells.Count)`
        # acima ja trata isso.
        continue
    }

    if ($estado -ne 'concluída') { continue }
    $concluidasCount++

    # Caso 1: E2E de cobertura = '-' (regra nao-aplicavel).
    if ($cobertura -eq '—' -or $cobertura -eq '-') {
        if ($pendencia -notmatch '(?i)regra\s+n[ãa]o[\s-]aplic[áa]vel') {
            $violations.Add("Fase ${fase}: 'E2E de cobertura' e '-' mas Pendencia nao declara 'regra nao-aplicavel' (texto atual: '$pendencia')")
        }
        continue
    }

    # Caso 2: E2E de cobertura tem 1+ `path::fn_name`.
    $tests = @($cobertura -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
    $passos = @($passo -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })

    if ($tests.Count -ne $passos.Count) {
        $violations.Add("Fase ${fase}: 'E2E de cobertura' tem $($tests.Count) item(ns) e 'Passo CI' tem $($passos.Count) - devem ter a mesma quantidade")
    }

    $hasNonIgnoredInPhase = $false
    $testInfos = New-Object System.Collections.Generic.List[object]

    foreach ($test in $tests) {
        if ($test -notmatch '^(.+)::(.+)$') {
            $violations.Add("Fase ${fase}: item '$test' nao esta no formato 'path::fn_name'")
            continue
        }
        $path = $Matches[1]
        $fn = $Matches[2]
        $s = Get-TestStatus -Path $path -FnName $fn
        if (-not $s.Ok) {
            $violations.Add("Fase ${fase}: $($s.Reason)")
            continue
        }
        $testInfos.Add([pscustomobject]@{ Path = $path; Fn = $fn; Ignored = $s.Ignored })
        if (-not $s.Ignored) { $hasNonIgnoredInPhase = $true }
    }

    $passCount = [Math]::Min($passos.Count, $tests.Count)
    for ($p = 0; $p -lt $passCount; $p++) {
        if (-not (Test-PassoCi -Passo $passos[$p])) {
            $violations.Add("Fase ${fase}: Passo CI '$($passos[$p])' nao encontrado em ci.yml/ci-nightly.yml nem em scripts/<arquivo>.ps1 nem e 'cargo test --workspace'")
        }
    }

    # Regra D2 do ADR-0026: teste `#[ignore]` exige twin
    # deterministico na mesma fase. Se TODOS os testes da
    # fase sao `#[ignore]`, falha.
    if ($testInfos.Count -gt 0 -and -not $hasNonIgnoredInPhase) {
        $names = ($testInfos | ForEach-Object { "$($_.Path)::$($_.Fn)" }) -join ', '
        $violations.Add("Fase ${fase}: teste(s) '$names' e(sao) #[ignore] (so noturno / so verify-external) e a fase nao tem twin deterministico - regressao pode ir pro main sem deteccao no CI de PR (ADR-0026 D2)")
    }
}

# ---- Resultado ------------------------------------------------------------

if ($violations.Count -gt 0) {
    Write-Host "[check-e2e-gate] $($violations.Count) violacao(oes) em $($concluidasCount) fase(s) concluida(s):" -ForegroundColor Red
    Write-Host ""
    Write-Host "O gate de E2E por fase (REGRAS-DO-PROJETO §3, ADR-0026)" -ForegroundColor Red
    Write-Host "exige que toda fase `concluida` em docs/status.md tenha" -ForegroundColor Red
    Write-Host "as colunas 'E2E de cobertura' (formato path::fn_name)" -ForegroundColor Red
    Write-Host "e 'Passo CI' (mesma quantidade de itens, onde o teste" -ForegroundColor Red
    Write-Host "roda) preenchidas e consistentes com o codigo." -ForegroundColor Red
    Write-Host ""
    $violations | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}

Write-Host "[check-e2e-gate] OK - $($concluidasCount) fase(s) concluida(s) com cobertura E2E e Passo CI consistentes." -ForegroundColor Green
exit 0
