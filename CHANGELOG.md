# Changelog

Todas as mudanças notáveis deste projeto são documentadas aqui. O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/), e este projeto adere ao [Semantic Versioning](https://semver.org/lang/pt-BR/).

## [Não publicado]

### Adicionado
- `REGRAS-DO-PROJETO.md` com §1.13 nova (estado do documento vs. regra de sincronia, com trava do caminho inverso via `status.md`).
- 9 especificações de arquitetura em `docs/architecture/` (todas como `especificado`): `product-vision`, `development-roadmap`, `software-architecture`, `process-architecture`, `agent-state-machine`, `tool-registry-specification`, `tool-permission-model`, `security-threat-model`, `testing-strategy`.
- `docs/status.md` com a Fase 0 em andamento e as demais como `não iniciada`.
- 4 ADRs em `docs/decisions/`: 0001 spec-vs-as-built, 0002 monorepo-layout, 0003 núcleo desacoplado da casca Tauri, 0004 document-worker em Python embutido.
- 9 stubs de especificações restantes em `docs/architecture/` (a serem aprofundados nas fases 3-6).
- `README.md` honesto (estado atual: nada funciona ainda).
- `.gitignore` para Rust, Node, Tauri, IDE, OneDrive e variáveis de ambiente.

### Notas
- Fase 0 (Fundação documental) em andamento. Nenhum código escrito ainda.
- Próxima fase a iniciar: Fase 1 (Fundação desktop, Tauri + React + Rust + SQLite).
- A especificação de origem do produto (PROMPT MESTRE) não é versionada neste repositório; vive como contexto do projeto e é refletida nos arquivos de `docs/architecture/`.

## [0.0.0] - 2026-07-27

### Adicionado
- Esqueleto do repositório: `LICENSE` e `README.md` mínimo.
