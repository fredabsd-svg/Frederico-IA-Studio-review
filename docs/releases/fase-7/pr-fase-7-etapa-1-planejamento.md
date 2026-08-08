# PR de Etapa 1 da Fase 7: planejamento (sem código Rust) — 6 ADRs + 2 specs novos + 4 specs atualizados

> **Status: a ser escrito quando o PR for aberto.** Este arquivo é a narrativa técnica do PR `fase-7-etapa-1-planejamento` que fecha a Etapa 1 da Fase 7. O PR ainda não foi aberto (a Etapa 1 é a fase de planejamento, e o commit acontece antes do push).

> **O que o PR contém (resumo):**
>
> - 6 ADRs novos (0031-0036) — as decisões estruturais da Fase 7.
> - 2 specs novos (`runtimes-architecture.md` + `exec-tools-specification.md`).
> - 4 specs atualizados (`windows-sandbox-design.md` aprofundado, `development-roadmap.md` re-numerado, `security-threat-model.md` com §"O que o sandbox NÃO protege", `tool-registry-specification.md` com §"Status por ferramenta da Fase 7").
> - `docs/releases/fase-7/README.md` (índice de narrativas da fase).
> - `docs/status.md` (Fase 7 promovida a `em andamento` com Etapa 1 fechada; Fase 8 re-numerada).
> - `CHANGELOG.md` (entrada "Fechado — Fase 7, Etapa 1 (planejamento)").

> **A ser preenchido com:**
>
> 1. **Contexto (transparente)** — por que a Fase 7 mudou de escopo, linkando o ADR-0032.
> 2. **O que entra** — 6 ADRs + 2 specs + 4 atualizações + README + status + CHANGELOG, na ordem dos commits.
> 3. **Verificações** — saída dos gates (`cargo fmt`, `cargo clippy`, `node scripts/check-docs.mjs`, `./scripts/check-e2e-gate.ps1`, `./scripts/check-core-purity.ps1`, `node scripts/check-doc-impact.mjs`). Sem testes Rust novos (planejamento puro).
> 4. **Lições de processo** — branching da main (regra 3ª ocorrência de PRs empilhadas), ordem dos commits (ADRs primeiro, specs depois, status/CHANGELOG por último).
> 5. **Próxima etapa** — Etapa 2 (Primitivas do sandbox: `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 4 testes de regressão com teste de negação).
