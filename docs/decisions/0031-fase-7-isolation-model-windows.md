# 0031 — Modelo de isolamento do sandbox Windows da Fase 7

## Contexto

A Fase 7 do PROMPT MESTRE introduz a execução de ferramentas perigosas (`exec.python`, `exec.node`, `exec.shell`, `files.write`, `files.edit`) no caminho de produção. Até a Fase 6, o `RunExecutor` só chamava `files.read` — operação de leitura, idempotente, sem efeito colateral. A Fase 7 é o primeiro salto em que o agente **mexe no filesystem do usuário** e **executa processos arbitrários**, com credenciais no `PATH` do processo pai, com acesso à rede do host.

O `PROMPT MESTRE` §22 fixa a restrição de execução: **sem Docker, sem WSL, sem alteração de `PATH` global**, primitivas do Windows. O stub `docs/architecture/windows-sandbox-design.md` lista três primitivas candidatas e deixa a escolha em aberto: "AppContainer vs. Restricted Token vs. Job Object — decidir com base no tipo de execução".

A escolha tem consequência estrutural. Cada primitiva implica um mecanismo distinto de spawn, de acesso a arquivo e de rede:

- **AppContainer** (`CreateAppContainerProfile` + `ATTRIBUTE_APP_CONTAINER`) — isola filesystem e rede porCapability. Mais restritivo, mas **quebra rotinas comuns de Python/Node** que esperam acesso a `%APPDATA%`, `%LOCALAPPDATA%`, `temp` com permissões amplas. Rede precisa de `networkLoopbackCapability` explícita por perfil.
- **Restricted Token** (`CreateRestrictedToken` com `SAFER_LEVEL::Disallowed`) — descarta privilégios elevados (SeDebug, SeBackup, SeRestore, SeTakeOwnership, SeLoadDriver, SeShutdown). Compatível com a maioria dos runtimes, mas **não isola filesystem nem rede** — o processo lê/escreve onde o usuário dono consegue.
- **Job Object** (`CreateJobObject` + `SetInformationJobObject` com `JOB_OBJECT_LIMIT_*` + `AssignProcessToJobObject`) — impõe limites de CPU, memória, processos filhos, **e garante tree-kill** quando o handle do Job é fechado (mesmo em `kill -9` do pai). Não isola filesystem, não isola rede, não descarta privilégios.

A decisão errada aqui é cara: trocar a primitiva depois da Etapa 2 (sandbox primitives) significa **reescrever o executor inteiro** (o spawn passa a ser diferente, o handle de kill-tree passa a ser diferente, a captura de env passa a ser diferente). O retrofit do ADR-0022 (criar `frederico-app::build_chat_orchestrator` depois da Etapa 2.B já ter hardcoded o `WorkerHandle`) mostrou que **decisão estrutural tardia é refazer, não refatorar**.

A regra de honestidade do `SECURITY.md` (REGRA 1.1) também pesa: o modelo de ameaça precisa dizer **o que o sandbox escolhido não protege**. Isolamento local sem contêiner é, por natureza, mais fraco que `runc`/`runsc`/WSL. A documentação não pode sugerir garantia que o mecanismo não dá.

## Decisões

### D1 — Três camadas combinadas, **por tipo de execução**

O sandbox da Fase 7 **não é uma primitiva única**: é uma combinação. Cada ferramenta executa sob a combinação adequada ao risco dela:

| Ferramenta | Jail (path) | Job Object (resource + tree-kill) | Restricted Token (privilege drop) | AppContainer (fs+net isolation) | Env zeroed + allowlist |
|---|---|---|---|---|---|
| `files.read` | sim | — | — | — | — |
| `files.list` | sim | — | — | — | — |
| `files.write` / `files.edit` | sim | — | — | — | — |
| `exec.python` / `exec.node` | sim (workspace only) | sim (CPU, mem, processos, wall_clock) | sim (Disallowed) | **não (Fase 8+)** | sim |
| `web.fetch` / `web.search` | — | — | — | — | — (passa pelo proxy local, ADR-0033) |

`exec.shell` foi tentado na Etapa 7 (2026-08-14) e descartado — não está no catálogo, sem linha nesta tabela (ver `docs/decisions/0034-fase-7-write-exec-approval-policy.md` §"Histórico de revisão").

