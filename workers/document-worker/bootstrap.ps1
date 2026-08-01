# bootstrap.ps1 - instala o runtime completo do `document-worker` v0.3.0
#
# O `document-worker` precisa de:
#   - Python 3.12.7 embeddable + pip (named pipes via pywin32)
#   - pywin32 (acesso a CreateNamedPipe / ConnectNamedPipe / ReadFile / WriteFile)
#   - python-docx, openpyxl, reportlab, pdfplumber (handlers reais, Etapa 2B+X)
#   - pytesseract (wrapper Python do Tesseract - Etapa 2B+Y, ADR-0019)
#   - Tesseract 5.4.0.20240606 (binario OCR - Etapa 2B+Y, ADR-0019)
#   - por + eng + osd traineddata (tessdata_fast 4.1.0 - Etapa 2B+Y, ADR-0019)
#   - Source Sans 3 + Source Serif 4 (TTF variable, identidade "Tinta e Latao" -
#     Adobe Fonts, conforme PROMPT MESTRE 16.3 e ADR-0004 / ADR-0018)
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
#
# Por que Tesseract 5.4.0.20240606 do UB-Mannheim GitHub Releases e nao
# a versao mais nova do mirror (5.5.0.20241111): o mirror da
# UB-Mannheim (`digi.bib.uni-mannheim.de`) tem dado 403 Forbidden pra
# varios IPs/UAs em 2026; a versao do GitHub Releases e identica em
# conteudo e tem URL estavel versionada (asset do release oficial).
# Detalhes no ADR-0019 §Decisao 1.
#
# Por que tessdata_fast (nao tessdata, nao tessdata_best): tessdata_fast
# e o conjunto integerized LSTM usado por Debian/Ubuntu e Tesseract.js -
# ~5x mais rapido, ~95% da acuracia. Suficiente pro caso de uso
# (Tinta e Latao). Detalhes no ADR-0019 §Decisao 2.
#
# Por que SHA-256 fixo em TUDO que baixamos: dois desenvolvedores, dois
# downloads, dois resultados. Sem o hash, o bootstrap deixa de ser
# reproduzivel e o worker pode usar artefatos diferentes em maquinas
# diferentes. Detalhes no ADR-0019 §Decisao 1.3 (raw/main nao e estavel;
# URL versionada com tag de release + SHA-256 do arquivo).
#
# Por que admin detection no bloco Tesseract: o instalador NSIS do
# UB-Mannheim tem manifesto `requireAdministrator` no PE - PowerShell
# 5.1 em contexto non-elevated bloqueia o Start-Process antes de
# executar, e em dev local nao-admin e o caminho comum. Em CI
# (GitHub Actions windows-latest) o runner roda como admin e o silent
# install funciona. Em dev local, o bootstrap pula o bloco com
# instrucoes claras. A instalacao real pra usuario final fica pro
# instalador NSIS do Tauri (Fase 9) - que roda elevado.

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
    # Etapa 5 da Fase 5 (ADR-0021): auditoria bloqueante do PDFPro
    # (§19.6) precisa de pikepdf (estrutural + PDF/A-2B), pypdfium2
    # (visual, rasterizacao PDFium) e fontTools (checagem de glifo
    # via cmap). Hard-fail do bootstrap se qualquer uma faltar
    # (D-FAIL-1) - §19.6 nao tem interruptor, e auditoria
    # silenciosamente reduzida devolvendo verde seria o mesmo
    # problema do interruptor com outro nome.
    & $PythonExe -m pip install python-docx openpyxl 'reportlab>=4.0' 'pdfplumber>=0.10' 'pikepdf>=9.0' 'pypdfium2>=4.0' 'fonttools>=4.50' --no-warn-script-location
    if ($LASTEXITCODE -ne 0) {
        Write-Host '[bootstrap] ERRO: pip install das bibliotecas falhou' -ForegroundColor Red
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Tesseract 5.4.0 (UB-Mannheim GitHub Releases) - Etapa 2B+Y
# ---------------------------------------------------------------------------
#
# Binario OCR de verdade. Instalador NSIS (Nullsoft Install System) do
# UB-Mannheim, com SHA-256 fixo pra reprodutibilidade.
#
# **Por que silent install com admin detection:**
# O instalador NSIS tem `requestedExecutionLevel=requireAdministrator`
# no manifesto PE. Em contexto non-elevated (PowerShell 5.1 dev local),
# o Start-Process detecta o manifesto e bloqueia antes de executar.
# Em CI (GitHub Actions windows-latest) o runner roda como admin e o
# silent install funciona. Pra dev local nao-admin, o bootstrap pula
# com instrucoes claras. A instalacao pro usuario final fica pro
# instalador NSIS do Tauri (Fase 9) - que ja roda elevado.
#
# **Por que Tesseract 5.4.0.20240606 do GitHub Releases (nao mirror):**
# O mirror `digi.bib.uni-mannheim.de` retornou 403 Forbidden em varios
# IPs/UAs testados (2026-07). O release no GitHub e identico em
# conteudo e tem URL estavel (asset de release oficial). Bump do SHA
# so quando o mantenedor cortar novo release no GitHub.
# Detalhes no ADR-0019 §Decisao 1.

$TesseractDir = Join-Path $RuntimeDir 'tesseract'
$TesseractExe = Join-Path $TesseractDir 'tesseract.exe'
$TesseractInstallerUrl = 'https://github.com/UB-Mannheim/tesseract/releases/download/v5.4.0.20240606/tesseract-ocr-w64-setup-5.4.0.20240606.exe'
$TesseractInstallerName = 'tesseract-ocr-w64-setup-5.4.0.20240606.exe'
# SHA-256 do instalador (calculado em 2026-07-30, verificado contra
# asset oficial do release v5.4.0.20240606 no GitHub do UB-Mannheim).
$TesseractInstallerSha256 = 'C885FFF6998E0608BA4BB8AB51436E1C6775C2BAFC2559A19B423E18678B60C9'

function Test-Administrator {
    # Retorna $true se o processo atual tem privilegios administrativos.
    # No Windows, verifica o role do token do processo via .NET.
    $id = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Se Tesseract ja esta em runtime/, pula. O instalador NSIS escreve
# o conteudo em runtime/tesseract/ via /D=path (silent).
if (Test-Path $TesseractExe) {
    Write-Host "[bootstrap] Tesseract ja presente em $TesseractDir - pulando." -ForegroundColor Yellow
} elseif (-not (Test-Administrator)) {
    # Nao-admin: pula com instrucoes. CI roda como admin; em dev local
    # o usuario pode rodar o bootstrap como admin OU instalar Tesseract
    # manualmente. Os testes E2E do document-worker vao falhar com
    # mensagem clara se Tesseract nao estiver (causando panic com
    # instrucao apontando pra ca).
    Write-Host '[bootstrap] AVISO: contexto non-elevated - bloco Tesseract pulado.' -ForegroundColor Yellow
    Write-Host '[bootstrap] Para instalar Tesseract:' -ForegroundColor Yellow
    Write-Host '[bootstrap]   1) Abra PowerShell como Administrador e rode este bootstrap de novo, OU' -ForegroundColor Yellow
    Write-Host '[bootstrap]   2) Baixe tesseract-ocr-w64-setup-5.4.0.20240606.exe do GitHub UB-Mannheim' -ForegroundColor Yellow
    Write-Host '[bootstrap]      e instale em C:\src\Frederico\workers\document-worker\runtime\tesseract' -ForegroundColor Yellow
    Write-Host '[bootstrap]   3) O instalador NSIS do Frederico (Fase 9) ja faz isso pro usuario final.' -ForegroundColor Yellow
} else {
    Write-Host "[bootstrap] instalando Tesseract 5.4.0 em $TesseractDir (silent install)" -ForegroundColor Cyan

    # Baixa o instalador.
    $InstallerPath = Join-Path $RuntimeDir $TesseractInstallerName
    Write-Host "[bootstrap]   baixando $TesseractInstallerUrl"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $TesseractInstallerUrl -OutFile $InstallerPath -UseBasicParsing
    } catch {
        Write-Host "[bootstrap] ERRO baixando Tesseract installer: $_" -ForegroundColor Red
        exit 1
    }

    # Valida SHA-256 antes de executar - defesa contra MITM ou download
    # corrompido. Se nao bater, aborta antes de invocar o instalador.
    $actualHash = (Get-FileHash $InstallerPath -Algorithm SHA256).Hash
    if ($actualHash -ne $TesseractInstallerSha256) {
        Write-Host "[bootstrap] ERRO: SHA-256 do instalador Tesseract nao confere" -ForegroundColor Red
        Write-Host "[bootstrap]   esperado: $TesseractInstallerSha256" -ForegroundColor Red
        Write-Host "[bootstrap]   obtido:   $actualHash" -ForegroundColor Red
        Write-Host "[bootstrap] O instalador pode ter sido trocado ou corrompido. Abortando." -ForegroundColor Red
        exit 1
    }
    Write-Host '[bootstrap]   SHA-256 OK'

    # CUIDADO: o instalador NSIS do UB-Mannheim IGNORA o `/D=<path>`
    # custom devido ao NSIS macro `MULTIUSER_INSTALLMODE_INSTDIR`
    # (issue tesseract-ocr/tesseract#4360 confirmada em 2026-07-30 -
    # o `MULTIUSER_INSTALLMODE_INSTDIR` sobrescreve o command line).
    # O instalador SEMPRE usa o path default do Windows
    # (tipicamente `C:\Program Files\Tesseract-OCR` no w64-setup),
    # mesmo passando `/D=...` corretamente. Tentamos `/D=...` em CI
    # run 30575610679 e 30576068265: instalador rodou em ~8s com exit
    # 0 mas tesseract.exe nao apareceu no path esperado (foi pro
    # `C:\Program Files\Tesseract-OCR`).
    #
    # **Estrategia adotada:** instala com `/S` no path default do
    # Windows + copia o conteudo pro nosso `runtime/tesseract/`. Self-
    # contained, reproduzivel, e o SHA-256 ainda protege contra MITM.
    if (Test-Path $TesseractDir) {
        Write-Host "[bootstrap]   removendo $TesseractDir pre-existente"
        Remove-Item -Path $TesseractDir -Recurse -Force
    }

    Write-Host '[bootstrap]   rodando silent install (NSIS /S - /D ignorado pelo instalador)...'
    $proc = Start-Process -FilePath $InstallerPath -ArgumentList '/S' -Wait -PassThru -NoNewWindow
    if ($proc.ExitCode -ne 0) {
        Write-Host "[bootstrap] ERRO: instalador Tesseract saiu com codigo $($proc.ExitCode)" -ForegroundColor Red
        exit 1
    }

    # Limpa instalador.
    Remove-Item $InstallerPath -Force

    # O instalador UB-Mannheim w64-setup instala em
    # `C:\Program Files\Tesseract-OCR` (verificado em 5.4.0 e
    # 5.5.0). Procuramos tesseract.exe no path default e copiamos
    # o conteudo pro nosso runtime/tesseract/.
    $DefaultTesseractDir = Join-Path $env:ProgramFiles 'Tesseract-OCR'
    $DefaultTesseractExe = Join-Path $DefaultTesseractDir 'tesseract.exe'
    if (-not (Test-Path $DefaultTesseractExe)) {
        # Fallback: alguns installers antigos usam path sem espaco.
        $DefaultTesseractDir = 'C:\Tesseract-OCR'
        $DefaultTesseractExe = Join-Path $DefaultTesseractDir 'tesseract.exe'
    }
    if (-not (Test-Path $DefaultTesseractExe)) {
        Write-Host "[bootstrap] ERRO: tesseract.exe nao apareceu em $DefaultTesseractExe (path default do instalador)" -ForegroundColor Red
        Write-Host '[bootstrap]   Procurando tesseract.exe em outros locais comuns:' -ForegroundColor Red
        @('C:\Program Files\Tesseract-OCR', 'C:\Program Files (x86)\Tesseract-OCR', 'C:\Tesseract-OCR', 'C:\Tesseract') | ForEach-Object {
            $candidate = Join-Path $_ 'tesseract.exe'
            if (Test-Path $candidate) { Write-Host "    ACHOU: $candidate" -ForegroundColor Red }
        }
        exit 1
    }

    # Copia o conteudo do path default pro nosso runtime/tesseract/.
    # Usamos `Copy-Item -Recurse -Force` pra manter a estrutura
    # (tesseract.exe, *.dll, tessdata/ com os idiomas default, etc).
    Write-Host "[bootstrap]   copiando de $DefaultTesseractDir para $TesseractDir"
    New-Item -ItemType Directory -Path $TesseractDir -Force | Out-Null
    Copy-Item -Path (Join-Path $DefaultTesseractDir '*') -Destination $TesseractDir -Recurse -Force

    # Verifica que tesseract.exe apareceu no destino final.
    if (-not (Test-Path $TesseractExe)) {
        Write-Host "[bootstrap] ERRO: tesseract.exe nao apareceu em $TesseractExe apos copia" -ForegroundColor Red
        exit 1
    }

    Write-Host "[bootstrap] Tesseract instalado:" -ForegroundColor Green
    & $TesseractExe --version 2>&1 | Select-Object -First 3 | ForEach-Object { Write-Host "    $_" }
}

# ---------------------------------------------------------------------------
# Tesseract tessdata (por + eng + osd) - Etapa 2B+Y
# ---------------------------------------------------------------------------
#
# Traineddata do Tesseract vem do repo oficial `tesseract-ocr/tessdata_fast`
# (NAO `tessdata` legacy, NAO `tessdata_best` pesado). Por que fast:
# ~5x mais rapido, ~95% da acuracia, e o que Debian/Ubuntu empacota.
#
# **Por que tag `4.1.0` fixa (nao `main`):** `raw/main` muda sem aviso;
# dois downloads, dois traineddata, dois resultados de OCR. Com a
# tag fixa + SHA-256, o bootstrap e reproduzivel. Bump quando o
# mantenedor cortar novo release.
#
# **Por que SHA-256 por arquivo:** alem da tag fixa, o hash ancora
# exatamente o arquivo. Defesa contra MITM, download corrompido, ou
# mudanca retroativa no release taggeado.
#
# **Por que `osd` (orientation/script detection):** o Tesseract usa
# automaticamente pra detectar orientacao da pagina. Sem ele, PDFs
# virados de cabeca pra baixo dao OCR vazio.

$TessdataDir = Join-Path $TesseractDir 'tessdata'

# Tesseract 5.4.0 instala tessdata/ com alguns idiomas de exemplo.
# Verificamos se os 3 que precisamos (por, eng, osd) estao la.
function Test-TessdataFile($name) {
    $p = Join-Path $TessdataDir $name
    if (-not (Test-Path $p)) { return $false }
    $size = (Get-Item $p).Length
    return $size -gt 100KB
}

$allTessdataOk = $true
foreach ($name in @('por.traineddata', 'eng.traineddata', 'osd.traineddata')) {
    if (-not (Test-TessdataFile $name)) { $allTessdataOk = $false; break }
}

if ($allTessdataOk) {
    Write-Host '[bootstrap] tessdata Tesseract (por, eng, osd) ja presentes - pulando.' -ForegroundColor Yellow
} elseif (-not (Test-Path $TesseractExe)) {
    # Tesseract nao instalado - nao da pra validar tessdata
    Write-Host '[bootstrap] AVISO: Tesseract nao instalado - bloco tessdata pulado.' -ForegroundColor Yellow
} else {
    Write-Host '[bootstrap] baixando tessdata Tesseract (por, eng, osd) - tessdata_fast 4.1.0' -ForegroundColor Cyan

    # Manifesto versionado: tag + SHA-256 por arquivo. Bump de release
    # = bump da tag E dos SHA-256 juntos.
    $TessdataTag = '4.1.0'
    $TessdataBaseUrl = "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/$TessdataTag"
    $TessdataFiles = @(
        @{ Name = 'por.traineddata'; Sha256 = 'C4932B937207A9514B7514D518B931A99938C02A28A5A5A553F8599ED58B7DEB' }
        @{ Name = 'eng.traineddata'; Sha256 = '7D4322BD2A7749724879683FC3912CB542F19906C83BCC1A52132556427170B2' }
        @{ Name = 'osd.traineddata'; Sha256 = '9CF5D576FCC47564F11265841E5CA839001E7E6F38FF7F7AACF46D15A96B00FF' }
    )

    New-Item -ItemType Directory -Path $TessdataDir -Force | Out-Null

    foreach ($f in $TessdataFiles) {
        $dest = Join-Path $TessdataDir $f.Name
        $url = "$TessdataBaseUrl/$($f.Name)"
        Write-Host "[bootstrap]   $($f.Name) (tag $TessdataTag)"
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
        } catch {
            Write-Host "[bootstrap] ERRO baixando $($f.Name): $_" -ForegroundColor Red
            exit 1
        }

        # Valida SHA-256 imediatamente.
        $actualHash = (Get-FileHash $dest -Algorithm SHA256).Hash
        if ($actualHash -ne $f.Sha256) {
            Write-Host "[bootstrap] ERRO: SHA-256 de $($f.Name) nao confere" -ForegroundColor Red
            Write-Host "[bootstrap]   esperado: $($f.Sha256)" -ForegroundColor Red
            Write-Host "[bootstrap]   obtido:   $actualHash" -ForegroundColor Red
            exit 1
        }
    }

    Write-Host '[bootstrap]   SHA-256 de todos os tessdata OK' -ForegroundColor Green
}

