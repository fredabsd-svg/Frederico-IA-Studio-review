# AGENTS.md — Frederico IA Studio

> Carta de navegação para qualquer IA (ou pessoa) que abrir o repositório.
> Antes de qualquer coisa, leia [`REGRAS-DO-PROJETO.md`](./REGRAS-DO-PROJETO.md)
> e [`docs/status.md`](./docs/status.md). Este arquivo é um **resumo**;
> ele não substitui as regras nem o status — só aponta onde achar.
>
> **Manutenção:** o PR que fecha uma fase atualiza a seção
> "Como está hoje" no mesmo commit (REGRAS §1.3). Este arquivo é o
> primeiro contato de qualquer sessão nova — desatualizado, ele não é
> neutro: manda a sessão para o lugar errado.

## O que é o projeto

Estúdio de IA desktop para Windows 10/11, distribuído como instalador
`.exe`. Conversa com múltiplos provedores/modelos, usa ferramentas
reais, gera documentos profissionais (Word, Excel, PDF) e ajuda a
desenvolver software, com auditoria completa do que a IA fez.

Mais em [`docs/architecture/product-vision.md`](./docs/architecture/product-vision.md).

## Como está hoje (estado real)

[`docs/status.md`](./docs/status.md) é a fonte da verdade — inclusive
sobre o que **não** funciona. O resumo abaixo é um índice; a coluna
"Pendências" de cada linha do `status.md` é onde moram as limitações
vivas, e nenhuma delas está repetida aqui.

- **Fase 0** (Fundação documental) — concluída: regras do projeto com
  §1.13, specs iniciais em `docs/architecture/`, ADRs 0001–0004.
- **Fase 1** (Fundação desktop) — concluída: monorepo conforme
  ADR-0002, casca Tauri 2 + React 18 + TypeScript + Vite, migração
  SQLite inicial, instalador NSIS empacotado (~3 MB).
- **Fase 2** (Chat e provedores) — concluída: adaptadores, catálogo de
  modelos, streaming, cancelamento e custos; credenciais em DPAPI real
  (`WindowsCredentialStore` sobre `CredWriteW`/`CredReadW`, crate
  `windows` 0.58) com 6 testes de integração.
- **Fase 3** (Motor de execução e ferramentas) — concluída:
  `frederico-execution-engine` com `RunExecutor` fechando o loop de
  `tool_call`, `validate_tool_call` de 10 passos, `BudgetEnforcer`,
  cancelamento e recovery, fila de aprovação. É o "fluxo vertical 1"
  do PROMPT MESTRE §33.
- **Fase 4** (Memória e continuidade) — concluída: crate
  `frederico-memory`, retrieval híbrido (FTS5/BM25 + embeddings),
  classificador LLM pós-resposta, correção/expiração/retomada, painel
  de memória com `ScoreBreakdown` visível. ADRs 0010–0014.
- **Fase 5** (Documentos) — concluída: `workers/document-worker` v0.4.0
  com 8 handlers (`docx.write/read`, `xlsx.write/read`,
  `pdf.write/read/audit`, `ocr.run`) e `frederico-document-kits` com
  WordPro, ExcelPro e PdfPro; auditoria estrutural do PDF é bloqueante.
- **Fase 5b** (Fase de Ligação) — concluída: `frederico-app` como
  camada de composição pura, `JailResolver` (ADR-0022),
  `DocumentWorkerLauncher` (ADR-0023), trait `WorkerInvoker`
  (ADR-0024), kits ligados ao Tool Registry e à casca, crate
  `frederico-e2e`.
- **Fase 6** (Multimodelo e subagentes) — concluída:
  `SpecialistRegistry`, `PermissionSet` fail-closed, `SubagentRunner`
  com orçamento herdado e teto anti-explosão, pipeline sequencial
  persistido, `RunEvent` journal (ADR-0029), UI do Modo Equipe.
