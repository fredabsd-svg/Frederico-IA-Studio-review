# Changelog

Todas as mudanças notáveis deste projeto são documentadas aqui. O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/), e este projeto adere ao [Semantic Versioning](https://semver.org/lang/pt-BR/).

## [Não publicado]

### Adicionado
- Fase 0 (Fundação documental) fechada: 9 specs, 4 ADRs, `REGRAS-DO-PROJETO.md` com §1.13, `status.md` disciplinado, `README.md` honesto, `.gitignore`. Promovida para `concluída` em `docs/status.md`.
- Fase 1 (Fundação desktop) fechada: monorepo estruturado conforme ADR-0002 (`apps/`, `crates/`, `workers/`, `packages/`, `tests/`); crates do núcleo `frederico-core`, `frederico-storage`, `frederico-diagnostics`, `frederico-security`; casca Tauri 2 + React 18 + TypeScript + Vite em `apps/desktop/`; migração SQLite inicial (`0001_initial.sql`) com tabela `app_info`; `services/` como única camada de IPC no frontend (regra do ADR-0003); camada de contratos compartilhados em `packages/shared-contracts/`; `docs/modules/{core,storage,diagnostics,security,desktop}.md`; scripts `scripts/verify.ps1` e `scripts/check-core-purity.ps1`; CI mínimo em `.github/workflows/ci.yml`. Suíte de testes 15/15 verde; `cargo clippy --workspace -- -D warnings` limpo; `cargo tauri build` empacota `Frederico IA Studio_0.1.0_x64-setup.exe` (~3 MB) via NSIS.

### Alterado
- `docs/architecture/software-architecture.md` promovido de `especificado` para `parcialmente implementado`.
- `docs/architecture/process-architecture.md` promovido de `especificado` para `parcialmente implementado`.
- `README.md` atualizado para refletir o estado real: app abre, navegação funciona, SQLite cria schema, instalador empacota (Fase 1 fechada).

### Notas
- Fase 0 (Fundação documental) concluída.
- Fase 1 (Fundação desktop) concluída — vertical fino, fim a fim. O usuário agora consegue instalar o `.exe` resultante e abrir o app.
- Próxima fase a iniciar: Fase 2 (Chat e provedores — adaptadores, catálogo, credenciais, streaming, conversas, custos, cancelamento).
- O toolchain de build é Rust stable GNU + MinGW-w64 (não MSVC), por simplicidade no ambiente deste repo. O CI no GitHub Actions usa o toolchain MSVC padrão; ambas as toolchains produzem o mesmo instalador.
- A especificação de origem do produto (PROMPT MESTRE) não é versionada neste repositório; vive como contexto do projeto e é refletida nos arquivos de `docs/architecture/`.

## [0.0.0] - 2026-07-27

### Adicionado
- Esqueleto do repositório: `LICENSE` e `README.md` mínimo.
