# 0047 — O `git-engine` usa `git2`; a preferência por `gix` foi medida e caiu

> **Substitui parcialmente o [ADR-0040](0040-git-engine-biblioteca-e-fronteira.md) §D2.** O §D1 (biblioteca linkada, nunca `Command::new("git")`), o §D3 (fronteira no `JailResolver`) e o §D4 (escrita exige aprovação) continuam valendo sem alteração. O que este ADR fecha é só a escolha que o §D2 deixou aberta de propósito.

## Contexto

O ADR-0040 §D2 recusou cravar a biblioteca de Git e definiu o método: spike na Etapa 3, com critério de saída de **um teste que faz commit real num repositório temporário e o lê de volta**. A preferência declarada era `gix` — Rust puro, sem toolchain C no build — e o próprio ADR dizia que ela cede se a escrita não cobrir as operações do ADR-0039 §D1.

Ela cedeu.

## Medição

Spike executado em 2026-08-17, nesta máquina de desenvolvimento (Windows 11, toolchain GNU `stable-x86_64-pc-windows-gnu`, MinGW-w64 UCRT, `rustc` 1.97.1), com `gix` 0.86.0 e `git2` 0.21.0. Os dois programas exercitaram as mesmas operações sobre um repositório temporário, e o resultado foi conferido **com o `git` real do sistema** — que é o crivo que importa, porque é o que o usuário tem quando abre a mesma pasta em outro cliente.

| Critério (ADR-0040 §D2) | `gix` 0.86 | `git2` 0.21 |
|---|---|---|
| Build sob MinGW-w64 | compila, 3m04s | **compila**, 4m11s (`libgit2-sys` + `libz-sys`) |
| Dependências (crates) | **142** | **19** |
| C no build | nenhum | 260 arquivos, ~195 mil linhas (libgit2 vendorizado) |
| `commit` | só montando a árvore à mão (`write_blob` + `objs::Tree` + `write_object`) | direto (índice → árvore → commit) |
| **`.git/index` após o commit** | **não escreve** | escreve |
| `status` | OK | OK |
| `log` | OK | OK |
| `diff` unificado | só `diff_tree_to_tree`; patch montado à mão | `DiffFormat::Patch` pronto |
| Criar branch | **falha** sem `user.name`/`user.email` no config (`MissingCommitter`) | OK (assinatura explícita) |
| **Trocar de branch** | **ausente** — o facade expõe `checkout_options()` e não `checkout`; a função real vive no `gix-worktree-state`, crate que o `gix` não reexporta | OK (`checkout_head`) |
| Roundtrip do §D2 | passa | passa |

### O achado que decidiu

Os dois passam no critério literal do ADR-0040 §D2 — commit escrito, commit lido de volta. **O critério era insuficiente, e o spike mostrou por quê.**

Depois do commit pela `gix`, o `git` real lê o repositório assim:

```text
$ git status --short
D  a.txt
?? a.txt
?? b.txt
```

O objeto de commit é válido (`git log` mostra, `git fsck` passa limpo) e o `.git/index` **não existe**. Para qualquer outro cliente Git — o `git` do usuário, o VS Code, o diff viewer da Etapa 6 — o arquivo que acabou de ser commitado aparece como apagado do índice e não rastreado na árvore.

Ler o próprio commit de volta pela mesma biblioteca que o escreveu não prova que o repositório ficou íntegro. Prova que a biblioteca é consistente consigo mesma.

Pelo mesmo crivo, o repositório escrito pela `git2` devolve `git status --short` **vazio**, com dois commits no log, duas branches e `git fsck` limpo.

### O que faltaria construir para ficar com a `gix`

Três peças, todas de escrita: popular o índice, derivar a árvore do índice (`write_tree` não existe em nenhum lugar da `gix` nem da `gix-index`) e atualizar a árvore de trabalho ao trocar de branch. É reimplementar em cima da `gix` exatamente a parte que o ADR-0040 alternativa 3 rejeitou como projeto próprio — com a agravante de que errar aí **corrompe o repositório do usuário em silêncio**, que foi o motivo declarado da rejeição.

## Decisão

### D1 — `git2` 0.21 é a biblioteca do `git-engine`

A preferência do ADR-0040 §D2 tinha peso "alto" em um critério (sem toolchain C) e "alto" em outro (cobertura das operações). O primeiro **não se materializou**: o `git2` compila sob o MinGW desta máquina sem configuração adicional. O segundo eliminou a `gix` para as operações de escrita, que são metade do §D1 da fase.

O saldo dos critérios de peso médio também não sustenta a preferência: 19 dependências contra 142 é menos superfície de cadeia de suprimentos, não mais.

### D2 — O C do `libgit2` entra como custo declarado, não como detalhe

São ~195 mil linhas de C rodando no processo, alcançadas por FFI. O `unsafe_code = "forbid"` do crate continua verdadeiro e continua significando o que sempre significou — **o nosso** código não tem `unsafe` —, e não deve ser lido como se a árvore inteira fosse Rust seguro. O `git2` já era, antes desta decisão, a biblioteca mais exercitada do ecossistema para este uso; é o que se compra com o C.

Consequência operacional: o build do workspace passa a exigir um compilador C. Nesta máquina o MinGW já estava lá por causa do Tauri; no CI, o runner do GitHub tem o seu. Isso vira dependência de build de primeira classe, e o `README` diz.

### D3 — O critério de saída de spike de escrita passa a incluir verificação por cliente externo

Regra para as próximas etapas e fases: quando o spike mede **escrita** em formato que outro programa vai ler, ele não fecha só com a leitura pela própria biblioteca. Fecha conferindo o artefato com a ferramenta de referência.

Está fixado em teste, não só aqui: `git_commit_escreve_o_indice_e_nao_so_o_objeto` falha se o `.git/index` não for escrito.

## Alternativas descartadas

1. **Ficar com a `gix` e construir índice, árvore e checkout à mão.** Rejeitada pelo ADR-0040 alternativa 3, cujo argumento se aplica inteiro aqui.
2. **`gix` para leitura e `git2` para escrita.** Rejeitada: duas bibliotecas de Git no mesmo binário, com dois modelos de objeto, para economizar o C que já entra pela escrita. Paga o custo das duas.
3. **Adiar a decisão e começar pelas operações de leitura**, onde a `gix` basta. Rejeitada: é escolher a biblioteca por qual metade é mais fácil, e descobrir o problema depois de a API pública estar de pé — o padrão que o ADR-0040 §D2 citava do ADR-0033.
4. **Manter a preferência e reabrir depois**, tratando o índice como pendência. Rejeitada: capacidade incompleta é capacidade indisponível, a regra que tirou `exec.python`/`exec.node` do catálogo na Fase 7 Etapa 5+. Um `git.commit` que deixa o repositório ilegível para o resto do mundo é pior que ausência de `git.commit`.

## Consequências

- **Fica mais fácil:** entregar as cinco operações do ADR-0039 §D1 sem escrever código de formato Git.
- **Fica mais difícil:** afirmar que a árvore de dependências é Rust seguro. Não é, e o §D2 acima diz isso em vez de deixar o `forbid` do crate sugerir o contrário.
- **O `README` ganha o compilador C como requisito de build.**
- **O ADR-0040 §D2 fica historicamente correto e operacionalmente substituído.** O método que ele fixou é o que produziu este resultado — inclusive o de contrariar a preferência de quem o escreveu.

## Histórico de revisão

- 2026-08-17 — versão inicial. Etapa 3 da Fase 8, PR de spike.
