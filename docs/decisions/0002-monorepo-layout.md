# 0002 — Layout do monorepo

## Contexto

O `PROMPT MESTRE` §5.4 sugere uma estrutura de pastas com `apps/`, `crates/`, `workers/`, `packages/`, `tests/` e `docs/`, cada qual com subdivisões específicas. Antes de criar a primeira linha de código, é preciso decidir se seguimos essa árvore literalmente, se a adaptamos, e como organizamos o workspace Rust.

## Decisão

Adotamos a árvore do `PROMPT MESTRE` §5.4 **literalmente** para os diretórios de primeiro nível:

```text
apps/desktop/           # casca Tauri + React + TypeScript + Vite
crates/                 # núcleo Rust (um crate por subsistema)
workers/                # sidecars empacotados (executáveis separados)
packages/               # código compartilhado entre casca e workers (TS/Rust)
docs/                   # toda a documentação
tests/                  # suítes (unit, integration, e2e, security, recovery, documents, installer)
```

Adicionalmente:

- **Um único workspace Rust na raiz** (`Cargo.toml` workspace) agrega todos os crates em `crates/`, `workers/` e `packages/` (os que forem Rust). Frontend em `apps/desktop/` tem seu próprio `package.json` independente.
- **Um crate nasce com responsabilidade clara e teste próprio** (`PROMPT MESTRE` §5.3 "Pragmatismo"). Não criamos crate por substantivo da especificação. A lista inicial de crates prevista está em `docs/architecture/software-architecture.md`; crates novos só são extraídos quando um existente crescer demais.
- **`docs/modules/<nome>.md`** é criado no mesmo commit em que o crate/worker/pacote nasce (`REGRAS §1.4`).

## Alternativas descartadas

- **Multi-repo por crate.** Descartada: mudanças cross-cutting (ex: alterar um tipo compartilhado entre motor e tool-registry) viram N PRs em N repositórios, e o versionamento de contratos entre eles vira pesadelo.
- **Workspace Rust sem diretório `crates/`** (código Rust solto na raiz). Descartada: polui a raiz e conflita com a presença de `apps/desktop/`, `workers/` e `docs/`.
- **Adotar a árvore mas com subdivisões extras que o §5.4 não pede** (ex: `crates/agents/`, `crates/agents/subagents/`). Descartada: viola a recomendação de pragmatismo do §5.3 — pastas vazias envelhecem mal.

## Consequências

**Mais fácil:**
- Toda nova área do produto tem lugar óbvio para nascer.
- O CI tem diretórios claros para varrer por testes, documentação e artefatos.
- A navegação por IA ou humano é consistente: "se é Rust do núcleo, está em `crates/`".

**Mais difícil:**
- Builds incrementais precisam ser inteligentes para não recompilar tudo sempre — provavelmente exige configuração de `cargo` workspaces com afinidade e exclusão de crates pesados por padrão.
- A árvore pode ficar profunda conforme o produto cresce; vale auditar periodicamente se algum subdiretório está só com um arquivo e merece merge.
- Qualquer desvio da árvore (ex: criar `crates/agents/old/`) precisa de ADR ou vira defeito.
