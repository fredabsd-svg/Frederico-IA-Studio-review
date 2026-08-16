# Security Policy

> Frederico IA Studio — modelo de segurança do sandbox Windows
> (Fase 7, `em andamento`).
>
> **Última atualização:** 2026-08-16 (ADR-0037 — `exec.shell` saiu
> do catálogo depois que a allowlist de comandos foi medida e se
> mostrou contornável por qualquer separador do `cmd.exe`; a Fase 7
> voltou a `em andamento`).
>
> Antes: 2026-08-14 (Etapa 7 — allowlist de rede carregada de
> perfil TOML em vez de hardcoded vazia; DNS intercept via `netsh`
> tentado e removido depois de verificação provar que não protegia
> nada).

## Resumo

O `frederico-tool-registry` (camada de tools) combina 4 camadas
de isolamento no subsistema `exec.*` (`exec.python`, `exec.node`)
quando roda no Windows:

1. **Path safety (Mandatory Label\Low)** — `SetFileSecurityW`
   no workdir aplica `SYSTEM_MANDATORY_LABEL_ACE_TYPE` no SACL
   com policy `NO_WRITE_UP`. Um processo com
   `TokenIntegrityLevel=Low` (nosso child) **não** consegue
   ler/escrever em paths com label Medium (default do
   filesystem), incluindo o parent do workdir, `%LOCALAPPDATA%`,
   `%APPDATA%`, `%TEMP%`, etc.
2. **Job Object per-invocation** — `KILL_ON_JOB_CLOSE` + limites
   de memória. Quando o `SandboxedProcess` é droppado, o Job
   handle fecha e o Windows mata **toda a árvore** (filho +
   netos + bisnetos). Garante tree-kill confiável mesmo que o
   `TerminateProcess` direto no PID falhe.
3. **Restricted Token** — `CreateRestrictedToken` dropa 6
   privilégios elevados (`SeDebug`, `SeBackup`, `SeRestore`,
   `SeTakeOwnership`, `SeLoadDriver`, `SeShutdown`) e seta
   `TokenIntegrityLevel = Low`. O child não consegue, por
   exemplo, abrir handles de outros processos (debug), ler
   arquivos de backup do sistema, ou carregar drivers.
4. **Env Filter** — allowlist fail-closed (`REQUIRED` +
   `ALLOWED` + `DENIED`). O env block do pai (que pode ter
   secrets em `OPENROUTER_API_KEY`, tokens, etc.) não vaza pro
   child sem filtragem explícita.

As 4 camadas juntas fecham os 3 vetores da Fase 7 — tree-kill
garantido (Job), privilégio escalonado bloqueado (Token), e
path traversal bloqueado (Label). Veja
[`docs/architecture/security-threat-model.md`](docs/architecture/security-threat-model.md)
para o threat model completo.

## O que essa combinação **NÃO** protege

Ser honesto sobre limitações é parte do contrato de segurança.
A Etapa 5+ fechou a **path safety** (write-up), mas **duas
lacunas** permanecem e estão documentadas como pendências
nomeadas (roadmap da Fase 7 → Fase 8). Não escondemos — o que
não está protegido está aqui.

### 1. **Read-up** (child pode ler paths Medium-labeled)

**Sintoma:** um child com `TokenIntegrityLevel=Low` consegue
**ler** paths com `Mandatory Label\Medium` (que é o default do
filesystem). Isso inclui, entre outros:

- `%LOCALAPPDATA%\studio\frederico\ia\data\frederico.db` (banco
  de produção)
- `%APPDATA%` (configurações de outros apps)
- `%USERPROFILE%\Documents` (documentos do user)
- Qualquer arquivo Medium sem label explícita

O Mandatory Label só bloqueia **write-up** (`NO_WRITE_UP`).
Para bloquear **read-up** também seria preciso `NO_READ_UP` na
policy, mas a **causa raiz** é que o child compartilha o token
do user (com SIDs do user, BUILTIN\Users, etc.) — o Windows
faz o DACL check **antes** do Mandatory Label, e o DACL permite
o user ler seus próprios arquivos.

