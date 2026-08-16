# Fase 7 (Modo Desenvolvedor — núcleo: execução isolada): narrativas de release

<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-16
Fase correspondente: 7 (Etapas 1-6+1 fechadas; Etapa 7 reaberta pelo ADR-0037 — fase `em andamento`)
-->

Índice das narrativas de processo (descrições de PR, lições de
execução) associadas à **Fase 7** do Frederico IA Studio.
Foco no coração da fase — sandbox Windows, runtimes portáteis
e ferramentas de escrita/execução sob isolamento. Git, GitHub,
diff, projetos e checkpoints migraram para a **Fase 8** pelo
[ADR-0032](../../decisions/0032-fase-7-scope-reduction.md).

**A fase esteve concluída por dois dias e foi reaberta.** Ela
fechou em 2026-08-14 (PR #52, `e535b7f`) com 7 etapas — incluindo
as duas intercaladas que o plano não previa (5+ e 6+1). Em
2026-08-16 o [ADR-0037](../../decisions/0037-exec-shell-fora-do-catalogo.md)
mediu o `exec.shell` e o tirou do catálogo: a allowlist de
comandos não era uma barreira. Como o critério de "done" da fase
cita a ferramenta nominalmente, a **Etapa 7 reabre** e a fase
volta a `em andamento`. Os três requisitos para fechá-la de novo
estão no §D5 daquele ADR.

Este README manteve por dois dias a versão anterior desta
história, e o registro de que ela existiu fica aqui de propósito.
O `docs/status.md` é a fonte da verdade do **estado**; este
arquivo é a fonte da verdade do **percurso** — e o percurso
inclui ter fechado cedo demais.

**Não duplica o `CHANGELOG.md`**, que registra só o efeito pro
usuário (§1.7 do `REGRAS-DO-PROJETO.md`). O que mora aqui é a
história técnica — o que aconteceu em cada PR, quais decisões
foram tomadas no caminho, e o que se aprendeu.

## Índice

| PR | Arquivo | Assunto |
|----|---------|--------|
| **PR de Etapa 1** | [`pr-fase-7-etapa-1-planejamento.md`](./pr-fase-7-etapa-1-planejamento.md) (PR #41, `f7d1ab3`) | Etapa 1 — 6 ADRs (0031-0036) + 2 specs novos (`runtimes-architecture.md`, `exec-tools-specification.md`) + `windows-sandbox-design.md` aprofundado + 4 specs atualizados (`tool-registry-specification.md`, `development-roadmap.md`, `security-threat-model.md`, `tool-permission-model.md`) + este README + `status.md` + `CHANGELOG.md`. Sem código Rust — o planejamento de uma fase estrutural é código também, e ele tem que virar commit. |
| Etapa 2 | sem narrativa própria (PR #42, `930c098`) — registro em `CHANGELOG.md` e `status.md` | Primitivas do sandbox: `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 4 testes de regressão (com teste de negação) — fecha `I1` do threat model. |
| Etapa 3 | sem narrativa própria (PR #43, `da9e98f2`) — registro em `CHANGELOG.md` e `status.md` | Runtimes embutidos: `crates/runtimes/` com Python 3.12.4 + Node 20.16.0 portáteis, SHA-256 pinned, bootstrap idempotente, resolução de caminho. |
| Etapa 4 | [`etapa-4-narrativa.md`](./etapa-4-narrativa.md) (PR #44) | `exec.python` / `exec.node` no `ToolRegistry`, sob sandbox, com aprovação `OneTurn` por default + audit + comando exato. **Saga do CI flake (5 falhas → verde) documentada.** **Achado crítico:** o `SecurityJailResolver` v1 (Job + Token + EnvFilter) **NÃO tem path safety enforcement** — o test `child_cannot_write_outside_workspace` provou que python escapa via `open('..\\evil.txt')` relativo ao workdir. Etapa 5+ adiciona AppContainer/ACLs. |
| Etapa 5 | [`etapa-5-narrativa.md`](./etapa-5-narrativa.md) (PR #45 + #46) | `files.write` / `files.edit` / `files.list` no `ToolRegistry`, sob Jail, com semântica de sobrescrita (atomic + backup + audit). **Regra do user 2026-08-10: 2 PRs, não 3** — `files.write` e `files.edit` compartilham a mesma máquina de escrita atômica, backup e auditoria. PR #45 = `files.list` (read-only, sem approval); PR #46 = `files.write` + `files.edit` (destrutivo, exige approval + `expected_sha256` race defense). |
| Etapa 5+ | sem narrativa própria (PR #47, `9610a53` + `435a755`) — registro em `CHANGELOG.md` | Path safety no `SecurityJailResolver`. Fechada **com ressalva**: `exec.python`/`exec.node` saíram do catálogo até a barreira de caminho existir de verdade — a regra "capacidade incompleta é capacidade indisponível" aplicada pela primeira vez na fase. |
| Etapa 6 + 6+1 | [`etapa-6-7-narrativa.md`](./etapa-6-7-narrativa.md) (PR #51, `2fbaf73`) | Rede do sandbox: proxy HTTP/CONNECT local com deny-by-default + `network_audit` + wiring real em `exec.python`/`exec.node`. **4 causas-raiz empilhadas** que faziam o wiring parecer funcionar sem funcionar. |
| Etapa 7 | [`etapa-6-7-narrativa.md`](./etapa-6-7-narrativa.md) (PR #52, `e535b7f`; **reaberta** pelo ADR-0037) | Entregou `exec.shell` com denylist + allowlist, mais a allowlist de rede por perfil e o descarte do DNS intercept. A parte de rede permanece; **`exec.shell` foi removido do catálogo dois dias depois** — a allowlist era contornável por qualquer separador do `cmd.exe`. A etapa fecha quando o §D5 do ADR-0037 fechar. |

**Sobre a numeração:** o plano da Etapa 1 previa `exec.shell` como
Etapa 6 e rede como Etapa 7. Na prática a ordem se inverteu — a
rede fechou primeiro (PR #51) e `exec.shell` fechou a fase (PR
#52). As linhas acima descrevem o que **foi entregue**, não o que
foi planejado; o `windows-sandbox-design.md` já carrega a mesma
correção na tabela dele.

## Por que a Fase 7 mudou de escopo

O `docs/architecture/development-roadmap.md` (criado na Fase 0) listava a Fase 7 como um "pacote único" — projetos, arquivos, diff, sandbox, runtimes, Git, GitHub, testes, checkpoints. É um agrupamento legítimo do ponto de vista do **usuário final** ("tudo que o usuário faz como desenvolvedor"), mas é um agrupamento desastroso do ponto de vista de **engenharia de entrega**: reúne 3 naturezas incompatíveis.

A Etapa 1 (PR #41) cortou o nó górdio antes da Etapa 2 entrar em código:

1. **Execução isolada** (sandbox + runtimes + file ops + exec) — primitivas locais, testes determinísticos em `cargo test --workspace`, cobertura E2E de PR via `crates/e2e/tests/`. Sem rede, sem segredo, sem serviço externo. **Fase 7.**
2. **Integração com serviço externo autenticado** (Git local + GitHub + push + PR) — token no DPAPI, operação destrutiva remota, E2E precisa de rede + secret + serviço GitHub real, **só noturno** (regra D2 do ADR-0026: cobertura fraca por natureza). **Fase 8.**
3. **UI de projeto** (projetos, diff, checkpoints, run UI) — frontend React, navegação, persistência, polimento visual. **Fase 8** (subdivisão).

A análise completa está no [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md). O resumo: colar as 3 naturezas em uma fase tem 3 consequências ruins — critério de done impossível de fechar honestamente ("PR criado pelo app" só roda noturno, e fase fica `em andamento` por meses ou é marcada `concluída` por pressão), duas lentes de revisão no mesmo PR (sandbox + OAuth), e infraestrutura de CI diferente (sandbox precisa Windows runner, GitHub precisa secret + serviço real).

## Como esta fase é dividida

| Etapa | Status | Próxima | Bloqueia | Foco |
|---|---|---|---|---|
| **Etapa 1 — Planejamento** | **concluída** (PR #41, `f7d1ab3`) | Etapa 2 | nenhuma | 6 ADRs + 2 specs novos + 4 specs atualizados + `docs/releases/fase-7/README.md` + `status.md` + `CHANGELOG.md` |
| **Etapa 2 — Primitivas do sandbox** | **concluída** (PR #42, `930c098`) | Etapa 3 | nenhuma | `crates/security/src/{job_object,restricted_token,env_filter,jail}.rs` + 4 testes de regressão (com teste de negação) |
| **Etapa 3 — Runtimes embutidos** | **concluída** (PR #43, `da9e98f2`) | Etapa 4 | nenhuma | `crates/runtimes/` com Python + Node portáteis, bootstrap idempotente, resolução de caminho |
| **Etapa 4 — `exec.python` / `exec.node` no registro** | **concluída** (PR #44, `66bb37a` → CI run `#31384435313` verde após 5 flakes) | Etapa 5 | Etapa 2 + Etapa 3 | `FilesExecTool::Python` + `FilesExecTool::Node` com aprovação `OneTurn` + audit + comando exato. Saga do CI flake e achado do path safety na narrativa. |
| **Etapa 5 — `files.write` / `files.edit` / `files.list`** | **concluída** (PR #45 files.list + PR #46 files.write+files.edit) | Etapa 6 | Etapa 2 | `FilesWriteTool` + `FilesEditTool` + `FilesListTool` com Jail + atomicidade + backup + audit. **Regra do user 2026-08-10: 2 PRs, não 3** — as ferramentas destrutivas juntas num só diff. |
| **Etapa 5+ — Path safety no `SecurityJailResolver`** | **concluída com ressalva** (PR #47, `9610a53`; complemento em `435a755`) | Etapa 6 | Etapa 2 + Etapa 4 | Barreira de caminho real no Jail. `exec.python`/`exec.node` **removidos do catálogo** até ela existir — não iam ficar expostos com o escape de `..\` que a Etapa 4 provou por teste |
| **Etapa 6 + 6+1 — Rede do sandbox (proxy) + wiring** | **concluída** (PR #51, `2fbaf73`) | Etapa 7 | Etapa 2 + Etapa 4 | `crates/security/src/network.rs` + `network_audit_sink.rs` + migration `0031_network_audit.sql` + `NetworkProxyGuard` RAII injetando `HTTP_PROXY`/`HTTPS_PROXY` no filho |
| **Etapa 7 — `exec.shell` + allowlist de rede por perfil** | **reaberta** (PR #52 `e535b7f` fechou; ADR-0037 reabriu em 2026-08-16) | — | Etapa 2 + Etapa 4 + Etapa 6 | A allowlist de rede por perfil **permanece entregue**. `FilesExecShellTool` saiu do catálogo; volta quando os 3 requisitos do ADR-0037 §D5 fecharem |

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

A Etapa 1 fechou os 6 ADRs em **um único PR** (#41). É a mesma forma do Etapa 1 da Fase 6 (que planejou o Pipeline Sequencial, ADR-0028) e do Etapa 1 da Fase de Ligação (que conectou motor à casca, ADR-0022).

Nenhum dos 6 sobreviveu intacto ao contato com o código, e isso está registrado neles próprios: o **ADR-0033** perdeu o DNS intercept (§D1 corrigido) e a feature flag (§D7 fechado); o **ADR-0034** ganhou nota de fechamento com 3 divergências entre o plano e o que o código faz; o **ADR-0031** teve o AppContainer adiado e substituído pela path safety da Etapa 5+. Decisão estrutural que não é revisitada quando o código contradiz é decisão que virou dogma.

## O que esta fase **NÃO** é

- **Não é "Modo Desenvolvedor" no sentido completo do roadmap original.** Git, GitHub, diff, projetos, checkpoints, UI de projeto são Fase 8.
- **Não é execução de produção** de `pip install` em primeiro launch. A Etapa 4 entrou com rede bloqueada (degradação declarada); a Etapa 6 ligou o proxy com allowlist, e a Etapa 7 passou a carregar a allowlist do perfil TOML em vez de deixá-la hardcoded vazia.
- **Não fecha DNS exfiltration.** O intercept foi construído, testado fim-a-fim e **removido** — o Windows não consulta a interface Loopback pra resolver DNS, então o mecanismo nunca protegeu nada. Código que resolve hostname via `getaddrinfo()` direto continua fora da allowlist. Ver a narrativa das Etapas 6/7 e o `SECURITY.md` §"Rede".
- **Não é container** (Docker/runc). O sandbox é primitivas do Windows combinadas, sem contêiner. **Não é VPN, não é firewall, não é kernel hardening.** É defesa em profundidade contra a classe "filho malicioso/invadido" das ameaças STRIDE, com lacunas explícitas documentadas em `security-threat-model.md` §"O que o sandbox NÃO protege".
- **Não é cross-platform.** Windows only (mesma restrição do sandbox do projeto). Linux é roadmap, retorna `Err(NotSupported)` se chamado.

## O que a fase entregou — e o que ela deixou aberto

Seis etapas e meia fechadas em 12 PRs. O que existe no produto
hoje:

- **Sandbox de 3 camadas** (Jail + Job Object + Restricted Token + env zeroed), com path safety real depois da Etapa 5+.
- **Runtimes portáteis** Python 3.12.4 e Node 20.16.0, SHA-256 pinned, sem depender do PATH da máquina.
- **8 ferramentas** no catálogo: `files.read` / `files.list` / `files.write` / `files.edit`, `docs.generate` / `docs.inspect`, `exec.python` / `exec.node`.
- **Rede deny-by-default** para o filho do sandbox, com auditoria append-only em `network_audit` e allowlist vinda do perfil TOML do usuário ∩ projeto.

**O que a fase anunciou e teve de retirar:** `exec.shell` esteve
no catálogo entre 14 e 16 de agosto. O
[ADR-0037](../../decisions/0037-exec-shell-fora-do-catalogo.md)
mediu e removeu: `is_allowed` validava só o primeiro token, mas o
comando inteiro ia para o `cmd.exe /c`, que trata `&`, `&&`, `|`
e `||` como separadores — `ver` sozinho era recusado, `echo
marcador & ver` executava os dois. E 7 dos 9 binários da
allowlist são MSYS2, que morrem sob o rótulo de integridade baixa
com `STATUS_ACCESS_DENIED`. A barreira não impedia o que devia
nem permitia o que prometia.

É a terceira aplicação da regra **capacidade incompleta é
capacidade indisponível** nesta fase — depois de `exec.python`/
`exec.node` (Etapa 5+) e do DNS intercept (Etapa 6). A diferença,
desta vez, é que a capacidade já tinha sido anunciada como
concluída, e a fase teve de voltar a `em andamento` por causa
disso.

**As lacunas ficam nomeadas, não escondidas** — todas em
`SECURITY.md` e no `security-threat-model.md`, e todas repetidas
na coluna "Pendências" da Fase 7 no `status.md`:

| Lacuna | Por que continua aberta |
|---|---|
| **DNS exfiltration** | O intercept foi construído e removido: o Windows não resolve DNS pela interface Loopback. Fechar exige filtro no nível de processo (WFP/WDAC). |
| **Bypass por socket raw** | `HTTP_PROXY` é convenção, não imposição. Fixado em teste (`e2e_network_raw_socket_bypasses_proxy_documented`) — o teste afirma o bypass, não finge que ele não existe. |
| **Denylist de shell é substring, não parser** | `rm -r -f` (flags separadas) não casa `rm -rf`. Fixado em `denylist_hit_documents_split_flag_bypass`. A barreira real contra dano ao host é o Jail, não a denylist. |
| **Allowlist de shell lê só o primeiro token** | `git status` (2 tokens) não é expressável. Pendência nomeada do ADR-0034. |
| **Cache de aprovação por escopo não existe** | `OneTurn`/`OneSession` estão na spec e em nenhuma linha de código: toda tool com `requires_user_approval` pede aprovação em toda invocação. Achado da Etapa 7, roadmap Fase 8. |
| **Allowlist de rede é process-wide** | `ExecDeps` é construído uma vez no boot, então o layer de assistant (que exige `assistant_id`) não entra na interseção. Refinar exige `ExecDeps` per-run — Fase 8. |
| **Read-up de paths Medium-labeled** | O Jail impede escrita fora do workspace, não leitura de tudo que o usuário lê. Documentado no `security-threat-model.md` §"O que o sandbox NÃO protege". |

Nenhuma delas é "pendência envergonhada" no sentido do ADR-0032
§D4: cada uma tem teste que a fixa, ou linha de roadmap, ou as
duas. O que a fase se recusou a fazer foi entregar mecanismo que
parece proteger e não protege — foi por isso que o DNS intercept
saiu inteiro em vez de virar nota de rodapé.

## Onde ler mais

- **O que esta fase entrega:** [`windows-sandbox-design.md`](../../architecture/windows-sandbox-design.md) (aprimorado nesta Etapa 1).
- **O sandbox que envolve `exec.*`:** [`windows-sandbox-design.md`](../../architecture/windows-sandbox-design.md).
- **Os runtimes que `exec.python` / `exec.node` consomem:** [`runtimes-architecture.md`](../../architecture/runtimes-architecture.md) (novo, Etapa 1).
- **As ferramentas `exec.*` em si:** [`exec-tools-specification.md`](../../architecture/exec-tools-specification.md) (novo, Etapa 1).
- **O catálogo do Tool Registry:** [`tool-registry-specification.md`](../../architecture/tool-registry-specification.md) (atualizado, §"Status por ferramenta da Fase 7").
- **As 6 decisões:** [`docs/decisions/0031-*.md`](../../decisions/0031-fase-7-isolation-model-windows.md) a [`0036-*.md`](../../decisions/0036-security-jail-resolver-windows-job-objects.md).
- **O que a fase 8 absorveu:** [`docs/architecture/development-roadmap.md`](../../architecture/development-roadmap.md) (atualizado) + [ADR-0032](../../decisions/0032-fase-7-scope-reduction.md).
- **O threat model atualizado com o que o sandbox NÃO protege:** [`security-threat-model.md`](../../architecture/security-threat-model.md) §"O que o sandbox NÃO protege".
- **O percurso das duas últimas etapas (rede, wiring, `exec.shell`, DNS descartado):** [`etapa-6-7-narrativa.md`](./etapa-6-7-narrativa.md).

## Histórico de revisão

- 2026-08-08 — Etapa 1 (planejamento) em revisão. 6 ADRs (0031-0036) + 2 specs novos + 4 specs atualizados + este README + `status.md` + `CHANGELOG.md`. Sem código Rust — o planejamento de uma fase estrutural é código também, e ele tem que virar commit. Validação pelo user (via `ask_user`): "A — planejamento primeiro" + "C — tirar Git/GitHub da Fase 7" + a regra de "teste de negação" por etapa. A precedência é a mesma do Etapa 1 da Fase 6 (que planejou o Pipeline Sequencial, ADR-0028): cortar escopo na Etapa 1 é o que destrava a fase.
- 2026-08-08 — Etapa 2 (primitivas do sandbox) fechada (PR #42, `930c098`). 4 primitivas Rust em `crates/security/src/`. Documentação inline em `status.md` §7.
- 2026-08-08 — Etapa 3 (runtimes embutidos) fechada (PR #43, `da9e98f2`). Novo crate `frederico-runtimes` com Python 3.12.4 + Node 20.16.0 portáteis, SHA-256 pinned, 5 testes de regressão. Documentação inline em `status.md` §7.
- 2026-08-10 — Etapa 4 (`exec.python` / `exec.node` no registro) fechada em PR #44 (CI run final verde `#31384435313` 9m2s após 5 falhas consecutivas). **Achado crítico:** o `SecurityJailResolver` v1 (Job + Token + EnvFilter) **NÃO tem path safety enforcement** — o test `child_cannot_write_outside_workspace` (I3) provou que python escapa via `open('..\\evil.txt')` relativo ao workdir. Box::leak + .zip copy + `can_run_python` foram os fixes certos pra fazer o python rodar de verdade; o test catching a falha de path safety é exatamente o que teste de negação existe pra fazer. 3 testes `#[ignore]` serão reabertos na Etapa 5+ quando o sandbox ganhar AppContainer ou ACLs no Restricted Token. Saga completa do CI flake (5 erros distintos: `doc_lazy_continuation`, `build_default_tools` 1-arg duplicate, regex Python quebrando `//` + backticks, unused vars, os error 3 com 5 tentativas de fix) documentada no `status.md` §7 e na narrativa de Etapa 4.
- 2026-08-16 — **Etapa 7 reaberta; a fase volta a `em andamento`** ([ADR-0037](../../decisions/0037-exec-shell-fora-do-catalogo.md)). `exec.shell` saiu do catálogo depois de medido: a allowlist era contornável por qualquer separador do `cmd.exe`, e 7 dos 9 binários que ela permitia não rodam sob integridade baixa. Este README e a narrativa das Etapas 6/7 tinham sido escritos horas antes descrevendo a fase como concluída com 9 ferramentas; foram corrigidos no mesmo PR. **Lição do episódio:** o `README.md` da raiz vinha dizendo desde o PR #54 que a ferramenta fora descartada, contra todos os outros documentos — e estava certo. Uma revisão de código conferiu o catálogo, viu `exec.shell` registrado e "corrigiu" o README de volta. Conferir contra o código não bastou, porque o código era a coisa em disputa; só a medição do comportamento resolveu.
- 2026-08-16 — **Fechamento documental da fase.** Este README estava afirmando "Etapas 1-5 fechadas; Etapa 6 não iniciada" dois dias depois de a fase ter fechado, e a tabela de etapas ainda descrevia o plano (`exec.shell` na 6, rede na 7) em vez do que foi entregue (rede na 6, `exec.shell` na 7). Corrigido: cabeçalho, as duas tabelas, a numeração real com as etapas 5+ e 6+1 que o plano não previa, seção nova "O que a fase entregou — e o que ela deixou aberto" com as 7 lacunas nomeadas, e a narrativa das Etapas 6/7. Registrada também a pendência do CI noturno (abaixo). **Achado do fechamento:** o `CI Nightly` nunca ficou verde — 12 falhas consecutivas desde 2026-08-05, todas por `OPENROUTER_API_KEY` ausente no repositório. A cobertura noturna que o ADR-0026 §D2 e o ADR-0019 tratam como "mais fraca por natureza" era, na prática, **inexistente**; o passo de `check-core-purity` que vem depois dela nunca chegou a rodar no noturno.
- 2026-08-14 — **Etapa 7 fechada (PR #52, `e535b7f`) — fase concluída.** `exec.shell` com denylist + allowlist sempre ativas (`risk_level: Critical`) + `PermissionSet.network_allowlist` carregado do perfil TOML usuário ∩ projeto, substituindo o `NetworkAllowlist::new()` hardcoded vazio. No mesmo dia: 2 das 3 pendências de rede fechadas (feature flag `FREDERICO_NETWORK_PROXY_V1` removida; bug de fail-open do `PermissionSet.network` corrigido) e **a 3ª descartada** — o DNS intercept foi wireado, testado com privilégio elevado real, provado não-funcional e removido por inteiro. Detalhe na [narrativa das Etapas 6/7](./etapa-6-7-narrativa.md).
- 2026-08-13 — **Etapas 6 e 6+1 fechadas (PR #51, `2fbaf73`).** Proxy HTTP/CONNECT local com deny-by-default + auditoria em `network_audit` (Etapa 6), e o wiring real em `exec.python`/`exec.node` (Etapa 6+1). O wiring destravou 4 causas-raiz empilhadas — `CREATE_UNICODE_ENVIRONMENT` faltando, um fallback silencioso que anulava o `EnvFilter` inteiro, `SystemRoot`/`windir` fora do `EnvAllowlist::REQUIRED`, e o `run_id` errado no audit sink. A 2ª era uma **regressão de segurança silenciosa** vivendo em produção: quando a 1ª falhava, o filho herdava o ambiente inteiro do pai, credenciais incluídas, sem erro visível.
- 2026-08-12 — **Etapa 5+ fechada com ressalva (PR #47, `9610a53`; complemento em `435a755`).** Path safety no `SecurityJailResolver`. A ressalva: `exec.python`/`exec.node` foram **removidos do catálogo** enquanto a barreira não existia de verdade. Primeira aplicação na fase da regra "capacidade incompleta é capacidade indisponível" — a mesma que dois dias depois derrubaria o DNS intercept.
- 2026-08-10 — Etapa 5 (`files.write` + `files.edit` + `files.list`) fechada em 2 PRs (regra do user 2026-08-10: "2 PRs, não 3 — `files.write` e `files.edit` compartilham a mesma máquina de escrita atômica, backup e auditoria"). **PR #45 = `files.list`** (read-only, sem approval, mergeado). **PR #46 = `files.write` + `files.edit`** (destrutivo, exige approval + race defense via `expected_sha256` — regra do user: "files.edit tem que falhar se o conteúdo mudou"). **4 regras críticas honradas (decisão do user):** (1) atomicidade de verdade — temp no mesmo dir + fsync arquivo + fsync dir + rename (rename entre volumes falha no Windows, registrado no ADR-0035 D7); (2) aprovação obrigatória no manifesto (Passo 9 do validador); (3) `files.edit` recusa se `expected_sha256` não bate (defesa contra race read-modify-write); (4) testes de negação, não só de caminho feliz (3 E2E novos em `crates/e2e/tests/`: `e2e_files_list_under_jail`, `e2e_files_write_under_jail`, `e2e_files_edit_idempotent` — total 26 testes novos). Documentação inline em `status.md` §7.