# ---------------------------------------------------------------------------
# pytesseract (wrapper Python do Tesseract) - Etapa 2B+Y
# ---------------------------------------------------------------------------
#
# `pytesseract` e o wrapper Python oficial do Tesseract. So wrapper -
# o binario Tesseract (instalado acima) faz o trabalho pesado.
# Sentinel: o modulo `pytesseract` tem que ser importavel.

$pytesseractInstalled = $false
try {
    & $PythonExe -c 'import pytesseract' 2>$null
    if ($LASTEXITCODE -eq 0) { $pytesseractInstalled = $true }
} catch {}

if ($pytesseractInstalled) {
    Write-Host '[bootstrap] pytesseract ja instalado - pulando.' -ForegroundColor Yellow
} else {
    Write-Host '[bootstrap] instalando pytesseract'
    & $PythonExe -m pip install pytesseract --no-warn-script-location
    if ($LASTEXITCODE -ne 0) {
        Write-Host '[bootstrap] ERRO: pip install pytesseract falhou' -ForegroundColor Red
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
# Etapa 5 da Fase 5 (ADR-0021): auditoria bloqueante do PDFPro
# exige pikepdf + pypdfium2 + fonttools. Hard-fail se faltar
# (D-FAIL-1) - sem "plano B" silencioso.
for lib in ['docx', 'openpyxl', 'reportlab', 'pdfplumber', 'pytesseract', 'pikepdf', 'pypdfium2', 'fontTools']:
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

# 2b. Tesseract binary + tessdata (se instalado). Se nao foi instalado
#     (contexto non-elevated), pula com warning - nao falha. Os
#     testes E2E que dependem de Tesseract vao falhar com mensagem
#     clara no Rust (panic com instrucao).
if (Test-Path $TesseractExe) {
    Write-Host '[bootstrap] verificando Tesseract + tessdata' -ForegroundColor Cyan
    $tessVersion = & $TesseractExe --version 2>&1 | Select-Object -First 1
    Write-Host "[bootstrap]   tesseract: $tessVersion" -ForegroundColor Green
    $missingTessdata = @()
    foreach ($name in @('por.traineddata', 'eng.traineddata', 'osd.traineddata')) {
        if (-not (Test-Path (Join-Path $TessdataDir $name))) {
            $missingTessdata += $name
        }
    }
    if ($missingTessdata.Count -gt 0) {
        Write-Host "[bootstrap] ERRO: tessdata faltando: $($missingTessdata -join ', ')" -ForegroundColor Red
        exit 1
    }
    Write-Host '[bootstrap]   tessdata OK (por, eng, osd)' -ForegroundColor Green

    # **Verificacao de portabilidade (obrigatoria, ADR-0019 §Decisao 1):**
    # Tesseract funciona quando invocado de um path NAO-canônico
    # (copia runtime/tesseract pra outro lugar, TESSDATA_PREFIX apontando
    # pra lá, OCR num processo com env limpo)? Se a arvore depender de
    # registro ou variavel de ambiente global, o instalador nao serve
    # pro nosso self-contained. Pula com warning se ja rodamos
    # (idempotente), faz o teste full na primeira vez.
    $PortabilityMarker = Join-Path $TesseractDir '.portability_checked'
    if (Test-Path $PortabilityMarker) {
        Write-Host '[bootstrap]   portabilidade ja verificada - pulando.' -ForegroundColor Yellow
    } else {
        Write-Host '[bootstrap]   testando portabilidade da arvore Tesseract...' -ForegroundColor Cyan
        $PortabilityTemp = Join-Path $RuntimeDir '.tesseract-portability-test'
        if (Test-Path $PortabilityTemp) {
            Remove-Item -Recurse -Force $PortabilityTemp
        }
        # Copia a arvore (read-only pra OCR nao gravar no original).
        $robocpy = robocopy $TesseractDir $PortabilityTemp /MIR /NFL /NDL /NJH /NJS /NC /NS /NP | Out-Null
        # Verifica que tesseract.exe --list-langs funciona do path copiado,
        # com TESSDATA_PREFIX apontando pro tessdata local.
        $env:TESSDATA_PREFIX = $PortabilityTemp
        try {
            $listLangs = & "$PortabilityTemp\tesseract.exe" --list-langs 2>&1
        } finally {
            Remove-Item Env:TESSDATA_PREFIX -ErrorAction SilentlyContinue
        }
        $env:TESSDATA_PREFIX = $null
        if ($LASTEXITCODE -ne 0) {
            Write-Host '[bootstrap] AVISO: Tesseract NAO e portatil a partir de um path arbitrario.' -ForegroundColor Yellow
            Write-Host '[bootstrap]   O instalador provavelmente gravou chaves de registro/variaveis globais.' -ForegroundColor Yellow
            Write-Host '[bootstrap]   Pra self-contained do bundle, registre isso no ADR-0019 §Plano alternativo.' -ForegroundColor Yellow
        } else {
            Write-Host '[bootstrap]   portabilidade OK: tesseract --list-langs retornou: $($listLangs[0..3] -join '" "')' -ForegroundColor Green
            New-Item -ItemType File -Path $PortabilityMarker -Force | Out-Null
        }
        # Limpa temp.
        if (Test-Path $PortabilityTemp) {
            Remove-Item -Recurse -Force $PortabilityTemp
        }
    }
} else {
    Write-Host '[bootstrap] AVISO: Tesseract nao instalado - pular verificacao.' -ForegroundColor Yellow
    Write-Host '[bootstrap]   Documentos com `ocr.run` ou PDF 100% escaneado vao falhar.' -ForegroundColor Yellow
    Write-Host '[bootstrap]   Instale manualmente (veja instrucoes acima) ou rode o bootstrap como Admin.' -ForegroundColor Yellow
}

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
Write-Host "[bootstrap] OK - runtime completo do document-worker v0.3.0 em $RuntimeDir" -ForegroundColor Green
Write-Host "[bootstrap] Tamanho total: $totalSizeMB MB" -ForegroundColor Green
Write-Host '[bootstrap] Pra rodar o worker:' -ForegroundColor Green
Write-Host "    & '$PythonExe' '$ScriptDir\document-worker.py'"