- **Fase 7** (Execução isolada) — concluída em 2026-08-13, reaberta e
  reclosada em 2026-08-16 (ver a nota sobre `exec.shell` abaixo):
  sandbox do
  Windows (Job Object + Restricted Token + `CreateProcessAsUserW` +
  env filtrado), crate `frederico-runtimes` (Python 3.12.4 e Node
  20.16.0 portáteis, com SHA-256 pinado), proxy de rede com allowlist
  deny-by-default, e `files.list`/`files.write`/`files.edit` +
  `exec.python`/`exec.node`/`exec.shell` no Tool Registry.
  **Antes de tocar em sandbox, rede ou aprovação, leia a coluna
  "Pendências" da linha da Fase 7 no `status.md`** — ela nomeia
  limitações conhecidas e fixadas em teste.
  **A contradição sobre `exec.shell` foi resolvida em 2026-08-16**, por
  dois ADRs no mesmo dia: o [ADR-0037](docs/decisions/0037-exec-shell-fora-do-catalogo.md)
  mediu que a allowlist de comandos era contornável por qualquer
  separador do `cmd.exe`, tirou a ferramenta do catálogo e reabriu a
  fase; o [ADR-0044](docs/decisions/0044-exec-shell-com-resolucao-propria-de-programa.md)
  (Etapa 2b da Fase 8) a devolveu com um desenho em que **o `cmd.exe`
  não resolve programa** e reclosou a fase. Ao mexer nela, a regra a
  não quebrar é essa: o programa vem de uma lista fechada, resolvido
  por caminho absoluto em `System32` — reintroduzir busca por nome
  reabre execução arbitrária a partir de uma escrita de arquivo no
  workspace.
- **Fase 8** (Modo Desenvolvedor integrado) — **em andamento**. Escopo
  cortado pelo [ADR-0039](docs/decisions/0039-fase-8-escopo-e-etapas.md):
  Git local, GitHub, projetos, marcos e diff; o copiloto (Nino) saiu.
  Etapa 1 (planejamento, 6 ADRs + 3 specs) e Etapa 2b (`exec.shell`)
  fechadas; as demais não iniciadas. **O fechamento da fase ainda está
  travado** pela pré-condição 1 de 2 do ADR-0039 §D2: o workflow
  `CI Nightly` precisa de um run verde citável e acumula falhas desde
  2026-08-05 por secret ausente.
- **Fase 9** (Produção) — não iniciada.

Regra de promoção de fase (`status.md` §"Regra de promoção"): testes da
fase verdes, specs com carimbo de verificação, entrada no `CHANGELOG.md`,
PR de referência e as colunas de E2E preenchidas. Fase não vira
`concluída` por sensação de pronto.

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
7. [`docs/releases/`](./docs/releases/) — narrativas técnicas por fase
   (`fase-5/`, `fase-7/`, `fase-ligacao/`). É onde ficam as sagas de CI,
   os becos sem saída e o motivo de decisões que o ADR não registrou.
8. [`SECURITY.md`](./SECURITY.md) — o que o sandbox protege e,
   principalmente, o que ele **não** protege.

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
- **ADR-0004** — `document-worker` (Fase 5) é Python embutido, com
  Tesseract, fontes e bibliotecas empacotadas; comunicação por named
  pipes, **sem** `localhost`.

São **36 ADRs** hoje (`0001`–`0036`); o próximo a ser escrito é o
`0037`. Além das quatro fundacionais acima, estas são as que mais
restringem trabalho novo:

- **ADR-0022** — `JailResolver`: o jail de sistema de arquivos é
  resolvido por `ConversationId`.
- **ADR-0026** — gate de E2E por fase; é a origem da REGRA 3 e do
  `scripts/check-e2e-gate.ps1`.
- **ADR-0029** — `RunEvent` journal substitui `message_event`.
- **ADR-0032** — redução de escopo da Fase 7 e renumeração da Fase 8.
- **ADR-0033** — política de rede do sandbox (allowlist
  deny-by-default, sem MITM de TLS).
- **ADR-0034** — política de aprovação de `write`/`exec`.
- **ADR-0036** — `SecurityJailResolver` sobre Job Objects do Windows.

## Stack e onde mora o código

- **Casca:** `apps/desktop/` — Tauri 2 + React 18 + TypeScript + Vite.
  Binário Rust em `apps/desktop/src-tauri/`, frontend em
  `apps/desktop/src/`. A camada `apps/desktop/src/services/` é a
  **única** que faz `invoke` no Tauri (regra do ADR-0003).
