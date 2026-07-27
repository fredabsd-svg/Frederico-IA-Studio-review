<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 1
-->

# Módulo `security`

> Crate: [`crates/security/`](../../crates/security/)
> Nome do pacote: `frederico-security`

## O que faz

Declara os **traits de plataforma** que o núcleo usa para falar com o
sistema operacional, sem importar nada de Windows ou Tauri (ver
[ADR-0003](../../decisions/0003-nucleo-desacoplado-da-casca-tauri.md)).
A casca Tauri implementa esses traits e injeta na inicialização.
Em testes, implementações em `security::fake::*` substituem.

## O que expõe

- `trait Platform` — superfície de plataforma do núcleo.
- `trait Clock` — fonte de tempo injetável.
- `SystemClock` — implementação padrão usando `SystemTime`.
- `mod fake` — `FakePlatform`, `FakePaths`, `FakeClock` para testes.
- `SecurityError` — erros do módulo.

## De quem depende / quem depende dele

- **Depende de:** `frederico-core`, `frederico-storage` (para o trait `AppPaths`), `async-trait`, `thiserror`.
- **Usado por:** `frederico-desktop` (cria `FakePlatform` na Fase 1; a casca real entra na Fase 2).

## Decisões não óbvias / armadilhas

- O `Platform` da Fase 1 só tem `paths()` e `clock()`. As próximas
  variantes (`credentials`, `sandbox`, `notifier`) entram nas fases 2-7.
  Adicionar uma variante nova exige atualizar o `FakePlatform` e todos
  os call-sites.
- A guarda "núcleo sem dependências de plataforma" é verificada por
  `scripts/check-core-purity.ps1`. Ela falha se qualquer crate em
  `crates/` declarar `tauri`, `windows`, `winapi` ou `winrt` como
  dependência ou fizer `use` direto.

## Como testar isoladamente

```pwsh
cargo test -p frederico-security
```

## O que este módulo **não** faz

- Não implementa `Platform` para Windows (isso é papel da casca na Fase 2).
- Não tem credenciais, sandbox, paths do Windows, nem notificações. Esses vêm nas fases seguintes.
