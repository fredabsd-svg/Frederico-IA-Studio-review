# `frederico-runtimes` — Módulo

<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-08
Fase correspondente: 7 (Etapa 3)
-->

Gerencia runtimes portáteis (Python 3.12.4 + Node 20.16.0) para os
`exec.python` / `exec.node` da Fase 7 Etapa 4.

## Componentes principais

- **`Runtime` trait** — contrato abstrato de um runtime. Cada
  implementação concreta declara:
  - `source_url` (pinned no código, sem SQL v1)
  - `expected_sha256` (defesa contra MITM)
  - `expected_archive_size` (sanity check adicional)
  - `env_vars` que entram no `EnvAllowlist::REQUIRED` (ADR-0031 D5)
  - `bootstrap_if_needed` (idempotente: cache hit é no-op)
  - `validate` (`<runtime> --version` + sanity check)
- **`RuntimeRegistry`** — ponto único de acesso aos runtimes.
  Constrói a partir de `RuntimeConfig`. Hard-coda Python 3.12.4
  + Node 20.16.0 na v1.
- **`PythonRuntime`** / **`NodeRuntime`** — implementações concretas.
  Source URL e SHA-256 pinned como `const` em `python.rs`/`node.rs`.

## API pública

```rust
use frederico_runtimes::{
    PythonRuntime, NodeRuntime, RuntimeRegistry, RuntimeConfig,
    Runtime, RuntimeId, BootstrapError, BootstrapReport,
};

let config = RuntimeConfig::secure_default();
let registry = RuntimeRegistry::new(config)?;
let report: BootstrapReport = registry.bootstrap_all().await?;
// report.bootstrapped (acabou de baixar) + report.cached (já estava)
// + report.failed (com diagnóstico).
```

## Posição no grafo de dependências

`frederico-runtimes` é independente — não importa nenhum outro
crate do workspace. Será consumido por:

- **`frederico-app` (Etapa 4)** — `exec.python` / `exec.node` pegam
  o runtime via `RuntimeRegistry::get(id)` e populam
  `SandboxConfig` (ADR-0036).
- **`apps/desktop/src-tauri/` (Etapa 4)** — Tauri commands de
  `runtime.status` / `runtime.bootstrap` consumem o registry.

`frederico-runtimes` **não** depende de `frederico-security`
(poderiam ser irmãos sob `frederico-app`; a integração dos
dois fica na Etapa 4).

## Regras de pureza do núcleo

`check-core-purity.ps1` permite `tauri`/`windows` apenas em
`security` e `process-architecture`. `frederico-runtimes` é
puro (sem `unsafe_code`, sem dependência de Win32 direto).
Usa `reqwest::blocking` + `zip` (Rust-pura) + `sha2` (Rust-pura)
+ `directories` (cross-platform). I/O é std + tokio.

## v1 simplificações

- Source URL + SHA-256 pinned como `const` no source (não em
  `runtime.toml` em disco). Bump de versão = editar const +
  commit + release. v2 migra para `runtime.toml` + migration
  SQL.
- Sem `mirror_url` ativo (válvula de escape para ambiente
  corporativo; campo existe em `RuntimeConfig` mas não é usado).
- Sem virtualenv / pip-offline / npm-offline. `pip install` na
  Etapa 4 usa a rede do sandbox via proxy local (ADR-0033).
- Sem auto-update. Bump manual.
- Cross-platform: a `RuntimeConfig` resolve `install_root` via
  `directories::ProjectDirs` (Windows: `%LOCALAPPDATA%`), mas o
  `bootstrap_if_needed` não foi testado em Linux/macOS (a v1
  é Windows-only).

## Testes de regressão (5 tests, todos verdes)

| Teste | Tipo | O que prova |
|---|---|---|
| `tests/python_bootstrap.rs::python_env_vars_do_not_include_user_paths` | **negação** | PATH injetado não contém hijack patterns (Store, vendors) |
| `tests/node_bootstrap.rs::node_env_vars_do_not_include_user_paths` | **negação** | Mesma defesa para Node |
| `tests/bootstrap_idempotent.rs::bootstrap_twice_is_noop` | funcional | 2ª chamada >5x mais rápida (cache hit, sem rede) |
| `tests/bootstrap_offline.rs::offline_returns_error_for_missing_runtime` | funcional | `allow_download=false` + cache vazio = `Err(OfflineRequired)` (não panic) |
| `tests/manifest_corruption.rs::corrupted_manifest_triggers_redownload` | funcional | SHA-256 mismatch deleta + re-download |

## Referências

- [`docs/architecture/runtimes-architecture.md`](../architecture/runtimes-architecture.md) — spec completo
- [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md) — modelo de isolamento, `EnvAllowlist::REQUIRED`
- [ADR-0033](../decisions/0033-sandbox-network-policy.md) — política de rede do sandbox
- [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) — `SecurityJailResolver::spawn` consome `Runtime::env_vars`
