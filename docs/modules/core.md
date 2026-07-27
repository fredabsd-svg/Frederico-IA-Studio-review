<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 1
-->

# Módulo `core`

> Crate: [`crates/core/`](../../crates/core/)
> Nome do pacote: `frederico-core`

## O que faz

Tipos compartilhados, identificadores opacos e a versão semântica do app
(`APP_VERSION`). É a raiz da qual os outros crates dependem. Não tem
dependências de plataforma e não realiza I/O.

## O que expõe

- Identificadores opacos: `RunId`, `ConversationId`, `ProjectId`, `CheckpointId`, `ArtifactId`. Cada um envolve um `Uuid` e implementa `Serialize`/`Deserialize` em formato `serde(transparent)`.
- `AppVersion` e a constante `APP_VERSION` (alinhada com a versão do workspace).
- `CoreError`/`CoreResult` — base de erros usada por outros crates.
- `require_non_empty(field, value)` — helper de validação.

## De quem depende / quem depende dele

- **Depende de:** nada além de `serde`, `uuid`, `chrono`, `thiserror`.
- **Usado por:** todos os outros crates do núcleo (`storage`, `diagnostics`, `security`, `shared-contracts`, `apps/desktop/src-tauri`).

## Decisões não óbvias / armadilhas

- Os identificadores são `serde(transparent)`. **Não** trocar para uma representação com tag sem atualizar `crates/storage` e os contratos em `packages/shared-contracts/`.
- `CoreError` é deliberadamente pequeno. Erros específicos de cada crate têm seu próprio `thiserror::Error`. Não adicionar variantes "genéricas" para evitar crescer sem motivo.

## Como testar isoladamente

```pwsh
cargo test -p frederico-core
```

## O que este módulo **não** faz

- Não conhece caminhos do sistema, paths de banco, nem plataforma.
- Não lê nem grava nada em disco.
- Não faz logging (vive em `crates/diagnostics/`).
