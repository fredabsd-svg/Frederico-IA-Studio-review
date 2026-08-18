<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-18
Fase correspondente: 8 (Etapa 3)
-->

# `frederico-git-engine`

Operações de Git local sobre o workspace da conversa. É a base do
diff viewer (Etapa 6) e dos marcos de projeto
([ADR-0042](../decisions/0042-projetos-e-checkpoints-nomeados.md)).

Spec: [`git-integration-architecture.md`](../architecture/git-integration-architecture.md).
Decisões: [ADR-0040](../decisions/0040-git-engine-biblioteca-e-fronteira.md)
(fronteira e proibição de processo externo) e
[ADR-0047](../decisions/0047-git-engine-usa-git2-medido-por-spike.md)
(qual biblioteca, e por quê essa).

## 1. O que existe hoje

As cinco operações do [ADR-0039](../decisions/0039-fase-8-escopo-e-etapas.md) §D1,
mais as duas de ciclo de vida do repositório:

| API | O que faz |
|---|---|
| `GitRepo::iniciar(&Path)` | cria repositório no workspace |
| `GitRepo::abrir(&Path)` | abre repositório existente; recusa caminho que não é repositório |
| `GitRepo::status()` | mudanças pendentes, com `staged` separando índice de árvore de trabalho |
| `GitRepo::diff(bool)` | patch unificado; `true` = índice vs. `HEAD`, `false` = árvore vs. índice |
| `GitRepo::historico(usize)` | últimos N commits a partir do `HEAD` |
| `GitRepo::branches()` / `branch_atual()` | branches locais, com o corrente marcado |
| `GitRepo::criar_branch(&str, bool)` / `trocar_branch(&str)` | cria e troca; **não apaga** |
| `GitRepo::commitar(&str, &Autor)` | registra tudo que mudou, escreve índice, árvore e commit |

## 1.1 As ferramentas do agente

As cinco estão registradas no Tool Registry (`crates/tool-registry/src/git/`)
e na allowlist de run, pelo `build_default_tools` /
`build_default_allowed_for_run` do `frederico-app` — bump atômico
(ADR-0020 §3 D3).

| Ferramenta | Risco | Aprovação |
|---|---|---|
| `git.status` | `Safe` | não |
| `git.diff` | `Safe` | não |
| `git.log` | `Safe` | não |
| `git.branch` | `Moderate` | **sim** |
| `git.commit` | `High` | **sim** |

**Nenhuma delas aceita caminho de repositório.** Todas abrem
`ctx.jail.root()`, e o schema de entrada é fechado
(`additionalProperties: false`) sem nenhuma propriedade de caminho.
É o ADR-0040 §D3 em código: a fronteira do Jail é garantida pela
ausência do parâmetro, não por validação de string.

**O autor do commit é fixo** (`Frederico IA Studio`). Não vem do
modelo — um `autor` no schema deixaria a IA atribuir a mudança a
qualquer pessoa, e o histórico do Git é exatamente o registro que se
consulta para saber quem fez o quê. Também não vem do config da
máquina, pelo mesmo motivo do `Autor` explícito. A identidade real do
usuário chega com o `github-engine`
([ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md)).

## 2. O que este módulo não faz

- **Não invoca o `git` do PATH.** Proibido pelo ADR-0040 §D1, e o
  motivo é o ponto 3 daquele parágrafo: processo externo contorna o
  sandbox inteiro da Fase 7. Fixado pelo teste
  `git_has_no_process_spawn`.
- **Não faz rede.** Push e PR são `github-engine`
  ([ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md)).
- **Não descobre repositório subindo diretórios.** `abrir` usa
  `Repository::open`, não `open_from_env` nem descoberta ascendente —
  procurar acima do workspace sairia do Jail.
- **Não lê identidade do config da máquina.** O `Autor` vem sempre de
  fora, por assinatura explícita. Depender de `user.name`/`user.email`
  reintroduziria a dependência de ambiente que o ADR-0040 §D1 ponto 1
  rejeita — e foi um dos pontos em que a `gix` falhou no spike.

