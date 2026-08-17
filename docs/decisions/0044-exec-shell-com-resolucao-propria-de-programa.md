# 0044 — `exec.shell` volta ao catálogo: quem resolve o programa é o Frederico, não o `cmd.exe`

> **Substitui parcialmente o [ADR-0037](0037-exec-shell-fora-do-catalogo.md)** — mantém integralmente o diagnóstico do §D1 (a allowlist da v1 era contornável) e a decisão de tirar a ferramenta do catálogo enquanto ela fosse assim. Revisa o §D5 item 2, cuja premissa a medição desta etapa provou falsa, e reverte o §D1/§D3 agora que os três requisitos do §D5 estão cumpridos.

## Contexto

O [ADR-0037](0037-exec-shell-fora-do-catalogo.md) tirou `exec.shell` do catálogo em 2026-08-16 e listou, no §D5, os três requisitos para a ferramenta voltar:

1. recusa de metacaracteres de shell antes do spawn;
2. uma resposta ao problema dos binários MSYS2 sob integridade baixa — "ou a allowlist passa a listar só o que roda sob integridade baixa (o que a reduz a `echo` e `find`), ou o modelo de isolamento muda";
3. um teste de negação por caminho de fuga conhecido, cada um visto falhando antes de passar.

O [ADR-0039](0039-fase-8-escopo-e-etapas.md) §D3 alocou esse trabalho como **Etapa 2b da Fase 8**, e §D6 fez dele uma das duas pré-condições para a Fase 8 fechar. Este ADR é a decisão dessa etapa.

O item 2 exigia medir antes de decidir. A medição foi feita (Windows 11 26200.9168, `SecurityJailResolver::spawn` com `SecurityJailConfig::secure_default()`, arnês descartado depois de virar teste), e **contradisse a premissa em que o próprio item 2 se apoiava**.

### O que foi medido (2026-08-16)

**1. O filho não tem `PATH`. Nenhum comando externo resolve por nome — e essa, não a integridade baixa, era a causa.**

O env block que chega ao filho tem exatamente nove variáveis:

```
COMSPEC, PATHEXT, PROMPT, PYTHONIOENCODING, SystemRoot, TEMP, TMP, USERPROFILE, windir
```

`PATH` não está entre elas — o `EnvFilter` da Etapa 6 monta um env block mínimo (só o `EnvAllowlist::REQUIRED`), e `PATH` ficou de fora. O `where.exe`, chamado por caminho absoluto, confirma de dentro do sandbox: `ERRO: variável de ambiente "PATH" não encontrada`.

A consequência para a v1 é total. Dos 9 comandos da `SHELL_ALLOWLIST_DEFAULT`, **apenas `echo` funcionava** — e só porque é builtin do `cmd.exe`, que não precisa de `PATH`. Os outros 8 falhavam todos com `'x' não é reconhecido como um comando interno ou externo`, **inclusive `find`**, que o ADR-0037 contava como um dos dois sobreviventes. A ferramenta que a Etapa 7 declarou entregue executava, na prática, um comando.

**2. Binários nativos do `System32` rodam sob `Mandatory Label\Low`.**

Chamados por caminho absoluto — contornando a ausência de `PATH` —, todos executaram com `exit=0` e saída real: `findstr`, `sort`, `more`, `fc`, `tree`, `whoami`, `hostname`, `certutil`, `tasklist`, `chcp`, `curl`, `tar`, `attrib`, `ipconfig`. O mesmo vale com spawn direto, sem `cmd.exe` no meio.

**3. Os binários MSYS2 também rodam.** 

`ls -la`, `cat arquivo`, `grep alfa arquivo`, `wc -l arquivo` e `pwd`, do Git for Windows, executaram sob o sandbox com `exit=0` e saída correta — trabalho real, não só `--version`. O `0xC0000022` que o ADR-0037 registrou **não se reproduziu** no estado atual do código.

Não é acusação de erro de medida: aquele erro é conhecido por depender de contexto (o runtime MSYS2 tenta criar objeto nomeado em `\BaseNamedObjects`, e o resultado varia conforme já exista um processo MSYS2 em outro nível de integridade). O que a medição estabelece é mais estreito e suficiente: **a incompatibilidade não é estrutural**, e portanto não sustenta encolher a allowlist nem reabrir o [ADR-0031](0031-fase-7-isolation-model-windows.md).

**4. Caminho de fuga que ninguém tinha nomeado: sequestro de binário pelo diretório corrente.**

