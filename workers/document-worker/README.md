# `document-worker`

Worker sidecar do Frederico IA Studio que gera documentos profissionais
(DOCX, XLSX, PDF) e roda OCR. Python embutido (ADR-0004),
comunica com o app via **named pipes** do Windows.

## Estado atual (Etapa 2B continuação, 2026-07-29)

**Stub mínimo de protocolo + transporte.** O esqueleto implementa:

- **Manifesto versionado** (`manifest.json`) — ID, capabilities, dependências, compatibilidade.
- **Handshake do worker** — gera `pipe_name` único, cria `NamedPipeServer`
  via `pywin32`, imprime `READY <pipe_name>` no stdout, espera connect,
  envia `worker.hello`.
- **Loop de dispatch** — `app.ping` → `worker.pong`; `tool.invoke` →
  `tool.result` com `ok: false` e mensagem "handler stub" (handlers
  reais entram em etapas seguintes); `app.shutdown` → fecha e sai.
- **Validação de token** — após `app.ack`, o token é exigido em
  toda `tool.invoke`; `worker.error` com `code: "process_unauthorized"`
  em caso de mismatch (mesma regra do `FakeWorker`).

**O que NÃO está implementado (próximas etapas da Fase 5):**

- Handlers reais de `docx.write`/`docx.read`/`xlsx.write`/... — entram
  em etapas 3+ da Fase 5 quando a integração com o `document-engine`
  (Etapa 1, já fechado) começar.
- OCR (Tesseract) e fontes "Tinta & Latão" (ADR-0004) — dependem do
  `bootstrap.ps1` desta entrega baixar os binários adicionais. Hoje o
  `bootstrap` instala só Python + pip + pywin32; Tesseract + fontes
  entram numa entrega de bootstrap estendido.
- Revogação de token por lista negra (hardening — registrado como
  pendência 4 em `docs/modules/process-architecture.md`).

## Como rodar localmente

```pwsh
# 1. Instala Python embeddable + pip + pywin32 em runtime/
pwsh -NoProfile -ExecutionPolicy Bypass -File .\bootstrap.ps1

# 2. Roda o worker standalone
.\runtime\python.exe .\document-worker.py
# imprime `READY <name>` no stdout. Sem cliente, fica bloqueado
# em ConnectNamedPipe.

# 3. (Etapa 3+) O WorkerManager::spawn_external no Rust abre
#    esse mesmo python.exe com cwd = este diretório e os args
#    certos via ExternalSpawnConfig.
```

## Layout

```text
document-worker/
├── README.md          ← este arquivo
├── manifest.json      ← manifesto versionado (worker.hello payload)
├── pyproject.toml     ← deps (pywin32>=306) + build config
├── document-worker.py ← entry point — protocolo + loop
├── bootstrap.ps1      ← instala Python embeddable + pywin32 em runtime/
├── tests/             ← pytest roundtrip (NÃO roda no CI ainda —
│                       pytest não está no stack do Frederico; entra
│                       quando o Python stub for promovido a worker real)
├── .gitignore         ← ignora runtime/ e __pycache__/
└── runtime/           ← criado pelo bootstrap; não versionado
    ├── python.exe
    ├── python311.dll
    ├── ...
    └── Lib/site-packages/
        └── win32/...
```

## Integração com o Rust

Quando o `WorkerManager::spawn_external` (Fase 5, Etapa 2B
continuação, já implementado) quiser abrir o `document-worker`,
a casca Tauri monta:

```rust
ExternalSpawnConfig::new("workers/document-worker/runtime/python.exe")
    .with_args(vec!["workers/document-worker/document-worker.py".into()])
    .with_cwd("workers/document-worker")
    .with_env(&[
        ("PYTHONIOENCODING", "utf-8"),
        ("PYTHONUNBUFFERED", "1"),  // crítico: print() flush imediato
    ])
```

A Etapa 3 (integração com o `ToolRegistry`) faz esse wire-up.
A Etapa 2B só fecha o **transporte** (o que este PR entrega
do lado Rust).
