# AGENTS.md — Frederico IA Studio

> Carta de navegação para qualquer IA (ou pessoa) que abrir o repositório.
> Antes de qualquer coisa, leia [`REGRAS-DO-PROJETO.md`](./REGRAS-DO-PROJETO.md)
> e [`docs/status.md`](./docs/status.md). Este arquivo é um **resumo**;
> ele não substitui as regras nem o status — só aponta onde achar.

## O que é o projeto

Estúdio de IA desktop para Windows 10/11, distribuído como instalador
`.exe`. Conversa com múltiplos provedores/modelos, usa ferramentas
reais, gera documentos profissionais (Word, Excel, PDF) e ajuda a
desenvolver software, com auditoria completa do que a IA fez.

Mais em [`docs/architecture/product-vision.md`](./docs/architecture/product-vision.md).

## Como está hoje (estado real)

[`docs/status.md`](./docs/status.md) é a fonte da verdade. Resumo do
que já foi fechado:

- **Fase 0** (Fundação documental) — fechada: 9 specs em
  `docs/architecture/`, 4 ADRs em `docs/decisions/`, regras do projeto
  com §1.13.
- **Fase 1** (Fundação desktop: Tauri 2 + Rust + SQLite + React +
  TypeScript + Vite) — fechada: monorepo conforme ADR-0002, 4 crates
  do núcleo (`core`, `storage`, `diagnostics`, `security`), casca
  Tauri com 2 rotas, migração SQLite inicial, instalador NSIS
  empacotado (~3 MB).
- **Fase 2** (Chat e provedores) — **não iniciada**, depende da
  Fase 1.

## Ordem de leitura obrigatória em qualquer sessão nova

1. [`REGRAS-DO-PROJETO.md`](./REGRAS-DO-PROJETO.md) — vale para
   qualquer IA ou pessoa. Violação de regra é defeito, igual a bug.
2. [`docs/status.md`](./docs/status.md) — estado real por fase.
   Nada é marcado como concluído sem os testes da fase passando
   (REGRAS §1.8).
3. [`CHANGELOG.md`](./CHANGELOG.md) — o que mudou por versão.
4. [`docs/architecture/development-roadmap.md`](./docs/architecture/development-roadmap.md)
   — fases, pré-requisitos entre fases, critério de "done" de cada
   uma. Regra dura: pular pré-requisito exige ADR.
5. [`docs/decisions/`](./docs/decisions/) — ADRs. Toda decisão
   estrutural está aqui.
6. [`docs/modules/`](./docs/modules/) — um arquivo por crate/worker
   (REGRAS §1.4). Módulo novo sem esse doc não é considerado
   entregue.

## Decisões estruturais vigentes

- **ADR-0001** — Specs podem ser `especificado`,
  `parcialmente implementado` ou `implementado` (com cabeçalho e
  carimbo de verificação). A trava do caminho inverso impede que
  um spec fique `especificado` depois que a fase está em andamento.
- **ADR-0002** — Layout literal do PROMPT MESTRE §5.4: `apps/`,
  `crates/`, `workers/`, `packages/`, `tests/`, `docs/`. Workspace
  Cargo único na raiz.
- **ADR-0003** — Núcleo desacoplado da casca Tauri. Nenhum crate
  em `crates/` pode importar `tauri`, `windows`, `winapi` ou
  `winrt`. Coberto por `scripts/check-core-purity.ps1`.
- **ADR-0004** — `document-worker` (Fase 5) será Python embutido,
  com Tesseract, fontes e bibliotecas empacotadas; comunicação por
  named pipes, **sem** `localhost`.

## Stack e onde mora o código

- **Casca:** `apps/desktop/` — Tauri 2 + React 18 + TypeScript + Vite.
  Binário Rust em `apps/desktop/src-tauri/`, frontend em
  `apps/desktop/src/`. A camada `apps/desktop/src/services/` é a
  **única** que faz `invoke` no Tauri (regra do ADR-0003).
- **Núcleo:** `crates/{core,storage,diagnostics,security}/`. Sem
  dependência de plataforma.
- **Contratos compartilhados:** `packages/shared-contracts/`.
- **Workers sidecar:** `workers/` (vazio até a Fase 5).
- **Documentação:** `docs/architecture/` (specs), `docs/decisions/`
  (ADRs), `docs/modules/` (1 doc por crate), `docs/security/`,
  `docs/testing/`, `docs/releases/`, `docs/status.md`.

## Stack de build (validado na Fase 1, ambiente Windows)

- **Rust:** stable + toolchain GNU `stable-x86_64-pc-windows-gnu` (não
  MSVC — o ambiente não tem Visual Studio Build Tools e o
  toolchain GNU com MinGW-w64 compila o Tauri inteiro).
- **MinGW-w64:** WinLibs POSIX/UCRT em
  `C:\Users\conta\AppData\Local\mingw64\`.
- **Node:** LTS (24.x) em `C:\Program Files\nodejs\`.
- **Tauri CLI 2.x:** `cargo install tauri-cli --version "^2.0"`.
- **NSIS:** baixado sob demanda pelo Tauri durante `tauri build`,
  não precisa estar no sistema.
- **Junction sem espaços:** o repositório é acessado em
  `C:\src\Frederico` → `C:\Users\conta\OneDrive\Documentos\Studio review\Frederico-IA-Studio-review`.
  Windres e Rollup rejeitam paths com espaço. O
  `apps/desktop/vite.config.ts` define `resolve.preserveSymlinks: true`
  pra Rollup não surtar.

## Como verificar (REGRAS §1.10)

```pwsh
# Tudo da Fase N:
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1

# Só o guardrail do ADR-0003 (núcleo sem tauri/windows):
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\check-core-purity.ps1

# Build do instalador (gera target\release\bundle\nsis\*.exe):
cargo tauri build
```

A suíte de testes atual (Fase 1): 15/15 verde (cargo test),
`cargo clippy --workspace -- -D warnings` limpo, `npm run build`
verde, guard de pureza OK, `cargo tauri build` empacota o NSIS.

## Convenções de commit/PR (REGRAS §1.12)

- Commit: primeira linha até 72 caracteres, imperativa, efeito claro
  ("adiciona auditoria bloqueante ao PDFPro" — não "wip", "fix",
  "ajustes", "update").
- Commit gigante misturando fases é proibido.
- PR preenche o formato do PROMPT MESTRE §31 (Implementado /
  Arquivos / Decisões / Testes executados / Limitações / Riscos /
  Próxima etapa) com referências aos ADRs.
- Sem força-push em branch compartilhada.
- Documentação, ADRs, changelog, mensagens de commit, descrições de
  PR: **português do Brasil**, frases curtas, voz ativa. Identificadores
  e termos técnicos consagrados em inglês (`tool_calls`, `run_id`,
  `commit`).

## O que **não** está decidido ainda (precisa de ADR se for tocado)

- Stack de E2E (`tauri-driver` vs. Playwright vs. custom) — ver
  `docs/architecture/testing-strategy.md` §Não-objetivos.
- Provedor simulado: replay de fita (golden files) vs. gerador
  determinístico.
- Onde rodar testes de "máquina limpa" (runner self-hosted, GitHub
  Actions, Buildkite, outro).

## Conversas anteriores sobre este projeto

O usuário tem mantido uma conversa por fase. O contexto dessas
conversas (decisões, debates, detalhes que não entraram nos ADRs)
**não vive** neste repositório — vive na memória do Mavis (user
memory + agent memory). Em qualquer sessão nova, faça um
`memory` search por "Frederico IA Studio" pra puxar o que já foi
discutido.
