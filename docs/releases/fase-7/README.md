# Fase 7 (Modo Desenvolvedor — núcleo: execução isolada): narrativas de release

<!--
Estado: em andamento
Verificado contra o código em: 2026-08-08
Fase correspondente: 7 (Etapa 1 — planejamento)
-->

Índice das narrativas de processo (descrições de PR, lições de
execução) associadas à **Fase 7** do Frederico IA Studio.
Foco no coração da fase — sandbox Windows, runtimes portáteis
e ferramentas de escrita/execução sob isolamento. Git, GitHub,
diff, projetos e checkpoints migraram para a **Fase 8** pelo
[ADR-0032](../../decisions/0032-fase-7-scope-reduction.md).

**Não duplica o `CHANGELOG.md`**, que registra só o efeito pro
usuário (§1.7 do `REGRAS-DO-PROJETO.md`). O que mora aqui é a
história técnica — o que aconteceu em cada PR, quais decisões
foram tomadas no caminho, e o que se aprendeu.

## Índice

| PR | Arquivo | Assunto |
|----|---------|--------|
| **PR de Etapa 1** | [`pr-fase-7-etapa-1-planejamento.md`](./pr-fase-7-etapa-1-planejamento.md) (a ser escrito) | Etapa 1 — 6 ADRs (0031-0036) + 2 specs novos (`runtimes-architecture.md`, `exec-tools-specification.md`) + `windows-sandbox-design.md` aprofundado + 4 specs atualizados (`tool-registry-specification.md`, `development-roadmap.md`, `security-threat-model.md`, `tool-permission-model.md`) + este README + `status.md` + `CHANGELOG.md`. Sem código Rust — o planejamento de uma fase estrutural é código também, e ele tem que virar commit. |
| Etapa 2 | (a ser planejado) | Primitivas do sandbox: `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 4 testes de regressão (com teste de negação) — fecha `I1` do threat model. |
| Etapa 3 | (a ser planejado) | Runtimes embutidos: `crates/runtimes/` com Python + Node portáteis, bootstrap idempotente, resolução de caminho. |
| Etapa 4 | (a ser planejado) | `exec.python` / `exec.node` no `ToolRegistry`, sob sandbox, com aprovação `OneTurn` por default + audit + comando exato. |
| Etapa 5 | (a ser planejado) | `files.write` / `files.edit` / `files.list` no `ToolRegistry`, sob Jail, com semântica de sobrescrita (atomic + backup + audit). |
| Etapa 6 | (a ser planejado) | `exec.shell` com `Denylist` + `Allowlist` + aprovação `OneExecution` sempre. |
| Etapa 7 | (a ser planejado) | Rede do sandbox (proxy local) + fechamento da fase: `crates/security/src/{network,dns_intercept}.rs` + UI de `NetworkAccessLog` + remoção da feature flag `FREDERICO_SANDBOX_V1`. |

## Por que a Fase 7 mudou de escopo

O `docs/architecture/development-roadmap.md` (criado na Fase 0) listava a Fase 7 como um "pacote único" — projetos, arquivos, diff, sandbox, runtimes, Git, GitHub, testes, checkpoints. É um agrupamento legítimo do ponto de vista do **usuário final** ("tudo que o usuário faz como desenvolvedor"), mas é um agrupamento desastroso do ponto de vista de **engenharia de entrega**: reúne 3 naturezas incompatíveis.

A Etapa 1 (este PR) corta o nó górdio antes da Etapa 2 entrar em código:

1. **Execução isolada** (sandbox + runtimes + file ops + exec) — primitivas locais, testes determinísticos em `cargo test --workspace`, cobertura E2E de PR via `crates/e2e/tests/`. Sem rede, sem segredo, sem serviço externo. **Fase 7.**
2. **Integração com serviço externo autenticado** (Git local + GitHub + push + PR) — token no DPAPI, operação destrutiva remota, E2E precisa de rede + secret + serviço GitHub real, **só noturno** (regra D2 do ADR-0026: cobertura fraca por natureza). **Fase 8.**
3. **UI de projeto** (projetos, diff, checkpoints, run UI) — frontend React, navegação, persistência, polimento visual. **Fase 8** (subdivisão).

A análise completa está no [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md). O resumo: colar as 3 naturezas em uma fase tem 3 consequências ruins — critério de done impossível de fechar honestamente ("PR criado pelo app" só roda noturno, e fase fica `em andamento` por meses ou é marcada `concluída` por pressão), duas lentes de revisão no mesmo PR (sandbox + OAuth), e infraestrutura de CI diferente (sandbox precisa Windows runner, GitHub precisa secret + serviço real).

## Como esta fase é dividida

| Etapa | Status | Próxima | Bloqueia | Foco |
|---|---|---|---|---|
| **Etapa 1 — Planejamento** | **em revisão** (este PR) | Etapa 2 | nenhuma | 6 ADRs + 2 specs novos + 4 specs atualizados + `docs/releases/fase-7/README.md` + `status.md` + `CHANGELOG.md` |
| Etapa 2 — Primitivas do sandbox | não iniciada | Etapa 3 | nenhuma | `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 4 testes de regressão (com teste de negação) |
| Etapa 3 — Runtimes embutidos | não iniciada | Etapa 4 | nenhuma | `crates/runtimes/` com Python + Node portáteis, bootstrap idempotente, resolução de caminho |
| Etapa 4 — `exec.python` / `exec.node` no registro | não iniciada | Etapa 5 | Etapa 2 + Etapa 3 | `FilesExecTool::Python` + `FilesExecTool::Node` com aprovação `OneTurn` + audit + comando exato |
| Etapa 5 — `files.write` / `files.edit` / `files.list` | não iniciada | Etapa 6 | Etapa 2 | `FilesWriteTool` + `FilesEditTool` + `FilesListTool` com Jail + atomicidade + backup + audit |
| Etapa 6 — `exec.shell` com allowlist | não iniciada | Etapa 7 | Etapa 2 + Etapa 4 | `TerminalPermission::Allowlist` + `Denylist` + aprovação `OneExecution` |
| Etapa 7 — Rede do sandbox (proxy) + fechamento | não iniciada | — | Etapa 2 | `crates/security/src/network.rs` + `dns_intercept.rs` + UI de `NetworkAccessLog` + remoção da feature flag `FREDERICO_SANDBOX_V1` |

