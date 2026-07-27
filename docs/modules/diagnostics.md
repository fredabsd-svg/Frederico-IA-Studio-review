<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 1
-->

# Módulo `diagnostics`

> Crate: [`crates/diagnostics/`](../../crates/diagnostics/)
> Nome do pacote: `frederico-diagnostics`

## O que faz

Inicializa o subscriber global de `tracing` para logs estruturados. Filtro
configurável por `RUST_LOG` (default `info,sqlx=warn,tao=warn`).

## O que expõe

- `diagnostics::init()` — idempotente. Inicializa o subscriber **uma única vez** (via `OnceLock`).

## De quem depende / quem depende dele

- **Depende de:** `tracing`, `tracing-subscriber` (`env-filter`, `fmt`, `registry`).
- **Usado por:** `frederico-desktop` (chama `init()` antes de abrir o banco).

## Decisões não óbvias / armadilhas

- `init()` é idempotente via `OnceLock`. Chamar duas vezes **não** panica, mas só a primeira configuração vale. Não expor uma flag "force re-init" sem um ADR — testes futuros podem precisar.
- O filtro default silencia `sqlx` e `tao` (barulhentos). Se algo parar de funcionar, use `RUST_LOG=debug`.

## Como testar isoladamente

```pwsh
cargo test -p frederico-diagnostics
```

O teste atual só garante que `init()` não panica em chamadas repetidas — não é possível testar o subscriber global de outra forma sem afetar o estado do processo.

## O que este módulo **não** faz

- Não exporta telemetria. Tela de diagnóstico, exportação e OpenTelemetry chegam nas fases seguintes.
- Não lê nem grava arquivos. Toda a saída vai para stderr via `tracing-subscriber::fmt`.
