# 0048 — A superfície de ferramentas de marco e GitHub, e o que fica fora dela

## Contexto

As Etapas 4 e 5 entregaram motores que o agente não alcança: `project-engine` (projetos e marcos) e `github-engine` (push e PR) existem, com testes, e nenhuma linha do Tool Registry os menciona. Só o Git local da Etapa 3 chegou ao catálogo.

Capacidade construída e não entregue é um padrão que esta base já pagou duas vezes: o `CheckpointRepo` que o ADR-0032 mandava estender e que nunca existiu, e o `ChatOrchestratorParts.network_allowlist` que era campo nunca lido. O ADR-0042 nomeou o padrão — **estrutura declarada não é capacidade entregue**. O inverso também vale: motor sem porta é motor que ninguém usa e que ninguém percebe estar quebrado.

Mas expor não é decisão neutra. O `github-engine` executa a **primeira operação irreversível em serviço externo** do produto (ADR-0041), e o `restaurar_marco` é a primeira operação que mexe na árvore de trabalho inteira de uma vez. Qual superfície o agente enxerga determina o que uma injeção de prompt bem-sucedida consegue fazer.

Este ADR decide a superfície, e o §D4 fecha uma lacuna que a Etapa 5 deixou aberta e nomeou no PR #74.

## Decisões

### D1 — O agente não abre nem registra projeto

`abrir_projeto` **não vira ferramenta**. O ADR-0042 §D4 é explícito: abrir projeto amplia o que o **usuário** alcança pela UI, não o que o agente alcança. Uma ferramenta `project.open` inverteria isso — o agente escolheria caminhos arbitrários do disco para registrar, e o registro é o que a UI depois oferece ao usuário.

O agente opera **sobre o projeto do workspace da conversa**, resolvido pelo caminho do Jail. Se esse workspace não é um projeto registrado, as ferramentas de marco recusam com essa mensagem. Não há parâmetro de projeto em nenhuma delas — mesma proteção estrutural das ferramentas de Git da Etapa 3 (ADR-0040 §D3): a fronteira é garantida por ausência de parâmetro, não por validação.

### D2 — Três ferramentas de marco, com a assimetria de sempre

| Ferramenta | Risco | Aprovação | Por quê |
|---|---|---|---|
| `milestone.list` | `Safe` | não | leitura |
| `milestone.create` | `Moderate` | **sim** | escreve tag e commit no repositório do usuário |
| `milestone.restore` | `High` | **sim** | mexe na árvore de trabalho inteira |

`milestone.restore` é `High` e não `Critical` porque o ADR-0042 §D3 já garante que ela não descarta trabalho: pendências viram marco automático antes, e a restauração é commit novo, não `reset`. O dano máximo é um commit indesejado no histórico, que o usuário desfaz com o Git dele. `Critical` fica reservado para o que não tem desfazer.

**Apagar marco não existe**, pela mesma regra do `git.branch` da Etapa 3: a operação não está no schema, então não há entrada.

### D3 — Duas ferramentas de GitHub, ambas `Critical`

| Ferramenta | Risco | Aprovação |
|---|---|---|
| `github.push` | `Critical` | **sim** |
| `github.create_pr` | `Critical` | **sim** |

`Critical` e não `High`, ao contrário de `git.commit`: é o único nível que força `ApprovalRequest.mandatory = true` mesmo sem UI de escopo (`validate.rs::with_mandatory_for_risk`), e foi por isso que o ADR-0044 o escolheu para `exec.shell`. Aqui a razão é mais forte — commit local se desfaz, push para o repositório de outras pessoas não.

O pedido de aprovação mostra **repositório, branch e contagem de commits** (ADR-0041 §D4), não "o agente quer usar o GitHub".

**Sem token ou sem matriz, as duas ferramentas não entram no catálogo.** Bump atômico (ADR-0020 §3 D3), igual ao `exec.*`: ou catálogo, allowlist e permissão se movem juntos, ou nenhum se move. O agente não vê ferramenta que não pode funcionar.

### D4 — O `push` passa a conferir que o remoto é o repositório autorizado

**Lacuna encontrada na Etapa 5 e registrada no PR #74:** a matriz autoriza `owner/repo`, mas o `git2` empurra para onde o `origin` do workspace apontar. Um remoto trocado empurraria para outro lugar carregando a autorização do repositório certo.

Enquanto o motor não tinha porta para o agente, a lacuna exigia que alguém alterasse o remoto do workspace à mão. Com a ferramenta, o cenário muda: o agente pode escrever arquivos no workspace, e `.git/config` está no workspace.

**O `push` passa a comparar a URL do remoto com o `owner/repo` autorizado, e recusa se não baterem.** A comparação aceita as formas que o GitHub publica (`https://github.com/owner/repo(.git)`, `git@github.com:owner/repo(.git)`, com ou sem barra final) e recusa qualquer host que não seja `github.com` — um remoto apontando para outro serviço não é o repositório da matriz, por definição.

Isto é pré-requisito da ferramenta, não melhoria: sem ele, a matriz autoriza um nome e a operação acontece em outro lugar.

## Alternativas descartadas

1. **Expor `project.open` ao agente**, por simetria com as demais. Rejeitado pelo §D1: inverte a direção do ADR-0042 §D4.
2. **`github.push` como `High`**, alinhando com `git.commit`. Rejeitado pelo §D3: `High` não força `mandatory` na fila de aprovação, e a diferença entre desfazer um commit local e desfazer um push não permite o mesmo nível.
3. **Resolver o §D4 validando `.git/config` contra escrita** (impedir o agente de editar o arquivo). Rejeitado: seria uma lista de arquivos proibidos dentro do Jail, que é frágil por natureza — `.git/config` tem irmãos (`.git/hooks/`, `includeIf`) e a lista envelheceria. Comparar no momento do uso não depende de enumerar o que proteger.
4. **Deixar a lacuna do §D4 para depois**, já que exige remoto adulterado. Rejeitado: a premissa "exige alguém alterar o remoto à mão" deixa de valer no instante em que o agente ganha a ferramenta, e é este ADR que a concede.

## Consequências

- **Fica mais fácil:** usar o que as Etapas 4 e 5 construíram. Até aqui era motor sem porta.
- **Fica mais difícil:** empurrar para um repositório cujo remoto não corresponde ao autorizado — inclusive em casos legítimos, como um fork com `origin` apontando para o upstream. O erro nomeia as duas URLs, e o usuário corrige a matriz ou o remoto.
- **Uma verificação de rede a mais no caminho do `push`**, feita localmente (leitura de `.git/config`), sem custo de latência.
- **O `PermissionSet::github` continua sendo o enum escalar da Fase 3.** Este ADR não o troca pela matriz — a matriz segue sendo portão do motor, aplicado, e o enum segue sendo declaração. Trocar é mudança de contrato que toca `permission.rs`, o `permission_loader` e os perfis em TOML, e merece PR próprio. Fica registrado como pendência nomeada, não como esquecimento.

## Histórico de revisão

- 2026-08-19 — versão inicial.