O `cmd.exe` procura o programa no diretório corrente **antes** do `PATH`. O diretório corrente do filho é o workspace da conversa — exatamente onde o `files.write` escreve. Plantar `find.bat` no workspace e pedir `find alfa arquivo.txt` pelo caminho real da ferramenta executou **o arquivo plantado** (`exit=0`, saída `SEQUESTRADO-PELO-WORKDIR`). Com `findstr.com` plantado, o `cmd /c findstr ...` escolheu o impostor sobre o de `System32`.

Isto é execução arbitrária dentro do sandbox usando **só** capacidades já concedidas: uma escrita de arquivo e um comando allowlisted. A allowlist de comandos não tinha como impedir, porque ela valida o nome e o `cmd.exe` decide o arquivo.

**5. Aspas não sobrevivem ao caminho da v1.** O `build_cmdline` do `SecurityJailResolver` cita o argumento inteiro e duplica aspas internas; com o comando indo como um único argumento pro `cmd /c`, `find "alfa" arquivo.txt` chegava mutilado. Qualquer comando com aspas estava quebrado.

### O que a medição junta

A v1 não falhava em um ponto. Ela **não impedia** o que prometia impedir (itens 4 e 1 do ADR-0037), **não permitia** quase nada do que prometia permitir (item 1 acima), e **quebrava** o que permitia (item 5). A causa comum dos quatro é uma só: **o `cmd.exe` era quem resolvia o programa e interpretava a linha**. É isso que este ADR muda.

## Decisões

### D1 — O `cmd.exe` deixa de resolver programa

`frederico_security::exec_patterns::plan_command` passa a ser a porta única: recebe o command string cru e devolve o par (programa, `argv`), ou a razão da recusa. Dois caminhos de execução, e só dois:

- **Builtin do `cmd.exe`** (`cd`, `dir`, `echo`, `type`, `ver`, `vol`) — não é arquivo, vive dentro do `cmd.exe`. Executado como `cmd.exe /d /v:off /c <nome> <args…>`, com os argumentos já tokenizados como `argv`. O `/d` pula o `AutoRun` do registro (`HKCU\…\Command Processor\AutoRun` é execução arbitrária a cada `cmd /c`); o `/v:off` desliga a expansão atrasada, que o registro também liga.
- **Executável do `System32`** — spawn **direto** de `%SystemRoot%\System32\<arquivo>`. Sem `cmd.exe`, sem `PATH`, sem diretório corrente, sem `PATHEXT`. Não há busca, então não há o que sequestrar.

Isto fecha o item 4 da medição por construção, não por filtro: não existe etapa de resolução para um atacante influenciar.

### D2 — Metacaracteres são recusados antes do spawn (item 1 do ADR-0037 §D5)

`SHELL_METACHARACTERS` lista `&`, `|`, `<`, `>`, `^`, `(`, `)`, `%`, `!`, `\n`, `\r` e `\0`. Comando que contenha qualquer um é recusado — nunca escapado, conforme o próprio §D5 ("as regras de quoting do `cmd.exe` são notoriamente inconsistentes, e um escapador próprio seria superfície nova").

`&&` e `||` não precisam de entrada própria: são o mesmo caractere repetido. O `!` e o `/v:off` do D1 cobrem a expansão atrasada pelos dois lados, porque uma proteção sozinha depende de a outra nunca ser removida por engano.

A aspa dupla **não** está na lista, deliberadamente: é o único jeito de um argumento conter espaço, e sem `cmd.exe` resolvendo ela não separa comando nenhum. O tokenizador a consome como delimitador e exige balanceamento.

### D3 — A allowlist nova é medida, e o critério de entrada é declarado (item 2 do ADR-0037 §D5)

Onze programas, todos verificados rodando sob `Mandatory Label\Low` pelo caminho real da ferramenta:

| Tipo | Programas |
|---|---|
| Builtins do `cmd.exe` | `cd`, `dir`, `echo`, `type`, `ver`, `vol` |
| `System32`, por caminho absoluto | `fc` (`fc.exe`), `findstr` (`findstr.exe`), `more` (`more.com`), `sort` (`sort.exe`), `tree` (`tree.com`) |

O critério: **inspeção read-only do workspace**. Rodar sob o sandbox é necessário e não é suficiente. O que foi medido rodando e mesmo assim ficou de fora, com o motivo:

