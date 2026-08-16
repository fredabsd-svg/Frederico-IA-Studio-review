<div align="center">

<img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="88" alt="Ícone do Frederico IA Studio" />

# Frederico IA Studio

**Seu estúdio de inteligência artificial no Windows.**

Converse com vários modelos, deixe a IA usar ferramentas reais dentro de um sandbox
e gere documentos Word, Excel e PDF — com auditoria completa de tudo o que ela fez.

<br/>

[![CI](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/actions/workflows/ci.yml/badge.svg)](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/actions/workflows/ci.yml)
![Windows 10/11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-1.75%2B-B7410E?logo=rust&logoColor=white)
![TypeScript](https://img.shields.io/badge/TypeScript-React-3178C6?logo=typescript&logoColor=white)
![Licença GPL-3.0](https://img.shields.io/badge/licen%C3%A7a-GPL--3.0-4c8dae)

[**Status do projeto**](docs/status.md) · [**Roadmap**](docs/architecture/development-roadmap.md) · [**Instalação**](#instalação) · [**Documentação**](#documentação) · [**Segurança**](SECURITY.md)

</div>

---

## O que é o Frederico IA Studio?

Um aplicativo desktop de IA para Windows 10/11, distribuído como instalador `.exe`, que reúne em um único lugar o que normalmente exige várias ferramentas: chat com múltiplos provedores e modelos, execução de ferramentas reais (arquivos, Python, Node) sob isolamento, geração de documentos profissionais e memória de longo prazo — tudo persistido em SQLite local e registrado em trilhas de auditoria.

O princípio inegociável do projeto: **o sistema nunca finge o que não fez.** A interface mostra o estado real — incluindo falhas, cancelamentos e limitações — e este README segue a mesma regra.

## Por que Frederico IA Studio?

<table>
  <tr>
    <td width="50%" valign="top">
      <b>🤖 Multi-modelos</b><br/>
      Catálogo embutido de 13 modelos cobrindo 8 provedores — OpenAI, Anthropic, OpenRouter, DeepSeek, Mistral, NVIDIA NIM, Ollama e LM Studio — mais um provedor simulado para testes. Streaming, custo por uso e cancelamento real; chaves guardadas no Windows Credential Manager (DPAPI).
    </td>
    <td width="50%" valign="top">
      <b>🛠️ Ferramentas reais</b><br/>
      A IA lê, lista, escreve e edita arquivos, executa Python e Node, e roda comandos de inspeção do terminal — sempre dentro do workspace da conversa, com aprovação explícita do usuário para ações de risco.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <b>📄 Documentos profissionais</b><br/>
      Word, Excel e PDF gerados por kits próprios (WordPro, ExcelPro, PdfPro) via worker Python embutido, com validação bloqueante, auditoria PDF/A-2B e OCR.
    </td>
    <td valign="top">
      <b>🧠 Memória que se explica</b><br/>
      Busca híbrida (léxica FTS5 + embeddings) com painel que mostra o <i>score</i> de cada memória recuperada, além de correção, expiração e revisão de conteúdo externo.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <b>🔒 Execução isolada</b><br/>
      Sandbox Windows por invocação: Job Objects, token restrito, ambiente filtrado e rede <i>deny-by-default</i> com allowlist. Runtimes Python e Node portáteis, com SHA-256 pinado.
    </td>
    <td valign="top">
      <b>🔍 Auditoria completa</b><br/>
      Cada ferramenta executada e cada tentativa de acesso à rede ficam em trilhas <i>append-only</i> no SQLite — o que rodou, com quais argumentos, com qual resultado.
    </td>
  </tr>
</table>

## Status do projeto

🚧 **Em desenvolvimento ativo.** O desenvolvimento segue fases com critérios de aceite verificados por testes; a fonte da verdade é [`docs/status.md`](docs/status.md).

| Fase | Escopo | Estado |
|------|--------|--------|
| 0–1 | Fundação documental e desktop (Tauri + Rust + SQLite) | ✅ Concluída |
| 2 | Chat e provedores | ✅ Concluída |
| 3 | Motor de execução e ferramentas | ✅ Concluída |
| 4 | Memória e continuidade | ✅ Concluída |
| 5 | Documentos (Word, Excel, PDF) + Fase de Ligação | ✅ Concluída |
| 6 | Multimodelo e subagentes (Modo Equipe) | ✅ Concluída |
| 7 | Execução isolada (sandbox, runtimes, `exec.*`) | ✅ Concluída |
| 8 | Modo Desenvolvedor integrado (Git, GitHub, diff, projetos, marcos) | 🚧 Em andamento |
| 9 | Produção (assinatura, atualização, release estável) | 🧭 Não iniciada |

> Ainda **não há release publicada**: o instalador NSIS é gerado localmente a partir do código-fonte (ver [Instalação](#instalação)). A suíte de testes do workspace inteira — centenas de testes, incluindo E2E que atravessam o caminho de produção — roda verde no CI a cada PR.

## O que funciona hoje

**Conversa** — chat com streaming de tokens, catálogo embutido de modelos, tradução de erros de provedor para PT-BR com ação sugerida, custo por `Usage`, cancelamento que derruba a conexão HTTP de verdade e *journal* de eventos no SQLite: fechar o app no meio de uma resposta não perde nada.

**Ferramentas** — `files.read` · `files.list` · `files.write` · `files.edit` · `docs.generate` · `docs.inspect` · `exec.python` · `exec.node` · `exec.shell`. Escrita atômica com backup automático e hashes SHA-256; edição recusa alterar arquivo que mudou desde a leitura; ferramentas de risco exigem aprovação do usuário em modal. O `exec.shell` roda uma lista fechada de 11 comandos de inspeção (`dir`, `type`, `findstr`, `sort`, `tree`, …) e recusa sintaxe de shell — sem pipe, sem redirecionamento, sem encadeamento.

**Documentos** — geração real de `.docx`, `.xlsx` e `.pdf` pelos kits WordPro, ExcelPro e PdfPro, com identidade visual própria, fontes embutidas e auditoria estrutural bloqueante (um PDF que não passa nas verificações de PDF/A-2B não é entregue como válido).

**Memória** — captura automática pós-resposta com classificador LLM, retrieval híbrido com orçamento de 2 s e *fallback* léxico, painel de memórias com correção ("corrija para X"), fixação, expiração e fila de revisão para conteúdo vindo de fora.

**Modo Equipe** — pipeline sequencial de estágios com modelos diferentes, subagentes com 8 especialistas embutidos (revisor, pesquisador, testador, validador, sumador, arquiteto, crítico, executor), herança de permissões *fail-closed* e teto de orçamento e profundidade herdado do pai.

**Segurança** — permissões por interseção de perfis (usuário ∩ projeto ∩ assistente), sandbox de processo com *kill tree* garantido, proxy de rede com allowlist por host e auditoria de cada decisão. As limitações conhecidas não são escondidas: estão nomeadas e testadas em [`SECURITY.md`](SECURITY.md).

### O que ainda não existe

Integração Git/GitHub, visualizador de diff, projetos, checkpoints e o copiloto Nino pertencem à **Fase 8**; assinatura de código, atualização automática e release estável pertencem à **Fase 9**. Nenhuma dessas fases começou. O plano completo vive no [roadmap](docs/architecture/development-roadmap.md); o estado real, sempre em [`docs/status.md`](docs/status.md).

## Arquitetura

O núcleo é 100% desacoplado da casca ([ADR-0003](docs/decisions/0003-nucleo-desacoplado-da-casca-tauri.md)): nenhum crate do núcleo depende de `tauri` ou `windows`, e essa pureza é cobrada por script no CI — o que mantém aberta a porta para um modo servidor no futuro.

```text
┌──────────────────────────────────────────────────┐
│            Casca desktop — Tauri 2               │
│          React + TypeScript + Vite               │
├──────────────────────────────────────────────────┤
│               IPC (contratos tipados)            │
├──────────────────────────────────────────────────┤
│            Núcleo Rust (17 crates)               │
│   provider-engine · execution-engine ·           │
│   tool-registry · memory · agent-engine ·        │
│   document-kits · security · storage (SQLite)    │
├────────────────────────┬─────────────────────────┤
│   Runtimes portáteis   │     document-worker     │
│   Python 3.12 · Node   │   (Python embutido:     │
│   20 — SHA-256 pinado  │    docx · xlsx · pdf)   │
└────────────────────────┴─────────────────────────┘
```

<details>
<summary><b>O monorepo, em uma linha por componente</b></summary>
<br/>

| Membro | Papel |
|---|---|
| `core` | Tipos e contratos base do domínio |
| `storage` | SQLite, migrações e repositórios (runs, mensagens, auditoria, memória) |
| `provider-engine` | Adapters de provedores (OpenAI-compat e Anthropic), streaming SSE |
| `model-catalog` | Catálogo embutido de modelos e registro de especialistas |
| `execution-engine` | `RunExecutor`: loop de tool calls, watchdog, recovery, pipeline multimodelo |
| `tool-registry` | Ferramentas, manifestos, permissões e o *jail* de paths por conversa |
| `agent-engine` | Subagentes: orçamento, profundidade, invariantes anti-explosão |
| `memory` | Retrieval híbrido, classificador, correção e expiração |
| `document-engine` / `document-kits` | Contratos de documento e os kits WordPro, ExcelPro, PdfPro |
| `process-architecture` | Ator que gerencia o worker de documentos (pipes Windows) |
| `security` | Sandbox (Job Object, token restrito, env filter), DPAPI, proxy de rede |
| `runtimes` | Bootstrap dos runtimes portáteis Python e Node |
| `diagnostics` | Logs estruturados via `tracing` |
| `app` | Camada de composição pura consumida pela casca e pelos testes E2E |
| `e2e` / `test-support` | Testes ponta a ponta no caminho de produção e utilitários de teste |
| `shared-contracts` | Contratos IPC compartilhados núcleo ↔ casca |
| `apps/desktop` (+ `src-tauri`) | A casca: UI React e o binário Tauri |
| `workers/document-worker` | Worker Python com os handlers de docx/xlsx/pdf/OCR |

</details>

Cada crate tem um documento próprio em [`docs/modules/`](docs/modules/) com API pública, dependências, armadilhas conhecidas e limites explícitos.

## Instalação

Requisitos: **Windows 10/11 64 bits**. Como ainda não há release publicada, o instalador é gerado a partir do código-fonte — para isso você precisa de [Rust](https://www.rust-lang.org/tools/install) 1.75+, [Node](https://nodejs.org/) 20+, MinGW-w64 (toolchain GNU) e o CLI do Tauri 2.x (`cargo install tauri-cli --version "^2.0"`).

```pwsh
git clone https://github.com/fredabsd-svg/Frederico-IA-Studio-review.git
cd Frederico-IA-Studio-review
./scripts/verify.ps1        # compila e valida tudo (a 1ª execução demora alguns minutos)

cd apps/desktop
npm install
cargo tauri build           # gera o instalador NSIS
```

O instalador fica em `target/release/bundle/nsis/`, na raiz do repositório (ex.: `Frederico IA Studio_0.1.0_x64-setup.exe`).

## Desenvolvimento

```pwsh
cd apps/desktop
npm install
cargo tauri dev             # ou: cargo run -p frederico-desktop
```

> **Path sem espaços:** acesse o projeto por uma *junction* sem espaços no caminho
> (ex.: `C:\src\Frederico` → `C:\Users\...\Frederico-IA-Studio-review`).
> Windres (Tauri) e Rollup rejeitam paths com espaço em algumas versões.

Verificações que o CI cobra e que você pode rodar localmente:

```pwsh
./scripts/verify.ps1              # fmt + clippy + testes do workspace + build do front + guardrails
./scripts/check-core-purity.ps1   # só o guardrail do ADR-0003 (núcleo sem tauri/windows)

cd apps/desktop
npm run typecheck; npm run build  # apenas o frontend
```

Antes de tocar no código, leia o contrato do crate em `docs/modules/<crate>.md` e as decisões estruturais em [`docs/architecture/`](docs/architecture/). Cada spec de arquitetura declara no cabeçalho o próprio estado: `especificado`, `parcialmente implementado` ou `implementado`.

## Documentação

**Começando**

- [`docs/status.md`](docs/status.md) — estado real por fase, com evidências e pendências. Em qualquer sessão nova (humana ou IA), é o segundo arquivo a ler, depois das regras.
- [`docs/architecture/development-roadmap.md`](docs/architecture/development-roadmap.md) — as fases, os critérios de "pronto" e o que entra em cada uma.
- [`CHANGELOG.md`](CHANGELOG.md) — histórico detalhado por versão e por fase.

**Arquitetura e decisões**

- [`docs/architecture/`](docs/architecture/) — 21 especificações, da [visão de produto](docs/architecture/product-vision.md) ao [design do sandbox](docs/architecture/windows-sandbox-design.md).
- [`docs/decisions/`](docs/decisions/) — 36 ADRs imutáveis: o que foi decidido, alternativas descartadas e consequências.
- [`docs/modules/`](docs/modules/) — um documento por crate/worker/pacote.

**Segurança e testes**

- [`SECURITY.md`](SECURITY.md) — o modelo de isolamento e, com o mesmo destaque, **o que ele não protege**.
- [`docs/architecture/security-threat-model.md`](docs/architecture/security-threat-model.md) — ameaças mapeadas e cobertura por teste.
- [`docs/architecture/testing-strategy.md`](docs/architecture/testing-strategy.md) — a estratégia de testes e a fronteira do que os E2E cobrem.
- [`docs/releases/`](docs/releases/) — narrativas técnicas dos PRs que fecharam cada fase.

## Contribuição

Antes de contribuir, leia [`REGRAS-DO-PROJETO.md`](REGRAS-DO-PROJETO.md). As regras valem para **pessoas e IAs**, e violação de regra é tratada como defeito — em especial: a documentação descreve o que o código faz hoje, acompanha o código no mesmo commit, e CI vermelho não se contorna. Agentes de IA trabalhando no repositório devem ler também [`AGENTS.md`](AGENTS.md).

## Licença

Distribuído sob a licença **GPL-3.0** — veja [`LICENSE`](LICENSE).

---

<div align="center">
<sub>
<b>Frederico IA Studio</b> — inteligência artificial com ferramentas reais, documentos profissionais e transparência total.<br/>
Desenvolvido com foco em IA, automação e produtividade no Windows.
</sub>
</div>