## 3. Pureza e dependências

`unsafe_code = "forbid"`. Sem `tauri`, sem `windows`; o
`check-core-purity.ps1` cobra.

**O `forbid` vale para o nosso código, e não para a árvore inteira.** O
`git2` liga o `libgit2` — cerca de 195 mil linhas de C alcançadas por
FFI. É custo declarado no
[ADR-0047](../decisions/0047-git-engine-usa-git2-medido-por-spike.md) §D2,
não descuido, e a consequência prática é que o build do workspace passa
a exigir um compilador C.

## 4. Por que `git2` e não `gix`

O ADR-0040 §D2 preferia `gix` e mandou medir antes de cravar. A medição
está no [ADR-0047](../decisions/0047-git-engine-usa-git2-medido-por-spike.md);
o resumo de uma linha: **a `gix` escreve o objeto de commit mas não o
`.git/index`**, e o repositório resultante é lido por qualquer outro
cliente Git como se o arquivo commitado tivesse sido apagado.

Isso vale como aviso para quem for mexer: `commitar` escreve o índice
antes da árvore de propósito. O teste
`git_commit_escreve_o_indice_e_nao_so_o_objeto` falha se essa ordem se
perder.

## 5. Testes

`crates/git-engine/tests/spike_escrita_real.rs`, 4 testes (PR de spike):

| Teste | Prova |
|---|---|
| `git_commit_then_log_roundtrip` | dois commits reais, lidos de volta com pai e resumo corretos |
| `git_commit_escreve_o_indice_e_nao_so_o_objeto` | o `.git/index` existe depois do commit, e um commit sem mudança é recusado |
| `git_abrir_recusa_caminho_que_nao_e_repositorio` | **negação** — erro nomeado, sem criar repositório em silêncio |
| `git_has_no_process_spawn` | **negação** — o crate não spawna processo |

`crates/git-engine/tests/leitura_e_branch.rs`, 8 testes (PR de
implementação):

| Teste | Prova |
|---|---|
| `git_status_distingue_rastreado_de_nao_rastreado` | arquivo recém-criado aparece como `nao_rastreado`, modificado como `modificado` |
| `git_status_de_arvore_limpa_e_vazio` | controle positivo do anterior |
| `git_diff_mostra_a_linha_acrescentada` | o patch tem a linha nova e não marca como nova a que não mudou |
| `git_diff_staged_e_worktree_respondem_perguntas_diferentes` | por que o booleano existe |
| `git_branch_cria_troca_e_lista` | ciclo completo, com exatamente um branch corrente |
| `git_branch_recusa_nome_repetido_e_branch_inexistente` | **negação** — e o `HEAD` não se move depois de uma troca recusada |
| `git_branch_sem_commit_recusa_em_vez_de_panicar` | **negação** — repositório sem `HEAD` dá erro nomeado |
| `git_rejects_path_outside_workspace` | **negação** — `abrir` não sobe diretório atrás de `.git`, com controle positivo |

O `git_rejects_path_outside_workspace` é o que o spec previa. Ele foi
**visto falhando** antes de passar: trocando `Repository::open` por
`Repository::discover` no `abrir`, o teste fica vermelho com
"abrir subiu diretório e achou o repositório do pai". Sem essa
verificação, um teste de negação prova apenas que compila.

`crates/tool-registry/src/git/mod.rs`, 8 testes das ferramentas:
ciclo `status` → `commit` → `log` pelo contrato do `Tool`, recusa de
commit vazio, de mensagem vazia e de workspace sem repositório, a
assimetria de aprovação do ADR-0034, `git.branch` sem ação de apagar, e
`nenhuma_ferramenta_de_git_aceita_caminho_de_repositorio` — o teste
estrutural que quebra se alguém acrescentar `path`, `repo` ou `cwd` a
qualquer um dos cinco schemas.
