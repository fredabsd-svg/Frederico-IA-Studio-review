# 0034 — Política de aprovação de ferramentas de escrita e execução

## Contexto

A Fase 7 introduz o primeiro salto de risco real do agente: ferramentas que **modificam estado do usuário** (`files.write`, `files.edit`) e ferramentas que **executam código arbitrário** (`exec.python`, `exec.node`, `exec.shell`). Até a Fase 6, o `RunExecutor` só chamava `files.read` — operação idempotente, sem efeito colateral, com barreira de Jail que já cobre `I3` (path traversal).

A Fase 3 Etapa 3 definiu o modelo de `PermissionSet` com 18 campos, hierarquia de 8 camadas (do `PROMPT MESTRE` §8) e invariante "subagente ⊆ pai". O modelo está pronto, mas a **política** de quando aprovar, em que granularidade, e como o usuário consente ainda está em aberto. O `PROMPT MESTRE` §22.5 diz: "Comandos aprovados são exibidos ao usuário exatamente como serão executados, sem abreviação" — isso é o **display**, não a **decisão de quando exibir**.

O `PermissionSet` tem `RuntimePermission` (None / ReadOnly / Sandboxed / Unrestricted) e `TerminalPermission` (None / RequireApproval / Denylist / Allowlist). Esses enums dão o formato, mas a política concreta ("quando `RequireApproval` é acionado? a cada invocação? por sessão? por primeira vez?") é decisão de Fase 7. Sem essa política:

1. O usuário tem que aprovar **tudo**, o que mata o uso (UX inviável para "instalar dependência e rodar testes").
2. O usuário tem que aprovar **nada**, o que mata a segurança (defeito da Fase 4 do projeto anterior, quando o agente rodava `rm -rf` sem pedir).
3. O default "depende" (política não-declarada) vira implementação silenciosa que ninguém lembra, ninguém testa, e ninguém confia.

A Fase 6 da Multimodelo (ADR-0027) tratou o problema análogo de budget: definir **default deny** + **mecanismo explícito de opt-in** + **auditoria de cada decisão**. O mesmo princípio se aplica aqui: default deny, opt-in explícito por escopo, auditoria de cada decisão no `DbAuditSink`.

A regra do `PROMPT MESTRE` §22.3 ("Acesso externo ao workspace só com: seleção pelo usuário, concessão de permissão, definição de leitura/escrita, registro, possibilidade de revogação") fixa os 5 elementos da aprovação: quem concede, qual permissão, em que modo (read/write), com registro, e revogável. A Etapa 1 fecha o **mecanismo**; a Etapa 7 (UI) fecha a **superfície** de revogação.

## Decisões

### D1 — Default deny para toda escrita e execução

Toda ferramenta nova da Fase 7 nasce com o campo correspondente do `PermissionSet` em **None** (a variante mais restritiva). O usuário (ou o `assistant`, ou o `project`) liga explicitamente, com escopo, e a decisão é auditada.

A frase que entra no `docs/modules/tool-registry.md` e na UI é literal: **"Ferramenta perigosa nasce desligada. Ligar é decisão consciente."** (mesma frase que o cabeçalho atual do `tool-permission-model.md` já usa para `PermissionSet` como um todo).

### D2 — Aprovação por escopo, não por invocação

A política é: **uma aprovação por escopo, não por chamada**. O usuário não quer aprovar "files.write" toda vez que o agente cria um arquivo de log, mas também não quer dar acesso permanente sem entender o que está liberando.

Escopos disponíveis (modelados como `ApprovalScope` no `PermissionSet`, extensão do `ApprovalScope` da Fase 3 Etapa 3):

