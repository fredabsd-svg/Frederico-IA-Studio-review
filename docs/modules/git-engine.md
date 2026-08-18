<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-17
Fase correspondente: 8 (Etapa 3 — PR de spike)
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

**Só o caminho de escrita e a leitura que o valida.** Este é o PR de
spike da Etapa 3, e o que ele entrega é o que o spike precisava provar:

| API | O que faz |
|---|---|
| `GitRepo::iniciar(&Path)` | cria repositório no workspace |
| `GitRepo::abrir(&Path)` | abre repositório existente; recusa caminho que não é repositório |
| `GitRepo::commitar(&str, &Autor)` | registra tudo que mudou, escreve índice, árvore e commit |
| `GitRepo::historico(usize)` | últimos N commits a partir do `HEAD` |

`status`, `diff` e `branch` **não existem ainda** — entram no PR de
implementação da mesma etapa. A tabela de ferramentas do agente
(`git.status`, `git.diff`, `git.log`, `git.branch`, `git.commit`) está
no spec e nenhuma delas está registrada no Tool Registry.

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

`crates/git-engine/tests/spike_escrita_real.rs`, 4 testes:

| Teste | Prova |
|---|---|
| `git_commit_then_log_roundtrip` | dois commits reais, lidos de volta com pai e resumo corretos |
| `git_commit_escreve_o_indice_e_nao_so_o_objeto` | o `.git/index` existe depois do commit, e um commit sem mudança é recusado |
| `git_abrir_recusa_caminho_que_nao_e_repositorio` | **negação** — erro nomeado, sem criar repositório em silêncio |
| `git_has_no_process_spawn` | **negação** — o crate não spawna processo |

O `git_rejects_path_outside_workspace` previsto no spec entra com a
integração ao `JailResolver`, no PR de implementação: hoje não há API
que receba caminho para negar.
