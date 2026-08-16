# 0041 — Projetos e checkpoints: construir, não estender

## Contexto

O ADR-0032 §D2 descreve o que a Fase 8 faz com checkpoints:

> **Checkpoints** (estado nomeado do workspace, retorno nomeado) — extensão do `CheckpointRepo` da Fase 3 Etapa 4, com nome humano, listagem, restore.

**O `CheckpointRepo` não existe.** A varredura de 2026-08-16 (`grep -rn "CheckpointRepo" --include=*.rs`) não retorna nenhuma ocorrência no repositório, e nenhum arquivo Rust lê ou escreve a tabela `checkpoints`. O que existe é a **tabela**, criada pela migração `0003_runs_and_checkpoints.sql` da Fase 3: `id`, `run_id` com `ON DELETE CASCADE`, `seq` do último evento e `state` do run, com `CHECK` sobre os 15 estados da máquina.

Ou seja: há schema sem repositório, e um ADR planejando estender código que nunca foi escrito. Vale nomear o padrão, porque é o terceiro caso da mesma família em duas fases — o `ChatOrchestratorParts.network_allowlist` era campo nunca lido (removido na Fase 7 Etapa 7), o cache de aprovação por escopo estava na spec e em nenhuma linha de código, e agora a tabela sem dono. **Estrutura declarada não é capacidade entregue**, e planejar a partir de um documento em vez do código propaga o erro para a fase seguinte.

Há também uma decisão de semântica escondida no schema. O `run_id` com `ON DELETE CASCADE` amarra checkpoint a **run**. Mas o que o ADR-0032 pede — "estado nomeado do workspace, retorno nomeado" — é checkpoint de **projeto**, que sobrevive ao run que o criou. São dois conceitos com o mesmo nome.

## Decisões

### D1 — Dois conceitos, dois nomes

- **Checkpoint de run** (o da tabela `checkpoints`, Fase 3): ponto de retomada interno da máquina de estados, amarrado ao run, apagado com ele. Continua sem repositório; a Fase 8 **não** o constrói, porque nada hoje precisa dele e construir por simetria é criar mais estrutura sem dono.
- **Marco de projeto** (`project_milestones`, novo): estado nomeado do workspace criado deliberadamente pelo usuário, com nome humano, que **sobrevive** a runs e conversas.

Nomes distintos porque a confusão já custou um ADR. Chamar os dois de "checkpoint" garante que a próxima leitura do ADR-0032 erre de novo.

### D2 — Marco de projeto é commit no `git-engine`, não cópia de arquivos

Um marco é uma referência Git criada pelo `git-engine` (ADR-0039) no repositório do workspace, mais uma linha de metadados no banco (nome humano, descrição, quando, qual conversa originou).

Rejeitada a alternativa óbvia — copiar a árvore de arquivos para uma pasta de backup — porque duplica dados do usuário, não escala com o tamanho do workspace, e reimplementa mal o que o Git faz bem. O custo é a dependência: **marco exige o workspace sob Git**, e um workspace sem repositório não tem marcos. É limitação declarada, e a UI diz isso em vez de oferecer um botão que falha.

### D3 — Restaurar é operação destrutiva, com aprovação e sem `--force`

Restaurar um marco descarta trabalho não salvo. Portanto: aprovação por invocação (ADR-0034), pedido mostrando **nome do marco, data e contagem de arquivos afetados**, e — pela mesma regra do ADR-0040 §D3 — nenhuma API que descarte mudanças sem checkpoint automático anterior.

### D4 — Projeto é o workspace com metadados, não uma entidade nova

`crates/project-engine/` guarda: caminho do workspace, nome, quando foi aberto pela última vez, e qual perfil de permissão se aplica. Um projeto **é** um diretório de workspace que o usuário nomeou; não há importação, migração nem formato proprietário.

A resolução de caminho continua passando pelo `JailResolver` (ADR-0022/0036). Abrir projeto não amplia o alcance do agente: amplia o que o **usuário** consegue alcançar pela UI.

### D5 — A tabela `checkpoints` fica como está, e o schema morto é registrado

Nada é apagado nesta fase — `DROP TABLE` de tabela vazia é seguro, mas migração destrutiva por limpeza estética é risco sem retorno. A tabela permanece, e a documentação passa a dizer que ela **não tem dono em código**, para que a próxima leitura não a confunda com capacidade existente. Remover ou adotar é decisão de quem for construir a retomada de run.

## Alternativas descartadas

1. **Estender a tabela `checkpoints` com `nome`**, como o ADR-0032 sugeria. Rejeitado pelo §D1: mistura ponto de retomada de máquina de estados com marco de usuário, e o `ON DELETE CASCADE` para `runs` apagaria o marco do usuário quando o run fosse apagado — perda de dados silenciosa embutida no schema.
2. **Marco como cópia de diretório.** Rejeitado pelo §D2.
3. **Construir o `CheckpointRepo` que falta**, por completude. Rejeitado: nada consome; seria estrutura nova sem dono, exatamente o defeito que este ADR nomeia.
4. **Projeto como formato próprio** (`.frederico-project` com manifesto). Rejeitado: cria migração, versionamento de formato e um caminho de corrupção, para armazenar quatro campos que cabem numa tabela.

## Consequências

- **Fica mais fácil:** explicar o produto. "Marco é um commit com nome" é verdade verificável no repositório do usuário, com `git log`, sem confiar no app.
- **Fica mais difícil:** oferecer marcos fora de Git. É a limitação aceita do §D2, declarada em vez de contornada.
- **Uma tabela sem dono fica no schema**, agora documentada como tal.
- **O ADR-0032 §D2 fica parcialmente incorreto** quanto aos checkpoints. ADRs são imutáveis (§1.6); este o corrige, e o `development-roadmap.md` passa a apontar para cá.
- **Aprendizado registrado:** planejamento de fase parte do código, não do ADR anterior. Os dois PRs de planejamento seguintes começam por varrer o código do que dizem estender.

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 1 da Fase 8. Corrige a premissa do ADR-0032 §D2 sobre o `CheckpointRepo`.
