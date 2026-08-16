<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 8
-->

# Projetos e marcos (`project-engine`)

**Este documento descreve o que ainda não existe.** Nenhuma linha do `crates/project-engine/` foi escrita. Estado `especificado` conforme §1.13 e [ADR-0037](../decisions/0037-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md). O real está em [`docs/status.md`](../status.md).

Decisão que governa este spec: [ADR-0041](../decisions/0041-projetos-e-checkpoints-nomeados.md).

## Duas coisas com o mesmo nome

O [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) §D2 planejou os checkpoints da Fase 8 como "extensão do `CheckpointRepo` da Fase 3". A varredura de 2026-08-16 mostrou que **o `CheckpointRepo` não existe**: `grep -rn "CheckpointRepo" --include=*.rs` não retorna nada, e nenhum arquivo Rust lê ou escreve a tabela `checkpoints`. O que existe é a tabela, criada pela migração `0003_runs_and_checkpoints.sql`, com `run_id` e `ON DELETE CASCADE`.

Daí a separação de nomes do ADR-0041 §D1:

| | **Checkpoint de run** | **Marco de projeto** |
|---|---|---|
| Origem | máquina de estados (Fase 3) | usuário, deliberadamente |
| Vida | morre com o run (`CASCADE`) | sobrevive a runs e conversas |
| Estado hoje | tabela sem dono em código | não existe |
| Fase 8 constrói? | **não** | **sim** |

A Fase 8 **não** constrói o `CheckpointRepo`: nada o consome, e criar estrutura sem dono é o defeito que este spec nomeia. A tabela fica onde está, agora documentada como órfã, para que a próxima leitura não a confunda com capacidade existente.

Vale registrar o padrão, porque é o terceiro caso da mesma família em duas fases — o `ChatOrchestratorParts.network_allowlist` era campo nunca lido (removido na Fase 7 Etapa 7), o cache de aprovação por escopo estava na spec e em nenhuma linha de código, e agora a tabela sem dono. **Estrutura declarada não é capacidade entregue.**

## O que é um projeto

Um diretório de workspace que o usuário nomeou. Não há formato proprietário, importação nem migração — o `project-engine` guarda quatro campos: caminho, nome, último acesso e perfil de permissão aplicável.

Abrir projeto **não amplia o alcance do agente**: a resolução continua passando pelo `JailResolver` ([ADR-0022](../decisions/0022-jail-resolver-v1.md)/[ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md)). Amplia o que o **usuário** alcança pela UI.

## O que é um marco

Uma referência Git criada pelo [`git-engine`](./git-integration-architecture.md) no repositório do workspace, mais uma linha de metadados no banco: nome humano, descrição, quando, e qual conversa o originou.

**Não é cópia de árvore de arquivos** (ADR-0041 §D2). Copiar duplicaria dados do usuário, não escalaria com o tamanho do workspace e reimplementaria mal o que o Git faz bem.

O custo é uma dependência declarada: **marco exige workspace sob Git**. Workspace sem repositório não tem marcos, e a UI diz isso — em vez de oferecer um botão que falha.

A vantagem é verificabilidade: "marco é um commit com nome" é conferível pelo usuário com `git log`, sem confiar no aplicativo.

## Restaurar é destrutivo

Restaurar descarta trabalho não salvo. Portanto (ADR-0041 §D3): aprovação por invocação, com **nome do marco, data e contagem de arquivos afetados** no pedido; e nenhuma API que descarte mudanças sem marco automático anterior — mesma regra do force-push do [ADR-0040](../decisions/0040-github-auth-e-matriz-de-autorizacao.md) §D3.

## Testes previstos

| Teste | Prova |
|---|---|
| `project_open_and_list_roundtrip` | Caminho feliz |
| `milestone_create_then_restore` | Marco criado, restaurado, conteúdo confere |
| `milestone_requires_git_workspace` | **Negação** — sem repositório, recusa com erro claro (não cria pela metade) |
| `project_path_stays_inside_jail` | **Negação** — caminho fora do Jail é recusado |

## Referências

- [ADR-0041](../decisions/0041-projetos-e-checkpoints-nomeados.md), [ADR-0038](../decisions/0038-fase-8-escopo-e-etapas.md)
- [`git-integration-architecture.md`](./git-integration-architecture.md)
- [`agent-state-machine.md`](./agent-state-machine.md) — onde o checkpoint de run se encaixaria