| Escopo | Significado | Duração | Exemplo de uso |
|---|---|---|---|
| `OneExecution` | Aprova esta invocação apenas | até o tool_call retornar | "criar este arquivo `.env` específico" |
| `OneTurn` | Aprova todas as invocações da mesma `message_id` | até o `RunState::Completed` ou `RunState::Failed` | "rodar `pip install` + `python main.py`" |
| `OneSession` | Aprova todas as invocações da `conversation_id` | até a conversa ser arquivada | "instalar deps e rodar testes até eu fechar" |
| `OneProject` | Aprova todas as invocações de um `project_id` | até o projeto ser deletado | "este projeto confia em `exec.python`" |
| `Forever` | Aprova para o usuário inteiro | até revogação explícita | "eu sempre deixo `files.read` ligado" |

A escolha entre escopos é do usuário, **na hora da aprovação**, via UI modal. Default proposto pela UI: `OneTurn` para exec (cobre o caso "instalar + rodar"), `OneExecution` para file ops (cobre o caso "criar este arquivo"). O usuário pode mudar o default antes de confirmar.

### D3 — `exec.shell` é sempre `RequireApproval` por invocação

A única exceção à regra D2 é `exec.shell`: a política é **sempre `OneExecution`** (escopo `OneExecution` é o default, e o usuário não pode aumentar o escopo na hora da aprovação — só confirmar ou cancelar). Razão: a fronteira entre `ls` (seguro) e `rm -rf /` (catastrófico) é invisível para o `PermissionSet` (que vê só "terminal"). Sem a primitiva Allowlist/Denylist de comandos destrutivos na Etapa 6, **cada invocação precisa de confirmação explícita**.

A Etapa 6 da Fase 7 introduz `TerminalPermission::Allowlist(Vec<String>)` (lista branca de comandos "seguros": `ls`, `cat`, `grep`, `head`, `tail`, `find`, `wc`, `pwd`, `echo`, `git status` etc., todos **read-only**). Com Allowlist, o usuário pode dar escopo `OneSession` para os comandos da lista. Com Denylist (comandos explicitamente proibidos), o resto do `exec.shell` cai em `RequireApproval`. A v1 da Fase 7 entra com `RequireApproval` para tudo; a Etapa 6 introduz Allowlist/Denylist como extensão.

### D4 — `files.write` / `files.edit` seguem Jail + escopo `OneTurn` por padrão

A barreira primária é o `Jail` (D2 do ADR-0031): toda escrita passa pelo path safety antes de chegar ao disco. A aprovação é **adicional**, não no lugar do Jail.

Comportamento:

- Path **dentro do workspace** + escopo `OneTurn` aprovado pelo usuário → escreve silenciosamente, com entrada no `DbAuditSink` (`kind: 'file_write'`, `path`, `bytes`, `approved_scope: 'OneTurn'`).
- Path **fora do workspace** + escopo `OneExecution` aprovado pelo usuário → escreve, com entrada no `DbAuditSink` (`kind: 'file_write_external'`, `path`, `bytes`, `approved_scope: 'OneExecution'`). A entrada tem flag `external: true` que a UI usa para dar destaque (cor diferente, "escrita fora do workspace" no log).
- Path **fora do workspace** + escopo `OneSession`/`Forever` aprovado → escreve, com entrada idêntica, mas o log marca `external: true, escalated_scope: true` para que o usuário veja que está usando um escopo amplo para um path externo.

A regra de sobrescrita (atomic write + backup) é o ADR-0035. Esta D4 só fala de **quando pedir aprovação**, não do que fazer com o arquivo.

### D5 — Comando exato é exibido, sem abreviação

`PROMPT MESTRE` §22.5 final: "Comandos aprovados são exibidos ao usuário exatamente como serão executados, sem abreviação."

Implementação:

- A UI mostra a string literal que vai para `CreateProcess` (ou `subprocess.run`, ou `fs.write`), incluindo todos os args, na ordem, com aspas preservadas.
- Para `exec.python` com script multilinha, a UI mostra o script em bloco de código com syntax highlight.
- Para `files.write`, a UI mostra um diff do que vai ser escrito (se `files.edit`, mostra o contexto antes/depois).
- Botão "Aprovar" fica desabilitado até o usuário rolar o comando até o fim (UX padrão de EULAs, para forçar leitura).

