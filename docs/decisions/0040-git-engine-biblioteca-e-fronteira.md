# 0040 — `git-engine`: biblioteca embutida, nunca o `git` do PATH

## Contexto

A Fase 8 (ADR-0039 §D1) entrega Git local: status, diff, log, branch e commit sobre o workspace da conversa. Três caminhos existem, e a escolha define o que dá para prometer honestamente.

O contexto do projeto restringe mais do que o de um app genérico:

- **Nada de dependência do ambiente da máquina.** A Fase 7 já embutiu Python e Node portáteis com SHA-256 pinned (ADR-0031, `runtimes-architecture.md`) justamente para não depender do PATH do usuário.
- **`unsafe_code = "forbid"` nos crates do núcleo**, e o `check-core-purity.ps1` guarda a pureza (ADR-0003).
- **Toolchain GNU (MinGW-w64)** no Windows — dependência que exige compilar C torna o build mais frágil, e o `README` já lista MinGW como requisito por causa disso.
- O workspace **não tem hoje nenhuma dependência de Git** (`Cargo.lock` não contém `git2`, `gix` nem `libgit2-sys`).

## Decisão

### D1 — Git vem de biblioteca linkada, nunca de `Command::new("git")`

Invocar o `git` do PATH está **proibido** no `git-engine`. Os motivos são os mesmos que levaram aos runtimes portáteis, e um a mais:

1. **Não reprodutível.** A versão do `git` do usuário determina o comportamento, e o projeto não a controla.
2. **Não diagnosticável.** Erro de `git` chega como texto em stderr, em idioma que depende da locale da máquina. Traduzir isso para `ErrorView` com ação sugerida (§ do `chat-and-providers.md`) exigiria parsear prosa.
3. **Superfície de execução.** O agente já tem `exec.shell` sob sandbox, com denylist e aprovação por invocação (Fase 7, Etapa 7). Uma ferramenta de Git que faz `Command::new("git")` **contorna** essa camada inteira: seria execução de processo sem denylist, sem Jail, sem aprovação. Não é um detalhe de engenharia — é um buraco no modelo de ameaça construído na fase anterior.

O ponto 3 é decisivo e vale registrar em separado: se o Git chegar como processo externo, tudo que a Fase 7 construiu para conter execução deixa de valer para ele.

### D2 — A escolha da biblioteca é decidida por spike na Etapa 3, entre `gix` e `git2`

Este ADR **não** crava a biblioteca. Ele crava os critérios e o método, porque a informação necessária para escolher não está disponível sem experimento no repositório real:

| Critério | Peso | Por quê |
|---|---|---|
| Sem toolchain C no build | alto | `git2` liga `libgit2-sys`, que compila C. `gix` é Rust puro. Um build que quebra no MinGW é custo recorrente. |
| Cobertura das operações do §D1 do ADR-0039 | alto | Ler (status, diff, log) é diferente de escrever (commit, branch). Escrita é onde as implementações divergem em maturidade. |
| `unsafe` na árvore de dependências | médio | O crate é do núcleo; `forbid` vale para o nosso código, mas a dependência entra no binário. |
| Superfície de API estável | médio | Trocar de biblioteca depois custa o crate inteiro. |

**O spike da Etapa 3 é um PR próprio, e o critério de saída dele é um teste que faz commit real num repositório temporário e o lê de volta** — não uma leitura de README. A preferência declarada é `gix`, pelo critério de build; ela cede se o spike mostrar que a escrita não cobre o §D1.

Registrar a preferência sem cravá-la é deliberado: o ADR-0033 da Fase 7 cravou o DNS intercept sem experimento e o mecanismo teve de ser removido inteiro depois de construído. O padrão a evitar é decidir mecanismo por plausibilidade.

### D3 — O `git-engine` é puro; a fronteira é o `JailResolver`

O crate não conhece o sistema de arquivos além do caminho que recebe. Toda operação recebe o workspace resolvido pelo `JailResolver` (ADR-0022/0036), como as ferramentas de arquivo da Fase 7 Etapa 5. Consequência: o agente não consegue rodar `git` fora do workspace da conversa, porque não existe API para isso.

### D4 — Escrita exige aprovação; leitura não

Alinhado ao ADR-0034: `git.status`, `git.diff`, `git.log` são leitura, sem aprovação. `git.commit`, `git.branch` alteram estado e exigem aprovação por invocação. `git.push` não pertence a este crate — é GitHub, ADR-0041.

## Alternativas descartadas

1. **`Command::new("git")`** — mais simples e cobre 100% das operações desde o primeiro dia. Rejeitado pelo §D1, principalmente pelo ponto 3: fura o sandbox da Fase 7.
2. **Embutir o binário do `git` portátil**, como Python e Node. Rejeitado: resolve reprodutibilidade mas não os pontos 2 e 3 — continua sendo processo externo com erro em prosa e fora do modelo de aprovação. E acrescenta dezenas de MB ao instalador, que o ADR-0004 já registra como custo sensível.
3. **Implementar o formato do Git à mão.** Rejeitado sem discussão longa: é um projeto inteiro, e errar em silêncio corrompe o repositório do usuário.
4. **Cravar `gix` agora, sem spike.** Rejeitado pelo precedente do ADR-0033 descrito no §D2.

## Consequências

- **Fica mais fácil:** conter o agente. Git passa a ser API tipada dentro do Jail, não processo com shell.
- **Fica mais difícil:** cobrir operações exóticas. O que a biblioteca não expõe, o produto não faz — e diz que não faz, em vez de cair para o `git` do sistema. Um fallback silencioso para `Command` seria a versão Git do fallback de ambiente que a Fase 7 Etapa 6+1 teve de remover.
- **A Etapa 3 começa com um PR de spike**, cujo resultado pode contradizer a preferência declarada. Se contradisser, um ADR novo substitui este §D2 — ADRs são imutáveis (§1.6).
- **`exec.shell` continua podendo rodar `git`**, sob denylist e aprovação. Isto não é contradição: é o usuário pedindo explicitamente, com o comando à vista, e não a ferramenta contornando a camada por dentro.

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 1 da Fase 8.
