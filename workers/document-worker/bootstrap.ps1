# bootstrap.ps1 - instala o runtime completo do `document-worker` v0.2.0
#
# O `document-worker` precisa de:
#   - Python 3.12.7 embeddable + pip (named pipes via pywin32)
#   - pywin32 (acesso a CreateNamedPipe / ConnectNamedPipe / ReadFile / WriteFile)
#   - python-docx, openpyxl, reportlab, pdfplumber (handlers reais, Etapa 2B+X)
#   - Source Sans 3 + Source Serif 4 (TTF variable, identidade "Tinta e Latao" -
#     Adobe Fonts, conforme PROMPT MESTRE 16.3 e ADR-0004 / ADR-0018)
#
# Tesseract OCR fica FORA deste bootstrap - vai pra Etapa 2B+Y (pendencia
# separada, registrada no docs/modules/process-architecture.md).
#
# Estrategia: Python embeddable do python.org (zip distribuido oficialmente).
# E o que a ADR-0004 "Python embutido" pede - nao depende de instalador do
# sistema, e auto-contido.
#
# Idempotente: cada bloco checa a presenca do artefato final antes de
# baixar/instalar. Pra reinstalar do zero: apague runtime/ e rode de novo.
#
# Por que Python 3.12 (nao 3.13/3.14): 3.12 e o ultimo estavel com pywin32
# totalmente testado em Windows 10 1903+. Quando estabilizar, bump aqui.
#
# Por que TTF variable e nao OTF: o Source Serif 4 release notes avisa que
# Windows 10/11 tem bug com CFF2 variable OTF (corrompe texto). Mesma
# mitigacao pro Source Sans 3 (consistencia). Detalhes no ADR-0018.

# NOTA sobre encoding: este script usa SOMENTE ASCII. Em-dash, crase, "e"
# comercial e outros caracteres nao-ASCII foram removidos propositalmente.
# PowerShell 5.1 (sem UTF-8 BOM detection) le arquivos UTF-8 sem BOM como
# ANSI, o que corrompe em-dash e faz o "&" virar operador reservado. A
# documentacao detalhada usa "Tinta e Latao" com "e" no lugar de "&" - o
# nome canonico da marca continua sendo "Tinta & Latao" no README.md
# (escrito em UTF-8, sem restricao de encoding).

$ErrorActionPreference = 'Stop'

# Caminhos (resolvidos relativos a este script).
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RuntimeDir = Join-Path $ScriptDir 'runtime'
$PythonExe = Join-Path $RuntimeDir 'python.exe'

# Versao do Python embeddable. Centralizado aqui pra facil bump.
$PythonVersion = '3.12.7'
$PythonZipUrl = "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip"
$PythonZipName = "python-$PythonVersion-embed-amd64.zip"
$GetPipUrl = 'https://bootstrap.pypa.io/get-pip.py'
$GetPipName = 'get-pip.py'

# Se ja existe Python valido em runtime/, pula o bloco de Python (mas ainda
# checa libs e fontes).
$PythonAlreadyInstalled = Test-Path $PythonExe

