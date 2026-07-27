# workers/

Workers sidecar do Frederico IA Studio (processos externos empacotados
com o app). Conforme [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md)
e [process-architecture.md](../architecture/process-architecture.md),
nenhum worker abre porta em `localhost` — comunicação é por JSON
serializável via contrato em `packages/shared-contracts/`.

Estado atual: **vazio**. Os workers nascem nas fases seguintes:

- **Fase 5** — `document-worker` (Python embutido, ver ADR-0004).
- **Fase 7** — `sandbox-runner`, `runtime-manager`.
- **Fase 5/6** — `browser-worker` (Rust + headless).

Quando o primeiro worker entrar, este diretório ganha:

```text
workers/
  document-worker/
    pyproject.toml
    src/
    tests/
  sandbox-runner/
    Cargo.toml
    src/
    tests/
```
