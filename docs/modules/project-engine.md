<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-18
Fase correspondente: 8 (Etapa 4)
-->

# `frederico-project-engine`

Projetos (workspace com nome) e marcos nomeados sobre o
[`git-engine`](./git-engine.md).

Spec: [`project-and-milestones-architecture.md`](../architecture/project-and-milestones-architecture.md).
Decisão: [ADR-0042](../decisions/0042-projetos-e-checkpoints-nomeados.md).

## 1. O que existe hoje

| API | O que faz |
|---|---|
| `ProjectEngine::abrir_projeto` | registra um diretório como projeto; reabrir o mesmo caminho devolve o mesmo projeto |
| `ProjectEngine::listar_projetos` | do último acesso para o mais antigo |
| `ProjectEngine::projeto` | pelo `ProjectId` |
| `ProjectEngine::criar_marco` | tag anotada no repositório + metadados no banco |
| `ProjectEngine::listar_marcos` / `marco` | leitura dos marcos do projeto |
| `ProjectEngine::restaurar_marco` | volta o workspace ao estado do marco, sem descartar nada |

Tabelas: `projects` e `project_milestones`, migração
`0032_projects_and_milestones.sql`.

## 2. Restaurar não descarta trabalho, e a garantia é estrutural

O ADR-0042 §D3 exige que nenhuma API descarte mudanças sem marco
automático anterior. Aqui isso vale em duas camadas:

1. **Trabalho pendente vira marco automático** antes da restauração —
   commit com nome, não lixo perdido. O marco se declara
   `automatico: true` para a UI não poluir a lista.
2. **A restauração é um commit novo** com a árvore do marco, não um
   `reset`. O histórico continua inteiro, e o usuário confere com
   `git log`.

A camada 1 não é cortesia do chamador: o `GitRepo::restaurar_tag`
**recusa** árvore suja. Sem o marco automático, a operação falha — a
garantia não depende de alguém lembrar.

## 3. O que este módulo **não** faz

- **Não copia árvore de arquivos** (ADR-0042 §D2). Marco é referência
  Git, não backup de diretório.
- **Não constrói o `CheckpointRepo`.** A tabela `checkpoints` da
  migração `0003` segue sem dono em código (ADR-0042 §D5).
- **Não valida caminho contra Jail.** Ver §5.
- **Não expõe ferramentas ao agente.** O registro no Tool Registry
  não faz parte desta entrega — ver §6.

## 4. Marco exige Git, e a recusa é declarada

Workspace sem repositório não tem marcos. `criar_marco` devolve
`WorkspaceSemGit` com o caminho e a causa, e **nada é gravado pela
metade** — não há metadado no banco apontando para tag inexistente.

A ordem importa: a tag é criada **antes** do metadado. Se o banco
falhar depois, sobra uma tag sem metadados — visível pelo `git tag` do
usuário, perda recuperável. A ordem inversa deixaria registro
mentindo.

## 5. O teste que o spec previu está errado, e o certo está no lugar

O spec prevê `project_path_stays_inside_jail`. Esse teste é
**incompatível com o ADR-0042 §D4**: o caminho de um projeto é escolha
do usuário e vive fora de qualquer jail — o jail é resolvido por
conversa (ADR-0022), não por projeto. Um projeto obrigado a ficar
dentro do jail seria um projeto que só existe dentro da pasta da
conversa, o que não é um projeto.

O invariante verdadeiro é o outro lado, e é esse que está fixado em
`abrir_projeto_nao_amplia_o_alcance_do_agente`: **registrar projeto
não amplia o alcance do agente**. A outra metade da prova vive na
Etapa 3 — `nenhuma_ferramenta_de_git_aceita_caminho_de_repositorio`
garante que nenhuma ferramenta aceita caminho, e todas abrem
`ctx.jail.root()`.

O que existe aqui é uma guarda de **usabilidade**: `abrir_projeto`
recusa caminho que não é diretório, para um erro de digitação não
virar linha permanente apontando para lugar nenhum.

## 6. Testes

`crates/project-engine/tests/projetos_e_marcos.rs`, 12 testes:

| Teste | Prova |
|---|---|
| `project_open_and_list_roundtrip` | caminho feliz (nome do spec) |
| `milestone_create_then_restore` | marco criado, restaurado, conteúdo confere (nome do spec) |
| `milestone_requires_git_workspace` | **negação** — sem Git, recusa e não grava metade (nome do spec) |
| `abrir_projeto_nao_amplia_o_alcance_do_agente` | o invariante que substitui o `project_path_stays_inside_jail` do spec (§5) |
| `restaurar_salva_trabalho_pendente_num_marco_automatico` | o §D3 na prática: o rascunho não commitado sobrevive |
| `reabrir_o_mesmo_caminho_nao_duplica_projeto` | reabrir é o caso comum, não erro |
| `marcos_nao_vazam_entre_projetos` | isolamento, com o mesmo nome permitido em projetos distintos |
| `marco_com_nome_repetido_e_recusado` | **negação**, com o primeiro intacto |
| `restaurar_marco_inexistente_nao_deixa_lixo` | **negação** — alvo verificado antes de qualquer escrita |
| `projeto_com_caminho_inexistente_e_recusado` | **negação** — diretório inexistente e arquivo |
| `projeto_sem_nome_e_recusado` | **negação** |
| `operacao_em_projeto_inexistente_e_recusada` | **negação** |

As primitivas de tag ficam no `git-engine`
([`git-engine.md`](./git-engine.md) §5), com 8 testes próprios.

## 6.1 Uma armadilha ao rodar a suíte local

O `sqlx::migrate!("./migrations")` do `frederico-storage` embute as
migrações **em tempo de compilação**, e acrescentar arquivo novo ao
diretório nem sempre invalida o build em cache do crate. Sintoma
medido em 2026-08-18: os 12 testes deste crate passam sozinhos e
falham todos na suíte inteira, com
`no such table: projects`.

Não é regressão nem flake: é o binário de teste carregando o conjunto
de migrações de antes da `0032`. A CI não sofre disso — compila do
zero. Localmente, o conserto é uma linha:

```pwsh
cargo clean -p frederico-storage
```

Fica registrado aqui porque o sintoma (12 vermelhos de uma vez, todos
no mesmo ponto) parece defeito grave e não é.

## 7. Pureza e dependências

`unsafe_code = "forbid"`. Depende de `frederico-core`,
`frederico-git-engine` e `sqlx`. Sem `tauri`, sem `windows` — o
`check-core-purity.ps1` cobra.

Conhece o banco, ao contrário do `git-engine` (que é estritamente
local ao repositório). Foi assim que o spec desenhou: metadado de
marco é do `project-engine`.