A regra "comando exato" é **testada** (regra do user: "teste de negação"): a Etapa 4 da Fase 7 entrega `crates/e2e/tests/e2e_approval_display.rs::approved_command_matches_actual_invocation` que spawna `exec.python` com `args: ['-c', 'print(2+2)']`, captura o que o `audit_records_approval` gravou como `command_displayed`, e compara **byte-a-byte** com o que o `CreateProcess` de fato executou. Sem essa invariante, abre-se porta para "UI mostra `ls`, executor roda `rm -rf`" — exatamente a classe de bug que a regra do prompt mestre existe para evitar.

### D6 — Revogação é imediata e auditada

O usuário pode revogar qualquer escopo `OneSession`/`OneProject`/`Forever` a qualquer momento, via painel de permissões (Etapa 7 UI/Polish). A revogação:

1. Marca o campo correspondente do `PermissionSet` do `assistant`/`project`/`user` de volta para `None` (ou a variante mais restritiva).
2. Adiciona entrada em `DbAuditSink` (`kind: 'permission_revoked'`, `actor: 'user'`, `permission_field`, `previous_scope`).
3. **Não** interrompe invocações em curso (a execução atual termina com o que já estava aprovado; a próxima invocação verifica de novo). Razão: interromper mid-execution pode deixar estado inconsistente (arquivo meio escrito, processo meio iniciado). A próxima invocação é o ponto natural de checagem.

A Etapa 7 da Fase 7 implementa a UI; a Etapa 4 (exec tools) implementa o backend (`PermissionSet::revoke(scope) -> AuditEntry`).

### D7 — Subagente herda escopo do pai, com `is_subset_of` mantido

O invariante "subagente ⊆ pai" do `PermissionSet` (Fase 3 Etapa 3) é **estrito**: o subagente nunca pode escrever onde o pai não pode. A aprovação do pai **não é** propagada para o subagente — o subagente pede aprovação de novo, com o escopo que o pai tem. Razão: a aprovação é do **usuário**; o subagente não pode "estender" a confiança.

Concretamente: se o pai tem `python: RuntimePermission::Sandboxed` com escopo `OneSession` aprovado, o subagente pode chamar `exec.python` **se** o pai tem o escopo aprovado, e o subagente vê o mesmo escopo (via herança). A aprovação é por **invocação do subagente**, não por invocação herdada — o subagente dispara seu próprio modal de aprovação, mostrando o mesmo escopo do pai, e o usuário re-aprova. Sem essa re-aprovação, o invariante vira "subagente herda aprovação" — mesma classe de bug que o `subagentBudget` da Fase 6 Etapa 4 PR 1 (esquecido de propagar).

A regra "teste de negação" da Etapa 4 da Fase 7 entrega `crates/e2e/tests/e2e_subagent_approval.rs::subagent_requires_own_approval_even_if_parent_approved` — pai aprova `exec.python` com escopo `OneSession`, subagente tenta a mesma operação, **modal aparece de novo** (não é bypassado pela herança do pai).

## Consequências

