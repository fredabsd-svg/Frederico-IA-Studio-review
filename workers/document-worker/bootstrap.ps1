# bootstrap.ps1 — instala Python embeddable + pip + pywin32 em runtime/
#
# O `document-worker` precisa de Python com `pywin32` pra ter
# acesso a `CreateNamedPipe` / `ConnectNamedPipe` / `ReadFile` /
# `WriteFile` (named pipes do Windows). Esta é a dependência
# central do worker — `python-docx`, `openpyxl`, `reportlab`,
# `pytesseract` vêm em cima dessa base.
#
# **Estratégia:** Python embeddable do python.org (zip
# distribuído oficialmente). É o que a ADR-0004 §"Python
# embutido" pede — não depende de instalador do sistema, é
# auto-contido, ~10 MB.
#
# **Idempotente:** se `runtime/` já existe com `python.exe`,
# pula tudo (assume que a instalação anterior tá OK).
# Pra reinstalar do zero: apague `runtime/` e rode de novo.
#
# **Por que 3.12 e não 3.13/3.14:** 3.12 é o último LTS
# estável com `pywin32` testado em todas as plataformas
# Windows que o Frederico suporta (10 1903+). 3.13/3.14 são
# muito novos e o suporte a `pywin32` ainda está em validação
# (issue #xxx upstream). Quando estabilizar, bump aqui.

$ErrorActionPreference = 'Stop'

# Caminhos (resolvidos relativos a este script).
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RuntimeDir = Join-Path $ScriptDir 'runtime'
$PythonExe = Join-Path $RuntimeDir 'python.exe'

# Versão do Python embeddable. Centralizado aqui pra fácil bump.
$PythonVersion = '3.12.7'
$PythonZipUrl = "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip"
$PythonZipName = "python-$PythonVersion-embed-amd64.zip"
$GetPipUrl = 'https://bootstrap.pypa.io/get-pip.py'
$GetPipName = 'get-pip.py'

# Se já existe Python válido em runtime/, pula.
if (Test-Path $PythonExe) {
    Write-Host "[bootstrap] runtime/ já tem python.exe — pulando instalação." -ForegroundColor Yellow
    Write-Host "[bootstrap] Pra reinstalar do zero: apague runtime/ e rode de novo." -ForegroundColor Yellow
    & $PythonExe --version
    exit 0
}

# Garante que runtime/ não existe (ou está vazio).
if (Test-Path $RuntimeDir) {
    Write-Host "[bootstrap] ERRO: $RuntimeDir existe mas não tem python.exe." -ForegroundColor Red
    Write-Host "[bootstrap] Apague o diretório e rode de novo." -ForegroundColor Red
    exit 1
}

Write-Host "[bootstrap] instalando Python $PythonVersion embeddable em $RuntimeDir" -ForegroundColor Cyan

# 1. Cria runtime/.
New-Item -ItemType Directory -Path $RuntimeDir -Force | Out-Null

# 2. Baixa Python embeddable (zip ~10 MB).
$PythonZipPath = Join-Path $RuntimeDir $PythonZipName
Write-Host "[bootstrap] baixando $PythonZipUrl"
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $PythonZipUrl -OutFile $PythonZipPath -UseBasicParsing
} catch {
    Write-Host "[bootstrap] ERRO baixando Python: $_" -ForegroundColor Red
    exit 1
}

# 3. Extrai o zip em runtime/.
Write-Host "[bootstrap] extraindo Python embeddable"
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory($PythonZipPath, $RuntimeDir)
Remove-Item $PythonZipPath -Force

# Verifica que python.exe apareceu.
if (-not (Test-Path $PythonExe)) {
    Write-Host "[bootstrap] ERRO: python.exe não apareceu em $RuntimeDir após extração" -ForegroundColor Red
    exit 1
}
Write-Host "[bootstrap] Python extraído:" -ForegroundColor Green
& $PythonExe --version

# 4. Baixa get-pip.py. O Python embeddable vem SEM pip —
#    precisa instalar manualmente.
$GetPipPath = Join-Path $RuntimeDir $GetPipName
Write-Host "[bootstrap] baixando $GetPipUrl"
try {
    Invoke-WebRequest -Uri $GetPipUrl -OutFile $GetPipPath -UseBasicParsing
} catch {
    Write-Host "[bootstrap] ERRO baixando get-pip.py: $_" -ForegroundColor Red
    exit 1
}

# 5. Roda get-pip.py pra instalar pip no runtime/. O
#    embeddable tem `python._pth` que **bloqueia** o
#    site-packages por default — editamos pra liberar.
$PthFile = Join-Path $RuntimeDir 'python312._pth'
if (Test-Path $PthFile) {
    Write-Host "[bootstrap] editando $PthFile (uncomment #import site)"
    $content = Get-Content $PthFile -Raw
    # Uncommenta `#import site` (libera site-packages).
    $content = $content -replace '(?m)^#import site$', 'import site'
    Set-Content -Path $PthFile -Value $content -NoNewline
}

Write-Host "[bootstrap] instalando pip"
& $PythonExe $GetPipPath --no-warn-script-location
Remove-Item $GetPipPath -Force

# 6. Instala pywin32. É a única dep obrigatória do
#    `document-worker` (named pipes do Windows via Python).
#    Outras (python-docx, openpyxl, pytesseract) entram
#    quando os handlers reais forem implementados.
Write-Host "[bootstrap] instalando pywin32"
& $PythonExe -m pip install pywin32 --no-warn-script-location

# 7. Sanity check: importa win32pipe e valida versão.
Write-Host "[bootstrap] verificando instalação"
$check = & $PythonExe -c "import win32pipe, win32file, pywintypes; print('pywin32 OK')"
if ($LASTEXITCODE -ne 0) {
    Write-Host "[bootstrap] ERRO: pywin32 não importou" -ForegroundColor Red
    exit 1
}
Write-Host "[bootstrap] $check" -ForegroundColor Green

Write-Host ""
Write-Host "[bootstrap] OK — Python $PythonVersion + pywin32 instalados em $RuntimeDir" -ForegroundColor Green
Write-Host "[bootstrap] Pra rodar o worker:" -ForegroundColor Green
Write-Host "    & '$PythonExe' '$ScriptDir\document-worker.py'"
