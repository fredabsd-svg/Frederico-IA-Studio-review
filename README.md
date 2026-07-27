# Frederico IA Studio

> Antes de contribuir, leia [`REGRAS-DO-PROJETO.md`](./REGRAS-DO-PROJETO.md).

## O que é

Estúdio de IA desktop para Windows 10/11, distribuído como instalador `.exe`, que conversa com múltiplos provedores e modelos, usa ferramentas reais, gera documentos profissionais (Word, Excel, PDF) e ajuda a desenvolver software — com auditoria completa do que a IA fez.

## O que funciona hoje

**Fase 1 (Fundação) — vertical fino, fim a fim.** A aplicação desktop **abre**, a **navegação** entre rotas funciona, o **SQLite** cria o schema inicial e a migração roda, o **IPC** entre a casca e o núcleo responde a `ping` e `get_app_info`, e o **instalador** empacota via Tauri bundler (NSIS, quando disponível; binário standalone caso contrário).

Acompanhe o que está em andamento em [`docs/status.md`](./docs/status.md).

## Como instalar

**Para usuários finais:** o instalador empacotado está em `apps/desktop/src-tauri/target/release/bundle/` após `cargo tauri build`. Por enquanto apenas o bundle NSIS é gerado (Windows 10/11 64 bits).

**Para desenvolvimento:** clone o repositório, instale Rust (1.75+), Node (20+) e MinGW-w64 (toolchain GNU), e rode:

```pwsh
git clone <repo>
cd Frederico-IA-Studio-review
./scripts/verify.ps1
```

A primeira compilação demora alguns minutos. Em seguida:

```pwsh
cd apps/desktop
npm install
cargo tauri dev    # ou: cargo run -p frederico-desktop
```

> O projeto é acessado preferencialmente via uma junction sem espaços
> no path (ex: `C:\src\Frederico` → `C:\Users\...\OneDrive\...\Studio review\Frederico-IA-Studio-review`).
> Windres (Tauri) e Rollup rejeitam paths com espaço em algumas versões.

## Como desenvolver

Consulte `docs/modules/<crate>.md` para o contrato de cada crate do núcleo, e `docs/architecture/` para as decisões estruturais. O guardrail do ADR-0003 (núcleo sem `tauri`/`windows`) é cobrado por `scripts/check-core-purity.ps1`.

```pwsh
# Tudo:
./scripts/verify.ps1

# Só o guardrail do núcleo:
./scripts/check-core-purity.ps1

# Apenas o frontend:
cd apps/desktop; npm run typecheck; npm run build
```

## Onde está o resto da documentação

- [`REGRAS-DO-PROJETO.md`](./REGRAS-DO-PROJETO.md) — regras que valem para qualquer IA ou pessoa trabalhando no repositório. **Leia primeiro.**
- [`docs/architecture/`](./docs/architecture/) — especificações da arquitetura. Cada arquivo começa com `Estado: especificado | parcialmente implementado | implementado` e descreve o que o sistema **será** ou **é**.
- [`docs/decisions/`](./docs/decisions/) — ADRs (decisões estruturais e por quê).
- [`docs/development-roadmap.md`](./docs/architecture/development-roadmap.md) — fases de desenvolvimento e o que entra em cada uma.
- [`docs/modules/`](./docs/modules/) — um documento por crate/worker/pacote (REGRAS §1.4).
- [`docs/status.md`](./docs/status.md) — estado real por fase. **Segundo arquivo a ler em qualquer sessão nova**, depois das regras.
- [`CHANGELOG.md`](./CHANGELOG.md) — histórico por versão.
- [`LICENSE`](./LICENSE) — licença do código.