- `PermissionSet` ganha 1 campo novo: `extra: HashMap<PermissionField, Vec<ApprovalScope>>` para os escopos concedidos. **Default é vazio** — a primeira concessão é via UI, não via default.
- `ApprovalScope` (existente da Fase 3) ganha 1 variante nova: `OneExecution` (a Fase 3 só tem `OneTurn`/`OneSession`/`OneProject`/`Forever`).
- O `RunExecutor` da Fase 3 ganha 1 hook: `executor.check_approval(tool_call, run) -> Result<Decision, RequireApproval>`. O hook é chamado em **toda invocação de ferramenta com risco > `safe`** (vide `risk_level` do `ToolManifest`, ADR-0026).
- O `ApprovalModal` da UI (Fase 3 Etapa 3) ganha 1 componente: `ApprovalScopeSelector` (radio button com os 5 escopos, default contextual, e o diff do comando que vai rodar).
- O `DbAuditSink` ganha 3 `kind` novos: `'approval_granted'`, `'approval_revoked'`, `'approval_denied'`. Migration `0037_audit_kinds.sql` quando entrar.
- A Etapa 4 (exec tools), Etapa 5 (file ops) e Etapa 6 (exec.shell) implementam o **backend** de D2-D5. A Etapa 7 (UI/Polish) fecha a UI de D6 (revogação).
- O `docs/architecture/tool-permission-model.md` ganha §"Política de aprovação da Fase 7" linkando para este ADR (em vez de descrever a política no próprio spec — o spec descreve o **modelo**, o ADR descreve a **política concreta**).

## Alternativas consideradas

1. **Sempre pedir aprovação em toda invocação** (escopo fixo `OneExecution` para tudo). Rejeitado por UX inviável: o usuário que programa quer rodar `pytest` 50 vezes sem 50 cliques. O caso "instalei uma dep, agora roda os testes" é o coração do Modo Desenvolvedor — quebrar o coração por excesso de cautela é defeito.
2. **Nunca pedir aprovação** (escopo fixo `Forever` por default). Rejeitado por insegurança: é o que o projeto anterior fazia, e o resultado foi o agente rodando `rm -rf` sem pedir (defeito documentado no relatório de adaptação de regras).
3. **Aprovação por risk_level do ToolManifest** (Ferramentas `safe` não pedem, `moderate` pedem em escopo curto, `high` pedem em escopo longo, `critical` exigem texto digitado). Considerado, mas rejeitado por premature optimization: a v1 da Fase 7 tem 3 risk_levels (a Fase 3 Etapa 3 não populou `high` nem `critical` ainda). Quando a matriz de risco for densamente povoada, vira ADR-003X de revisão.
4. **Aprovação por comando, não por ferramenta** (Allowlist de "comandos seguros" no estilo `sudo` do Linux). É a primitiva `TerminalPermission::Allowlist(Vec<String>)` da Etapa 6, e entra como **extensão**, não como substituto. A v1 entra com `RequireApproval` por invocação, e a Allowlist aparece na Etapa 6 como refinamento.
5. **Auto-aprovação por padrão de uso** (se o usuário aprovou 5 vezes seguidas a mesma tool, a 6ª é auto). Rejeitado por (a) o "5 vezes" é heurística, e o projeto anterior morreu por heurística não-auditada, (b) auto-aprovação esconde o que está acontecendo do usuário (degradação silenciosa, memory 2026-08-03), (c) o usuário que confia pode dar escopo `OneSession` explicitamente.

## Pendências

