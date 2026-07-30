"""`document-worker` — sidecar Python do Frederico IA Studio (Fase 5, Etapa 2B).

Worker que gera documentos profissionais (DOCX, XLSX, PDF) e roda
OCR. Comunica com o app principal via **named pipes** do Windows
sobre o **envelope IPC** do `frederico-process-architecture`
(line-delimited JSON, 8 opcodes estáveis em snake_case com
prefixo de direção: `worker.hello`, `app.ack`, `app.ping`,
`worker.pong`, `app.shutdown`, `worker.error`, `tool.invoke`,
`tool.result`).

## Protocolo (resumo do handshake)

1. Worker sobe, gera um `pipe_name` único, cria o
   `NamedPipeServer` (via `pywin32.CreateNamedPipe`), e imprime
   `READY <pipe_name>` no **stdout** (handshake invertido,
   ADR-0017 §Decisão 2).
2. Worker espera o app conectar (`ConnectNamedPipe`).
3. Worker envia `worker.hello` com o manifesto carregado de
   `manifest.json` (gera `request_id` UUID v4).
4. App responde `app.ack` com o `WorkerAuth` (token de curta
   duração). Worker salva o token.
5. Loop: worker lê linhas JSON do pipe, dispatch por `op`:
   - `app.ping` → `worker.pong` com `status: "ok"`.
   - `app.shutdown` → fecha o pipe e sai. **Não** responde
     (o manager detecta EOF).
   - `tool.invoke` → valida token (se já temos um) e
     dispatcha pro handler da `capability` declarada
     (handlers são **stubs** nesta versão — retornam
     `tool.result` com `ok: false` e mensagem "handler stub").
6. Worker nunca inicia — é o app que conecta (inversão do
   handshake, ADR-0017 §Decisão 2).

## Status atual

**Stub mínimo** (Etapa 2B continuação, 2026-07-29). O
`spawn_external` no Rust já está implementado e coberto por
3 integration tests com stub PowerShell (`tests/stubs/worker-stub.ps1`).
Os handlers reais (`docx.write`, `ocr.run`, etc) entram em
etapas seguintes da Fase 5 — o `document-worker` stub é só
o esqueleto do **protocolo + transporte** + manifesto. A
Etapa 2B fecha quando o `bootstrap.ps1` instala o runtime e
o `document-worker` consegue subir e fazer o handshake.

## Como rodar localmente

```pwsh
# 1. Instala runtime (Python embeddable + pywin32) em .\runtime\
pwsh -NoProfile -ExecutionPolicy Bypass -File .\bootstrap.ps1

# 2. Testa o handshake standalone (sem o app — abre cliente
#    PowerShell que conecta e envia `app.ping`):
.\runtime\python.exe .\document-worker.py
# vai imprimir `READY <name>` no stdout. Use
# `tests\smoke_local.ps1` (Etapa 2B continuação) pra fechar
# o ciclo.
```
"""

from __future__ import annotations

import json
import logging
import os
import sys
import uuid
from pathlib import Path
from typing import Any

# `pywin32` é instalado pelo `bootstrap.ps1` (ADR-0004). Try/except
# claro pra que o erro apareça no stderr do worker, não no app.
try:
    import win32pipe  # type: ignore[import-untyped]
    import win32file  # type: ignore[import-untyped]
    import pywintypes  # type: ignore[import-untyped]
except ImportError as exc:
    print(
        f"[document-worker] ERRO: pywin32 não está instalado ({exc}). "
        "Rode o bootstrap.ps1 pra instalar Python + pywin32 em runtime/. "
        "Ver ADR-0004.",
        file=sys.stderr,
        flush=True,
    )
    raise SystemExit(2)

# Versão do envelope IPC — bump MAJOR em mudanças incompatíveis
# (mesmo número que `IpcMessage::current_protocol_version()` no
# Rust). Espelhado aqui pra validação no decode.
PROTOCOL_VERSION: int = 1

# Tamanho do buffer de leitura (bytes). Cada `IpcMessage` JSON
# é uma linha terminada em `\n`; documentos grandes podem ter
# payloads > 4 KB — o loop interno acumula até encontrar o `\n`.
READ_BUFFER_SIZE: int = 4096

# Timeout do `ConnectNamedPipe` (ms). 60s é folgado — o app
# conecta imediatamente após o `READY <name>`. Se o app travar,
# o worker fica preso aqui (e o `child` no manager detecta
# timeout pelo `app.ping` que nunca volta).
CONNECT_TIMEOUT_MS: int = 60_000

# Logging básico. Vai pro stderr, que o `WorkerManager::spawn_external`
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