- **Núcleo:** 18 crates em `crates/` — `core`, `storage`,
  `diagnostics`, `security`, `runtimes`, `provider-engine`,
  `model-catalog`, `agent-engine`, `tool-registry`,
  `execution-engine`, `memory`, `git-engine`, `document-engine`,
  `document-kits`, `process-architecture`, `test-support`, `app`,
  `e2e`. Sem
  dependência de plataforma (ADR-0003). O `crates/app` é a camada de
  composição pura que a casca consome.
- **Contratos compartilhados:** `packages/shared-contracts/`.
- **Workers sidecar:** `workers/document-worker/` (Python embutido,
  ADR-0004).
- **Workspace Cargo:** 20 membros (os 18 crates +
  `packages/shared-contracts` + `apps/desktop/src-tauri`).
- **Documentação:** `docs/architecture/` (21 specs), `docs/decisions/`
  (36 ADRs), `docs/modules/` (21 docs, 1 por crate/worker),
  `docs/releases/` (narrativas por fase), `docs/status.md`.
- **Divergência conhecida:** a REGRA §1.2 prevê `docs/testing/` e
  `docs/security/`, mas esses diretórios não existem. Na prática,
  `testing-strategy.md` e `security-threat-model.md` vivem em
  `docs/architecture/`, e o `SECURITY.md` está na raiz. Quem for
  reconciliar isso mexe na REGRA ou move os arquivos — não invente um
  terceiro caminho.

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
- **Caminho sem espaços:** o repositório é trabalhado em
  `C:\src\Frederico-IA-Studio-review`. Windres e Rollup rejeitam paths
  com espaço, então o repositório não é aberto direto de dentro de
  `OneDrive\Documentos\...`. Se o acesso for por junction ou symlink,
  o `apps/desktop/vite.config.ts` já define
  `resolve.preserveSymlinks: true` pra Rollup não surtar.

## Como verificar (REGRAS §1.10)

```pwsh
# Tudo da fase corrente:
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1

# Guardrail do ADR-0003 (núcleo sem tauri/windows):
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\check-core-purity.ps1

# Gate de E2E por fase (REGRA 3) — mecânico, sem válvula de escape:
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\check-e2e-gate.ps1

# Gates de documentação:
node scripts/check-docs.mjs
node scripts/check-doc-impact.mjs

# Suíte e lint:
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Build do instalador (gera target\release\bundle\nsis\*.exe):
cargo tauri build
```

**`pwsh` não está instalado nesta máquina de desenvolvimento** (só o
Windows PowerShell 5.1). Troque o prefixo por
`powershell -NoProfile -ExecutionPolicy Bypass -File`, ou rode
`& .\scripts\<script>.ps1` de dentro do console. Os scripts rodam nas
duas edições — verificado em 2026-08-16.

**Não cite aqui um número de testes.** A contagem verde por fase mora na
coluna "Evidência" do [`docs/status.md`](./docs/status.md), que é
atualizada no mesmo commit que fecha cada etapa (REGRAS §1.3). Número
duplicado neste arquivo envelhece sem ninguém perceber — foi exatamente
o que aconteceu com a versão anterior desta seção.

O CI vive em `.github/workflows/ci.yml` (todo PR) e
`.github/workflows/ci-nightly.yml` (testes `#[ignore]` que dependem de
rede real). Cobertura só-noturna é mais fraca por natureza e exige twin
determinístico — REGRAS §3.3.

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

- **Subir o binário Tauri em E2E** (`tauri-driver` vs. Playwright vs.
  custom) — ver `docs/architecture/testing-strategy.md` §Não-objetivos.
  O `crates/e2e` de hoje exercita o caminho de produção em Rust, sem
  levantar a janela.
- **Onde rodar testes de "máquina limpa"** (runner self-hosted, GitHub
  Actions, Buildkite, outro).

Já **saiu** desta lista: provedor simulado, decidido pelo **ADR-0008**
(golden files pra fidelidade + gerador determinístico pra patologia, no
nível do transporte).

## Conversas anteriores sobre este projeto

O usuário tem mantido uma conversa por fase. O contexto dessas
conversas (decisões, debates, detalhes que não entraram nos ADRs)
**não vive** neste repositório — vive na memória do Mavis (user
memory + agent memory). Em qualquer sessão nova, faça um
`memory` search por "Frederico IA Studio" pra puxar o que já foi
discutido.