**Regra de teste de negação (do prompt do user, 2026-08-08):** cada etapa da 2 em diante entrega **pelo menos um teste que prova o que o sandbox bloqueia**, não só o que ele permite. Sandbox se prova impedindo, não funcionando. O `windows-sandbox-design.md` §"Mapa de E2E planejado por etapa" lista os 12 testes planejados (1-2 por etapa).

**Regra de PRs empilhadas:** PR aberta depois que a anterior entrou em main, sempre. Esta fase segue o mesmo padrão da Fase 6 e da Fase de Ligação (memory cross-project de 2026-08-02: "PRs empilhadas, 3ª ocorrência").

**Carry-over da Fase de Ligação:** a Etapa 7 da Fase de Ligação foi REMOVIDA daquela fase por depender de fase futura. A pendência ("`SecurityJailResolver` com Job Objects para tree-kill") é agora trabalho da **Etapa 2** da Fase 7 (ADR-0036).

## Decisões críticas (que moldam tudo o que vem depois)

Os 6 ADRs desta fase são interdependentes e fecham na seguinte ordem:

1. **[ADR-0032](../../decisions/0032-fase-7-scope-reduction.md)** — escopo. Sem isso, a fase tem critério de done impossível.
2. **[ADR-0031](../../decisions/0031-fase-7-isolation-model-windows.md)** — modelo de isolamento (3 camadas combinadas: Jail + Job Object + Restricted Token + env zeroed; AppContainer adiado). Sem isso, o executor é construído em cima de primitiva errada.
3. **[ADR-0036](../../decisions/0036-security-jail-resolver-windows-job-objects.md)** — `SecurityJailResolver` (carry-over da Fase de Ligação, Etapa 2). Sem isso, não há tree-kill real.
4. **[ADR-0033](../../decisions/0033-sandbox-network-policy.md)** — política de rede (deny-by-default, proxy local, log visível). Sem isso, o filho do sandbox conecta direto à internet do host.
5. **[ADR-0034](../../decisions/0034-fase-7-write-exec-approval-policy.md)** — política de aprovação (default deny, escopo variável, comando exato, revogação imediata). Sem isso, o usuário não tem como consentir com granularidade.
6. **[ADR-0035](../../decisions/0035-fase-7-file-ops-overwrite-semantics.md)** — semântica de sobrescrita (atomic write, backup `.bak`, audit com hashes). Sem isso, `files.write` é um vetor de perda de dados.

