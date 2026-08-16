<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 8
-->

# Integração com Git (`git-engine`)

**Este documento descreve o que ainda não existe.** Nenhuma linha do `crates/git-engine/` foi escrita; o estado `especificado` é literal, e a isenção da §1.3 vale enquanto ele estiver assim (§1.13, com a exceção da Etapa 1 do [ADR-0037](../decisions/0037-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md)). A fonte da verdade do que está pronto é o [`docs/status.md`](../status.md).

Decisões que governam este spec: [ADR-0038](../decisions/0038-fase-8-escopo-e-etapas.md) (escopo da fase) e [ADR-0039](../decisions/0039-git-engine-biblioteca-e-fronteira.md) (biblioteca e fronteira).

## O que este módulo faz

Expõe operações de Git sobre o workspace da conversa como API tipada em Rust: `status`, `diff`, `log`, `branch`, `commit`. É a base do diff viewer (Etapa 6) e dos marcos de projeto ([ADR-0041](../decisions/0041-projetos-e-checkpoints-nomeados.md)).

## O que este módulo NÃO faz

Vale mais do que a lista do que ele faz, porque é o que impede o crate de virar um cliente Git genérico:

- **Não invoca o `git` do PATH.** Proibido pelo ADR-0039 §D1. Se a biblioteca não expõe a operação, o produto não a oferece — não há fallback para `Command`. Um fallback assim seria o mesmo defeito que a Fase 7 Etapa 6+1 teve de remover do `jail.rs`: caminho silencioso que anula a camada de contenção.
- **Não faz push, fetch nem qualquer rede.** Isso é `github-engine` ([ADR-0040](../decisions/0040-github-auth-e-matriz-de-autorizacao.md)). Este crate é estritamente local.
- **Não reescreve histórico.** Sem rebase, sem amend, sem `reset --hard`.
- **Não sai do workspace.** Todo caminho vem resolvido pelo `JailResolver`; não há API que aceite caminho absoluto arbitrário.
- **Não resolve conflito de merge.** Detecta e reporta; resolver é do usuário.

## Fronteira e dependências

```text
tool-registry ──> git-engine ──> (biblioteca Git)
                      │
                      └─ recebe workspace já resolvido pelo JailResolver
```

- **Puro** (`unsafe_code = "forbid"`), sem Tauri, sem `frederico-storage`. Segue o ADR-0003; o `check-core-purity.ps1` cobra.
- **Não conhece o banco.** Metadados de marco são do `project-engine`.

## Ferramentas expostas ao agente

| Ferramenta | Risco | Aprovação | Observação |
|---|---|---|---|
| `git.status` | Low | não | Arquivos modificados, staged, untracked |
| `git.diff` | Low | não | Patch unificado; `staged: bool` |
| `git.log` | Low | não | Últimos N commits |
| `git.branch` | Medium | **sim** | Criar e trocar; não apaga |
| `git.commit` | High | **sim** | Pedido mostra arquivos e mensagem |

A assimetria é a do [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md): leitura livre, escrita com consentimento por invocação.

**Nota de realidade herdada da Fase 7:** o cache de aprovação por escopo não existe em código — toda tool com `requires_user_approval` pede aprovação a cada invocação. A coluna acima descreve o comportamento real, e a Etapa 7 (ADR-0038 §D4) precisa preservá-lo ao construir o cache.

## A escolha da biblioteca é um experimento, não uma premissa

O ADR-0039 §D2 **não** crava a biblioteca. A Etapa 3 abre com um spike cujo critério de saída é um teste que faz commit real num repositório temporário e o lê de volta. A preferência declarada é `gix` (Rust puro, sem toolchain C no build); ela cede se a escrita não cobrir as operações da tabela acima.

Este spec será atualizado com o resultado no mesmo commit em que o crate entrar, e o estado promovido para `parcialmente implementado` — como manda a §1.13.

## Testes previstos

Cada etapa entrega ao menos um **teste de negação**, regra herdada da Fase 7 (foi um deles que expôs o escape de path do sandbox na Etapa 4 daquela fase):

| Teste | Prova |
|---|---|
| `git_status_reads_real_repo` | Caminho feliz sobre repositório temporário |
| `git_commit_then_log_roundtrip` | Escrita real, lida de volta (critério do spike) |
| `git_rejects_path_outside_workspace` | **Negação** — caminho fora do Jail é recusado |
| `git_has_no_process_spawn` | **Negação** — o crate não spawna processo; falha se alguém reintroduzir `Command` |

O último é o que impede a erosão da decisão do ADR-0039 §D1. Regra que só vive em prosa é regra que volta na primeira urgência.

## Referências

- [ADR-0038](../decisions/0038-fase-8-escopo-e-etapas.md), [ADR-0039](../decisions/0039-git-engine-biblioteca-e-fronteira.md), [ADR-0041](../decisions/0041-projetos-e-checkpoints-nomeados.md)
- [`tool-registry-specification.md`](./tool-registry-specification.md) — contrato de ferramenta
- [`security-threat-model.md`](./security-threat-model.md) — por que processo externo é problema