**Por que não fechamos agora:** a solução real é um **SID
restritivo próprio** (random por invocação) adicionado como
`TokenRestrictedSids` no child token, **mais** um DACL custom
no workdir permitindo só esse SID. O child então **só** consegue
acessar o que o DACL permite (o workdir) — nem mesmo o resto
do home do user. Isso é trabalho de uma Etapa futura (roadmap
Fase 8) — mais complexo (geração/gestão de SID próprio,
serialização pra audit sink, ciclo de vida), mas é **o** caminho
que fecha o read-up.

**Risco concreto hoje:** um script Python rodando via
`exec.python` pode fazer `open(r"C:\Users\conta\AppData\Local\studio\frederico\ia\data\frederico.db", "rb")`
e ler o banco (que tem conversas, memórias, etc.). **Write**
é bloqueado — o child não consegue **modificar** o banco (o
que invalidaria a estratégia de "atômico: ou bumpa tudo ou
não bumpa nada"). Mas **read** está aberto.

**Mitigação temporária:** o Etapa 5+ mantém `exec.python` /
`exec.node` **fora** da allowlist default (são bumpados só
quando o usuário ativa explicitamente). O modelo default
não os vê no schema — usuário precisa aprovar primeiro.

### 2. **Rede** (proxy ligado no caminho real, mas com lacunas conhecidas)

**Estado atual (Etapa 6 + 6+1, ADR-0033):** o child **não**
herda mais a rede do host sem filtro. `exec.python`/`exec.node`
sobem um proxy local (`127.0.0.1:<porta efêmera>`) por
invocação de verdade — `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`
são injetados no env filtrado do child. O proxy é
**deny-by-default**: sem o host na allowlist, toda request
HTTP recebe `502 Bad Gateway` e todo `CONNECT` (HTTPS) também.
HTTPS é um tunnel byte-opaco (`CONNECT` puro, sem MITM, sem CA
custom no trust store) — o proxy decide pelo **host do
CONNECT**, nunca vê o path ou o body; o audit trail grava
`path_redacted="<redacted>"` pra HTTPS (o log não promete mais
do que entrega). Toda decisão (allow/deny) é persistida em
`network_audit` via `DbNetworkAuditSink`.

**O que isso NÃO fecha — 3 lacunas nomeadas:**

1. **Bypass por socket direto (inclui DNS exfiltration).**
   `HTTP_PROXY`/`HTTPS_PROXY` são **convenção**, não imposição do
   SO. Um child que chama `socket.socket(AF_INET, SOCK_STREAM)` e
   conecta direto (ignorando as env vars do proxy) **não passa
   pelo proxy** — `urllib`/`requests` do Python respeitam
   `HTTP_PROXY`, mas código que abre socket raw não. Esse
   comportamento está **fixado em teste**, não escondido:
   `crates/e2e/tests/e2e_network_proxy.rs::e2e_network_raw_socket_bypasses_proxy_documented`
   prova que a conexão raw funciona sem passar pelo proxy.
   **DNS é a mesma lacuna, não uma camada separada:** a Etapa 7
   tentou interceptar a resolução de DNS via `netsh interface ip
   set dns name=Loopback source=static address=127.0.0.1`
   (Windows) com um responder real do outro lado
   (`frederico-security::dns_proxy`, removido). Verificação
   manual fim-a-fim (Admin, `nslookup` sem servidor explícito,
   comparando resolução antes/durante/depois do `netsh`) provou
   que o Windows **não** consulta a interface Loopback pra
   resolver DNS de verdade — ele usa os adaptadores de rede reais
   (Ethernet, Wi-Fi). O mecanismo não entregava proteção nenhuma,
   em nenhum cenário (nem como Admin), só exigia privilégio e
   mexia na config de DNS da máquina do usuário à toa —
   `dns_intercept.rs` e `dns_proxy.rs` foram **removidos por
   inteiro** (2026-08-14; regra "capacidade incompleta é
   capacidade indisponível", `docs/status.md`). Um child que
   resolve hostname via `socket.getaddrinfo()` (com ou sem
   conectar via socket raw depois) sempre usa o DNS real do host,
   sem passar pela allowlist. A defesa real pros dois casos é a
   mesma: filtro de rede no nível de processo (Windows Filtering
   Platform / WDAC) — exige driver ou service, deployment
   complexo, roadmap Fase 8+. Ver ADR-0033 §D1 (correção).
2. **HTTP/3 (QUIC) bypassa.** O proxy fala TCP + forward
   HTTP/CONNECT; um client que negocia QUIC (UDP) conecta
   direto ao destino, sem passar pelo proxy.
3. **Allowlist configurável via perfil, mas só 2 dos 3 layers
   (Etapa 7, 2026-08-14).** `PermissionSet.network_allowlist:
   Vec<String>` (campo novo) é carregado do perfil TOML do
   usuário (`~/.config/frederico/profiles/default.toml`) ∩
   projeto (`./.frederico/project.toml`) antes da casca montar
   o `ExecDeps` — a interseção fail-closed dos 2 layers vira o
   `NetworkAllowlist` do proxy. **O layer de assistant não
   entra**: ele precisa de um `assistant_id` que não existe no
   momento do boot do processo (`ExecDeps` é construído uma
   única vez, process-wide — não por conversa/assistant
   escolhido). Sem nenhum perfil configurado, o comportamento
   é o mesmo de antes (deny total). O campo `ChatOrchestratorParts.network_allowlist`
   vestigial citado numa versão anterior deste documento foi
   **removido** (nunca era lido por `build_chat_orchestrator`;
   a allowlist real sempre viajou só via `ExecDeps`).
   `PermissionSet.network: bool` (o gate mestre, separado da
   allowlist) é lido desde 2026-08-14 via
   `frederico_app::composition::effective_network_allowlist_hosts`
   — um perfil com `network: false` zera a allowlist
   incondicionalmente, mesmo que `network_allowlist` tenha hosts
   (bug de fail-open corrigido; antes, só `network_allowlist` era
   consultado e `network: false` era ignorado).

**Risco concreto hoje:** um script que evita `urllib`/`requests`
e abre socket raw ainda alcança qualquer host — combinado com
o §1 (read-up do banco), continua sendo o vetor de
**exfiltração** mais sério do produto, só que agora só pra
código que deliberadamente contorna a convenção do proxy (em
vez de qualquer `requests.get(...)` trivial, como era antes da
Etapa 6).

**Mitigação:** o `PermissionSet` continua com `network: bool`
controlado por invocação na UI de aprovação — a diferença é
que "network: true" agora significa "proxy deny-by-default
ligado", não "rede aberta sem filtro" como antes da Etapa 6.

### 3. **Pipes stdout/stderr sem label** (child pode escrever
   em pipes anônimos Medium)

**Sintoma:** os pipes stdout/stderr do child são criados pelo
`CreatePipe` (no orchestrator), **sem** `Mandatory Label\Low`
no SACL. Decisão documentada no `crates/security/src/jail.rs`:
a tentativa de criar o pipe **com** label (passando
`SECURITY_ATTRIBUTES.lpSecurityDescriptor` com SACL) falha com
`ERROR_PRIVILEGE_NOT_HELD` (0x80070522) — `SeSecurityPrivilege`
é exigido pra criar kernel objects com SACL, e o app roda
sem privilégio elevado (decisão de projeto, instalação
per-user sem UAC).

**Impacto:** child (Low) **consegue** escrever nos pipes
porque eles são anônimos sem label — o Mandatory Label check
só compara contra objetos **com** label (objetos sem label
default = Medium, e o token Low é `<` Medium, então... espera,
Low < Medium, o check deveria bloquear).

**Esclarecimento:** pipes anônimos **sem label** passam o
Mandatory Label check porque o check é "se o objeto tem label
e a integridade do token < label, deny". Sem label no objeto,
não há deny — child escreve livre. Isso é **intencional** no
Windows: o Mandatory Label é uma proteção **opt-in** do owner
do objeto, não default deny. Sem label = sem proteção.

**Risco concreto:** child pode floodar stdout/stderr com dados
que o parent lê — mas o parent é o `frederico-tool-registry`,
que confia no child (é o `exec.python` que o próprio parent
invocou). O risco é **negativo** — não é uma escalada, é só
"o child consegue se comunicar de volta". Aceitável.

**Por que não fechamos agora:** exigiria `SeSecurityPrivilege`
no processo parent, que viola a decisão de projeto de rodar
sem UAC. Roadmap: em algum momento futuro, o instalador pode
criar um service Windows que aplica os labels (roda como
SYSTEM com SeSecurityPrivilege); o app user-mode chama o
service via RPC. Complexo, Fases 8+.

### 4. **Não existe execução de shell — `exec.shell` saiu do
   catálogo (ADR-0037)**

Entre 2026-08-14 e 2026-08-16, `exec.shell` esteve no catálogo e
esta seção descrevia a denylist/allowlist de comandos como defesa
em profundidade. **A allowlist foi medida e não é uma barreira.**

`frederico_security::exec_patterns::is_allowed` valida só o
**primeiro token** do comando, e o `build_args` entregava o
command string **inteiro** pro `cmd.exe /c` — que interpreta
`&`, `&&`, `||` e `|` como separadores. Medição pelo caminho real
da ferramenta:

| Comando | Resultado |
|---|---|
| `ver` | recusado pela allowlist |
| `echo marcador & ver` | **executou os dois** |

Ou seja: qualquer comando arbitrário passava atrás de um `echo`.
Somado a isso, 7 dos 9 binários da allowlist (`ls`, `cat`,
`head`, `tail`, `grep`, `wc`, `pwd`) vêm do MSYS2 e morrem sob o
rótulo de integridade baixa com `NtCreateDirectoryObject ...
0xC0000022` — a allowlist não impedia o que devia impedir e não
permitia quase nada do que prometia.

Pela regra **capacidade incompleta é capacidade indisponível** (a
mesma que tirou `exec.python`/`exec.node` do catálogo na Etapa 5+
e deletou o `dns_intercept` na Etapa 6), a ferramenta saiu. O
código continua em `crates/tool-registry/src/exec/shell.rs`, sem
ser registrado; os 3 requisitos pra voltar estão no
[ADR-0037](docs/decisions/0037-exec-shell-fora-do-catalogo.md)
§D5. A ausência é fixada em teste
(`crates/e2e/tests/e2e_exec_shell_out_of_catalog.rs::exec_shell_is_not_in_default_catalog`).

**Consequência prática:** hoje o produto não executa comandos de
terminal. Execução arbitrária existe só via `exec.python` e
`exec.node`, que rodam sob as 4 camadas acima e exigem aprovação
do usuário a cada invocação.

## Como reportar vulnerabilidades

Achou um bug de segurança? Abra uma issue com o label
`security` (ou envie email direto se preferir —
[Mavis@mavis.local] — **NÃO** poste detalhes em issue
pública antes do fix estar pronto). O Frederico IA Studio
trata vulnerabilidades como **P0** até análise.

## Auditoria

Toda decisão de segurança fica registrada em
[`docs/decisions/`](docs/decisions/) (ADRs) e referenciada
do código. As ADRs da Fase 7 relevantes:

- **ADR-0031** — Isolation model (Windows Job + Restricted Token
  + Env Filter)
- **ADR-0033** — Política de rede do sandbox (deny-by-default,
  proxy local, `CONNECT` sem MITM, log visível)
- **ADR-0036** — `SecurityJailResolver` Windows (4 primitivas
  do sandbox)
- **ADR-0007** — Fronteira Win32 (apenas módulo `windows` tem
  `unsafe_code = "deny"`; resto é `forbid`)

Cobertura E2E em `crates/e2e/tests/`:

- `e2e_exec_python_under_sandbox.rs` (3 tests: path safety,
  wall-clock, hello world)
- `e2e_exec_node_under_sandbox.rs` (espelho Node)
- `e2e_network_proxy.rs` (7 tests: allow/deny por allowlist,
  tunnel CONNECT, bypass por socket raw documentado, audit
  trail)
- `e2e_network_proxy_wired_into_exec_python.rs` /
  `e2e_network_proxy_wired_into_exec_node.rs` (prova do meio —
  `HTTP_PROXY` funcionando de verdade dentro do child — antes
  de aceitar a prova do fim; `DbNetworkAuditSink` persistindo
  `run_id` correto)
- `tree_kill.rs` (Job Object)
- `jobs_test.rs` (Restricted Token)
- `env_filter.rs` (Env Filter)

Toda mudança na Etapa 5+ foi precedida de **TDD**: os 3 tests
acima foram **vistos falhando** (TDD passo 1) antes de
implementar a fix. Ver [Fase 7 Etapa 5+](docs/releases/fase-7/README.md).