if ($PythonAlreadyInstalled) {
    Write-Host '[bootstrap] runtime/ ja tem python.exe - pulando instalacao do Python.' -ForegroundColor Yellow
    Write-Host '[bootstrap] Pra reinstalar do zero: apague runtime/ e rode de novo.' -ForegroundColor Yellow
    & $PythonExe --version
} else {
    # Garante que runtime/ nao existe (ou esta vazio).
    if (Test-Path $RuntimeDir) {
        Write-Host "[bootstrap] ERRO: $RuntimeDir existe mas nao tem python.exe." -ForegroundColor Red
        Write-Host '[bootstrap] Apague o diretorio e rode de novo.' -ForegroundColor Red
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
    Write-Host '[bootstrap] extraindo Python embeddable'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($PythonZipPath, $RuntimeDir)
    Remove-Item $PythonZipPath -Force

    # Verifica que python.exe apareceu.
    if (-not (Test-Path $PythonExe)) {
        Write-Host "[bootstrap] ERRO: python.exe nao apareceu em $RuntimeDir apos extracao" -ForegroundColor Red
        exit 1
    }
    Write-Host '[bootstrap] Python extraido:' -ForegroundColor Green
    & $PythonExe --version

    # 4. Baixa get-pip.py. O Python embeddable vem SEM pip - precisa
    #    instalar manualmente.
    $GetPipPath = Join-Path $RuntimeDir $GetPipName
    Write-Host "[bootstrap] baixando $GetPipUrl"
    try {
        Invoke-WebRequest -Uri $GetPipUrl -OutFile $GetPipPath -UseBasicParsing
    } catch {
        Write-Host "[bootstrap] ERRO baixando get-pip.py: $_" -ForegroundColor Red
        exit 1
    }

    # 5. Roda get-pip.py pra instalar pip no runtime/. O embeddable tem
    #    python._pth que BLOQUEIA o site-packages por default - editamos
    #    pra liberar.
    $PthFile = Join-Path $RuntimeDir 'python312._pth'
    if (Test-Path $PthFile) {
        Write-Host "[bootstrap] editando $PthFile (uncomment #import site)"
        $content = Get-Content $PthFile -Raw
        # Uncommenta `#import site` (libera site-packages). Sem ancoras
        # `^...$` porque PowerShell 5.1 com `(?m)` so casa `$` antes de
        # `\n`, nao antes de `\r\n` - o pattern simples e seguro porque
        # `#import site` e unico no arquivo.
        $content = $content -replace '#import site', 'import site'
        Set-Content -Path $PthFile -Value $content -NoNewline
    }

    Write-Host '[bootstrap] instalando pip'
    & $PythonExe $GetPipPath --no-warn-script-location
    Remove-Item $GetPipPath -Force
}

# ---------------------------------------------------------------------------
# pywin32 - named pipes do Windows via Python
# ---------------------------------------------------------------------------

# Sanity check rapido: win32pipe ja importavel?
$pywin32Installed = $false
try {
    & $PythonExe -c 'import win32pipe' 2>$null
    if ($LASTEXITCODE -eq 0) { $pywin32Installed = $true }
} catch {}

if ($pywin32Installed) {
    Write-Host '[bootstrap] pywin32 ja instalado - pulando.' -ForegroundColor Yellow
} else {
    Write-Host '[bootstrap] instalando pywin32'
    & $PythonExe -m pip install pywin32 --no-warn-script-location
}

# ---------------------------------------------------------------------------
# Bibliotecas Python (Etapa 2B+X - handlers reais)
# ---------------------------------------------------------------------------
#
# Estas sao as bibliotecas que os 6 handlers (docx.write/read, xlsx.write/read,
# pdf.write/read) consomem. Versoes fixadas em pyproject.toml. Pula o bloco
# se a primeira ja estiver importavel.

$LibSentinels = @{
    'python-docx'  = 'docx'
    'openpyxl'     = 'openpyxl'
    'reportlab'    = 'reportlab'
    'pdfplumber'   = 'pdfplumber'
}

$libsOk = $true
foreach ($sentinel in $LibSentinels.Values) {
    try {
        & $PythonExe -c "import $sentinel" 2>$null
        if ($LASTEXITCODE -ne 0) { $libsOk = $false; break }
    } catch {
        $libsOk = $false
        break
    }
}

if ($libsOk) {
    Write-Host '[bootstrap] bibliotecas Python (python-docx, openpyxl, reportlab, pdfplumber) ja instaladas - pulando.' -ForegroundColor Yellow
} else {
    Write-Host '[bootstrap] instalando bibliotecas Python (python-docx, openpyxl, reportlab, pdfplumber)'
    & $PythonExe -m pip install python-docx openpyxl 'reportlab>=4.0' 'pdfplumber>=0.10' --no-warn-script-location
    if ($LASTEXITCODE -ne 0) {
        Write-Host '[bootstrap] ERRO: pip install das bibliotecas falhou' -ForegroundColor Red
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Fontes "Tinta e Latao" - Adobe Source Sans 3 + Source Serif 4 (TTF variable)
# ---------------------------------------------------------------------------
#
# Fontes SIL Open Font License 1.1, versao TTF variable (OTF variable tem
# bug de corrupcao de texto no Windows 10/11 - ver ADR-0018 Decisao 2b).
# Download direto do branch release do repo oficial Adobe. 4 arquivos:
#   - SourceSans3VF-Upright.ttf      (corpo, sem italico)
#   - SourceSans3VF-Italic.ttf       (corpo, italico)
#   - SourceSerif4Variable-Roman.ttf (titulos, sem italico)
#   - SourceSerif4Variable-Italic.ttf (titulos, italico)

$FontsDir = Join-Path $RuntimeDir 'fonts'
$FontAdobeBase = 'https://raw.githubusercontent.com/adobe-fonts'

$FontsToFetch = @(
    @{ Name = 'SourceSans3VF-Upright.ttf';        Url = "$FontAdobeBase/source-sans/release/VF/SourceSans3VF-Upright.ttf" }
    @{ Name = 'SourceSans3VF-Italic.ttf';         Url = "$FontAdobeBase/source-sans/release/VF/SourceSans3VF-Italic.ttf" }
    @{ Name = 'SourceSerif4Variable-Roman.ttf';   Url = "$FontAdobeBase/source-serif/release/VAR/SourceSerif4Variable-Roman.ttf" }
    @{ Name = 'SourceSerif4Variable-Italic.ttf';  Url = "$FontAdobeBase/source-serif/release/VAR/SourceSerif4Variable-Italic.ttf" }
)

# Se todos os 4 arquivos ja existem com tamanho > 50 KB, pula.
$allFontsPresent = $true
foreach ($f in $FontsToFetch) {
    $path = Join-Path $FontsDir $f.Name
    if (-not (Test-Path $path)) { $allFontsPresent = $false; break }
    $size = (Get-Item $path).Length
    if ($size -lt 50KB) { $allFontsPresent = $false; break }
}

if ($allFontsPresent) {
    Write-Host "[bootstrap] fontes Tinta e Latao (4 TTFs) ja presentes em $FontsDir - pulando." -ForegroundColor Yellow
} else {
    Write-Host '[bootstrap] baixando fontes Tinta e Latao (Adobe Source Sans 3 + Source Serif 4)' -ForegroundColor Cyan
    New-Item -ItemType Directory -Path $FontsDir -Force | Out-Null

    foreach ($f in $FontsToFetch) {
        $dest = Join-Path $FontsDir $f.Name
        Write-Host "[bootstrap]   $($f.Name)"
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri $f.Url -OutFile $dest -UseBasicParsing
        } catch {
            Write-Host "[bootstrap] ERRO baixando $($f.Name): $_" -ForegroundColor Red
            exit 1
        }
    }
}

# ---------------------------------------------------------------------------
# Verificacao final
# ---------------------------------------------------------------------------

Write-Host '[bootstrap] verificando instalacao' -ForegroundColor Cyan

# 1. Python + pywin32. Usa here-string pra evitar problema de aspas
#    internas no PowerShell 5.1 com `-c '...'` (aspas duplas dentro de
#    aspas simples as vezes sao corrompidas pelo shell).
$Pywin32CheckScript = @'
import win32pipe, win32file, pywintypes
print('pywin32 OK')
'@
$pywin32Check = & $PythonExe -c $Pywin32CheckScript
if ($LASTEXITCODE -ne 0) {
    Write-Host '[bootstrap] ERRO: pywin32 nao importou' -ForegroundColor Red
    Write-Host $pywin32Check
    exit 1
}
Write-Host "[bootstrap] $pywin32Check" -ForegroundColor Green

# 2. Bibliotecas (cada uma no seu proprio subprocess pra isolar falha).
# Script Python via arquivo temporario pra evitar problemas de quoting
# do PowerShell 5.1 com here-strings e `$` interpolation.
$LibCheckPath = Join-Path $env:TEMP 'frederico_lib_check.py'
@'
import sys
errors = []
for lib in ['docx', 'openpyxl', 'reportlab', 'pdfplumber']:
    try:
        __import__(lib)
    except Exception as e:
        errors.append(lib + ': ' + str(e))
if errors:
    for e in errors:
        print('FAIL', e)
    sys.exit(1)
print('libs OK')
'@ | Set-Content -Path $LibCheckPath -NoNewline -Encoding UTF8
$libsCheck = & $PythonExe $LibCheckPath
if ($LASTEXITCODE -ne 0) {
    Write-Host '[bootstrap] ERRO: bibliotecas Python nao importam' -ForegroundColor Red
    Write-Host $libsCheck
    Remove-Item $LibCheckPath -Force -ErrorAction SilentlyContinue
    exit 1
}
Write-Host "[bootstrap] $libsCheck" -ForegroundColor Green
Remove-Item $LibCheckPath -Force -ErrorAction SilentlyContinue

# 3. Fontes (4 arquivos, cada um > 50 KB, TTF magic header).
$FontCheckPath = Join-Path $env:TEMP 'frederico_font_check.py'
$FontCheckScriptContent = @"
import sys
from pathlib import Path
fonts_dir = Path(r'$FontsDir')
expected = [
    'SourceSans3VF-Upright.ttf',
    'SourceSans3VF-Italic.ttf',
    'SourceSerif4Variable-Roman.ttf',
    'SourceSerif4Variable-Italic.ttf',
]
errors = []
for name in expected:
    p = fonts_dir / name
    if not p.is_file():
        errors.append(name + ': ausente')
        continue
    size = p.stat().st_size
    if size < 50_000:
        errors.append(name + ': tamanho suspeito ' + str(size))
        continue
    with open(p, 'rb') as f:
        magic = f.read(4)
    # TTF magic: 0x00010000 (TrueType) ou 'OTTO' (CFF) ou 'true'/'typ1' (Apple).
    if magic not in (b'\x00\x01\x00\x00', b'OTTO', b'true', b'typ1'):
        errors.append(name + ': magic header invalido ' + magic.hex())
if errors:
    for e in errors:
        print('FAIL', e)
    sys.exit(1)
print('fonts OK: ' + ', '.join(expected))
"@
# Substitui o placeholder $FontsDir pelo path real. $FontsDir nao tem
# aspas problematicas porque e um path Windows.
$FontCheckScriptContent = $FontCheckScriptContent -replace '\$FontsDir', $FontsDir
Set-Content -Path $FontCheckPath -Value $FontCheckScriptContent -NoNewline -Encoding UTF8
$fontsCheck = & $PythonExe $FontCheckPath
if ($LASTEXITCODE -ne 0) {
    Write-Host '[bootstrap] ERRO: fontes Tinta e Latao nao conferem' -ForegroundColor Red
    Write-Host $fontsCheck
    Remove-Item $FontCheckPath -Force -ErrorAction SilentlyContinue
    exit 1
}
Write-Host "[bootstrap] $fontsCheck" -ForegroundColor Green
Remove-Item $FontCheckPath -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Resumo final
# ---------------------------------------------------------------------------

$totalSize = (Get-ChildItem $RuntimeDir -Recurse -File | Measure-Object -Property Length -Sum).Sum
$totalSizeMB = [math]::Round($totalSize / 1MB, 1)

Write-Host ''
Write-Host "[bootstrap] OK - runtime completo do document-worker v0.2.0 em $RuntimeDir" -ForegroundColor Green
Write-Host "[bootstrap] Tamanho total: $totalSizeMB MB" -ForegroundColor Green
Write-Host '[bootstrap] Pra rodar o worker:' -ForegroundColor Green
Write-Host "    & '$PythonExe' '$ScriptDir\document-worker.py'"
