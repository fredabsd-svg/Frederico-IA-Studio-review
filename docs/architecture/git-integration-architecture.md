<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-18
Fase correspondente: 8
-->

# Integração com Git (`git-engine`)

**As cinco operações existem e as cinco ferramentas estão registradas** desde 2026-08-18 (PR de implementação da Etapa 3). O que este spec ainda descreve como futuro é a UI: o diff viewer é a Etapa 6, e nada do que está aqui aparece em tela hoje. A fonte da verdade do que está pronto é o [`docs/status.md`](../status.md); o as-built do crate é [`docs/modules/git-engine.md`](../modules/git-engine.md).

Decisões que governam este spec: [ADR-0039](../decisions/0039-fase-8-escopo-e-etapas.md) (escopo da fase) e [ADR-0040](../decisions/0040-git-engine-biblioteca-e-fronteira.md) (biblioteca e fronteira).

## O que este módulo faz

Expõe operações de Git sobre o workspace da conversa como API tipada em Rust: `status`, `diff`, `log`, `branch`, `commit`. É a base do diff viewer (Etapa 6) e dos marcos de projeto ([ADR-0042](../decisions/0042-projetos-e-checkpoints-nomeados.md)).

## O que este módulo NÃO faz

Vale mais do que a lista do que ele faz, porque é o que impede o crate de virar um cliente Git genérico:

- **Não invoca o `git` do PATH.** Proibido pelo ADR-0040 §D1. Se a biblioteca não expõe a operação, o produto não a oferece — não há fallback para `Command`. Um fallback assim seria o mesmo defeito que a Fase 7 Etapa 6+1 teve de remover do `jail.rs`: caminho silencioso que anula a camada de contenção.
- **Não faz push, fetch nem qualquer rede.** Isso é `github-engine` ([ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md)). Este crate é estritamente local.
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

**Nota de realidade herdada da Fase 7:** o cache de aprovação por escopo não existe em código — toda tool com `requires_user_approval` pede aprovação a cada invocação. A coluna acima descreve o comportamento real, e a Etapa 7 (ADR-0039 §D4) precisa preservá-lo ao construir o cache.

## A escolha da biblioteca foi um experimento, e o resultado contrariou a preferência

O ADR-0040 §D2 não cravou a biblioteca: fixou os critérios e mandou medir. O spike rodou em 2026-08-17 e a preferência por `gix` **caiu**. A decisão está no [ADR-0047](../decisions/0047-git-engine-usa-git2-medido-por-spike.md); o crate usa `git2` 0.21.

O que decidiu não foi o critério original. Os dois candidatos passavam nele — commit escrito, commit lido de volta pela mesma biblioteca. O que a `gix` não fazia era escrever o `.git/index`: o objeto de commit ficava válido e o repositório ficava ilegível para qualquer outro cliente Git, que via o arquivo recém-commitado como apagado. Somaram-se a isso a ausência de troca de branch no facade e a falha ao criar branch sem identidade no config.

Fica a regra que o ADR-0047 §D3 tirou disso: **spike de escrita não fecha lendo pela própria biblioteca — fecha conferindo o artefato com a ferramenta de referência.**

## Testes previstos

Cada etapa entrega ao menos um **teste de negação**, regra herdada da Fase 7 (foi um deles que expôs o escape de path do sandbox na Etapa 4 daquela fase):

| Teste | Prova | Estado |
|---|---|---|
| `git_status_distingue_rastreado_de_nao_rastreado` | Caminho feliz sobre repositório temporário | entregue |
| `git_commit_then_log_roundtrip` | Escrita real, lida de volta (critério do spike) | entregue |
| `git_rejects_path_outside_workspace` | **Negação** — `abrir` não sobe diretório atrás de `.git`; visto falhando contra `Repository::discover` | entregue |
| `git_has_no_process_spawn` | **Negação** — o crate não spawna processo; falha se alguém reintroduzir `Command` | entregue |
| `nenhuma_ferramenta_de_git_aceita_caminho_de_repositorio` | **Negação estrutural** — nenhum dos 5 schemas aceita `path`, `repo` ou `cwd` | entregue |

Os dois últimos impedem a erosão das decisões do ADR-0040 §D1 e §D3. Regra que só vive em prosa é regra que volta na primeira urgência.

O inventário completo está em [`docs/modules/git-engine.md`](../modules/git-engine.md) §5: 12 testes no crate e 8 nas ferramentas.

## Referências

- [ADR-0039](../decisions/0039-fase-8-escopo-e-etapas.md), [ADR-0040](../decisions/0040-git-engine-biblioteca-e-fronteira.md), [ADR-0042](../decisions/0042-projetos-e-checkpoints-nomeados.md)
- [`tool-registry-specification.md`](./tool-registry-specification.md) — contrato de ferramenta
- [`security-threat-model.md`](./security-threat-model.md) — por que processo externo é problema
