# Fase 8, Etapa 2b — o `cmd.exe` sai da decisão

<!--
Estado: implementado
Verificado contra o código em: 2026-08-16
Fase correspondente: 8
-->

Narrativa técnica da Etapa 2b: como a pendência herdada do [ADR-0037](../../decisions/0037-exec-shell-fora-do-catalogo.md) foi fechada, o que a medição contradisse, e o caminho de fuga que ninguém tinha visto.

**Não duplica o `CHANGELOG.md`** (§1.7), que registra o efeito pro usuário. Aqui mora o processo.

## A etapa que a fase herdou sem escolher

O [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md) §D6 abriu a Fase 8 com a Fase 7 reaberta e absorveu a pendência como Etapa 2b, com uma trava explícita: a Fase 8 não fecha antes de a Fase 7 fechar. A Etapa 2b é essa dívida sendo paga.

O ADR-0037 §D5 deixou três requisitos escritos. O primeiro e o terceiro eram trabalho de engenharia com forma conhecida. O segundo era diferente — ele pedia **uma resposta a um fato**:

> ou a allowlist passa a listar só o que roda sob integridade baixa (o que a reduz a `echo` e `find`, e aí vale perguntar se a ferramenta se justifica), ou o modelo de isolamento muda para um que os binários MSYS2 tolerem (o que reabre o ADR-0031).

Duas saídas, as duas ruins. A primeira entregava uma ferramenta com dois comandos. A segunda afrouxava o isolamento que a Fase 7 inteira construiu. A etapa começou tentando escolher entre elas — e terminou descobrindo que a pergunta estava mal posta.

## Medir antes de escolher

O precedente que obrigou a medir é o do [ADR-0033](../../decisions/0033-sandbox-network-policy.md): o DNS intercept foi cravado por plausibilidade, implementado, e removido inteiro quando alguém finalmente o exercitou. O [ADR-0040](../../decisions/0040-git-engine-biblioteca-e-fronteira.md) §D1 tirou disso uma regra — biblioteca se escolhe por spike, não por plausibilidade. Aqui a regra virou: **allowlist se escolhe por medição, não por herança de ADR.**

O arnês rodava candidatos pelo `SecurityJailResolver` real, com `SecurityJailConfig::secure_default()`, e imprimia exit code, stdout e stderr. Três rodadas, e cada uma mudou a pergunta da seguinte.

### Rodada 1 — nada funciona, e não pelo motivo esperado

Todos os 31 candidatos externos falharam com a mesma mensagem: `'x' não é reconhecido como um comando interno ou externo`. Não `0xC0000022`. Não crash. **Não encontrado.**

Isso não é a assinatura de incompatibilidade com integridade baixa — é a assinatura de um binário que não está no `PATH`. Os únicos que passaram foram os builtins do `cmd.exe`: `echo`, `dir`, `type`, `ver`, `vol`, `cd`, `set`.

### Rodada 2 — o filho não tem `PATH`

O `set` de dentro do sandbox mostrou nove variáveis:

```
COMSPEC, PATHEXT, PROMPT, PYTHONIOENCODING, SystemRoot, TEMP, TMP, USERPROFILE, windir
```

`PATH` não está lá. O `EnvFilter` da Etapa 6 monta um env block mínimo — só o `EnvAllowlist::REQUIRED` — e `PATH` ficou de fora. O `where.exe`, chamado por caminho absoluto, confirmou de dentro: `ERRO: variável de ambiente "PATH" não encontrada`.

Chamados por caminho absoluto, os nativos do `System32` rodaram todos: `findstr`, `sort`, `more`, `fc`, `tree`, `whoami`, `hostname`, `certutil`, `tasklist`, `chcp`, `curl`, `tar`, `attrib`, `ipconfig`. Exit 0, saída real.

**A premissa do §D5 item 2 caiu aqui.** A allowlist da v1 não estava sendo derrotada pelo rótulo de integridade baixa. Ela estava sendo derrotada por não haver `PATH` — e isso vale para **todos** os 8 comandos externos dela, inclusive `find`, que o ADR-0037 contava como um dos dois sobreviventes. A ferramenta que a Etapa 7 declarou entregue executava, na prática, `echo`.

### Rodada 3 — o MSYS2 também roda

Chamados por caminho absoluto e fazendo trabalho real (não só `--version`), os binários do Git for Windows executaram sob o sandbox: `ls -la`, `cat arquivo`, `grep alfa arquivo`, `wc -l arquivo`, `pwd`. Todos exit 0.

O `0xC0000022` do ADR-0037 não se reproduziu. Isso não faz daquela medição um erro — o erro do runtime MSYS2 depende de já existir um processo MSYS2 em outro nível de integridade, então varia com o contexto. Mas estabelece o suficiente: **não é incompatibilidade estrutural**, e portanto não sustenta nem encolher a allowlist nem reabrir o ADR-0031.

As duas saídas do §D5 item 2 caíram juntas. Sobrou a saída que ele não tinha previsto: **manter o modelo de isolamento e trocar quem resolve o programa.**

## O caminho de fuga que a medição achou de brinde

Enquanto testava a resolução por nome, um caso entrou no arnês quase por reflexo: plantar um impostor no diretório de trabalho.

```
`find "alfa" amostra.txt` com find.bat plantado -> exit=0 | out=SEQUESTRADO-PELO-WORKDIR
```

O `cmd.exe` procura o programa no diretório corrente **antes** do `PATH`. O diretório corrente do filho é o workspace da conversa — exatamente onde o `files.write` escreve.

