# 0037 — `exec.shell` sai do catálogo: a allowlist de comandos não é uma barreira

> **Substituído parcialmente pelo [ADR-0044](0044-exec-shell-com-resolucao-propria-de-programa.md)** (2026-08-16). Os três requisitos do §D5 foram cumpridos na Etapa 2b da Fase 8, então `exec.shell` voltou ao catálogo (§D1 revertido) e a Fase 7 voltou a `concluída` (§D3 revertido). O diagnóstico deste ADR — a allowlist da v1 não era barreira — continua valendo integralmente, e é ele que justifica o desenho novo. **A premissa do §D5 item 2 foi revista:** a medição da Etapa 2b mostrou que os binários da allowlist não falhavam por incompatibilidade com o rótulo de integridade baixa, e sim porque o filho não tem `PATH` — inclusive `find`, que este ADR contava como sobrevivente. Binários MSYS2 rodam sob o sandbox. Ver o §Contexto do ADR-0044.

## Contexto

A Etapa 7 da Fase 7 (PR #52, 2026-08-14) entregou `exec.shell` e, com ela, promoveu a Fase 7 a `concluída`. A ferramenta executa `cmd.exe /c "<command>"` sob o sandbox (Job Object + Restricted Token + env filtrado + proxy de rede) e se defende, além disso, com duas listas de comandos em `frederico_security::exec_patterns`:

- `SHELL_DENYLIST` — substring case-insensitive contra o comando inteiro (`rm -rf`, `format`, `reg delete`, ...).
- `SHELL_ALLOWLIST_DEFAULT` — **primeiro token** do comando (`ls`, `cat`, `head`, `tail`, `grep`, `find`, `wc`, `pwd`, `echo`).

Dois dias depois, o PR #54 (redesenho do README, 2026-08-16) removeu as três menções a `exec.shell` do README afirmando que a ferramenta "foi descartada no mesmo dia (ver PR #52) por bypass de allowlist via `cmd.exe` e incompatibilidade estrutural dos binários MSYS2 com o token de integridade baixa do sandbox". Nenhum código foi removido, e `docs/status.md`, `CHANGELOG.md`, `SECURITY.md` e `exec-tools-specification.md` continuaram descrevendo a ferramenta como entregue.

O repositório passou a contar **duas histórias incompatíveis** sobre a mesma capacidade. Este ADR resolve a contradição decidindo qual das duas é verdadeira — e a resposta foi medida, não arbitrada.

### O que foi medido (2026-08-16)

**1. A allowlist é contornável por qualquer separador do `cmd.exe`.**

`is_allowed` valida só o primeiro token (`exec_patterns.rs`), e `FilesExecShellTool::build_args` entrega o command string **inteiro** pro `cmd.exe /c` (`shell.rs`). O `cmd.exe` interpreta `&`, `&&`, `||` e `|` como separadores de comando. Logo, qualquer comando arbitrário viaja de carona atrás de um token allowlisted.

Rodado pelo caminho real da ferramenta, antes da remoção:

| Comando | Resultado |
|---|---|
| `ver` | **recusado** — `ver` não está na allowlist |
| `echo marcador & ver` | **executado**, com a saída dos dois comandos no `stdout` |

Não é um bypass de canto que exige entrada exótica: é a allowlist não existindo. Qualquer `command` que comece com `echo` executa o que vier depois do `&`. A denylist tampouco pega o carona — ela casa substring literal, e `rm -r -f` (flags separadas) já era um bypass conhecido e fixado em teste desde a Etapa 7.

**2. Os binários da allowlist não rodam sob o rótulo de integridade baixa.**

Dos 9 comandos da `SHELL_ALLOWLIST_DEFAULT`, só `echo` (builtin do `cmd.exe`) e `find` (nativo do Windows) existem numa instalação limpa. Os outros 7 — `ls`, `cat`, `head`, `tail`, `grep`, `wc`, `pwd` — são binários MSYS2 que vêm do Git for Windows. Ao executar um deles dentro do sandbox, o processo morre na inicialização:

```
whoami.exe: *** fatal error - NtCreateDirectoryObject(\BaseNamedObjects\msys-2.0S5-...): 0xC0000022
```

`0xC0000022` é `STATUS_ACCESS_DENIED`: o runtime MSYS2 exige criar objetos nomeados no kernel, o que o Mandatory Label\Low do sandbox nega por construção. Isso não é um bug a corrigir do lado do Frederico — é incompatibilidade estrutural entre o runtime MSYS2 e o modelo de isolamento escolhido no ADR-0031.

O efeito combinado: a allowlist **não impede** o que deveria impedir, e **não permite**, na prática, quase nada do que prometia permitir. A Etapa 7 já registrava a limitação do "primeiro token" como pendência nomeada; o que não estava registrado é que a limitação esvazia a barreira inteira.

## Decisões

### D1 — `exec.shell` sai do catálogo

`build_default_exec_tools` deixa de construir `FilesExecShellTool`; `build_default_allowed_for_run` deixa de incluir `exec.shell` na allowlist; o bump de `PermissionSet::terminal` para `TerminalMode::Allowlist` volta para o default `None`. Os três se movem **juntos** — bump atômico nos dois sentidos (ADR-0020 §3 D3). Anunciar `terminal: Allowlist` num `PermissionSet` sem nenhuma ferramenta de terminal registrada seria relatar capacidade inexistente.

A regra aplicada é a que este projeto já aplicou duas vezes: **capacidade incompleta é capacidade indisponível**. Foi ela que tirou `exec.python`/`exec.node` do catálogo na Etapa 5+ (path safety não fechada) e que deletou `dns_intercept`/`dns_proxy` na Etapa 6 (mecanismo que nunca protegeu nada). A diferença aqui é só que a capacidade chegou a ser anunciada como concluída.

### D2 — O código de `shell.rs` fica no repositório

Diferente do `dns_intercept` (removido por inteiro, porque o mecanismo era irrecuperável), `exec.shell` tem conserto conhecido — ver D5. O arquivo permanece, com o construtor marcado `#[allow(dead_code)]` e a razão documentada no topo. Nada o constrói enquanto o ADR não for reaberto.

### D3 — A Fase 7 volta para `em andamento`

A Fase 7 foi promovida a `concluída` tendo `exec.shell` como uma das entregas, e o critério de "done" do roadmap cita explicitamente "`exec.shell` com `Denylist` recusa comandos destrutivos". Com a ferramenta fora do catálogo, o critério não está cumprido.

Manter a fase `concluída` exigiria ou reescrever o critério de done (mudar a régua depois da medição) ou declarar cumprido o que não está. As duas saídas são piores que a honesta: a fase volta a `em andamento` com a pendência nomeada, e fecha quando D5 fechar.

### D4 — Os dois E2E de `exec.shell` são apagados; a ausência ganha cobertura

`crates/e2e/tests/e2e_exec_shell_allowlist.rs` e `crates/e2e/tests/e2e_exec_shell_denylist.rs` testavam o comportamento de uma ferramenta que saiu do produto, e ambos estão nomeados na coluna `E2E de cobertura` do `status.md` — a REGRA §3.4 proíbe apagar teste nomeado **sem ADR**. Este é o ADR, e a autorização é explícita.

No lugar entra `crates/e2e/tests/e2e_exec_shell_out_of_catalog.rs`, com dois testes:

- `exec_shell_is_not_in_default_catalog` — negação: a ferramenta não volta ao catálogo sem ADR novo. Tem controle positivo (`exec.python` e `exec.node` continuam presentes), senão passaria com o catálogo vazio.
- `allowlist_that_justified_the_tool_is_still_defeated_by_cmd_separators` — fixa a **razão** da remoção. Quando o `exec_patterns` aprender a recusar separadores, este teste falha, e a falha é o sinal de que D5 pode ser reaberta.

A prova de unidade equivalente vive em `frederico_security::exec_patterns::tests::allowlist_is_defeated_by_cmd_exe_command_separators`.

### D5 — O que `exec.shell` precisa entregar para voltar

Os três, simultaneamente:

1. **Recusa de metacaracteres de shell antes do spawn** — `&`, `&&`, `|`, `||`, `>`, `<`, `^`, `(`, `)`, `%`, `\n`. Sem isso a allowlist é decorativa. Recusar é preferível a tentar escapar: o `cmd.exe` tem regras de quoting notoriamente inconsistentes, e um parser próprio seria uma superfície nova.
2. **Uma resposta ao problema dos binários MSYS2** — ou a allowlist passa a listar só o que roda sob integridade baixa (o que a reduz a `echo` e `find`, e aí vale perguntar se a ferramenta se justifica), ou o modelo de isolamento muda para um que os binários MSYS2 tolerem (o que reabre o ADR-0031).
3. **Um teste de negação por caminho de fuga conhecido**, cada um visto falhando antes de passar.

Enquanto os três não fecharem, a ferramenta fica fora — e a Fase 7 fica `em andamento`.

## Alternativas consideradas

**1. Corrigir a allowlist agora (recusar metacaracteres) e manter a ferramenta.** Tecnicamente pequeno e resolveria o item 1 do D5. Rejeitada como resolução **desta** contradição por não resolver o item 2: mesmo com a allowlist funcionando, 7 dos 9 comandos permitidos não executam sob o sandbox, e a ferramenta entregaria `echo` e `find`. Consertar metade e manter a capacidade anunciada repetiria exatamente o erro que este ADR corrige — declarar entregue o que funciona pela metade. O conserto continua desejável, e é o item 1 do D5.

**2. Manter no catálogo e documentar a allowlist como não-efetiva.** Menor esforço, e honesto no papel. Rejeitada porque deixaria no produto uma ferramenta `risk_level: Critical` cuja única barreira real é o Jail — e a aprovação obrigatória por invocação, que protege contra a IA agindo sozinha, não protege contra um usuário que aprova um comando cujo lado direito do `&` ele não leu. O `SECURITY.md` passaria a descrever um controle decorativo como camada de defesa.

**3. Reescrever o critério de "done" da Fase 7 para não citar `exec.shell`, mantendo a fase `concluída`.** Rejeitada: mudar a régua depois de medir é a forma mais silenciosa de perder a régua. A REGRA §1.8 e o `status.md` §"Regra de promoção" existem para impedir exatamente isso.

## Consequências

- A Fase 7 volta a `em andamento`; a Fase 8, que depende dela (`8 → 3 + 4 + 6 + 7`), herda a pendência. Como a Fase 8 absorve o Modo Desenvolvedor integrado, é razoável que D5 seja retomada como etapa dela.
- O `README.md` já está correto e **não muda** — foi o único documento que contava a história verdadeira.
- Nenhuma funcionalidade que o usuário usava se perde: a ferramenta nunca chegou a uma release publicada.
- O `PermissionSet` reportado pela casca deixa de anunciar `terminal: allowlist`. Perfis TOML que já configuravam terminal continuam válidos — o campo existe, só não é bumpado por padrão.
- Fica registrado o precedente de que **um documento sozinho pode estar certo contra todos os outros**. O README estava; a contradição só apareceu porque alguém foi conferir. Vale a leitura no `AGENTS.md`: o primeiro contato de uma sessão nova precisa dizer onde a verdade mora.
