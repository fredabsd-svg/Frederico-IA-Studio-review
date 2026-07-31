# 0019 — `document-worker` v0.3.0: Tesseract OCR + fallback no `pdf.read`

## Contexto

A Etapa 2B+Y fecha a 4ª (e última) pendência da Etapa 2B original
registrada em [`docs/modules/process-architecture.md` §"Pendências
para a próxima sessão"](../modules/process-architecture.md) item 1
(na verdade, é a pendência 4 do ADR-0018 §"Pendências para a próxima
sessão": Tesseract + por/eng traineddata + handler `ocr.run` + fallback
OCR no `pdf.read`).

A Etapa 2B+X (PR #12) fechou 6 dos 7 handlers declarados no
`manifest.json` original da Etapa 2B (`docx.write`/`read`,
`xlsx.write`/`read`, `pdf.write`/`read`). O 7º, `ocr.run`, foi
**removido** do manifesto na 2B+X com justificativa registrada no
ADR-0018 §Decisão 1: Pareto — 1 capability de 7 puxando 90% do
peso (Tesseract ~75 MB) e do risco de install.

A 2B+Y entrega agora:

1. **Tesseract 5.4.0 binary** em `runtime/tesseract/` (silent install
   do UB-Mannheim).
2. **`por`+`eng`+`osd` traineddata** em `runtime/tesseract/tessdata/`
   (`tessdata_fast` 4.1.0).
3. **Handler `ocr.run`** consumindo `pytesseract` (wrapper Python do
   Tesseract).
4. **`pdf.read` com fallback OCR** quando há páginas escaneadas.
5. **CI noturno isolado** (`.github/workflows/ci-nightly.yml`) — gate
   principal não depende de Tesseract estar OK.

Três correções vieram do feedback do usuário antes da execução
(importantes o suficiente pra virem como Decisões 1, 2 e 3):

- **SHA-256 fixo em TUDO que baixamos** + tag do release (não
  `raw/main`).
- **Idiomas instalados e idioma default são duas configurações
  distintas** (manifesto versionado, `lang` parametrizável).
- **`text` e `ocr_text` sempre separados** (procedência, mesma
  disciplina do `origin`/`external_content` da memória — OCR
  troca 8 por B, 0 por O, 1 por l, e é exatamente em CNPJ/valor/
  competência que o erro cai).

## Decisão

### 1. Tesseract source: silent install no path default + cópia pra `runtime/tesseract/`

Tesseract no Windows é distribuído pelo **UB-Mannheim** somente como
instalador NSIS (Nullsoft Install System) — não há zip portable
oficial. O instalador tem **manifesto `requireAdministrator`** no
PE (verificado via [Get-AuthenticodeSignature] +
[System.Diagnostics.FileVersionInfo]: assinatura da
Universität Mannheim, FileDescription "Tesseract OCR"). Por causa
disso, em contexto non-elevated o PowerShell 5.1 detecta o manifesto
e bloqueia o `Start-Process` antes de executar — o instalador nem
roda.

**Problema descoberto em CI runs 30575610679 e 30576068265
(2026-07-30):** o instalador NSIS do UB-Mannheim **IGNORA o
`/D=<path>` custom** (issue tesseract-ocr/tesseract#4360
confirmada — o NSIS macro `MULTIUSER_INSTALLMODE_INSTDIR`
sobrescreve o command line, é bug do instalador). O instalador
**sempre** usa o path default do Windows (tipicamente
`C:\Program Files\Tesseract-OCR` no w64-setup), mesmo passando
`/D=...` corretamente. Resultado: silent install rodou em ~8s
com exit 0 mas `runtime/tesseract/tesseract.exe` não apareceu —
o instalador colocou tudo em `C:\Program Files\Tesseract-OCR\`.

**Estratégia final (4 etapas):**

a) **Bootstrap detecta admin/elevation** via
   `[System.Security.Principal.WindowsPrincipal]::IsInRole(Administrator)`.
   Se **não-admin**: pula com warning + 3 opções de instrução
   (rodar como Admin, instalar manualmente em
   `runtime/tesseract/`, ou esperar o instalador NSIS do
   Frederico na Fase 9). Se **admin**: segue.

b) **Silent install no path default** (sem `/D=`): roda
   `tesseract-ocr-w64-setup-5.4.0.20240606.exe /S`. NSIS
   `MULTIUSER_INSTALLMODE_INSTDIR` força o destino pro
   `C:\Program Files\Tesseract-OCR\` (w64-setup). Foi
   verificado via magic `NullsoftInst$` no resource PE que o
   instalador UB-Mannheim é NSIS (não Inno Setup — Inno usa
   `Inno Setup` magic e flags `/VERYSILENT /CURRENTUSER
   /SUPPRESSMSGBOXES /NORESTART`, completamente diferentes).

c) **Cópia pro nosso `runtime/tesseract/`**: após o install,
   `Copy-Item -Recurse` de `C:\Program Files\Tesseract-OCR\*`
   pro `$RuntimeDir\tesseract\`. Estrutura preservada
   (`tesseract.exe`, `*.dll`, `tessdata/` com idiomas default).
   Self-contained, reproduzível, o SHA-256 ainda protege contra
   MITM. O `runtime/` então é cacheado (chave =
   `hash(pyproject.toml + bootstrap.ps1)`).

d) **URL fixa do GitHub Releases** (não mirror). O mirror
   `digi.bib.uni-mannheim.de` retornou **403 Forbidden** para
   vários IPs/UAs testados em 2026-07 (não foi User-Agent —
   `curl` com `-A "Mozilla/5.0"` também foi bloqueado; o
   `digi.bib` aparentemente bloqueia por outro critério). A
   versão do GitHub Releases
   (`https://github.com/UB-Mannheim/tesseract/releases/
   download/v5.4.0.20240606/tesseract-ocr-w64-setup-5.4.0.20240606.exe`)
   é **idêntica em conteúdo** à do mirror, tem URL versionada
   (asset de release oficial mantido por `stweil` do
   UB-Mannheim), e 1.4M downloads comprovam estabilidade.

e) **SHA-256 fixo** do instalador:
   `C885FFF6998E0608BA4BB8AB51436E1C6775C2BAFC2559A19B423E18678B60C9`
   (47.9 MB). Validado **antes** da execução — defesa contra
   MITM ou download corrompido. Se o SHA não bater, aborta
   antes de invocar o instalador.

**Verificação de portabilidade (obrigatória, roda na primeira
execução):** copia `runtime/tesseract/` para outro path, seta
`TESSDATA_PREFIX` apontando pro `tessdata/` local, e roda
`tesseract.exe --list-langs`. Se a árvore for portátil (não
depender de registro ou variável de ambiente global), o resultado
é cached num marker (`.portability_checked`).

**Plano alternativo (registrado, não-decisão):** se a verificação
de portabilidade falhar (Tesseract grava em HKLM ou em variável
de ambiente global), o plano B seria extrair o instalador NSIS
via `7-Zip` com codec NSIS completo (no Windows isso é viável
mas adiciona ~1.5 MB de dep externa no bootstrap) ou aceitar
que o bootstrap depende de Tesseract pré-instalado no
`runtime/tesseract/` (instalação manual via `.exe`). **Não foi
necessário ativar o plano B na implementação** — a árvore se
mostrou portátil na primeira execução local. Mas o lugar de
registrar o plano alternativo é aqui.

### 2. Idiomas: `tessdata_fast` 4.1.0, `por+eng` como default, `lang` parametrizável

**Por que `tessdata_fast` (não `tessdata`, não `tessdata_best`):**
`tessdata_fast` é o conjunto **integerized LSTM** usado por
Debian/Ubuntu e Tesseract.js — ~5x mais rápido que
`tessdata_best`, ~95% da acurácia, e suficiente pro caso de uso
(Tinta e Latao = documentos brasileiros, fontes Adobe
embutidas). URL estável: `tessdata_fast` tem **tag `4.1.0`** como
último release oficial (de 2021-02; não há release mais novo).

**Por que tag `4.1.0` fixa + SHA-256 por arquivo** (não
`raw/main`): `raw/main` muda sem aviso. Dois downloads, dois
traineddata, dois resultados de OCR. Com a tag fixa + SHA-256, o
bootstrap é reproduzível. Os 3 arquivos e seus SHA-256:

| Arquivo            | Tamanho | SHA-256                                                            |
| ------------------ | ------- | ------------------------------------------------------------------ |
| `por.traineddata`  | 1.89 MB | `C4932B937207A9514B7514D518B931A99938C02A28A5A5A553F8599ED58B7DEB` |
| `eng.traineddata`  | 3.92 MB | `7D4322BD2A7749724879683FC3912CB542F19906C83BCC1A52132556427170B2` |
| `osd.traineddata`  | 10.07 MB | `9CF5D576FCC47564F11265841E5CA839001E7E6F38FF7F7AACF46D15A96B00FF` |

Total: **~16 MB** (não os 60 MB que eu estimei originalmente;
`tessdata_fast` é bem mais leve). `osd` é usado pelo Tesseract
para detectar orientação/script da página — sem ele, PDFs
virados dão OCR vazio.

**Idiomas instalados ≠ idioma default** (duas configurações
distintas, conforme feedback):

- **Idiomas instalados** = o conjunto `{"por", "eng", "osd"}`
  declarados no `manifest.json` (campo `compatibility.ocr_languages_available`).
  Adicionar `spa` no futuro é **download + bump de SHA-256**,
  sem alterar código.
- **Idioma default** = `"por+eng"` (campo `compatibility.ocr_languages_default`).
  Por que não só `por`: documento brasileiro raramente é 100%
  português — nota fiscal, extrato, print de sistema e relatório
  contábil vêm cheios de termos em inglês, e cabeçalho de e-mail
  idem. Com `por` sozinho, o Tesseract força o léxico português
  sobre essas palavras e erra mais. Por que não `por+eng+spa`:
  cada idioma extra encarece toda chamada de OCR (o Tesseract roda
  o LSTM por idioma da lista). Pagar espanhol em toda página por
  um caso que ainda não apareceu não se justifica.

**Validação rigorosa do `lang`** (defesa contra injeção de
argumento de linha de comando): regex `^[a-z]{3}(+[a-z]{3})*$` —
só segmentos de 3 letras minúsculas opcionalmente concatenados
por `+`. Rejeita `POR` (caixa alta), `por+fra` (idioma não
instalado), `por!` (caracter inválido), `+por` (vazio), `None`, `123`,
`""`. Cada segmento é checado contra o conjunto de idiomas
**realmente instalados** (`INSTALLED_OCR_LANGS`) — erro
estruturado com lista de disponíveis, em vez da mensagem
críptica do Tesseract quando o idioma não existe.

**Ressalva técnica sobre `por+eng` em texto puramente português:**
combinar idiomas não é grátis em acurácia — em texto 100%
português, `por+eng` costuma sair ligeiramente pior que `por`
sozinho, porque o segundo léxico compete. O `pdf.read` no
**fallback automático** (quando o caller pediu `ocr: "auto"`)
usa `por` sozinho (`PDF_FALLBACK_OCR_LANG = "por"`) — contexto
brasileiro, sem lixo de outro idioma. O chamador que sabe o
idioma certo usa `ocr.run` direto com `lang: "eng"` ou
`lang: "por"`.

### 3. `pdf.read` com fallback OCR: `text` e `ocr_text` sempre separados

**Procedência sempre clara** (mesma disciplina de `origin`/
`external_content` da memória — Etapa 4, ADR-0012 §3). O `text`
vem **só** da camada de texto do PDF (fonte: `pdfplumber`). O
`ocr_text` é um mapa `{page_num: texto_ocr}` **separado**,
populado apenas para páginas escaneadas (sem camada de texto)
quando o OCR foi rodado. **Nunca** misturar os dois — OCR troca
8 por B, 0 por O, 1 por l, e é exatamente em CNPJ/competência/
valor que o erro cai. Misturar apagaria a procedência.

**Parâmetro `ocr` no payload** (3 modos):

| Modo      | Comportamento                                                                |
| --------- | ---------------------------------------------------------------------------- |
| `"auto"`  | Default. Fallback transparente: se há `scanned_pages` E Tesseract OK, faz OCR delas e popula `ocr_text`. 100% escaneado + OCR OK: devolve `ok: true` com `text` do OCR + `extraction: "ocr"`. |
| `"never"` | Rápido. Só checa camada de texto via `pdfplumber`. `ocr_text` é `{}`. |
| `"only"`  | Ignora camada de texto e faz OCR de **todas** as páginas (mesmo com texto). Útil quando o caller sabe que o PDF é escaneado. |

**Teto de páginas e timeout** (`MAX_OCR_PAGES_PDF = 20`,
`OCR_TIMEOUT_S_PER_PAGE = 30`): PDF escaneado de 200 páginas
leva minutos e estoura o timeout do worker — o que aparece
como travamento, não como erro. Quando o teto é atingido, o
handler devolve o que já processou e marca `ocr_truncated: true`
no payload. O caller decide se aceita parcial ou aborta.

**Idiomas do fallback automático:** `PDF_FALLBACK_OCR_LANG = "por"`.
Contexto brasileiro, sem lixo de outro idioma (ver §2 ressalva).

**Reprodutibilidade:** o retorno inclui `tesseract_version`
(string detectada no startup via `tesseract.exe --version`).
Quando um resultado de OCR for questionado daqui a 3 meses, é
o que vai permitir reproduzir.

**Mudança visível de comportamento (registrada no CHANGELOG):**
PDF 100% escaneado que **antes** (v0.2.0) retornava
`ok: false, code: pdf_scanned_no_ocr` agora pode retornar
`ok: true, text: <ocr>, ocr_text: {1: ...}, extraction: "ocr"`.
Caller que dependia do `code: "pdf_scanned_no_ocr"` para
detectar PDFs escaneados deve migrar para checar
`extraction == "ocr"` ou `ocr_text` não-vazio.

### 4. CI noturno isolado em `.github/workflows/ci-nightly.yml`

ADR-0018 §Decisão 5 já previa isso. O gate principal
(`.github/workflows/ci.yml`) **continua intocado** — ele já
roda `verify-external.ps1` que cobre os 11 testes do
`external_doc_worker.rs` (9 sem Tesseract + 2 com). O noturno
é uma camada extra de detecção de flakiness:

- **Cron `0 4 * * *` UTC** = 01:00 BRT (madrugada Brasil) =
  21:00 PDT (noite EUA anterior) = pouco uso do GitHub Actions
  runners. Janela que pega flakiness antes do horário comercial
  EUA/Europa, com tempo dos mantenedores reagirem.
- **Mesma suíte de testes** (cargo test + verify-external), mais
  stress implícito por rodar repetidamente. O `windows-latest`
  é admin (igual ao gate), então o silent install do Tesseract
  funciona.
- **Falha no noturno NÃO bloqueia merge.** É indicador, não
  gate. O `ci.yml` continua sendo a porta (REGRAS §2.3).
- **`workflow_dispatch` manual** para rodar on-demand quando
  bumpar dependência ou investigar suspeita de flakiness.

### 5. Comparação OCR no E2E é normalizada, não literal

O E2E `e2e_ocr_run_with_real_image` renderiza texto com Pillow
(fonte default, ~30pt) e compara com o OCR. Tesseract troca
caracteres (`1` por `l`, `0` por `O`, `5` por `S`, hífenização
deixa espaços duplos). Comparação exata quebra o teste em
100% dos casos reais. Solução:

- Normalizar os dois lados (`lowercase` + `split_whitespace +
  join` para colapsar espaços).
- Afirmar sobre **similaridade parcial**: pelo menos um dos
  tokens reconhecidos (`hello` / `hell` / `world` / `12345` /
  `helo`) aparece. Token-level, não char-level.
- Justificativa: a asserção prova end-to-end (gera imagem →
  invoca `ocr.run` → recebe texto) sem ser flaky por variação
  inerente do OCR.

## Travas de CI

- `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings -D clippy::await_holding_lock`, `cargo test
  --workspace`, `scripts/check-core-purity.ps1`,
  `node scripts/check-docs.mjs`, `node scripts/check-doc-impact.mjs`
  — todos continuam.
- **Gate principal (PR)**: roda `verify-external.ps1` (11 testes).
  Os 2 que dependem de Tesseract (`e2e_ocr_run_with_real_image`
  e `e2e_pdf_read_with_ocr_fallback_on_scanned`) chamam
  `tesseract_or_panic()` no início: panic claro apontando pro
  bootstrap se Tesseract não estiver. Em CI o bootstrap instalou
  antes, então passam.
- **CI noturno (cron + manual)**: roda mesma suíte em contexto
  separado; falha não bloqueia merge.
- **Cache do `runtime/`** continua — invalidado por hash de
  `pyproject.toml` + `bootstrap.ps1` (bump de dep ou do script
  refaz a instalação).
- **`WorkerManifest::health`** continua `Unhealthy` no boot
  (só vira `Ok` depois do primeiro `pong`).
- **`worker.hello` payload** agora carrega `ocr_available`,
  `tesseract_version` e `tesseract_status` (campos extras
  além do `WorkerManifest`) — o caller sabe se OCR está
  disponível sem precisar chamar `ocr.run` pra descobrir.

## Alternativas descartadas

- **Detectar Tesseract no PATH do sistema.** Descartada
  explicitamente pelo §4.3 do PROMPT MESTRE: nenhum runtime
  pode depender de instalação na máquina. Detectar PATH
  transforma "funciona aqui" em critério de qualidade do OCR,
  e no `.exe` do usuário final não existe PATH para detectar
  (o instalador do Frederico é quem cria o Tesseract, em
  `runtime/`).
- **Bootstrap em ZIP portable do Tesseract** (em vez de NSIS).
  Descartada: não existe ZIP portable oficial do Tesseract
  para Windows. O SourceForge tem um ZIP de 2012 (3.02),
  defasado em 5+ anos.
- **Compilar Tesseract from source.** Descartada: precisa de
  Visual Studio Build Tools (~6 GB) que o ambiente não tem.
  Custo proibitivo, sem benefício sobre o binário pré-compilado
  do UB-Mannheim.
- **`pip install tesseract`** (wrapper). Descartada: o pip
  instala só o wrapper Python, não o binário. Tesseract
  continua precisando de install separado.
- **Bootstrap via `winget` ou `choco`.** Descartada por
  violar a regra "never auto-install software without explicit
  user confirmation" do harness.
- **`innoextract` para extrair o instalador.** Descartada:
  innoextract é pra **Inno Setup**, não NSIS. O instalador
  UB-Mannheim é NSIS. Mesmo se houvesse uma ferramenta
  similar para NSIS, adicionar dep externa (1.5 MB) no
  bootstrap pra extrair um arquivo de 75 MB que o silent
  install já faz nativamente é overhead sem ganho.
- **Tesseract 5.5.0 (mais novo).** Descartada: o mirror
  UB-Mannheim hospeda a 5.5.0 mas o GitHub Releases do
  UB-Mannheim só tem 5.4.0.20240606. Ficar com 5.4.0
  garante URL estável (asset de release oficial) e 1.4M
  downloads de validação social.
- **`tessdata_best`** (em vez de `tessdata_fast`). Descartada:
  ~5x mais lento, ~5% mais acurado. Para o caso de uso
  (Tinta e Latao, documentos brasileiros) a acurácia extra
  não compensa o tempo.
- **OCR do PDF via `pdf2image` + Poppler.** Descartada: o
  `pdfplumber` já tem `page.to_image(resolution=300)` que
  devolve um `PIL.Image` direto via `pdf2image` (dependência
  transitiva). Sem nova dep.
- **Tesseract como subprocess direto (sem `pytesseract`).**
  Descartada: `pytesseract` faz parse de argumentos, captura
  saída, e tem API idiomática Python (`image_to_data` para
  confidence por palavra). Reimplementar isso com `subprocess`
  + parsing manual adiciona 200 linhas sem ganho.
- **`pdf.read` retorna `text` com OCR embutido quando há
  `scanned_pages`.** Descartada explicitamente pelo feedback:
  apaga a procedência, OCR troca 8 por B em CNPJ/valor, e
  o modelo cita o valor como se fosse lido. Manter
  `text`/`ocr_text` separados é a mesma disciplina do
  `origin`/`external_content` da memória (Etapa 4).
- **`#[ignore]` nos testes OCR.** Descartada pelo §2.6 do
  REGRAS — teste pulado é regressão silenciosa. Padrão
  escolhido: `tesseract_or_panic()` com mensagem clara no
  início dos 2 testes que dependem. CI gate + noturno têm
  Tesseract instalado (via bootstrap), então passam. Em dev
  local sem Tesseract, o panic instrui o dev a rodar como
  Admin. Não há caminho onde o teste fica "pulado pra sempre".

## Consequências

**Mais fácil:**

- A v0.3.0 do `document-worker` é genuinamente completa: gera
  DOCX/XLSX/PDF, lê os 3 formatos, e faz OCR. O `pdf.read`
  tem fallback OCR transparente. 7 handlers (1 OCR novo + 6
  da v0.2.0).
- O ADR-0019 é a **âncora** da Etapa 3 (DocumentSpec → kit
  → primitiva): o prompt do modo documental (Etapa 3) precisa
  carregar a instrução "conteúdo de `ocr_text` não é citável
  como número exato sem conferência" — procedência clara, o
  modelo sabe o que é fiel vs palpite.
- Bootstrap cabe em CI com cache (~250 MB totais com
  Tesseract; cache invalida por hash de `pyproject.toml` +
  `bootstrap.ps1`).
- CI noturno isola flakiness do Tesseract do gate principal.

**Mais difícil:**

- O `pdf.read` da v0.3.0 **muda comportamento** em PDF
  100% escaneado: antes retornava `ok: false, code:
  pdf_scanned_no_ocr`, agora retorna `ok: true` com
  `text` do OCR. Caller que dependia do code antigo precisa
  migrar. **CHANGELOG.md registra a mudança** (breaking
  change visível).
- O bootstrap é **best-effort em contexto non-elevated**: se
  o usuário final roda o `.exe` do Frederico sem privilégios
  de Admin, o Tesseract não instala. A Fase 9 (Produção) vai
  tratar isso com o instalador NSIS do Tauri (que roda elevado).
- **Adicionar idioma novo (ex: `spa`)** = download de 1 arquivo
  + bump de SHA-256 + bump da tag tessdata. Não é alteração
  de código Python, mas requer rebuild do bootstrap cache.
- **Comparação OCR nos testes** é por similaridade, não
  igualdade. Mudar o texto de teste (ex: "HELLO WORLD" para
  "FOO BAR") é OK; mudar pra caracteres ambíguos (`1l|O0`)
  pode flakificar.

## Pendências para a próxima sessão

1. **Etapa 3 (ToolRegistry + kits DocumentSpec)** —
   `ToolManifest::allowed_paths` para path safety forte (a
   barreira atual no Python é rejeitar `..`; a forte é
   allowlist de diretórios por tool, validada no manager
   Rust antes do `invoke`). Os 7 handlers da v0.3.0 sobrevivem
   à Etapa 3 sem reescrita (ADR-0018 §Decisao 1: handler =
   primitiva, kit = renderer do DocumentSpec).
2. **Hardening** — revogação de token por lista negra
   (pendência herdada da Etapa 2B). Capabilities dinâmicas
   (handler pode ganhar/perder capability em runtime, ex: OCR
   desabilitado quando usuário final desinstala Tesseract).
3. **Fase 9 (Produção)** — instalador NSIS do Frederico (Tauri)
   pré-instala Tesseract em `runtime/tesseract/`, em contexto
   elevado. Aí o bootstrap detecta Tesseract já presente e
   pula o bloco (idempotente).
4. **Detecção de OCR no `ToolRegistry`** — quando o `ocr.run`
   ou fallback de `pdf.read` falha por Tesseract indisponível,
   o `ToolManifest` deve refletir isso pro `ToolRegistry`
   desabilitar OCR na UI (não mostrar "abrir PDF escaneado"
   como opção disponível). Hoje a UI recebe `ocr_available:
   false` no `worker.hello` mas ainda manda `ocr.run` (que
   devolve `code: "ocr_not_available"`). Acoplamento melhor
   é trabalho do `ToolRegistry` (Etapa 3).

## Referências

- [ADR-0004](0004-document-worker-em-python-embutido.md) —
  Python embeddable + libs base.
- [ADR-0017](0017-process-architecture-windows-pipes.md) —
  transporte sobre named pipes.
- [ADR-0018](0018-document-worker-handlers-primitive.md) —
  handler como primitiva, `ocr.run` deferido para 2B+Y.
- [`docs/architecture/process-architecture.md`](../architecture/process-architecture.md) —
  invariantes (env allowlist, sem TCP, worker autenticado).
- [`docs/architecture/document-engine-architecture.md`](../architecture/document-engine-architecture.md) —
  `DocumentSpec` v0.1 (20 blocos, Etapa 1 fechada).
- [`docs/modules/process-architecture.md`](../modules/process-architecture.md) —
  pendência 4 do ADR-0018 (§"Pendências para a próxima
  sessão" da 2B+X) sai.
- `PROMPT MESTRE` §4.3, §7.3, §16.3-§16.6, §22.5