Encadeando: o assistente escreve `findstr.bat` no workspace (capacidade que ele tem, com aprovação de escrita) e chama `exec.shell` com `findstr` (comando allowlisted, com aprovação de execução). Duas permissões legítimas, nenhuma violada, e o resultado é execução arbitrária dentro do sandbox. A allowlist de comandos não tinha como impedir: ela valida o **nome**, e quem escolhe o **arquivo** é o `cmd.exe`.

Não estava em nenhum ADR, spec ou threat model. Agora está — e a lição é mais larga que a ferramenta: **qualquer coisa que deixe o `cmd.exe` resolver nome de programa herda esse buraco.**

## O desenho que saiu disso

Uma frase: **o `cmd.exe` não resolve programa.**

- **Builtins** (`cd`, `dir`, `echo`, `type`, `ver`, `vol`) rodam como `cmd.exe /d /v:off /c <nome> <args…>`. O `/d` pula o `AutoRun` do registro — que é execução arbitrária a cada `cmd /c` —, o `/v:off` desliga a expansão atrasada.
- **Externos** (`fc`, `findstr`, `more`, `sort`, `tree`) são spawn direto de `%SystemRoot%\System32\<arquivo>`. Sem `cmd.exe`, sem `PATH`, sem diretório corrente, sem `PATHEXT`. Sem busca, não há o que sequestrar.
- **Metacaracteres são recusados**, não escapados: `&`, `|`, `<`, `>`, `^`, `(`, `)`, `%`, `!`, `\n`, `\r`, `\0`.
- **Argumentos viajam como `argv`.** A tokenização é mínima — espaço separa, aspa dupla agrupa, aspa desbalanceada é erro. A contrabarra não escapa nada, porque no Windows ela é separador de caminho.

O nome do arquivo faz parte da lista (`more.com`, não `more.exe`): resolver por extensão suposta erra.

Três programas ficaram de fora **apesar de rodarem**, e o motivo está registrado porque é o tipo de coisa que alguém desfaz sem querer daqui a seis meses: `curl`, `tar` e `certutil` são saída de rede e escrita em disco. `attrib` escreve quando recebe argumento. `whoami`, `hostname`, `tasklist` e `ipconfig` falam do host, não do workspace.

## Ver falhar antes de passar

O item 3 do §D5 pedia teste de negação por caminho de fuga, "cada um visto falhando antes de passar". A verificação foi feita reintroduzindo deliberadamente o comportamento da v1 — `cmd /c <comando inteiro>` com validação de primeiro token — e rodando a suíte nova contra ele:

| Teste | Sob a v1 |
|---|---|
| `refuses_command_smuggled_behind_a_separator` | **falhou** |
| `refuses_binary_planted_in_the_workspace` | **falhou** — executou o impostor |
| `refuses_program_outside_the_closed_list` | **falhou** |
| `refuses_unbalanced_quotes_instead_of_guessing` | **falhou** |
| `refuses_allowlisted_program_reached_by_absolute_path` | passou |
| `refuses_destructive_command_by_denylist` | passou |

Os dois últimos passaram porque aqueles caminhos nunca estiveram abertos. Registrar isso vale mais que fabricar uma falha: **um teste que nunca falhou prova menos, e o leitor tem o direito de saber quais são.**

Os três controles positivos também falharam sob a v1 — pelo motivo da rodada 2: sem `PATH`, o `findstr` não resolvia. O controle positivo não é decoração nesta suíte; sem ele, "não executou" leria como "recusou" em todas as negações.

## O que a etapa deixa aberto

- **Leitura fora do workspace continua possível.** Integridade baixa restringe escrita, não leitura: `type C:\caminho\fora.txt` lê. Mesma lacuna do `exec.python`, já nomeada no threat model, com fechamento dependendo de filtro no nível de processo (WFP/WDAC) — que o ADR-0039 §D4 manteve fora desta fase por ser de outra natureza. Fixada em teste, no padrão do `e2e_network_raw_socket_bypasses_proxy_documented`: o teste afirma o comportamento **real** e quebra no dia em que ele mudar.
- **A denylist virou redundante** e está declarada como tal, com a redundância verificada em teste. Ela fica como tripwire para quando a allowlist crescer.

## O padrão que já tem seis ocorrências

A Etapa 1 desta fase fechou com três ADRs corrigindo premissa falsa encontrada no código. A Etapa 2b acrescentou três achados que contradizem documentos deste repositório: `PATH` ausente, MSYS2 rodando, e o sequestro pelo workspace.

Seis em duas etapas seguidas deixa de ser coincidência e vira regra operacional, que fica registrada aqui e no [ADR-0044](../../decisions/0044-exec-shell-com-resolucao-propria-de-programa.md) §Consequências:

**Premissa de ADR anterior é hipótese, não fato. Medir é barato comparado a construir sobre ela.**

O caso desta etapa é o mais caro dos seis, porque a premissa errada estava dentro de um requisito de aceite: se ela tivesse sido cumprida ao pé da letra, a allowlist teria encolhido para `echo` + `find` — e `find` não funciona. A ferramenta teria "voltado" com um comando útil, cumprindo formalmente o §D5.

## Referências

- [ADR-0044](../../decisions/0044-exec-shell-com-resolucao-propria-de-programa.md) — a decisão desta etapa
- [ADR-0037](../../decisions/0037-exec-shell-fora-do-catalogo.md) — a remoção que a originou
- [ADR-0039](../../decisions/0039-fase-8-escopo-e-etapas.md) §D6 — por que a etapa é desta fase
- `crates/e2e/tests/e2e_exec_shell_hardened.rs` — os 11 testes
- [`exec-tools-specification.md`](../../architecture/exec-tools-specification.md) §`FilesExecShellTool`

## Histórico de revisão

- 2026-08-16 — Etapa 2b fechada. `exec.shell` de volta ao catálogo; Fase 7 reclosada; pré-condição 2 de 2 do ADR-0039 §D6 satisfeita.