- `curl`, `tar`, `certutil` — saída de rede e escrita em disco (`certutil -urlcache -split -f <url>` é LOLBIN de download conhecido). Uma allowlist de inspeção não os inclui.
- `attrib` — sem argumento lê, com argumento escreve. A allowlist não distingue, então não entra.
- `whoami`, `hostname`, `tasklist`, `ipconfig` — read-only, mas devolvem identidade e estado do **host**, não do workspace. Fora do que a ferramenta se propõe a fazer.
- `find` — exige aspas literais na linha de comando pro termo de busca, o que só existe quando um shell monta a linha; com `argv` controlado ele recusa (`FIND: formato de parâmetro incorreto`, medido). O `findstr` faz o mesmo, melhor.
- `where` — depende de `PATH`, que o filho não tem. Erraria sempre.
- **Binários MSYS2 (`ls`, `cat`, `grep`, `wc`, `pwd`)** — rodam, mas vêm do Git for Windows, que pode não estar instalado. Depender do ambiente da máquina do usuário é o que o [ADR-0031](0031-fase-7-isolation-model-windows.md) e o [ADR-0040](0040-git-engine-biblioteca-e-fronteira.md) §D1 rejeitam: comportamento não reprodutível, erro indiagnosticável. Ficam de fora **por essa razão**, não pela do ADR-0037.

O nome do arquivo é parte da lista (`more.com`, não `more.exe`) porque resolver por extensão suposta erra.

### D4 — Tokenização mínima, `argv` controlado

`split_command` separa por espaço e entende aspa dupla como delimitador de token com espaços. Nada além: sem escapes, sem expansão de variável, sem globbing, sem substituição de comando. Aspa não balanceada é erro, não chute.

A contrabarra **não** escapa: no Windows ela é separador de caminho, e tratá-la como escape quebraria `type sub\arquivo.txt`, que é o uso normal. Consequência declarada: um argumento não pode conter aspa literal.

Isto não é "um parser de shell próprio", que o §D5 alertava contra. É o oposto: é o mínimo necessário para **não** haver shell — os argumentos saem daqui como `argv` e o `build_cmdline` do `SecurityJailResolver` é quem os cita, uma única vez, no ponto onde o Win32 exige.

### D5 — A denylist fica, declarada redundante, com a redundância verificada

`SHELL_DENYLIST` continua sendo consultada primeiro. Hoje ela é **redundante por construção**: nada nela resolve na allowlist do D3, então `plan_command` já recusaria por outro gate. Ela fica como tripwire para o dia em que a allowlist crescer, e a mensagem de erro que ela produz é mais útil que "não está na lista".

O que muda em relação à v1 é o estatuto: ela deixa de ser apresentada como camada de defesa. E a redundância é **verificada em teste** (`exec_patterns::tests::denylist_is_redundant_with_allowlist`), não assumida — se alguém acrescentar à allowlist um programa que aparece na denylist, o teste quebra e a contradição fica visível, em vez de a denylist voltar a ser a única barreira sem ninguém notar.

### D6 — Autorização explícita para substituir os testes nomeados no `status.md`

A REGRA §3.4 proíbe apagar ou renomear teste nomeado na coluna `E2E de cobertura` sem ADR. Este é o ADR.

`crates/e2e/tests/e2e_exec_shell_out_of_catalog.rs` é apagado: ele cobria a **ausência** da ferramenta, e a ferramenta voltou. Seus dois testes tinham data de validade escrita neles — o próprio ADR-0037 §D4 dizia que o dia em que o `exec_patterns` recusasse separadores, `allowlist_that_justified_the_tool_is_still_defeated_by_cmd_separators` passaria a falhar, "e a falha é o sinal de que D5 pode ser reaberta". Foi o que aconteceu.

No lugar entra `crates/e2e/tests/e2e_exec_shell_hardened.rs`, com 11 testes: um por caminho de fuga conhecido (item 3 do §D5), mais três controles positivos.

### D7 — Um teste de negação por caminho de fuga, cada um visto falhando antes de passar (item 3 do ADR-0037 §D5)

Os testes foram rodados contra uma reintrodução deliberada e temporária do comportamento da v1 (`cmd /c <comando inteiro>` com validação de primeiro token). O resultado, registrado:

| Caminho de fuga | Teste | Sob a v1 |
|---|---|---|
| Contrabando atrás de separador | `refuses_command_smuggled_behind_a_separator` | **falhou** |
| Sequestro de binário pelo workspace | `refuses_binary_planted_in_the_workspace` | **falhou** — executou o impostor |
| Programa fora da lista fechada | `refuses_program_outside_the_closed_list` | **falhou** |
| Aspas não balanceadas | `refuses_unbalanced_quotes_instead_of_guessing` | **falhou** |
| Caminho absoluto como atalho pra allowlist | `refuses_allowlisted_program_reached_by_absolute_path` | passou (a v1 também recusava) |
| Comando destrutivo | `refuses_destructive_command_by_denylist` | passou (a denylist já existia) |

