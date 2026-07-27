<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 1
-->

# Módulo `desktop` (casca Tauri)

> Diretório: [`apps/desktop/`](../../apps/desktop/)
> Binário: `frederico-desktop` (Rust + Tauri 2)
> Frontend: React + TypeScript + Vite

## O que faz

A casca do Frederico IA Studio. É a única parte do app que conhece
Tauri e a plataforma Windows. Configura a janela, expõe comandos IPC
para o frontend, e delega toda a lógica para os crates do núcleo
(`frederico-core`, `frederico-storage`, `frederico-diagnostics`,
`frederico-security`, `frederico-shared-contracts`).

A Fase 1 entrega duas operações IPC:
- `app_version()` — devolve `frederico_core::APP_VERSION` como string.
- `ipc_dispatch(request)` — despacha um `IpcRequest` (do
  `frederico-shared-contracts`) para o handler correspondente.
  Operações da Fase 1: `ping` e `get_app_info`.

## O que expõe

### Rust (`apps/desktop/src-tauri/`)

- `pub fn main()` — inicializa logs, abre o banco, registra comandos Tauri, abre a janela.
- `#[tauri::command] fn ipc_dispatch(request, state) -> IpcResponse`
- `#[tauri::command] fn app_version() -> String`

### Frontend (`apps/desktop/src/`)

- `services/api.ts` — **única** camada que faz `invoke` no Tauri (ADR-0003).
- `services/contracts.ts` — tipos espelhados do `frederico-shared-contracts`. Virará arquivo gerado na Fase 2 (REGRAS §1.9).
- `routes/Home.tsx` — busca `app_info` via IPC e mostra o resultado.
- `routes/About.tsx` — texto estático.
- `App.tsx` — layout com `HashRouter` e 2 rotas.

## De quem depende / quem depende dele

- **Depende de:** Tauri 2, `tokio`, todos os crates do núcleo, `directories`, `tracing`.
- **Usado por:** o usuário final (janela desktop).

## Decisões não óbvias / armadilhas

- `tauri::async_runtime::block_on` é usado em `setup` para abrir o banco de forma síncrona antes da janela abrir. Trocar para `tokio::spawn` exigiria lidar com inicialização tardia dos comandos.
- O `services/` do frontend é a **única** camada que importa `@tauri-apps/api/core`. Componentes nunca chamam `invoke` diretamente. Lint futuro no CI deve verificar isso.
- O CSP em `tauri.conf.json` é restritivo. Recursos adicionais exigem atualização consciente.

## Como testar isoladamente

A Fase 1 não tem testes automatizados do app Tauri (entram na Fase 2
com `tauri-driver`). Verificações hoje:

```pwsh
# Compilação
cargo build -p frederico-desktop

# Lint do frontend
cd apps/desktop
npm run typecheck

# Build do frontend (gera apps/desktop/dist/)
npm run build
```

## O que este módulo **não** faz

- Não tem chat, tools, memória, documentos. É a casca mínima.
- Não tem testes E2E (chega na Fase 2 com `tauri-driver`).
- O instalador empacota via NSIS (target declarado em `tauri.conf.json`); a Fase 1 inclui o build do bundle, mas a Fase 9 cuida da produção final.