A Etapa 1 fecha os 6 ADRs em **um único PR** (este). É a mesma forma do Etapa 1 da Fase 6 (que planejou o Pipeline Sequencial, ADR-0028) e do Etapa 1 da Fase de Ligação (que conectou motor à casca, ADR-0022).

## O que esta fase **NÃO** é

- **Não é "Modo Desenvolvedor" no sentido completo do roadmap original.** Git, GitHub, diff, projetos, checkpoints, UI de projeto são Fase 8.
- **Não é execução de produção** de `pip install` em primeiro launch. A Etapa 4 entra com rede bloqueada (degradação declarada); a Etapa 7 da Fase 7 liga o proxy com allowlist.
- **Não é container** (Docker/runc). O sandbox é primitivas do Windows combinadas, sem contêiner. **Não é VPN, não é firewall, não é kernel hardening.** É defesa em profundidade contra a classe "filho malicioso/invadido" das ameaças STRIDE, com lacunas explícitas documentadas em `security-threat-model.md` §"O que o sandbox NÃO protege".
- **Não é cross-platform.** Windows only (mesma restrição do sandbox do projeto). Linux é roadmap, retorna `Err(NotSupported)` se chamado.

## Onde ler mais

- **O que esta fase entrega:** [`windows-sandbox-design.md`](../../architecture/windows-sandbox-design.md) (aprimorado nesta Etapa 1).
- **O sandbox que envolve `exec.*`:** [`windows-sandbox-design.md`](../../architecture/windows-sandbox-design.md).
- **Os runtimes que `exec.python` / `exec.node` consomem:** [`runtimes-architecture.md`](../../architecture/runtimes-architecture.md) (novo, Etapa 1).
- **As ferramentas `exec.*` em si:** [`exec-tools-specification.md`](../../architecture/exec-tools-specification.md) (novo, Etapa 1).
- **O catálogo do Tool Registry:** [`tool-registry-specification.md`](../../architecture/tool-registry-specification.md) (atualizado, §"Status por ferramenta da Fase 7").
- **As 6 decisões:** [`docs/decisions/0031-*.md`](../../decisions/0031-fase-7-isolation-model-windows.md) a [`0036-*.md`](../../decisions/0036-security-jail-resolver-windows-job-objects.md).
- **O que a fase 8 absorveu:** [`docs/architecture/development-roadmap.md`](../../architecture/development-roadmap.md) (atualizado) + [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md).
- **O threat model atualizado com o que o sandbox NÃO protege:** [`security-threat-model.md`](../../architecture/security-threat-model.md) §"O que o sandbox NÃO protege".

## Histórico de revisão

- 2026-08-08 — Etapa 1 (planejamento) em revisão. 6 ADRs (0031-0036) + 2 specs novos + 4 specs atualizados + este README + `status.md` + `CHANGELOG.md`. Sem código Rust — o planejamento de uma fase estrutural é código também, e ele tem que virar commit. Validação pelo user (via `ask_user`): "A — planejamento primeiro" + "C — tirar Git/GitHub da Fase 7" + a regra de "teste de negação" por etapa. A precedência é a mesma do Etapa 1 da Fase 6 (que planejou o Pipeline Sequencial, ADR-0028): cortar escopo na Etapa 1 é o que destrava a fase.
