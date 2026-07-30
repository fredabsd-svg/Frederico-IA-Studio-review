"""`document-worker` v0.2.0 - sidecar Python do Frederico IA Studio (Fase 5, Etapa 2B+X).

Worker que gera documentos profissionais (DOCX, XLSX, PDF) e le os tres
formatos. Comunica com o app principal via **named pipes** do Windows
sobre o **envelope IPC** do `frederico-process-architecture`
(line-delimited JSON, 8 opcodes estaveis em snake_case com prefixo
de direcao: `worker.hello`, `app.ack`, `app.ping`, `worker.pong`,
`app.shutdown`, `worker.error`, `tool.invoke`, `tool.result`).

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

## Handlers (Etapa 2B+X - 6 primitivas, 7a comeca na 2B+Y)

| Capability   | Input                                       | Output                                              |
| ------------ | ------------------------------------------- | --------------------------------------------------- |
| `docx.write` | `path`, `title`, `sections`                 | `path`, `size_bytes`, `sections_written`            |
| `docx.read`  | `path`                                      | `paragraphs`, `tables`, contagens                   |
| `xlsx.write` | `path`, `sheets`                            | `path`, `size_bytes`, `sheets_written`              |
| `xlsx.read`  | `path` (opcional `sheet`)                   | `sheets`, `n_sheets`                                |
| `pdf.write`  | `path`, `title`, `sections`                 | `path`, `size_bytes`, `pages_rendered`              |
| `pdf.read`   | `path`                                      | `text`, `page_count`, `scanned_pages`, `ocr_available` |

**`ocr.run` foi removido do manifesto nesta versao** (vai pra Etapa
2B+Y - Tesseract + por/eng traineddata). `pdf.read` retorna
`scanned_pages: [n, m, ...]` no payload e `code: "pdf_scanned_no_ocr"`
quando **todas** as paginas sao escaneadas - limitacao conhecida
registrada no CHANGELOG.md.

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

ADR-0018 §Decisao 1. Os 6 handlers sao primitivas de I/O sobre as
bibliotecas Python (`python-docx`, `openpyxl`, `reportlab`,
`pdfplumber`). Eles **nao** decidem margem, fonte, cor, numeracao
de pagina, header/footer, etc. - isso e trabalho do **kit**
(`WordPro`/`ExcelPro`/`PdfPro`, Etapa 3) que recebe o `DocumentSpec`
declarativo e traduz pra esses handlers. A v0.2.0 e deliberadamente
feia em tipografia; a beleza visual e o trabalho do kit.
"""

from __future__ import annotations

import json
import logging
import os
import sys
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

# Versao do envelope IPC - bump MAJOR em mudancas incompativeis
# (mesmo numero que `IpcMessage::current_protocol_version()` no Rust).
PROTOCOL_VERSION: int = 1

# Tamanho do buffer de leitura (bytes).
READ_BUFFER_SIZE: int = 4096

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


def handle_pdf_read(payload: dict) -> dict:
    """pdf.read: extrai texto de um .pdf e detecta paginas escaneadas.

    Input: `{"path": str}`
    Output:
      - Happy path: `{"ok": true, "text": str, "page_count": int, "scanned_pages": [int], "ocr_available": false}`
      - 100% escaneado: `{"ok": false, "code": "pdf_scanned_no_ocr", ...}`

    **Limitacao conhecida** (registrada no CHANGELOG.md, pendente
    2B+Y): paginas escaneadas (PDFs de imagens sem camada de texto)
    retornam texto vazio. Ate a 2B+Y entregar Tesseract + pytesseract,
    o caller precisa tratar `scanned_pages` e o code
    `pdf_scanned_no_ocr` como limitacao.
    """
    path = validate_path(_payload_field(payload, "path", str), "read")
    pages_text: list[str] = []
    scanned_pages: list[int] = []
    with pdfplumber.open(str(path)) as pdf:
        for i, page in enumerate(pdf.pages, start=1):
            text = page.extract_text() or ""
            text = text.strip()
            if not text:
                scanned_pages.append(i)
            pages_text.append(text)
    full_text = "\n".join(pages_text)
    page_count = len(pages_text)
    if page_count > 0 and len(scanned_pages) == page_count:
        # PDF 100% escaneado - sem camada de texto em pagina alguma.
        return {
            "ok": False,
            "code": "pdf_scanned_no_ocr",
            "message": (
                f"PDF escaneado detectado ({page_count} pagina(s) sem texto); "
                "OCR nao disponivel ate Etapa 2B+Y"
            ),
            "page_count": page_count,
            "scanned_pages": scanned_pages,
            "ocr_available": False,
        }
    return {
        "ok": True,
        "path": str(path),
        "text": full_text,
        "page_count": page_count,
        "scanned_pages": scanned_pages,
        "ocr_available": False,
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
    #    fontes T&L estao presentes ou em fallback.
    hello_payload = dict(manifest)
    hello_payload["font_status"] = font_status
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
