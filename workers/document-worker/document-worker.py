"""`document-worker` v0.3.0 - sidecar Python do Frederico IA Studio (Fase 5, Etapa 2B+Y).

Worker que gera documentos profissionais (DOCX, XLSX, PDF), le os tres
formatos e faz OCR de imagens e PDFs escaneados. Comunica com o app
principal via **named pipes** do Windows sobre o **envelope IPC** do
`frederico-process-architecture` (line-delimited JSON, 8 opcodes
estaveis em snake_case com prefixo de direcao: `worker.hello`,
`app.ack`, `app.ping`, `worker.pong`, `app.shutdown`, `worker.error`,
`tool.invoke`, `tool.result`).

## Protocolo (resumo do handshake)

1. Worker sobe, gera um `pipe_name` unico, cria o `NamedPipeServer`
   (via `pywin32.CreateNamedPipe`), e imprime `READY <pipe_name>` no
   **stdout** (handshake invertido, ADR-0017 Decisao 2).
2. Worker espera o app conectar (`ConnectNamedPipe`).
3. Worker envia `worker.hello` com o manifesto carregado de
   `manifest.json` (gera `request_id` UUID v4).
4. App responde `app.ack` com o `WorkerAuth` (token de curta duracao).
   Worker salva o token.
5. Loop: worker le linhas JSON do pipe, dispatch por `op`:
   - `app.ping` -> `worker.pong` com `status: "ok"`.
   - `app.shutdown` -> fecha o pipe e sai (manager detecta EOF).
   - `tool.invoke` -> valida token, dispatcha pro handler da
     `capability` declarada, e devolve `tool.result`.

## Handlers (Etapa 2B+Y - 7 primitivas)

| Capability   | Input                                       | Output                                              |
| ------------ | ------------------------------------------- | --------------------------------------------------- |
| `docx.write` | `path`, `title`, `sections`                 | `path`, `size_bytes`, `sections_written`            |
| `docx.read`  | `path`                                      | `paragraphs`, `tables`, contagens                   |
| `xlsx.write` | `path`, `sheets`                            | `path`, `size_bytes`, `sheets_written`              |
| `xlsx.read`  | `path` (opcional `sheet`)                   | `sheets`, `n_sheets`                                |
| `pdf.write`  | `path`, `title`, `sections`                 | `path`, `size_bytes`, `pages_rendered`              |
| `pdf.read`   | `path`, `ocr: "auto"|"never"|"only"`        | `text`, `ocr_text`, `page_count`, `scanned_pages`, `ocr_available`, `ocr_truncated`, `tesseract_version` |
| `ocr.run`    | `path`, `lang: "por+eng"` (opcional)        | `text`, `lang`, `conf`, `tesseract_version`         |

**`pdf.read` ganhou fallback OCR (Etapa 2B+Y, ADR-0019 §Decisao 3):**
- `text` so vem da camada de texto do PDF (fonte: `pdfplumber`).
- `ocr_text` e um mapa `{page_num: texto_ocr}` separado, populado
  apenas pra paginas escaneadas (sem camada de texto) **quando o
  OCR foi rodado**. **Nunca** mistura com o `text` - procedencia
  sempre clara (mesma disciplina do `origin`/`external_content`
  da memoria). OCR troca 8 por B, 0 por O, 1 por l - e exatamente
  em CNPJ/valor/competencia que o erro cai. Misturar no mesmo campo
  apagaria a procedencia.
- `ocr: "auto"` (default): fallback transparente - se ha scanned
  pages e Tesseract esta disponivel, faz OCR delas e popula
  `ocr_text`. `ocr: "never"`: rapido, so checa camada de texto.
  `ocr: "only"`: ignora camada de texto e faz OCR de TODAS as
  paginas.
- `ocr_truncated: true` quando o teto de paginas/timeout foi
  atingido (PDF escaneado de 200 paginas nao trava o worker).
- `tesseract_version` no retorno = reprodutibilidade. Quando um
  resultado de OCR for questionado daqui a tres meses, esse campo
  permite reproduzir.

**`ocr.run` (Etapa 2B+Y):** OCR de uma imagem (PNG/JPG/TIFF/BMP) via
Tesseract. `lang` e validado com regex estrita (`^[a-z]{3}(+[a-z]{3})*$`)
e contra os traineddata realmente instalados - erro estruturado se
o idioma nao existe, em vez de mensagem criptica do Tesseract.

## Path safety

Barreira minima (Etapa 7 - sandbox-runner - traz sandbox de OS):
- Path nao pode conter `..` como componente.
- Path absoluto ou relativo ao `cwd` do worker.
- Diretorio pai (write) ou arquivo (read) deve existir e ser gravavel
  ou legivel respectivamente.

A camada mais forte (allowlist de diretorios por tool) entra na Etapa 3
junto com o `ToolManifest::allowed_paths` - registrada como pendencia
no `docs/modules/process-architecture.md`.

## Decisao de design: handler = primitiva, nao renderer de DocumentSpec

ADR-0018 §Decisao 1. Os 7 handlers sao primitivas de I/O sobre as
bibliotecas Python (`python-docx`, `openpyxl`, `reportlab`,
`pdfplumber`, `pytesseract`). Eles **nao** decidem margem, fonte,
cor, numeracao de pagina, header/footer, etc. - isso e trabalho do
**kit** (`WordPro`/`ExcelPro`/`PdfPro`, Etapa 3) que recebe o
`DocumentSpec` declarativo e traduz pra esses handlers. A v0.3.0 e
deliberadamente feia em tipografia; a beleza visual e o trabalho do
kit.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path, PurePath
from typing import Any, Callable

# `pywin32` e instalado pelo `bootstrap.ps1` (ADR-0004). Try/except
# claro pra que o erro apareca no stderr do worker, nao no app.
try:
    import win32pipe  # type: ignore[import-untyped]
    import win32file  # type: ignore[import-untyped]
    import pywintypes  # type: ignore[import-untyped]
except ImportError as exc:
    print(
        f"[document-worker] ERRO: pywin32 nao esta instalado ({exc}). "
        "Rode o bootstrap.ps1 pra instalar Python + pywin32 em runtime/. "
        "Ver ADR-0004.",
        file=sys.stderr,
        flush=True,
    )
    raise SystemExit(2)

# Bibliotecas dos handlers (instaladas pelo bootstrap.ps1, ADR-0018
# Decisao 2a). Falta de qualquer uma = worker nao sobe.
try:
    import docx  # python-docx
    import openpyxl
    from reportlab.lib.pagesizes import A4
    from reportlab.lib.styles import ParagraphStyle
    from reportlab.lib.units import cm
    from reportlab.pdfbase import pdfmetrics
    from reportlab.pdfbase.ttfonts import TTFont
    from reportlab.platypus import (
        Paragraph,
        SimpleDocTemplate,
        Spacer,
    )
    import pdfplumber
except ImportError as exc:
    print(
        f"[document-worker] ERRO: biblioteca faltando ({exc}). "
        "Rode o bootstrap.ps1 pra instalar as dependencias. "
        "Ver ADR-0018 Decisao 2a.",
        file=sys.stderr,
        flush=True,
    )
    raise SystemExit(3)

# `pytesseract` (wrapper Python do Tesseract) e instalado pelo
# `bootstrap.ps1` (ADR-0019). Diferente das outras libs: a falta dele
# NAO impede o worker de subir - os handlers `docx`/`xlsx`/`pdf.write`
# continuam funcionando. So o `ocr.run` e o fallback OCR do `pdf.read`
# ficam indisponiveis. O `worker.hello` carrega `ocr_available: false`
# e o `pdf.read` retorna `code: "ocr_not_available"` quando
# alguem chama com `ocr: "auto"`/`"only"`. O caller (kit) decide se
# trata como erro ou segue.
try:
    import pytesseract  # type: ignore[import-untyped]
    from PIL import Image  # type: ignore[import-untyped]
    PYTESSERACT_AVAILABLE = True
except ImportError:
    pytesseract = None  # type: ignore[assignment]
    Image = None  # type: ignore[assignment]
    PYTESSERACT_AVAILABLE = False

# Versao do envelope IPC - bump MAJOR em mudancas incompativeis
# (mesmo numero que `IpcMessage::current_protocol_version()` no Rust).
PROTOCOL_VERSION: int = 1

# Tamanho do buffer de leitura (bytes).
READ_BUFFER_SIZE: int = 4096

# ---------------------------------------------------------------------------
# OCR (Tesseract + pytesseract) - Etapa 2B+Y, ADR-0019
# ---------------------------------------------------------------------------
#
# Diretorio do Tesseract binary (instalado pelo `bootstrap.ps1`).
# O `pytesseract` wrapper aceita `tesseract_cmd` apontando pro binario
# e `TESSDATA_PREFIX` apontando pro tessdata/ - definimos no startup
# (no `worker_main`) pra que esteja correto antes de qualquer
# chamada de OCR.
RUNTIME_TESSERACT_DIR = Path(__file__).resolve().parent / "runtime" / "tesseract"
TESSERACT_EXE = RUNTIME_TESSERACT_DIR / "tesseract.exe"
TESSERACT_TESSDATA_DIR = RUNTIME_TESSERACT_DIR / "tessdata"

# Idioma OCR default pro `ocr.run` (Tinta e Latao = Brasil; por sozinho
# perde em texto com termos em ingles, por+eng e mais robusto pro caso
# real - ADR-0019 §Decisao 2). O `pdf.read` no fallback automatico usa
# `por` sozinho (contexto brasileiro, sem lixo de outro idioma).
DEFAULT_OCR_LANG = "por+eng"
PDF_FALLBACK_OCR_LANG = "por"

# Idiomas que o bootstrap instalou (tessdata_fast 4.1.0, SHA-256 fixo).
# O handler valida o `lang` recebido contra esta lista - erro
# estruturado se faltar (em vez de mensagem criptica do Tesseract).
INSTALLED_OCR_LANGS = frozenset({"por", "eng", "osd"})

# Teto de paginas/timeout pro OCR automatico do `pdf.read`. Sem isso,
# um PDF escaneado de 200 paginas trava o worker por minutos. Quando
# bate o teto, o handler devolve o que ja processou e marca
# `ocr_truncated: true` no payload - o caller decide se aceita
# parcial ou aborta. **Tuning:** 20 paginas * 30s = 10 min maximo
# absoluto de OCR por `pdf.read` (ainda muito, mas dentro do timeout
# do manager de 60s por invoke? NAO. Ajustar pra caber.)
MAX_OCR_PAGES_PDF = 20
OCR_TIMEOUT_S_PER_PAGE = 30

# Versao do Tesseract. Calculada no startup (`worker_main`) e
# cacheada aqui. `None` se o Tesseract nao esta instalado (binario
# nao apareceu). Incluida em todo `ocr.run`/`pdf.read` com OCR
# no payload - reprodutibilidade (3 meses depois: "qual versao do
# Tesseract produziu esse texto?").
TESSERACT_VERSION: str | None = None
TESSERACT_VERSION_DETECTED_AT: float = 0.0

# Timeout do `ConnectNamedPipe` (ms). O `kill_on_drop` no manager
# Rust garante cleanup se o app travar antes de conectar.
CONNECT_TIMEOUT_MS: int = 60_000

# Diretorio de fontes Tinta e Latao. Auto-load no startup. Fallback
# pra fontes built-in do reportlab (Helvetica/Times-Roman) se nao
# encontrar.
RUNTIME_FONTS_DIR = Path(__file__).resolve().parent / "runtime" / "fonts"
WINDOWS_FONTS_DIR = Path(os.environ.get("WINDIR", "C:\\Windows")) / "Fonts"

# Mapeamento canonico das fontes da identidade visual "Tinta e Latao"
# (Adobe Source Sans 3 + Source Serif 4, ADR-0018 Decisao 2b). Esses
# nomes sao usados internamente no `reportlab.pdfmetrics` e nas
# `ParagraphStyle` do `pdf.write`.
FONT_BODY_NAME = "TintaLataoSans"      # Source Sans 3 - corpo
FONT_TITLE_NAME = "TintaLataoSerif"    # Source Serif 4 - titulos

FONT_FILES = {
    FONT_BODY_NAME: [
        RUNTIME_FONTS_DIR / "SourceSans3VF-Upright.ttf",
        RUNTIME_FONTS_DIR / "SourceSans3VF-Italic.ttf",
        WINDOWS_FONTS_DIR / "SourceSans3VF-Upright.ttf",  # fallback improvavel
    ],
    FONT_TITLE_NAME: [
        RUNTIME_FONTS_DIR / "SourceSerif4Variable-Roman.ttf",
        RUNTIME_FONTS_DIR / "SourceSerif4Variable-Italic.ttf",
        WINDOWS_FONTS_DIR / "SourceSerif4Variable-Roman.ttf",
    ],
}

# Logging basico. Vai pro stderr, que o `WorkerManager::spawn_external`
# (Rust) captura e loga via `tracing::warn!` (ver
# `crates/process-architecture/src/external.rs` §"Stderr pump").
logging.basicConfig(
    level=logging.INFO,
    format="[document-worker] %(asctime)s %(levelname)s %(message)s",
    stream=sys.stderr,
)
log = logging.getLogger("document-worker")


# ---------------------------------------------------------------------------
# IPC envelope (espelha `frederico-process-architecture::protocol::IpcMessage`)
# ---------------------------------------------------------------------------


def ipc_message(op: str, payload: dict[str, Any], auth: str | None = None, request_id: str | None = None) -> bytes:
    """Serializa uma `IpcMessage` como **uma linha** (line-delimited JSON).

    O `\\n` no final e o separador de mensagens. O
    `IpcMessage::decode_line` (Rust) faz `strip_suffix(b"\\\\n")` -
    sem o newline o decode falha.

    **request_id:** se passado, e ecoado no envelope (response do
    `tool.invoke` deve carregar o mesmo `request_id` do request - e
    o que o `WorkerManager` (ator) usa pra casar a response com o
    `pending` que gerou. Bug sutil na Etapa 2B original que gerava
    UUID novo na response - fazia o invoke timeoutar. O
    `worker_stub` Rust (Etapa 2B continuacao) ja fazia certo.
    """
    msg = {
        "protocol_version": PROTOCOL_VERSION,
        "request_id": request_id if request_id is not None else str(uuid.uuid4()),
        "op": op,
        "payload": payload,
    }
    if auth is not None:
        msg["auth"] = auth
    return (json.dumps(msg, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8")


def decode_line(line: bytes) -> dict[str, Any]:
    """Desserializa uma linha JSON. Valida `protocol_version`.

    Lanca `ValueError` em payload malformado ou versao errada.
    O `IpcMessage::decode_line` (Rust) faz o mesmo.
    """
    if line.endswith(b"\r\n"):
        line = line[:-2]
    elif line.endswith(b"\n"):
        line = line[:-1]
    msg = json.loads(line.decode("utf-8"))
    pv = msg.get("protocol_version")
    if pv != PROTOCOL_VERSION:
        raise ValueError(
            f"protocol_version {pv} nao e a atual {PROTOCOL_VERSION}"
        )
    return msg


# ---------------------------------------------------------------------------
# Path safety
# ---------------------------------------------------------------------------


class PathSafetyError(Exception):
    """Lancada quando um path falha a validacao minima (Etapa 2B+X)."""

    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def validate_path(path_str: str, kind: str) -> Path:
    """Valida um path de I/O. `kind` e "read" ou "write".

    Regras (barreira minima - a forte vem na Etapa 3 com
    `ToolManifest::allowed_paths` + Etapa 7 com sandbox-runner):
      1. Nao pode ser vazio.
      2. Nao pode conter `..` como componente.
      3. Deve ser absoluto, ou relativo ao `cwd` do worker.
      4. Para `kind == "write"`: o diretorio pai deve existir e ser
         gravavel. O arquivo pode nao existir ainda.
      5. Para `kind == "read"`: o arquivo deve existir e ser legivel.
    """
    if not path_str or not isinstance(path_str, str):
        raise PathSafetyError("invalid_path", "path ausente ou nao-string")
    p = PurePath(path_str)
    if ".." in p.parts:
        raise PathSafetyError(
            "path_traversal",
            f"path contem '..': {path_str!r}",
        )
    # Resolver para absoluto (mantem o que ja e absoluto).
    abs_path = Path(path_str).resolve() if Path(path_str).is_absolute() else (Path.cwd() / path_str).resolve()
    if kind == "write":
        parent = abs_path.parent
        if not parent.is_dir():
            raise PathSafetyError(
                "parent_missing",
                f"diretorio pai nao existe: {parent}",
            )
        # Teste de gravabilidade: cria e deleta um arquivo temporario.
        # Pode falhar por permissao - tratamos como erro.
        try:
            test_file = parent / ".frederico_write_probe"
            test_file.touch()
            test_file.unlink()
        except OSError as exc:
            raise PathSafetyError(
                "parent_not_writable",
                f"diretorio pai nao gravavel: {parent} ({exc})",
            ) from exc
        return abs_path
    elif kind == "read":
        if not abs_path.is_file():
            raise PathSafetyError(
                "file_missing",
                f"arquivo nao existe: {abs_path}",
            )
        if not os.access(abs_path, os.R_OK):
            raise PathSafetyError(
                "file_not_readable",
                f"arquivo nao legivel: {abs_path}",
            )
        return abs_path
    else:
        raise PathSafetyError("invalid_kind", f"kind deve ser 'read' ou 'write', veio {kind!r}")


# ---------------------------------------------------------------------------
# Fontes "Tinta e Latao" - auto-load no startup
# ---------------------------------------------------------------------------

_FONTS_REGISTERED = False
_FONT_STATUS: dict[str, str] = {}


def _try_register_font(name: str, candidates: list[Path]) -> bool:
    """Tenta registrar uma fonte TTF. Retorna True se conseguiu."""
    for path in candidates:
        if path.is_file() and path.stat().st_size > 50_000:
            try:
                pdfmetrics.registerFont(TTFont(name, str(path)))
                log.info("fonte %s registrada de %s", name, path)
                return True
            except Exception as exc:
                log.warning("falha registrando %s de %s: %s", name, path, exc)
    return False


def ensure_fonts_registered() -> dict[str, str]:
    """Garante que as fontes Tinta e Latao estao registradas.

    Retorna um dicionario `name -> "loaded" | "fallback"` pra incluir
    no `worker.pong` payload e em `worker.hello` (campo extra). O
    `pdf.write` consulta esse mesmo estado.

    **Bug fix v0.2.0:** a versao inicial retornava `{}` apos o
    primeiro registro (o `status` so era populado dentro do `if`).
    Isso fazia o `pong` reportar `font_status: {}` depois do boot,
    e os testes E2E nao conseguiam validar que as TTFs tinham
    carregado. Agora mantemos o status em cache no module-level e
    retornamos o memo.
    """
    global _FONTS_REGISTERED, _FONT_STATUS
    if _FONTS_REGISTERED:
        return _FONT_STATUS
    status: dict[str, str] = {}
    for name, candidates in FONT_FILES.items():
        if _try_register_font(name, candidates):
            status[name] = "loaded"
        else:
            # Fallback: reportlab ja tem Helvetica e Times-Roman
            # built-in. Mapear nomes canonicos pros built-ins
            # garante que o `pdf.write` nao quebra se as TTFs
            # nao estiverem instaladas.
            if name == FONT_BODY_NAME:
                pdfmetrics.registerFontFamily(name, normal="Helvetica", bold="Helvetica-Bold", italic="Helvetica-Oblique", bold_italic="Helvetica-BoldOblique")
            else:
                pdfmetrics.registerFontFamily(name, normal="Times-Roman", bold="Times-Bold", italic="Times-Italic", bold_italic="Times-BoldItalic")
            status[name] = "fallback"
            log.warning("fonte %s NAO encontrada - usando fallback built-in do reportlab", name)
    _FONTS_REGISTERED = True
    _FONT_STATUS = status
    return status


# ---------------------------------------------------------------------------
# OCR (Tesseract via pytesseract) - Etapa 2B+Y, ADR-0019
# ---------------------------------------------------------------------------
#
# Wrappers finos em torno do `pytesseract`:
#   - _get_tesseract_version(): detecta versao no startup, cacheia.
#   - _validate_lang(lang): regex estrita + checa contra INSTALLED_OCR_LANGS.
#   - _ocr_image_to_text(image_path, lang): OCR de uma imagem (PNG/JPG/TIFF/BMP).
#   - _ocr_pdf_page_to_text(pdf_path, page_num, lang): OCR de 1 pagina
#     de um PDF, via render com pdfplumber (nao precisa de Poppler).
#
# Por que regex estrita no lang: o valor vem do chamador e vira argumento
# de linha de comando do `tesseract.exe`. Sem validacao, um chamador
# hostil (ou buggy) pode injetar `; rm -rf /` ou coisa pior. Mesma
# disciplina da barreira de path dos outros handlers - o manager
# Rust nao tem como auditar isso, o handler Python tem.
#
# Por que `TESSERACT_VERSION` em module-level (e nao em `worker_main`):
# o handler e chamado em qualquer momento depois do startup. Calcular
# no `worker_main` e cachear evita spawnar `tesseract.exe --version`
# em todo handler (overhead ~50ms cada).
import re
_LANG_PATTERN = re.compile(r"^[a-z]{3}(\+[a-z]{3})*$")


def _tesseract_executable_present() -> bool:
    """Retorna True se `tesseract.exe` existe em runtime/tesseract/."""
    return TESSERACT_EXE.is_file() and TESSERACT_EXE.stat().st_size > 100_000


def _get_tesseract_version() -> str | None:
    """Detecta a versao do Tesseract. Cacheia em module-level.

    Retorna `None` se Tesseract nao esta instalado (bootstrap pulou
    em contexto non-admin, ou falhou). O chamador (handler `ocr.run`
    ou `pdf.read` com `ocr: "auto"`) decide como tratar.
    """
    global TESSERACT_VERSION, TESSERACT_VERSION_DETECTED_AT
    if TESSERACT_VERSION is not None or TESSERACT_VERSION_DETECTED_AT > 0:
        return TESSERACT_VERSION
    if not _tesseract_executable_present():
        TESSERACT_VERSION_DETECTED_AT = time.time()
        return None
    try:
        proc = subprocess.run(
            [str(TESSERACT_EXE), "--version"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if proc.returncode != 0:
            log.warning("tesseract --version saiu com codigo %d", proc.returncode)
            TESSERACT_VERSION_DETECTED_AT = time.time()
            return None
        # Output esperado: "tesseract 5.4.0.20240606\n ..." (primeira linha).
        first_line = proc.stdout.strip().splitlines()[0] if proc.stdout else ""
        # Pega so o token "5.4.0.20240606" (sem "tesseract ").
        m = re.search(r"(\d+\.\d+\.\d+(?:\.\d+)?)", first_line)
        TESSERACT_VERSION = m.group(1) if m else first_line
        TESSERACT_VERSION_DETECTED_AT = time.time()
        log.info("Tesseract version detectada: %s", TESSERACT_VERSION)
        return TESSERACT_VERSION
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError) as exc:
        log.warning("falha detectando versao do Tesseract: %s", exc)
        TESSERACT_VERSION_DETECTED_AT = time.time()
        return None


def _validate_lang(lang: str) -> str:
    """Valida o `lang` recebido no `ocr.run`. Retorna o `lang` normalizado.

    Regras (ADR-0019 §Decisao 2.5 - defesa contra injecao de argumento):
      1. Regex `^[a-z]{3}(+[a-z]{3})*$` - so segmentos de 3 letras
         minusculas, opcionalmente concatenados por `+`.
      2. Cada segmento tem que estar em `INSTALLED_OCR_LANGS` (o
         conjunto de traineddata realmente instalados).

    Lanca `ValueError` com mensagem clara listando os idiomas
    disponiveis (em vez da mensagem criptica do Tesseract quando
    o idioma nao existe).
    """
    if not isinstance(lang, str) or not _LANG_PATTERN.match(lang):
        raise ValueError(
            f"lang invalido: {lang!r}. Esperado segmentos de 3 letras "
            f"minusculas unidos por '+'. Exemplo: 'por', 'eng', 'por+eng'."
        )
    parts = lang.split("+")
    missing = [p for p in parts if p not in INSTALLED_OCR_LANGS]
    if missing:
        raise ValueError(
            f"idioma OCR nao instalado: {missing!r}. "
            f"Idiomas disponiveis: {sorted(INSTALLED_OCR_LANGS)}. "
            f"Re-adicione via bootstrap.ps1 (tag do tessdata + SHA-256 fixos)."
        )
    return lang


def _configure_pytesseract() -> None:
    """Configura `pytesseract` no startup do worker. Idempotente.

    Define `tesseract_cmd` (path do binario) e `TESSDATA_PREFIX` (env
    var que o tesseract consulta pra localizar `*.traineddata`).
    """
    if not PYTESSERACT_AVAILABLE:
        return
    if _tesseract_executable_present():
        pytesseract.pytesseract.tesseract_cmd = str(TESSERACT_EXE)
        # `TESSDATA_PREFIX` precisa ser setado **no env do processo**
        # (pytesseract le via os.environ no subprocess) e tambem
        # no fallback via config_data_dir. Setamos ambos pra ser
        # defensivo contra diferencas entre versoes do pytesseract.
        os.environ["TESSDATA_PREFIX"] = str(TESSERACT_TESSDATA_DIR)
        pytesseract.pytesseract.config_data_dir = str(TESSERACT_TESSDATA_DIR)


def _ocr_pil_image(img, lang: str, timeout_s: int = 60) -> dict:
    """OCR de um objeto PIL.Image. Retorna `{"text", "conf"}`."""
    data = pytesseract.image_to_data(
        img,
        lang=lang,
        output_type=pytesseract.Output.DICT,
        timeout=timeout_s,
    )
    words = []
    confs = []
    for i, txt in enumerate(data.get("text", [])):
        t = (txt or "").strip()
        if not t:
            continue
        words.append(t)
        try:
            c = int(data["conf"][i])
        except (ValueError, TypeError):
            continue
        if c >= 0:
            confs.append(c)
    text = " ".join(words)
    mean_conf = (sum(confs) / len(confs)) if confs else None
    return {"text": text, "conf": mean_conf}


def _ocr_image_to_text(image_path: Path, lang: str, timeout_s: int = 60) -> dict:
    """OCR de uma imagem em disco. Retorna `{"text", "conf"}`."""
    if not PYTESSERACT_AVAILABLE:
        raise RuntimeError("pytesseract nao esta instalado")
    if not _tesseract_executable_present():
        raise RuntimeError(
            f"Tesseract nao encontrado em {TESSERACT_EXE}. "
            "Rode o bootstrap.ps1 como Admin (veja instrucoes no script)."
        )
    if not image_path.is_file():
        raise FileNotFoundError(f"imagem nao encontrada: {image_path}")
    try:
        img = Image.open(str(image_path))
    except Exception as exc:
        raise RuntimeError(f"falha abrindo imagem: {exc}") from exc
    try:
        return _ocr_pil_image(img, lang, timeout_s)
    except pytesseract.TesseractError as exc:
        raise RuntimeError(f"Tesseract falhou: {exc}") from exc
    except RuntimeError as exc:
        # pytesseract 0.3.10 levanta RuntimeError em timeout.
        if "timeout" in str(exc).lower():
            raise RuntimeError(f"OCR excedeu timeout de {timeout_s}s") from exc
        raise


def _ocr_pdf_page_to_text(
    pdf_path: Path, page_num: int, lang: str, timeout_s: int = 60
) -> str:
    """OCR de UMA pagina especifica de um PDF. Retorna o texto.

    Renderiza a pagina como imagem via `pdfplumber` (ja temos) +
    Pillow (ja temos), e chama Tesseract. Sem dependencia extra
    de Poppler/pdf2image.
    """
    if not PYTESSERACT_AVAILABLE:
        raise RuntimeError("pytesseract nao esta instalado")
    if not _tesseract_executable_present():
        raise RuntimeError("Tesseract nao encontrado")
    if not pdf_path.is_file():
        raise FileNotFoundError(f"PDF nao encontrado: {pdf_path}")
    with pdfplumber.open(str(pdf_path)) as pdf:
        if page_num < 1 or page_num > len(pdf.pages):
            raise ValueError(
                f"page_num {page_num} fora de [1, {len(pdf.pages)}]"
            )
        page = pdf.pages[page_num - 1]
        # `to_image(resolution=300)` e o padrao de OCR. Devolve
        # um `PIL.Image` que o pytesseract aceita direto.
        img = page.to_image(resolution=300).original
    return _ocr_pil_image(img, lang, timeout_s)["text"]


# ---------------------------------------------------------------------------
# Handlers
# ---------------------------------------------------------------------------
#
# Convenção: cada handler recebe `payload: dict` e devolve `dict`
# (vai direto pro `tool.result.payload`). Se algo falhar, o handler
# lanca uma excecao; o dispatch (no main loop) converte pra
# `worker.error` com code/message.


def _payload_field(payload: dict, key: str, expected_type: type) -> Any:
    """Extrai campo obrigatorio do payload com type-check."""
    if key not in payload:
        raise ValueError(f"campo obrigatorio ausente: {key!r}")
    val = payload[key]
    if not isinstance(val, expected_type):
        raise ValueError(
            f"campo {key!r} esperado {expected_type.__name__}, "
            f"veio {type(val).__name__}"
        )
    return val


# ---- docx.write -----------------------------------------------------------


def handle_docx_write(payload: dict) -> dict:
    """docx.write: escreve um arquivo .docx com `title` + `sections`.

    Input: `{"path": str, "title": str, "sections": [{"heading": str, "paragraphs": [str]}]}`
    Output: `{"ok": true, "path": str, "size_bytes": int, "sections_written": int}`
    """
    path = validate_path(_payload_field(payload, "path", str), "write")
    title = _payload_field(payload, "title", str)
    sections = _payload_field(payload, "sections", list)

    document = docx.Document()
    document.core_properties.title = title
    document.add_heading(title, level=0)
    sections_written = 0
    for sec in sections:
        if not isinstance(sec, dict):
            raise ValueError("secao precisa ser um dict")
        heading = sec.get("heading", "")
        paragraphs = sec.get("paragraphs", [])
        if not isinstance(paragraphs, list):
            raise ValueError("'paragraphs' precisa ser uma lista de strings")
        if heading:
            document.add_heading(heading, level=1)
        for p in paragraphs:
            if not isinstance(p, str):
                raise ValueError("paragrafo precisa ser string")
            document.add_paragraph(p)
        sections_written += 1
    document.save(str(path))
    size = path.stat().st_size
    return {
        "ok": True,
        "path": str(path),
        "size_bytes": size,
        "sections_written": sections_written,
    }


# ---- docx.read ------------------------------------------------------------


def handle_docx_read(payload: dict) -> dict:
    """docx.read: extrai paragrafos e tabelas de um .docx.

    Input: `{"path": str}`
    Output: `{"ok": true, "paragraphs": [str], "tables": [[str]], "n_paragraphs": int, "n_tables": int}`
    """
    path = validate_path(_payload_field(payload, "path", str), "read")
    document = docx.Document(str(path))
    paragraphs = [p.text for p in document.paragraphs]
    tables = []
    for t in document.tables:
        rows = []
        for row in t.rows:
            rows.append([cell.text for cell in row.cells])
        tables.append(rows)
    return {
        "ok": True,
        "path": str(path),
        "paragraphs": paragraphs,
        "tables": tables,
        "n_paragraphs": len(paragraphs),
        "n_tables": len(tables),
    }


# ---- xlsx.write -----------------------------------------------------------


def handle_xlsx_write(payload: dict) -> dict:
    """xlsx.write: escreve um arquivo .xlsx com 1+ sheets.

    Input: `{"path": str, "sheets": [{"name": str, "headers": [str], "rows": [[]]}]}`
    Output: `{"ok": true, "path": str, "size_bytes": int, "sheets_written": int, "total_rows": int}`
    """
    path = validate_path(_payload_field(payload, "path", str), "write")
    sheets = _payload_field(payload, "sheets", list)
    wb = openpyxl.Workbook()
    # openpyxl cria uma sheet default "Sheet" - removemos e adicionamos as nossas.
    default = wb.active
    wb.remove(default)
    total_rows = 0
    sheets_written = 0
    for sh in sheets:
        if not isinstance(sh, dict):
            raise ValueError("sheet precisa ser um dict")
        name = sh.get("name", "")
        if not name:
            raise ValueError("sheet sem nome")
        headers = sh.get("headers", [])
        rows = sh.get("rows", [])
        if not isinstance(headers, list) or not isinstance(rows, list):
            raise ValueError("'headers' e 'rows' precisam ser listas")
        ws = wb.create_sheet(title=name)
        if headers:
            ws.append(headers)
        for row in rows:
            ws.append(row)
        total_rows += len(rows)
        sheets_written += 1
    wb.save(str(path))
    return {
        "ok": True,
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "sheets_written": sheets_written,
        "total_rows": total_rows,
    }


# ---- xlsx.read ------------------------------------------------------------


def handle_xlsx_read(payload: dict) -> dict:
    """xlsx.read: le um .xlsx e devolve sheets + dados.

    Input: `{"path": str, "sheet": str?}` - `sheet` filtra uma so sheet
    (opcional; default = todas).
    Output: `{"ok": true, "sheets": [{"name": str, "headers": [str], "rows": [[]]}], "n_sheets": int}`
    """
    path = validate_path(_payload_field(payload, "path", str), "read")
    sheet_filter = payload.get("sheet")
    if sheet_filter is not None and not isinstance(sheet_filter, str):
        raise ValueError("'sheet' precisa ser string")
    wb = openpyxl.load_workbook(str(path), read_only=True, data_only=True)
    sheets_out = []
    for ws in wb.worksheets:
        if sheet_filter is not None and ws.title != sheet_filter:
            continue
        rows_list = list(ws.iter_rows(values_only=True))
        headers: list = []
        data_rows: list = []
        if rows_list:
            headers = [str(c) if c is not None else "" for c in rows_list[0]]
            data_rows = [
                [str(c) if c is not None else "" for c in row]
                for row in rows_list[1:]
            ]
        sheets_out.append({
            "name": ws.title,
            "headers": headers,
            "rows": data_rows,
        })
    wb.close()
    return {
        "ok": True,
        "path": str(path),
        "sheets": sheets_out,
        "n_sheets": len(sheets_out),
    }


# ---- pdf.write ------------------------------------------------------------


def _build_pdf_styles() -> dict[str, ParagraphStyle]:
    """Constroi os ParagraphStyle usados pelo pdf.write.

    Titulos em Serif, corpo em Sans (ADR-0018 Decisao 1 - Tinta e
    Latao). Tamanhos e cores sao deliberadamente simples - a
    tipografia fina e trabalho do kit (Etapa 3).
    """
    return {
        "title": ParagraphStyle(
            "TintaTitle",
            fontName=FONT_TITLE_NAME,
            fontSize=24,
            leading=28,
            spaceAfter=12,
        ),
        "heading": ParagraphStyle(
            "TintaHeading",
            fontName=FONT_TITLE_NAME,
            fontSize=16,
            leading=20,
            spaceBefore=10,
            spaceAfter=6,
        ),
        "body": ParagraphStyle(
            "TintaBody",
            fontName=FONT_BODY_NAME,
            fontSize=11,
            leading=15,
            spaceAfter=4,
        ),
    }


def handle_pdf_write(payload: dict) -> dict:
    """pdf.write: escreve um arquivo .pdf com `title` + `sections`.

    Input: `{"path": str, "title": str, "sections": [{"heading": str, "body": [str]}]}`
    Output: `{"ok": true, "path": str, "size_bytes": int, "pages_rendered": int, "sections_written": int}`
    """
    path = validate_path(_payload_field(payload, "path", str), "write")
    title = _payload_field(payload, "title", str)
    sections = _payload_field(payload, "sections", list)
    ensure_fonts_registered()  # idempotente

    styles = _build_pdf_styles()
    doc = SimpleDocTemplate(
        str(path),
        pagesize=A4,
        leftMargin=2 * cm,
        rightMargin=2 * cm,
        topMargin=2 * cm,
        bottomMargin=2 * cm,
        title=title,
    )
    story = [Paragraph(title, styles["title"]), Spacer(1, 0.5 * cm)]
    sections_written = 0
    for sec in sections:
        if not isinstance(sec, dict):
            raise ValueError("secao precisa ser um dict")
        heading = sec.get("heading", "")
        body = sec.get("body", [])
        if not isinstance(body, list):
            raise ValueError("'body' precisa ser uma lista de strings")
        if heading:
            story.append(Paragraph(heading, styles["heading"]))
        for p in body:
            if not isinstance(p, str):
                raise ValueError("paragrafo precisa ser string")
            story.append(Paragraph(p, styles["body"]))
        story.append(Spacer(1, 0.3 * cm))
        sections_written += 1
    doc.build(story)
    # Reportlab nao expoe contagem de paginas depois de build. Como
    # heuristica simples, o numero de paginas e pelo menos 1.
    pages_rendered = 1
    return {
        "ok": True,
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "pages_rendered": pages_rendered,
        "sections_written": sections_written,
    }


# ---- pdf.read -------------------------------------------------------------


def handle_ocr_run(payload: dict) -> dict:
    """ocr.run: OCR de uma imagem (PNG/JPG/TIFF/BMP) via Tesseract.

    Input: `{"path": str, "lang": str?}`
      - `path`: path da imagem (validado pelo `validate_path`).
      - `lang`: idiomas concatenados por `+`. Default: `por+eng`.
        Validado contra `INSTALLED_OCR_LANGS` - erro estruturado
        (nao mensagem criptica do Tesseract) se faltar.

    Output: `{"ok": true, "path": str, "text": str, "lang": str,
              "conf": float|None, "tesseract_version": str}`

    Erros comuns (em `tool.result {ok: false, code, message}`):
      - `ocr_not_available`: Tesseract nao instalado (bootstrap pulou).
      - `invalid_lang`: `lang` mal formado ou idioma nao instalado.
      - `tesseract_failed`: Tesseract retornou erro.
      - `ocr_timeout`: timeout de 60s excedido.
      - `image_not_found`: path nao existe.
    """
    path = validate_path(_payload_field(payload, "path", str), "read")
    lang = payload.get("lang", DEFAULT_OCR_LANG)
    if not isinstance(lang, str):
        lang = DEFAULT_OCR_LANG
    try:
        lang = _validate_lang(lang)
    except ValueError as exc:
        return {
            "ok": False,
            "code": "invalid_lang",
            "message": str(exc),
            "path": str(path),
        }

    if not PYTESSERACT_AVAILABLE:
        return {
            "ok": False,
            "code": "ocr_not_available",
            "message": (
                "pytesseract nao instalado. Rode o bootstrap.ps1 pra "
                "instalar (pula silenciosamente em contexto non-elevated)."
            ),
            "path": str(path),
        }
    if not _tesseract_executable_present():
        return {
            "ok": False,
            "code": "ocr_not_available",
            "message": (
                f"Tesseract nao encontrado em {TESSERACT_EXE}. "
                "Instale manualmente ou rode o bootstrap.ps1 como Admin."
            ),
            "path": str(path),
        }

    tess_version = _get_tesseract_version()
    try:
        out = _ocr_image_to_text(path, lang, timeout_s=60)
    except FileNotFoundError:
        return {
            "ok": False,
            "code": "image_not_found",
            "message": f"imagem nao encontrada: {path}",
            "path": str(path),
        }
    except RuntimeError as exc:
        msg = str(exc)
        if "timeout" in msg.lower():
            code = "ocr_timeout"
        elif "Tesseract falhou" in msg:
            code = "tesseract_failed"
        else:
            code = "ocr_error"
        log.warning("ocr.run falhou: %s (%s)", code, msg)
        return {
            "ok": False,
            "code": code,
            "message": msg,
            "path": str(path),
            "lang": lang,
            "tesseract_version": tess_version,
        }
    return {
        "ok": True,
        "path": str(path),
        "text": out["text"],
        "lang": lang,
        "conf": out["conf"],
        "tesseract_version": tess_version,
    }


# ---- pdf.read -------------------------------------------------------------


def handle_pdf_read(payload: dict) -> dict:
    """pdf.read: extrai texto de um .pdf e detecta paginas escaneadas.

    Input: `{"path": str, "ocr": "auto"|"never"|"only"?}`
      - `ocr` (default `"auto"`):
        - `"auto"`: se ha paginas escaneadas e Tesseract esta
          disponivel, faz OCR delas e popula `ocr_text`. Se 100%
          escaneado + OCR OK: devolve `ok: true` com `text` do OCR
          (precedencia: `text` = OCR, com `extraction: "ocr"`).
        - `"never"`: rapido, so checa camada de texto via `pdfplumber`.
        - `"only"`: ignora camada de texto e faz OCR de TODAS as
          paginas (mesmo as com texto).

    Output: `{"ok": true, "text": str, "ocr_text": {page: str},
              "page_count": int, "scanned_pages": [int],
              "ocr_available": bool, "ocr_truncated": bool,
              "extraction": "text"|"ocr"|"mixed",
              "tesseract_version": str|null}`

    **Procedencia sempre clara** (ADR-0019 §Decisao 3): `text` e
    `ocr_text` sao campos SEPARADOS. `text` so vem da camada do PDF;
    `ocr_text` e o mapa de OCR por pagina. Nunca mistura os dois -
    OCR troca 8 por B, 0 por O, 1 por l, e e exatamente em
    CNPJ/competencia/valor que o erro cai. Misturar apagaria a
    procedencia (mesma disciplina de `origin`/`external_content`
    da memoria).

    **Teto de paginas** (`MAX_OCR_PAGES_PDF` = 20): PDF escaneado
    de 200 paginas leva minutos e estoura o timeout do worker.
    Quando bate o teto, devolve parcial com `ocr_truncated: true`
    e segue (caller decide se aceita ou aborta).

    **Idioma do fallback automatico:** `PDF_FALLBACK_OCR_LANG` =
    `"por"` (contexto brasileiro, sem lixo de outro idioma; o
    chamador que sabe o idioma certo usa `ocr.run` direto).
    """
    path = validate_path(_payload_field(payload, "path", str), "read")
    ocr_mode = payload.get("ocr", "auto")
    if ocr_mode not in ("auto", "never", "only"):
        raise ValueError(
            f"'ocr' deve ser 'auto', 'never' ou 'only'; veio {ocr_mode!r}"
        )

    # 1. Extrai texto via pdfplumber (camada de texto do PDF).
    pages_text: list[str] = []
    scanned_pages: list[int] = []
    page_count = 0
    with pdfplumber.open(str(path)) as pdf:
        for i, page in enumerate(pdf.pages, start=1):
            text = page.extract_text() or ""
            text = text.strip()
            if not text:
                scanned_pages.append(i)
            pages_text.append(text)
        page_count = len(pages_text)

    full_text = "\n".join(pages_text)
    tess_version = _get_tesseract_version() if PYTESSERACT_AVAILABLE else None
    ocr_available = bool(
        PYTESSERACT_AVAILABLE
        and _tesseract_executable_present()
        and tess_version is not None
    )

    # 2. Decide se faz OCR.
    ocr_text: dict[int, str] = {}
    ocr_truncated = False
    pages_to_ocr: list[int] = []

    if ocr_mode == "only":
        # Ignora camada de texto, faz OCR de TODAS as paginas.
        pages_to_ocr = list(range(1, page_count + 1))
    elif ocr_mode == "auto" and scanned_pages and ocr_available:
        # Fallback transparente: so as paginas escaneadas.
        pages_to_ocr = list(scanned_pages)
    # ocr_mode == "never" => pages_to_ocr fica vazio.

    if pages_to_ocr and not ocr_available:
        # Caller pediu OCR mas nao temos. Devolve code estruturado
        # em vez de quebrar - o caller decide.
        return {
            "ok": False,
            "code": "ocr_not_available",
            "message": (
                f"`ocr: {ocr_mode}` pedido mas Tesseract/pytesseract "
                "nao disponivel. Rode o bootstrap.ps1 como Admin pra "
                "instalar. PDF continua legivel via camada de texto "
                "(se houver) - use `ocr: 'never'` pra forcar esse modo."
            ),
            "path": str(path),
            "page_count": page_count,
            "scanned_pages": scanned_pages,
            "ocr_available": False,
        }

    if pages_to_ocr:
        # 3. Teto de paginas.
        if len(pages_to_ocr) > MAX_OCR_PAGES_PDF:
            log.warning(
                "pdf.read: %d paginas pra OCR > teto %d - truncando",
                len(pages_to_ocr), MAX_OCR_PAGES_PDF,
            )
            ocr_truncated = True
            pages_to_ocr = pages_to_ocr[:MAX_OCR_PAGES_PDF]
        # 4. Faz OCR pagina a pagina. Falha de uma nao aborta as
        #    outras (best-effort).
        for page_num in pages_to_ocr:
            try:
                text = _ocr_pdf_page_to_text(
                    path, page_num, PDF_FALLBACK_OCR_LANG,
                    timeout_s=OCR_TIMEOUT_S_PER_PAGE,
                )
            except Exception as exc:
                log.warning(
                    "OCR da pagina %d de %s falhou: %s",
                    page_num, path, exc,
                )
                ocr_text[page_num] = ""  # marca como tentado
                ocr_truncated = True
                continue
            ocr_text[page_num] = text

    # 5. Monta o retorno. `extraction` diz qual caminho produziu
    #    o `text` - util quando o caller quiser auditar.
    if ocr_mode == "only" and ocr_text:
        # `text` agora e o OCR de TODAS as paginas (camada ignorada).
        all_text = "\n".join(
            ocr_text.get(i, "").strip() for i in range(1, page_count + 1)
        )
        return {
            "ok": True,
            "path": str(path),
            "text": all_text,
            "ocr_text": ocr_text,
            "page_count": page_count,
            "scanned_pages": [],  # nao faz sentido no modo "only"
            "ocr_available": ocr_available,
            "ocr_truncated": ocr_truncated,
            "extraction": "ocr",
            "tesseract_version": tess_version,
        }

    if scanned_pages and page_count > 0 and len(scanned_pages) == page_count:
        # 100% escaneado. Se Tesseract OK (ocr: "auto" + ocr_available
        # + pages_to_ocr nao vazio), o OCR rodou e o ocr_text tem as
        # paginas todas. Caso contrario, devolve code estruturado.
        if ocr_text and len(ocr_text) == page_count:
            all_text = "\n".join(
                ocr_text.get(i, "").strip() for i in range(1, page_count + 1)
            )
            return {
                "ok": True,
                "path": str(path),
                "text": all_text,
                "ocr_text": ocr_text,
                "page_count": page_count,
                "scanned_pages": scanned_pages,
                "ocr_available": ocr_available,
                "ocr_truncated": ocr_truncated,
                "extraction": "ocr",
                "tesseract_version": tess_version,
            }
        return {
            "ok": False,
            "code": "pdf_scanned_no_ocr",
            "message": (
                f"PDF 100% escaneado ({page_count} pagina(s) sem "
                "camada de texto) e OCR nao disponivel. Instale "
                "Tesseract via bootstrap.ps1 como Admin ou use "
                "`ocr: 'never'` se quiser confirmar a limitacao."
            ),
            "path": str(path),
            "page_count": page_count,
            "scanned_pages": scanned_pages,
            "ocr_available": ocr_available,
        }

    # Caso geral: PDF tem pelo menos uma pagina com camada de texto.
    # `text` = camada do PDF (fonte: pdfplumber). `ocr_text` separado
    # so com paginas escaneadas (se rodou OCR).
    if ocr_text:
        extraction = "mixed"
    else:
        extraction = "text"
    return {
        "ok": True,
        "path": str(path),
        "text": full_text,
        "ocr_text": ocr_text,
        "page_count": page_count,
        "scanned_pages": scanned_pages,
        "ocr_available": ocr_available,
        "ocr_truncated": ocr_truncated,
        "extraction": extraction,
        "tesseract_version": tess_version,
    }


# ---------------------------------------------------------------------------
# Dispatch table
# ---------------------------------------------------------------------------

HANDLERS: dict[str, Callable[[dict], dict]] = {
    "docx.write": handle_docx_write,
    "docx.read": handle_docx_read,
    "xlsx.write": handle_xlsx_write,
    "xlsx.read": handle_xlsx_read,
    "pdf.write": handle_pdf_write,
    "pdf.read": handle_pdf_read,
    "ocr.run": handle_ocr_run,
}


def handle_tool_invoke(
    msg: dict[str, Any],
    auth_token: str | None,
) -> bytes:
    """Dispatcha o `tool.invoke` pro handler da capability.

    Retorna os bytes da resposta (`tool.result` ou `worker.error`),
    carregando o mesmo `request_id` do request de entrada (o ator
    Rust casa por ele - ver `ipc_message`).
    """
    request_id = msg["request_id"]
    auth = msg.get("auth")
    # Validacao de token (so se ja vimos um `app.ack`).
    if auth_token is not None and auth != auth_token:
        return ipc_message(
            "worker.error",
            {
                "code": "process_unauthorized",
                "message": "token ausente ou invalido",
            },
            request_id=request_id,
        )
    payload_in = msg.get("payload", {})
    if not isinstance(payload_in, dict):
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": "invalid_payload",
                "message": f"payload precisa ser dict, veio {type(payload_in).__name__}",
            },
            request_id=request_id,
            auth=auth,
        )
    capability = payload_in.get("capability", "")
    if not capability:
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": "missing_capability",
                "message": "payload.capability ausente",
            },
            request_id=request_id,
            auth=auth,
        )
    handler = HANDLERS.get(capability)
    if handler is None:
        declared = sorted(HANDLERS.keys())
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": "unknown_capability",
                "message": (
                    f"document-worker v0.2.0 nao implementa "
                    f"{capability!r}. Capabilities declaradas: {declared}. "
                    f"`ocr.run` entra na Etapa 2B+Y."
                ),
            },
            request_id=request_id,
            auth=auth,
        )
    try:
        result = handler(payload_in)
    except PathSafetyError as exc:
        log.warning("path safety falhou: %s (%s)", exc.code, exc.message)
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": exc.code,
                "message": exc.message,
            },
            request_id=request_id,
            auth=auth,
        )
    except ValueError as exc:
        log.warning("input invalido em %s: %s", capability, exc)
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": "invalid_input",
                "message": str(exc),
            },
            request_id=request_id,
            auth=auth,
        )
    except Exception as exc:  # noqa: BLE001 - borda do dispatch, queremos worker.error
        log.exception("handler %s falhou", capability)
        return ipc_message(
            "tool.result",
            {
                "ok": False,
                "code": "handler_error",
                "message": f"{type(exc).__name__}: {exc}",
            },
            request_id=request_id,
            auth=auth,
        )
    log.info("handler %s ok: %s", capability, {k: result.get(k) for k in ("ok", "path", "size_bytes") if k in result})
    return ipc_message("tool.result", result, request_id=request_id, auth=auth)


# ---------------------------------------------------------------------------
# Loop principal do worker
# ---------------------------------------------------------------------------


def load_manifest(manifest_path: Path) -> dict[str, Any]:
    """Carrega `manifest.json` ao lado do script."""
    with open(manifest_path, encoding="utf-8") as f:
        return json.load(f)


def worker_main(manifest_path: Path) -> int:
    """Loop principal: cria pipe, espera connect, dispatch."""
    manifest = load_manifest(manifest_path)
    worker_id = manifest["worker_id"]
    log.info("subindo %s %s", worker_id, manifest.get("version", "?"))

    # Carrega as fontes Tinta e Latao no startup (uma vez so).
    font_status = ensure_fonts_registered()
    log.info("fontes: %s", font_status)

    # Configura pytesseract (tesseract_cmd + TESSDATA_PREFIX) e
    # detecta a versao do Tesseract no startup. Idempotente.
    _configure_pytesseract()
    tess_version = _get_tesseract_version()
    ocr_available = (
        PYTESSERACT_AVAILABLE
        and _tesseract_executable_present()
        and tess_version is not None
    )
    log.info(
        "Tesseract: versao=%s disponivel=%s pytesseract=%s",
        tess_version, ocr_available, PYTESSERACT_AVAILABLE,
    )

    # 1. Gera nome unico pro pipe. O `<name>` e a parte depois de
    #    `\\\\.\\pipe\\`. O `PipeName::new` (Rust) valida.
    pipe_name = f"frederico-{worker_id}-{uuid.uuid4().hex[:12]}"
    pipe_path = rf"\\.\pipe\{pipe_name}"

    # 2. Cria o NamedPipeServer. `maxInstances=1` (so o app principal).
    try:
        pipe_handle = win32pipe.CreateNamedPipe(
            pipe_path,
            win32pipe.PIPE_ACCESS_DUPLEX,
            win32pipe.PIPE_TYPE_BYTE
            | win32pipe.PIPE_READMODE_BYTE
            | win32pipe.PIPE_WAIT,
            1,
            READ_BUFFER_SIZE,
            READ_BUFFER_SIZE,
            0,
            None,
        )
    except pywintypes.error as exc:
        log.error("CreateNamedPipe falhou para %s: %s", pipe_path, exc)
        return 1

    # 3. Anuncia o pipe pro app via stdout. PRIMEIRA linha do stdout.
    print(f"READY {pipe_name}", flush=True)

    # 4. Espera o app conectar.
    try:
        win32pipe.ConnectNamedPipe(pipe_handle)
    except pywintypes.error as exc:
        if exc.winerror not in (535,):
            log.error("ConnectNamedPipe falhou: %s", exc)
            win32file.CloseHandle(pipe_handle)
            return 1
    log.info("cliente conectou em %s", pipe_path)

    # 5. Envia `worker.hello`. Inclui o `font_status` no payload
    #    (extensao alem do WorkerManifest) pra o manager saber se as
    #    fontes T&L estao presentes ou em fallback. Tambem inclui
    #    `ocr_available` + `tesseract_version` + `tesseract_status`
    #    (Etapa 2B+Y / ADR-0019) pra o caller saber se OCR ta
    #    disponivel sem precisar chamar `ocr.run` pra descobrir.
    hello_payload = dict(manifest)
    hello_payload["font_status"] = font_status
    hello_payload["ocr_available"] = ocr_available
    hello_payload["tesseract_version"] = tess_version
    hello_payload["tesseract_status"] = {
        "binary_present": _tesseract_executable_present(),
        "pytesseract_imported": PYTESSERACT_AVAILABLE,
        "version": tess_version,
        "tessdata_dir": str(TESSERACT_TESSDATA_DIR),
    }
    try:
        win32file.WriteFile(pipe_handle, ipc_message("worker.hello", hello_payload))
    except pywintypes.error as exc:
        log.error("write do worker.hello falhou: %s", exc)
        win32file.CloseHandle(pipe_handle)
        return 1

    # 6. Loop: le linhas do pipe e dispatcha.
    auth_token: str | None = None
    buffer = b""
    while True:
        try:
            err, chunk = win32file.ReadFile(pipe_handle, READ_BUFFER_SIZE)
        except pywintypes.error as exc:
            if exc.winerror in (109, 232):
                log.info("peer fechou o pipe (EOF)")
                break
            log.error("ReadFile falhou: %s", exc)
            break

        if err == 0 and not chunk:
            log.info("ReadFile devolveu 0 bytes (EOF)")
            break

        buffer += chunk
        shutdown_received = False
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            if not line:
                continue
            try:
                msg = decode_line(line)
            except (ValueError, json.JSONDecodeError) as exc:
                log.warning("decode falhou: %s (linha ignorada)", exc)
                continue

            op = msg.get("op")
            log.debug("recv op=%s request_id=%s", op, msg.get("request_id"))

            if op == "app.ack":
                auth_token = msg.get("auth")
                log.info("handshake completo (auth salvo)")

            elif op == "app.ping":
                pong_payload = {
                    "status": "ok",
                    "env_received": {},
                    "font_status": ensure_fonts_registered(),
                }
                try:
                    win32file.WriteFile(
                        pipe_handle,
                        ipc_message("worker.pong", pong_payload, request_id=msg.get("request_id")),
                    )
                except pywintypes.error as exc:
                    log.error("write do pong falhou: %s", exc)
                    break

            elif op == "app.shutdown":
                log.info("app.shutdown recebido, saindo")
                shutdown_received = True
                break

            elif op == "tool.invoke":
                response = handle_tool_invoke(msg, auth_token)
                try:
                    win32file.WriteFile(pipe_handle, response)
                except pywintypes.error as exc:
                    log.error("write do tool.result falhou: %s", exc)
                    break

            else:
                log.warning("op desconhecido/ignorado: %r", op)

        # Se `app.shutdown` foi recebido, sai do loop principal.
        # **Bug fix v0.2.0:** a versao inicial tinha `break` direto
        # no `elif app.shutdown`, mas esse break estava dentro do
        # `while b"\\n" in buffer` (loop interno que processa linhas
        # do buffer). O loop externo (`while True:`) continuava, e
        # o worker fazia um novo `ReadFile` esperando dados que
        # nunca chegavam - travando o `actor_task.await` no
        # `manager.shutdown`. Agora levantamos a flag e quebramos
        # no escopo certo.
        if shutdown_received:
            break

    try:
        win32file.CloseHandle(pipe_handle)
    except pywintypes.error:
        pass
    log.info("worker %s saindo", worker_id)
    return 0


def main() -> int:
    """Entry point. Aceita `--manifest <path>` (default: `manifest.json` ao lado)."""
    args = sys.argv[1:]
    manifest_path = Path(__file__).resolve().parent / "manifest.json"
    i = 0
    while i < len(args):
        if args[i] == "--manifest" and i + 1 < len(args):
            manifest_path = Path(args[i + 1])
            i += 2
        else:
            log.warning("argumento ignorado: %r", args[i])
            i += 1
    if not manifest_path.is_file():
        log.error("manifesto nao encontrado em %s", manifest_path)
        return 1
    return worker_main(manifest_path)


if __name__ == "__main__":
    raise SystemExit(main())
