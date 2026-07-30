# workers/

Workers sidecar do Frederico IA Studio (processos externos empacotados
com o app). Conforme [ADR-0003](../docs/decisions/0003-nucleo-desacoplado-da-casca-tauri.md)
e [process-architecture.md](../docs/architecture/process-architecture.md),
nenhum worker abre porta em `localhost` — comunicação é por JSON
serializável via contrato IPC do `crates/process-architecture/`.

## Workers atuais

- **`document-worker/`** — Python embutido (ADR-0004). Gera
  DOCX/XLSX/PDF e le os tres formatos. **6 handlers reais**
  (Etapa 2B+X, 2026-07-30) consumindo python-docx + openpyxl +
  reportlab + pdfplumber, com fontes "Tinta e Latao" (Adobe
  Source Sans 3 + Source Serif 4) embutidas no PDF. `ocr.run`
  foi removido do manifesto (vai pra Etapa 2B+Y, sozinho).
  `bootstrap.ps1` instala Python 3.12 + pywin32 + 4 libs +
  4 TTFs em `runtime/`. Ver
  [`document-worker/README.md`](document-worker/README.md).

## Próximos workers (Fase 5+)

- **`sandbox-runner`** (Fase 7) — Rust, executa código não-confiável
  em sandbox. Ainda não criado.
- **`runtime-manager`** (Fase 7) — Rust, gerencia runtimes
  embarcados. Ainda não criado.
- **`browser-worker`** (Fase 5/6) — Rust + headless, automação de
  browser. Ainda não criado.
