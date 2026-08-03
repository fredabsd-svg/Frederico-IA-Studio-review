#!/usr/bin/env pwsh
# check-fase-5-untouched.ps1
#
# Guarda: a Etapa 2.B da Fase de Ligacao (este PR) integra os
# kits de documento ao ToolRegistry, SEM mexer no
# `document-worker` Python da Fase 5. Os 3 arquivos sensiveis
# do worker NAO podem ter diff contra `main` neste PR -
# caso contrario, esta atravessando o muro da Fase 5 (que tem
# seus proprios PRs e revisao).
#
# Por que isso e uma guarda automatizada (nao so um lembrete):
# as 2 verificacoes manuais com `git diff --stat` que faziamos
# antes (e que pularam em sessoes anteriores) SO rodam
# quando alguem lembra. Em script, rodam em todo CI e em
# todo pre-commit manual - e o tipo de coisa que ninguem
# desfaz depois.
#
# Arquivos guardados:
#   1) workers/document-worker/tests/test_pdf_audit.py
#      (Etapa 5 PR 3 fechou auditoria estrutural - nao mexe)
#   2) workers/document-worker/document-worker.py
#      (v0.4.0 do worker, 8 handlers - nao mexe)
#   3) workers/document-worker/tools/generate_srgb_icc.py
#      (gerador de ICC sRGB v2 deterministico - nao mexe)
#
# Falha com exit 1 se algum dos 3 arquivos mudou em relacao
# ao main local.
#
# Execucao:
#   pwsh scripts/check-fase-5-untouched.ps1

$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path "$PSScriptRoot/.."
Set-Location $repoRoot

# Arquivos criticos da Fase 5 que NAO podem mudar neste PR.
# Adicionar mais arquivos aqui quando a Fase 5 fechar mais
# superficies (v4 do ICC, sumario 2 passadas, etc.).
$guardedPaths = @(
    "workers/document-worker/tests/test_pdf_audit.py",
    "workers/document-worker/document-worker.py",
    "workers/document-worker/tools/generate_srgb_icc.py"
)

# Confirma que estamos num repo git e que `origin/main` existe.
if (-not (Test-Path ".git")) {
    Write-Host "[check-fase-5-untouched] .git nao encontrado em $repoRoot" -ForegroundColor Red
    exit 1
}
# Tenta atualizar o ref local do main (best-effort). Em CI, o
# `actions/checkout@v4` ja deixa `origin/main` fresco; em dev
# local, `git fetch` deixa o ref fresco. Se o fetch falhar
# (sem rede), segue com o ref local de `origin/main` que ja
# existe desde o `git clone`.
try {
    git fetch origin main 2>$null | Out-Null
} catch {
    Write-Host "[check-fase-5-untouched] aviso: git fetch origin main falhou - usando ref local de origin/main (pode estar stale)" -ForegroundColor Yellow
}
$originMain = git rev-parse --verify origin/main 2>$null
if (-not $originMain) {
    Write-Host "[check-fase-5-untouched] origin/main nao encontrado (esperado no Frederico)" -ForegroundColor Red
    exit 1
}
Write-Host "[check-fase-5-untouched] comparando contra origin/main = $originMain"

$violations = New-Object System.Collections.Generic.List[string]

foreach ($path in $guardedPaths) {
    # `git diff --exit-code --quiet` retorna exit 1 se HOUVER
    # diff, exit 0 se nao houve. Combinado com a flag --,
    # limita a comparacao aos paths guardados. O `--stat`
    # extra e so pra mensagem de erro.
    $diffStat = git diff --stat origin/main..HEAD -- $path 2>$null
    $diffEmpty = $true
    if ($null -ne $diffStat -and $diffStat.Trim().Length -gt 0) {
        $diffEmpty = $false
    }
    if (-not $diffEmpty) {
        $violations.Add("$path mudou em relacao ao origin/main:`n$diffStat")
    }
}

# ---- Resultado ------------------------------------------------------------
if ($violations.Count -gt 0) {
    Write-Host "[check-fase-5-untouched] $($violations.Count) violacao(oes) encontrada(s):" -ForegroundColor Red
    Write-Host ""
    Write-Host "A Etapa 2.B da Fase de Ligacao integra os kits do" -ForegroundColor Red
    Write-Host "document-worker ao ToolRegistry SEM mexer no" -ForegroundColor Red
    Write-Host "worker Python da Fase 5. Os arquivos abaixo" -ForegroundColor Red
    Write-Host "mudaram contra main - provavel atravessamento de" -ForegroundColor Red
    Write-Host "fronteira. Se a mudanca for intencional, abra um" -ForegroundColor Red
    Write-Host "PR separado na Fase 5; caso contrario, reverter." -ForegroundColor Red
    Write-Host ""
    $violations | ForEach-Object {
        Write-Host "  - $_" -ForegroundColor Red
        Write-Host ""
    }
    exit 1
}

Write-Host "[check-fase-5-untouched] OK - os 3 arquivos criticos da Fase 5 (test_pdf_audit.py, document-worker.py, generate_srgb_icc.py) estao intactos vs main." -ForegroundColor Green
exit 0