**AppContainer é deliberadamente adiado** (decisão D6 abaixo). Restricted Token é a camada que descarta privilégios no Windows sem a quebra de compatibilidade de AppContainer. Job Object é a única primitiva de tree-kill confiável — sem ela, um processo filho do sandbox sobrevive ao `kill -9` do app.

A coluna "Env zeroed" é **obrigatória para todo exec** (cumprir `I1` do `security-threat-model.md` — "Sandbox herda env do processo pai" — a única ameaça da Fase 6 com teste de regressão que ainda depende de Fase 7).

### D2 — `Jail` é a barreira primária de path safety, sempre

A combinação da Fase 6 Etapa 5.X (PR #25) deixou a barreira de path safety no `Jail` do `frederico-tool-registry` (`crates/tool-registry/src/workspace.rs`). Ela é **a primeira barreira**, não a última. As primitivas do sandbox complementam, não substituem.

Concretamente: a Fase 7 adiciona `files.write` / `files.edit` ao `ToolRegistry`. O `Jail` continua sendo invocado no `execute()` da ferramenta, **antes** de qualquer I/O. Se o `Jail` rejeita, o `Job Object` nem é instanciado — o processo filho não existe.

A regra é simétrica: `files.write` aceita `path` resolvido pelo `Jail`; recusa `..`/absoluto/UNC/letra de unidade/symlink (mesma matriz da Fase 6 Etapa 5.X). O ADR-0035 detalha a semântica de sobrescrita (atomic write + backup opcional).

### D3 — `Job Object` é a única primitiva de tree-kill

A Fase 5 da Fase de Ligação (carry-over) deixou pendência explícita em `docs/architecture/process-architecture.md`: **`SecurityJailResolver` em `crates/security/src/jail.rs` com Job Objects** para garantir kill-tree quando o parent morre, mesmo em `kill -9`. Esse é o ADR-0036 desta Etapa 1, e a Etapa 2 da Fase 7 implementa.

Mecanismo concreto:

1. App cria o `Job Object` antes do spawn, com `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` + `JOB_OBJECT_LIMIT_BREAKAWAY_OK` (este último permite que o filho crie netos sob o mesmo Job — necessário para `subprocess.Popen` em Python).
2. App faz `AssignProcessToJobObject` no handle do filho **antes** de retornar do `CreateProcess` (a janela é curta mas existe — o child precisa estar atribuído antes de o pai perder o controle; sem isso, o child pode escapar via race condition).
3. Quando o app morre (qualquer causa, inclusive `kill -9` do Windows), o OS fecha o handle do Job → `KILL_ON_JOB_CLOSE` derruba a árvore inteira.

**Teste de regressão obrigatório** (regra do usuário: "sandbox só se prova impedindo"): `cargo test --workspace` ganha um teste em `crates/security/tests/tree_kill.rs::child_survives_parent_kill9` que spawna um filho que cria um neto, mata o pai com `TerminateProcess` (equivalente a `kill -9` no Windows), e afirma que **ambos** (pai e neto) estão mortos em < 1s. Sem Job Object, o neto sobrevive — esse é o teste que prova que a camada funciona.

### D4 — `Restricted Token` descarta privilégios, não isola filesystem

A primitiva `SaferCreateLevel(SaferLevel::Disallowed, ...)` + `CreateRestrictedToken` produz um token que, herdado pelo filho, **não tem os 6 privilégios sensíveis** (SeDebug, SeBackup, SeRestore, SeTakeOwnership, SeLoadDriver, SeShutdown). Isso impede que um filho que conseguiu escalar privilégios via exploit de Python/Node faça coisas como ler `SAM` (SeBackup), ou instalar driver (SeLoadDriver).

**Não isola filesystem.** O filho lê/escreve onde o usuário dono consegue — daí a regra D2 (Jail como barreira primária) e D1 (Restricted Token combinada com Jail, não no lugar dele).

**Não isola rede.** O filho usa a rede do host — daí a regra do ADR-0033: rede do sandbox passa **sempre** por proxy local com allowlist.

Compatibilidade: Restricted Token **é compatível com Python e Node**, ao contrário de AppContainer. Validar no `crates/security/tests/restricted_token.rs::python_runs_under_restricted_token` — spawna `python.exe -c "print(2+2)"` sob Restricted Token, afirma que sai `4` (não falha com access denied). Sem isso, Restricted Token está desligado de fato.

### D5 — Env zerado e reconstruído por allowlist (executa `I1`)

`I1` do `security-threat-model.md` (Sandbox herda env do processo pai) é a única ameaça com teste de regressão na Fase 6 que **ainda depende** da Fase 7. Esta é a entrega que fecha I1.

Mecanismo:

1. App lê o `env::vars()` do processo pai em uma `Vec<(String, String)>`.
2. Aplica `ALLOWED_ENV_VARS` (allowlist versionada, `Vec<String>` no `frederico-security::config::EnvAllowlist`) — nomes como `PATH` (para o filho encontrar binários do runtime portátil), `TEMP`/`TMP` (scratch), `LANG`/`LC_ALL` (locale), `PYTHONHOME`/`NODE_PATH` (apontando pro runtime portátil), `HOME`/`USERPROFILE` (alguns runtimes precisam). **Não inclui** `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GITHUB_TOKEN`, `*_TOKEN`, `*_SECRET`, `*_KEY`.
3. Constrói `Vec<(String, String)>` final só com os allowlisted. Sobrescreve valores sensíveis com string vazia **antes** de remover (defesa contra `getenv` que vê o valor antigo via cache de libc).
4. Passa pro `CreateProcess` via `lpEnvironment` (não `lpCommandLine` — separação de args e env).
5. Auditoria: o `DbAuditSink` registra `env_vars_count_after_filter` por execução (quantas vars passaram), e o teste de regressão em `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets` injeta `OPENAI_API_KEY=test-secret-XXX` no env do app, spawna `python.exe -c "import os; print(os.environ.get('OPENAI_API_KEY', 'EMPTY'))"` e afirma que imprime `EMPTY`.

### D6 — `AppContainer` é adiado para Fase 8+

A primitiva mais forte de isolamento no Windows **quebra rotinas comuns** de Python (que escreve em `%APPDATA%`/`%LOCALAPPDATA%` esperando permissões amplas) e exige configuração explícita de Capability por perfil (loopback, internet, etc.). Em v1, o custo de fazer funcionar para `pip install` + `venv` + execução geral de scripts é desproporcional ao ganho (Restricted Token + Jail + Job Object + proxy de rede já cobrem as ameaças da Fase 7).

AppContainer entra na **Fase 8 (Modo Desenvolvedor estendido)** como camada opcional para execuções que o usuário marca como "alto risco" (ex.: rodar script não-confiável de terceiros). A Fase 8 ganha ADR próprio quando entrar.

### D7 — Sandbox é **opt-in por feature flag**, não silencioso

A primeira versão (`Etapa 2` da Fase 7) implementa o sandbox com **feature flag `FREDERICO_SANDBOX_V1`** (env var) que **default é ON** mas pode ser desligado para debugging. A feature flag é removida na Etapa 7 (Fase 7 concluída) — até lá, o sandbox pode ser desligado para investigar regressões sem desligar o caminho de produção.

A regra é: durante a Etapa 2-6, o sandbox é exercitado em todo PR (regressão obrigatória). Investigar uma falha do CI **desligando o sandbox temporariamente** é permitido (com log explícito) mas vira pendência na próxima etapa. A Etapa 7 fecha com o flag removido.

## Consequências

- `crates/security/` ganha `windows.rs` (Restricted Token + Job Object + env filtering) e `linux.rs` (stub retornando `Err("not supported")` para a v1 — Linux é roadmap, não v1, conforme `windows-sandbox-design.md` §"Não-objetivos").
- `crates/tool-registry/src/workspace.rs` (Jail) é **inalterado** — a Fase 6 Etapa 5.X já fixou a barreira primária.
- O `RunExecutor` da Fase 3 ganha dois hooks novos: `executor.spawn(under_sandbox: SandboxConfig, ...)` e `executor.terminate(sandbox_handle: JobHandle)`. A interface é simétrica em todas as plataformas: `linux` retorna `SandboxUnsupported` (degradação declarada, mesma regra do Etapa 2.A da Fase 5).
- O `PermissionSet` da Fase 3 Etapa 3 ganha 1 campo novo: `sandbox: SandboxLevel { None | Soft | Strict }`, com `Soft = Job Object + Restricted Token + env zeroed` e `Strict = Soft + AppContainer (Fase 8+)`. `None` = execução fora de sandbox (apenas para ferramentas explicitamente seguras como `files.read`).
- O `security-threat-model.md` ganha §"Sandbox: o que protege e o que NÃO protege" — a parte "NÃO protege" é o coração da honestidade (ver ADR-0032 e atualização separada de `security-threat-model.md` nesta Etapa 1).
- A regra de "teste de negação" (do prompt do user) vira parte do `testing-strategy.md` §"Fronteira do que os E2E cobrem" — toda etapa da Fase 2 em diante entrega **pelo menos um teste que prova o que o sandbox bloqueia**, não o que ele permite.

## Alternativas consideradas

1. **AppContainer único (Strongest Isolation)**. Rejeitado pela quebra de compatibilidade com Python/Node. `pip install` em AppContainer exige Capability `registryRead`, `userProfile`, e mesmo assim falha em vários setups (WindowsApps Python, virtualenv com symlinks). Custo de fazer funcionar é desproporcional ao ganho quando Restricted Token + Jail + Job Object + proxy cobrem as ameaças documentadas.
2. **Restricted Token único**. Rejeitado: não tem tree-kill, não tem limit de recurso, e um filho que conseguiu escalar privilégios via exploit pode usar SeDebug para inspecionar o processo pai. Job Object + Restricted Token combinados é o mínimo que cobre o conjunto de ameaças.
3. **Job Object único**. Rejeitado: não descarta privilégios. Um filho com SeDebug consegue ler a memória do pai — acesso a `OPENAI_API_KEY` na env do pai (que está zerada, mas pode estar em cache de DLL, em TLS, ou em estrutura de dados do adapter).
4. **Conta de usuário separada (per-execution)**. Rejeitado por custo: criar/destruir perfil de usuário Windows é caro (>1s), exige privilégio admin, e o cache de token/profile é instável. A v1 do Windows sandbox usa Restricted Token no mesmo usuário, com privilégios descartados. Conta separada é roadmap de Fase 8+ se a auditoria de segurança mostrar que Restricted Token é insuficiente.
5. **Container Linux via WSL** (`PROMPT MESTRE` §5.2 proíbe). Rejeitado pela regra dura do prompt mestre: WSL é dependência de instalação do usuário, viola §5.2. Fora de escopo.
6. **Docker Desktop** (`PROMPT MESTRE` §5.2 proíbe). Rejeitado pela mesma regra.
7. **Nada (degradação declarada)**. Rejeitado porque o salto de risco da Fase 7 (escrita em arquivo + execução de processo) é qualitativamente diferente da Fase 6 (só leitura). "Degradação declarada" é o caminho quando a primitiva **tentada** não está disponível (Etapa 2.A Fase 5); aqui, o caminho padrão precisa de sandbox para ser seguro. A degradação só vale para `linux` (não-objetivo de v1) — não para Windows, que é a única plataforma suportada.

## Pendências

- **Teste de regressão de I1 (env não vaza)** é o mesmo da Fase 6: `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets`. A Etapa 2 da Fase 7 implementa; a Etapa 7 fecha com o teste verde.
- **Mecanismo de "env override" para casos legítimos** (ex.: usuário quer passar `MY_CUSTOM_VAR` pro filho) ainda em aberto. Padrão atual: o usuário define a var no nível de configuração do app (`crates/security::config::EnvAllowlist`), não por execução. Próxima iteração pode ser allowlist por execução via `PermissionSet::extra_env: Vec<String>`.
- **Integração com `subagent_runner` da Fase 6 Etapa 4** — subagente herda permissões do pai. O sandbox do subagente é o mesmo do pai (D2 do ADR-0027), mas a Etapa 4 PR 2 não ganhou ainda o hook de sandbox. Etapa 2 da Fase 7 pluga.
- **UI de "sandbox ativo"** — a Etapa 7 (UI/Polish) precisa mostrar um indicador discreto no rodapé do chat quando uma execução está sob sandbox. Não é toggle (decisão do usuário já tomada no `PermissionSet`).
- **Auditoria de tree-kill** — o `JobHandle` é registrado no `DbAuditSink` em `crates/security::audit`, junto com o `parent_run_id`. Permite investigar incidentes pós-mortem.

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7. Validação pelo user (via `ask_user`): "o modelo de isolamento é essa decisão. Sem Docker, no Windows, 'sandbox' pode significar coisas muito diferentes. Cada uma implica um mecanismo distinto de spawn, de acesso a arquivo e de rede, e trocar depois não é refatoração: é reescrever o executor. Isso tem que sair como ADR antes de qualquer linha." A escolha das 3 camadas (Jail + Job Object + Restricted Token) com AppContainer adiado para Fase 8+ é o que destrava a fase sem fechar o caminho para endurecimento posterior.