def ipc_message(op: str, payload: dict[str, Any], auth: str | None = None) -> bytes:
    """Serializa uma `IpcMessage` como **uma linha** (line-delimited JSON).

    O `\n` no final é o separador de mensagens. O
    `IpcMessage::decode_line` (Rust) faz `strip_suffix(b"\\n")` —
    sem o newline o decode falha.
    """
    msg = {
        "protocol_version": PROTOCOL_VERSION,
        "request_id": str(uuid.uuid4()),
        "op": op,
        "payload": payload,
    }
    if auth is not None:
        msg["auth"] = auth
    # `separators` remove whitespace desnecessário, e
    # `ensure_ascii=False` permite UTF-8 cru (documentos em
    # pt-BR têm acentos — o decode do Rust aceita). O `\n`
    # final é o terminador.
    return (json.dumps(msg, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8")


def decode_line(line: bytes) -> dict[str, Any]:
    """Desserializa uma linha JSON. Valida `protocol_version`.

    Lança `ValueError` em payload malformado ou versão errada.
    O `IpcMessage::decode_line` (Rust) faz o mesmo.
    """
    # Strip `\n`/`\r\n` final. `pywin32` (kernel32 ReadFile) não
    # traduz line endings; o `decode_line` Rust também é
    # tolerante a `\r` extra.
    if line.endswith(b"\r\n"):
        line = line[:-2]
    elif line.endswith(b"\n"):
        line = line[:-1]
    msg = json.loads(line.decode("utf-8"))
    pv = msg.get("protocol_version")
    if pv != PROTOCOL_VERSION:
        raise ValueError(
            f"protocol_version {pv} não é a atual {PROTOCOL_VERSION}"
        )
    return msg


# ---------------------------------------------------------------------------
# Loop principal do worker
# ---------------------------------------------------------------------------


def load_manifest(manifest_path: Path) -> dict[str, Any]:
    """Carrega `manifest.json` ao lado do script.

    O `WorkerManifest` é o payload do `worker.hello` de boot
    (mesmo shape do `frederico-process-architecture::protocol::WorkerManifest`).
    """
    with open(manifest_path, encoding="utf-8") as f:
        return json.load(f)


def handle_tool_invoke(
    msg: dict[str, Any],
    auth_token: str | None,
    manifest: dict[str, Any],
) -> bytes:
    """Stub: dispatcha pro handler da capability declarada.

    **Versão Etapa 2B (stub):** os handlers reais (DOCX/XLSX/PDF
    writing, OCR) entram nas etapas seguintes. Esta versão:
    1. Valida o token se já temos um (mesma regra do
       `FakeWorker`).
    2. Devolve `tool.result` com `ok: false` e mensagem
       "handler stub — implementação na Etapa 2B+X".
    """
    request_id = msg["request_id"]
    auth = msg.get("auth")

    # Validação de token (só se já vimos um `app.ack`).
    if auth_token is not None and auth != auth_token:
        return ipc_message(
            "worker.error",
            {
                "code": "process_unauthorized",
                "message": "token ausente ou inválido",
            },
        )

    # Stub: não conhece a capability — só ecoa a capability
    # pedida. O `ToolRegistry` (Etapa 3) consome a lista
    # `capabilities` do manifesto pra filtrar tools.
    payload_in = msg.get("payload", {})
    capability = payload_in.get("capability", "<unknown>")
    log.info("tool.invoke capability=%s payload_keys=%s", capability, list(payload_in.keys()))

    return ipc_message(
        "tool.result",
        {
            "ok": False,
            "code": "handler_stub",
            "message": (
                f"document-worker 0.1.0 não implementa `{capability}` ainda "
                f"— handlers reais entram na Etapa 2B+X. "
                f"Capabilities declaradas no manifesto: {manifest.get('capabilities', [])}"
            ),
            "echo": payload_in,
        },
    )


def worker_main(manifest_path: Path) -> int:
    """Loop principal: cria pipe, espera connect, dispatch."""
    manifest = load_manifest(manifest_path)
    worker_id = manifest["worker_id"]
    log.info("subindo %s %s", worker_id, manifest.get("version", "?"))

    # 1. Gera nome único pro pipe. O `<name>` é a parte
    #    depois de `\\.\pipe\`. O `PipeName::new` (Rust) valida
    #    — sem `\`, sem `/`, ≤ 200 chars.
    pipe_name = f"frederico-{worker_id}-{uuid.uuid4().hex[:12]}"
    pipe_path = rf"\\.\pipe\{pipe_name}"

    # 2. Cria o NamedPipeServer. `maxInstances=1` (só o app
    #    principal — não aceitamos clients paralelos, mesma
    #    semântica do `first_pipe_instance(true)` no Rust).
    #    Byte stream, buffer 4096.
    try:
        pipe_handle = win32pipe.CreateNamedPipe(
            pipe_path,
            win32pipe.PIPE_ACCESS_DUPLEX,
            win32pipe.PIPE_TYPE_BYTE
            | win32pipe.PIPE_READMODE_BYTE
            | win32pipe.PIPE_WAIT,
            1,  # maxInstances
            READ_BUFFER_SIZE,
            READ_BUFFER_SIZE,
            0,  # default timeout
            None,  # default security
        )
    except pywintypes.error as exc:
        log.error("CreateNamedPipe falhou para %s: %s", pipe_path, exc)
        return 1

    # 3. Anuncia o pipe pro app via stdout. **PRIMEIRA linha do
    #    stdout** — `spawn_external` (Rust) parseia com
    #    `parse_ready_line`. Nada pode vir antes — `print` vai
    #    direto pro stdout (não buffered como o logger).
    #    `flush=True` garante que o `READY` chega antes do
    #    `ConnectNamedPipe` bloquear.
    print(f"READY {pipe_name}", flush=True)

    # 4. Espera o app conectar. Bloqueante. Se o app crashar
    #    antes de conectar, o `spawn_external` mata o worker
    #    (timeout 10s no READY, depois `kill+wait`).
    try:
        win32pipe.ConnectNamedPipe(pipe_handle)
    except pywintypes.error as exc:
        # ERROR_PIPE_CONNECTED (535) é "pipe já foi conectado"
        # — acontece se o `CreateFileW` do app chegou antes
        # do nosso `ConnectNamedPipe`. É OK — segue.
        if exc.winerror not in (535,):
            log.error("ConnectNamedPipe falhou: %s", exc)
            win32file.CloseHandle(pipe_handle)
            return 1

    log.info("cliente conectou em %s", pipe_path)

    # 5. Envia `worker.hello` (assim que conecta). O app
    #    responde com `app.ack` carregando o `WorkerAuth` —
    #    token que vai em toda `tool.invoke` subsequente.
    try:
        win32file.WriteFile(pipe_handle, ipc_message("worker.hello", manifest))
    except pywintypes.error as exc:
        log.error("write do worker.hello falhou: %s", exc)
        win32file.CloseHandle(pipe_handle)
        return 1

    # 6. Loop: lê linhas do pipe e dispatcha. EOF (peer
    #    fechou) sai do loop. O `actor_task` (Rust) detecta o
    #    EOF e o `shutdown` reapa o child.
    auth_token: str | None = None
    buffer = b""
    while True:
        try:
            # ReadFile bloqueante. Retorna `(errcode, bytes)`.
            # `errcode == 0` é sucesso.
            err, chunk = win32file.ReadFile(pipe_handle, READ_BUFFER_SIZE)
        except pywintypes.error as exc:
            # ERROR_BROKEN_PIPE (109) é "peer fechou" — saída
            # limpa do loop.
            if exc.winerror in (109, 232):  # 232 = ERROR_NO_DATA
                log.info("peer fechou o pipe (EOF)")
                break
            log.error("ReadFile falhou: %s", exc)
            break

        if err == 0 and not chunk:
            # EOF limpo.
            log.info("ReadFile devolveu 0 bytes (EOF)")
            break

        buffer += chunk
        # Processa linhas completas (`\n` no buffer).
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
                try:
                    win32file.WriteFile(
                        pipe_handle,
                        ipc_message(
                            "worker.pong",
                            {"status": "ok", "env_received": {}},
                        ),
                    )
                except pywintypes.error as exc:
                    log.error("write do pong falhou: %s", exc)
                    break

            elif op == "app.shutdown":
                # O manager não espera response — fecha o
                # pipe e sai. O EOF que o `read_line` do
                # manager vê é o sinal de "shutdown completo".
                log.info("app.shutdown recebido, saindo")
                break

            elif op == "tool.invoke":
                response = handle_tool_invoke(msg, auth_token, manifest)
                try:
                    win32file.WriteFile(pipe_handle, response)
                except pywintypes.error as exc:
                    log.error("write do tool.result falhou: %s", exc)
                    break

            else:
                log.warning("op desconhecido/ignorado: %r", op)

    # 7. Cleanup — fecha o handle do pipe (libera a instância
    #    nomeada). `DisconnectNamedPipe` não é necessário
    #    (vamos sair mesmo).
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
        log.error("manifesto não encontrado em %s", manifest_path)
        return 1
    return worker_main(manifest_path)


if __name__ == "__main__":
    raise SystemExit(main())