- **Migração de `PermissionSet` da Fase 3** — o `extra: HashMap<...>` é campo novo. A Etapa 1 da Fase 7 (este PR) atualiza o spec; a Etapa 4 (exec tools) implementa e roda a migração. Sem migração, a v1 com campo novo não roda.
- **UI de "comando exato" com diff de `files.write`** — exige o **diff de entrada vs saída** (não só o que vai ser escrito, mas o que será sobrescrito). A Etapa 5 da Fase 7 implementa; depende do `Jail` (já existe) + `files.read` (já existe) para o `before`.
- **Allowlist de comandos para `exec.shell`** (`TerminalPermission::Allowlist`) — Etapa 6 da Fase 7. Lista inicial conservadora: `ls`, `cat`, `head`, `tail`, `grep`, `find`, `wc`, `pwd`, `echo`, `git status`, `git log`, `git diff`, `git show`. Cada um marcado read-only (não escreve em arquivo). Lista **versionada**, editável pelo usuário.
- **Política para `exec.python` quando o `PermissionSet::python == Unrestricted`** (acima de `Sandboxed`) — D1 diz default deny, mas `Unrestricted` é o escape hatch. A Etapa 4 da Fase 7 documenta: `Unrestricted` é opt-in consciente, default `None`, e `Unrestricted` desabilita o sandbox para o filho. UI mostra banner persistente "execução fora de sandbox" quando ativo.
- **Auditoria de "comando exato"** — a invariante de D5 (UI mostra exatamente o que o executor roda) precisa de teste. A Etapa 4 da Fase 7 entrega; até lá, é defeito latente.

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7. Validação pelo user (via `ask_user`): "As ferramentas de escrita são o primeiro salto de risco real. Até hoje só existe `files.read`. `files.write` e `files.edit` destroem dados; a política de aprovação, a barreira de caminho já ligada no PR #25 e o comportamento em sobrescrita precisam ser decididos antes, não durante." A política de "default deny + escopo variável + comando exato + revogação imediata" é o que traduz a regra do `PROMPT MESTRE` §22.3/§22.5 em mecanismo concreto testável.
- **2026-08-14 — nota de fechamento (Etapa 7, `exec.shell`).** 3 divergências entre o que este ADR planejou e o que o código real entregou, registradas aqui em vez de reescrever a decisão original:
  1. **O hook `executor.check_approval` (D2/D3) não existe.** O `RunExecutor::handle_tool_call` sempre chama `validate_tool_call` com `approval: None` — nenhuma decisão de aprovação anterior é cacheada ou reusada, pra **nenhuma** tool (não só `exec.shell`). Consequência prática: `exec.python`/`exec.node` já se comportam como `OneExecution` hoje, mesmo tendo `OneTurn` como escopo "aceito" na spec — o cache de escopo é a peça que falta pra essa diferença aparecer, e ela é roadmap de Fase 8, não desta etapa.
  2. **`TerminalPermission::Allowlist(Vec<String>)` (D3, Alternativa 4) não é o tipo real.** `TerminalMode` (`crates/tool-registry/src/permission.rs`) é um enum **flat** — `None | RequireApproval | Denylist | Allowlist`, sem payload. A allowlist de comandos em si (`SHELL_ALLOWLIST_DEFAULT`) vive em `frederico_security::exec_patterns`, como uma constante independente do `PermissionSet`, não como dado carregado dentro da variante do enum. `exec.shell` aplica essa allowlist **incondicionalmente** (não lê `PermissionSet.terminal` — o `ToolContext` não carrega `PermissionSet`), então o enum `TerminalMode::Allowlist` hoje é só metadado bumpado atomicamente com o catálogo, sem um consumidor em runtime.
  3. **`git status`/`git log`/`git diff`/`git show` (Pendências, item 3) continuam pendentes.** A allowlist v1 reconhece só o **primeiro token** do comando (`ls`, `cat`, `echo`, etc.) — entradas de 2 tokens como `git status` exigiriam um mecanismo diferente (match de prefixo multi-palavra), não implementado nesta etapa.
- **2026-08-16 — nota de reversão (ADR-0037, `exec.shell` fora do catálogo).** A divergência 3 registrada acima ("a allowlist v1 reconhece só o primeiro token") era mais grave do que a nota de 2026-08-14 avaliou. Ela não limita apenas quais comandos são aceitos: ela **anula a allowlist**. Como o comando inteiro é entregue ao `cmd.exe /c`, que interpreta `&`, `&&`, `||` e `|`, validar só o primeiro token significa validar nada — `echo x & <qualquer coisa>` passa. Medido em 2026-08-16 pelo caminho real: `ver` sozinho é recusado, `echo marcador & ver` executa os dois. O [ADR-0037](0037-exec-shell-fora-do-catalogo.md) tira `exec.shell` do catálogo por isso, e a política de aprovação deste ADR passa a valer só para `files.write`, `files.edit`, `exec.python` e `exec.node`. D3 (`exec.shell` sempre `OneExecution`) fica suspenso enquanto a ferramenta estiver fora — volta junto com ela, se voltar.