Os dois últimos passaram sob a v1 porque aqueles caminhos não estavam abertos nela. Registrar isso é mais útil que fabricar uma falha: o teste que nunca falhou prova menos, e o leitor tem o direito de saber quais são.

Os controles positivos (`runs_an_allowlisted_command_for_real`, `runs_a_system32_program_for_real`, `quoted_argument_survives_as_one_argument`) também falharam sob a v1 — pelo motivo do item 1 da medição: sem `PATH`, o `findstr` não resolvia.

### D8 — `exec.shell` volta ao catálogo, e a Fase 7 volta a `concluída`

O bump é atômico nos três lugares, como o ADR-0037 §D1 o desfez (regra do ADR-0020 §3 D3): `build_default_exec_tools` volta a construir a ferramenta, `build_default_allowed_for_run` volta a incluir `exec.shell`, e `PermissionSet::terminal` volta a `TerminalMode::Allowlist`. Os três se movem juntos, e um teste dedicado (`composition::tests::exec_shell_returns_atomically_with_allowlist_and_permission`) verifica os três de uma vez — registrar a tool sem pôr na allowlist a torna inalcançável em silêncio, e anunciar `terminal: Allowlist` sem tool de terminal é relatar capacidade inexistente.

Com os três itens do ADR-0037 §D5 cumpridos, o critério de "done" da Fase 7 no roadmap volta a estar satisfeito e a fase volta a `concluída`. Isso fecha a **pré-condição 2 de 2** do [ADR-0039](0039-fase-8-escopo-e-etapas.md) §D6. A pré-condição 1 (run verde citável do `CI Nightly`) segue aberta e é a Etapa 2.

### D9 — O que esta ferramenta continua não protegendo

O rótulo de integridade baixa restringe **escrita**, não leitura. `type C:\caminho\fora\do\workspace.txt` lê o arquivo. Não é regressão desta etapa nem é específico do `exec.shell` — o `exec.python` faz o mesmo com um `open()`, e o `security-threat-model.md` já nomeia "read-up de paths Medium-labeled" entre o que o sandbox não protege. Fechar exige filtro no nível de processo (WFP/WDAC), que o ADR-0039 §D4 manteve fora da Fase 8 por ser de outra natureza.

A limitação é fixada em teste (`documented_limit_child_can_read_outside_workspace`), no mesmo padrão do `e2e_network_raw_socket_bypasses_proxy_documented`: o teste afirma o comportamento **real**, e quebra no dia em que ele mudar. E está declarada no `SECURITY.md`.

### D10 — A marca `somente-planejamento` da Fase 8 fica, com o sentido explicitado

O [ADR-0038](0038-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md) criou a marca literal `somente-planejamento` na coluna "Evidência" do `status.md` para afrouxar a trava da REGRA §1.13 enquanto uma fase tem só a Etapa 1 fechada, e disse que ela "some no primeiro PR de código da fase".

Esta etapa é o primeiro PR de código da Fase 8 e **a marca fica**. O motivo é que ela nunca foi sobre a fase ter código — foi sobre **os specs da fase** terem código. A Etapa 2b implementa `exec-tools-specification.md`, que é spec da **Fase 7** e já está em `parcialmente implementado`. Os três specs novos da Fase 8 (`git-integration-architecture.md`, `github-integration-architecture.md`, `project-and-milestones-architecture.md`) continuam sem uma linha de Rust, e `especificado` continua sendo o estado verdadeiro deles.

Tirar a marca agora obrigaria os três a declarar implementação inexistente — que é exatamente o defeito que o ADR-0038 foi escrito para corrigir, e que a Fase 7 cometeu no `f7d1ab3`. O sentido literal da frase do ADR-0038 apontaria para um lado; a razão dela aponta para o outro. Este ADR segue a razão e registra a diferença em vez de deixá-la implícita.

**A marca cai na Etapa 3** (`git-engine`), que é o primeiro código de um spec desta fase. O texto da célula no `status.md` passa a dizer isso, para que a marca não seja lida como "a fase não tem código" — que seria falso a partir deste PR.

## Alternativas descartadas

