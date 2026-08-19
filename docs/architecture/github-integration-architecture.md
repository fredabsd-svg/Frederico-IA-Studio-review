<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-18
Fase correspondente: 8
-->

# Integração com GitHub (`github-engine`)

**O `crates/github-engine/` existe desde 2026-08-18** (Etapa 5): matriz de autorização, `criar_pr` e `push`, com o twin determinístico rodando em todo PR. O que **não** existe: ferramenta registrada no Tool Registry (o agente não alcança nada disto), o eixo estruturado no `PermissionSet` — que continua sendo o enum escalar da Fase 3 — e a auditoria própria. O as-built está em [`docs/modules/github-engine.md`](../modules/github-engine.md); o real, em [`docs/status.md`](../status.md).

Decisão que governa este spec: [ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md).

## Por que este módulo é diferente de todos os anteriores

É a **primeira operação destrutiva em serviço externo** que o produto executa por conta do agente. Todas as anteriores eram locais e reversíveis por construção: `files.write` tem backup `.bak` e hashes no audit ([ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md)); `exec.*` roda em sandbox que morre com o run.

Aqui não há desfazer. Um `push` errado altera o repositório de outras pessoas. Um PR criado por engano notifica revisores e permanece no histórico mesmo fechado. Todo o desenho abaixo decorre disso.

## O que este módulo NÃO faz

- **Não faz force-push.** Não é opção com aprovação reforçada — é **ausência de API** (ADR-0041 §D3). Quem precisa tem `exec.shell`, com o comando à vista, denylist e aprovação.
- **Não apaga branch, não fecha issue, não faz merge.** A superfície é `read`, `push`, `create_pr`.
- **Não guarda token em arquivo nem em variável de ambiente.** Só Windows Credential Manager.
- **Não assume repositório.** Sem entrada na matriz de autorização, nada funciona — fail-closed.

## Credencial

`TargetName` no padrão `Frederico-IA-Studio:github:<conta>`, mesma trilha do `WindowsCredentialStore` da Fase 2 Hardening 1 (`CredWriteW`/`CredReadW`/`CredDeleteW`). Viaja como `SecretString`.

**Nunca no ambiente do processo.** A Fase 7 provou por teste (`crates/security/tests/env_credential_not_leaked.rs`) que credencial no ambiente do pai vaza para o filho do sandbox quando o `EnvFilter` falha — e que a falha pode ser silenciosa. O `EnvAllowlist` não recebe entrada de GitHub.

## Matriz de autorização

Permissão de GitHub não é booleano (ADR-0041 §D2). `PermissionSet` ganha eixo estruturado:

| Dimensão | Forma | Vazio significa |
|---|---|---|
| Repositórios | lista `owner/repo` | nenhum |
| Branches | padrão por repositório | nenhum |
| Operações | `read` / `push` / `create_pr` | negado |

Merge é interseção usuário ∩ projeto, fail-closed, como os demais eixos — **entregue em 2026-08-19** pelo [ADR-0049](../decisions/0049-matriz-de-github-no-permission-set.md), que trocou o enum escalar `GitHubPermission` pelo campo `github_repos` do perfil e desce a interseção até dentro da regra (repositório, branch e operação). `allow_all()` **não** vira curinga — mesma razão pela qual o `network_allowlist` da Fase 7 Etapa 7 manteve lista vazia em `allow_all()`: não existe "todos os repositórios" que o sistema saiba interpretar sem inventar comportamento.

**Branch protegida exige menção nominal.** `push` para `main` só passa se o padrão a incluir explicitamente.

## Aprovação

`push` e `create_pr` exigem aprovação por invocação, e o pedido mostra **repositório, branch e contagem de commits** — não "o agente quer usar o GitHub". O ADR-0034 estabeleceu que o pedido carrega o comando exato; o equivalente aqui é o alvo exato.

## Auditoria

Cada operação grava repositório, branch, decisão e resultado, no espírito do `network_audit` da Fase 7 (append-only, falha vira `warn`, auditoria é observabilidade e não controle).

## Testes: o twin determinístico não é opcional

O E2E que cria PR de verdade precisa de rede, secret e serviço externo: `#[ignore]`, noturno, pela REGRA §3.3. E a REGRA §3.3 proíbe promover fase com cobertura só-noturna sem twin determinístico.

| Teste | Onde | Prova | Estado |
|---|---|---|---|
| `github_create_pr_against_real_service` | noturno | Caminho completo contra o GitHub | entregue (`#[ignore]`; exige `GITHUB_TOKEN_E2E` e `GITHUB_REPO_E2E`) |
| `github_create_pr_against_local_stub` | **todo PR** | Twin: caminho de produção contra servidor HTTP local | entregue |
| `github_rejects_repo_outside_matrix` | todo PR | **Negação** — repositório fora da matriz é recusado | entregue |
| `github_has_no_force_push_api` | todo PR | **Negação** — a API não expõe force; falha se alguém a acrescentar | entregue |

Mais 9 em todo PR (curinga vs. branch protegida, matriz vazia, operação ausente, `owner/repo` mal formado, interseção fail-closed, recusa do serviço, e três do `push`). Inventário em [`docs/modules/github-engine.md`](../modules/github-engine.md) §6.

**O twin do `push` não cobre o callback de credencial.** Ele empurra para um repositório bare local, cujo transporte não autentica — o que ele prova é a autorização, o refspec e a chegada do commit. Quem exercita o callback é o noturno. Declarado em vez de contornado.

**Pré-condição de fechamento da fase (ADR-0039 §D2):** o `CI Nightly` precisa de ao menos um run verde citado no `status.md`. Em 2026-08-16 ele acumulava 12 falhas consecutivas desde 2026-08-05, todas por secret ausente — a cobertura noturna que o [ADR-0026](../decisions/0026-e2e-coverage-gate.md) §D2 classifica como "mais fraca por natureza" era, na prática, inexistente. Um E2E noturno num pipeline que nunca completa não é cobertura fraca: é cobertura nenhuma com aparência de cobertura.

## Referências

- [ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md), [ADR-0039](../decisions/0039-fase-8-escopo-e-etapas.md), [ADR-0026](../decisions/0026-e2e-coverage-gate.md)
- [`tool-permission-model.md`](./tool-permission-model.md) — eixos do `PermissionSet`
- [`git-integration-architecture.md`](./git-integration-architecture.md) — a metade local