1. **Aposentar `exec.shell` de vez**, que o ADR-0037 §D5 explicitamente permitia ("ou é aposentado por ADR novo"). Era a saída barata, e o argumento a favor existia: com `exec.python` e `exec.node` no catálogo, um script Python faz tudo que os 11 programas fazem. Rejeitada porque a medição mostrou que o conserto é pequeno e estrutural, não um remendo — e porque "listar arquivos" e "procurar texto" atravessarem um interpretador Python com aprovação `Critical` é pior ergonomia por nenhuma segurança a mais: `exec.python` é estritamente mais permissivo que os 11 programas desta lista.

2. **Encolher a allowlist para `echo` + `find`**, como o ADR-0037 §D5 item 2 sugeria. Rejeitada porque a premissa está errada nos dois nomes: `find` não roda (não resolve sem `PATH`, e recusa `argv` sem aspas literais), e a razão pela qual os outros não rodavam não era a integridade baixa. Seguir a sugestão ao pé da letra entregaria uma ferramenta com **um** comando útil, pelo motivo errado.

3. **Reabrir o [ADR-0031](0031-fase-7-isolation-model-windows.md)** e afrouxar o modelo de isolamento para acomodar o MSYS2 — a outra saída oferecida pelo §D5 item 2. Rejeitada porque não há o que acomodar: os binários MSYS2 rodam sob o modelo atual. Afrouxar isolamento para resolver um problema que não existe é o pior negócio disponível.

4. **Injetar `PATH` no env block do filho** para que os comandos resolvam por nome. Rejeitada por duas razões independentes. A primeira: reintroduz busca, e busca é sequestrável — seria desfazer o D1 para reabrir o item 4 da medição. A segunda: o `PATH` da máquina do usuário é ambiente não reprodutível, e o `EnvFilter` mínimo da Etapa 6 o excluiu deliberadamente. O caminho absoluto entrega o mesmo resultado sem nenhuma das duas.

5. **Escapar os metacaracteres em vez de recusá-los**, aceitando `echo a ^& b` como texto literal. Rejeitada pelo mesmo argumento do ADR-0037 §D5, que este ADR não tem motivo para revisitar: o quoting do `cmd.exe` é inconsistente o bastante para que o escapador vire a nova superfície de ataque. Recusar custa expressividade que esta ferramenta não promete ter.

6. **Manter a validação como allowlist de nomes, sem mudar quem resolve o programa** — ou seja, cumprir só o item 1 do §D5. Rejeitada porque foi exatamente essa a alternativa 1 do ADR-0037, rejeitada lá com o argumento que continua valendo: consertar metade e manter a capacidade anunciada é declarar entregue o que funciona pela metade. Sem o D1, o sequestro pelo workspace continuaria aberto — e ele derrota a allowlist de nomes inteira.

## Consequências

- **Fica mais fácil:** confiar na allowlist. Ela deixa de ser uma lista de nomes que o `cmd.exe` pode ignorar e passa a ser a única fonte do caminho absoluto que vai pro `CreateProcessAsUserW`. A distância entre o que a lista diz e o que executa virou zero.
- **Fica mais difícil:** usar a ferramenta como shell. Não há pipe, redirecionamento, encadeamento nem variável de ambiente — e a mensagem de erro diz isso em vez de falhar de um jeito enigmático. Quem precisa compor comandos usa `exec.python`.
- **A Fase 7 fecha, e com ela a pré-condição 2 de 2 da Fase 8.** A Fase 8 continua bloqueada pela pré-condição 1 (noturno verde), que é a Etapa 2.
- **Um caminho de fuga novo entrou no registro do projeto.** O sequestro de binário pelo diretório corrente não estava em nenhum ADR, spec ou threat model antes desta medição, e valia para qualquer ferramenta que deixasse o `cmd.exe` resolver nome de programa. O `security-threat-model.md` passa a nomeá-lo.
- **Três medições contradisseram documentos deste repositório** (`PATH` ausente, MSYS2 rodando, sequestro pelo workspace), somando-se às três da Etapa 1 registradas no ADR-0039. O padrão já tem seis ocorrências em duas etapas seguidas e merece ser dito como regra e não como coincidência: **premissa de ADR anterior é hipótese, não fato — e medir é barato comparado a construir sobre ela.**
- **A `SHELL_ALLOWLIST_DEFAULT` e a função `is_allowed` deixam de existir**, substituídas por `resolve_program` e pelas duas listas. Quem dependia dos nomes antigos quebra na compilação, que é onde se quer quebrar.

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 2b da Fase 8; fecha o §D5 do ADR-0037 e reclosa a Fase 7.
