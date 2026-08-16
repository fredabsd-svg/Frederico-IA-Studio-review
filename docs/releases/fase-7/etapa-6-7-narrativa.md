# Etapas 6, 6+1 e 7 da Fase 7: rede do sandbox, o wiring que não estava wireado, e o mecanismo que foi removido

<!--
Estado: implementado
Verificado contra o código em: 2026-08-16
Fase correspondente: 7 (Etapas 6, 6+1 e 7 — fecham a fase)
-->

Narrativa técnica das três últimas etapas da Fase 7 — PR #51
(`2fbaf73`, 2026-08-13) e PR #52 (`e535b7f`, 2026-08-14). Não
duplica o `CHANGELOG.md`, que registra o efeito pro usuário
(§1.7). O que mora aqui é o percurso: o que se descobriu no
caminho, e o que foi desfeito.

As três etapas têm um fio comum, e vale dizê-lo antes do detalhe:
**duas vezes nestas etapas um mecanismo que parecia funcionar
não funcionava**, e nas duas o que revelou isso foi verificação
de ponta a ponta, não leitura de código. A primeira virou 4
correções; a segunda virou uma remoção.

## Etapa 6 — o proxy (PR #51)

O mecanismo em si foi direto. `frederico-security::network`
sobe um `TcpListener` Tokio em `127.0.0.1:0`, faz forward de
HTTP via `reqwest` e túnel byte-opaco para HTTPS via `CONNECT`,
com `NetworkAllowlist` casando por sufixo literal
(`pypi.org` casa `files.pypi.org`, não casa
`pypi.org.attacker.com`). Toda decisão vira uma linha
append-only em `network_audit` (migration `0031`, 3 índices).

Duas decisões de forma merecem registro:

**HTTPS sem MITM.** O proxy decide pelo nome do host do
`CONNECT`, antes do TLS, e depois só repassa bytes. Fazer MITM
exigiria instalar uma CA custom no trust store do Windows —
ou seja, passar a ver todo o tráfego do usuário em claro para
poder auditá-lo. O preço aceito é que o log vê **host**, nunca
**path**, em HTTPS. O campo no audit é literalmente
`path_redacted = "<redacted>"`: o log não promete mais do que
entrega.

**O bypass entrou como teste, não como nota de rodapé.**
`e2e_network_raw_socket_bypasses_proxy_documented` afirma que
um `TcpStream::connect` cru, sem `HTTP_PROXY`, conecta direto.
`HTTP_PROXY` é convenção que bibliotecas escolhem respeitar, não
imposição do sistema — e um teste que fixa a limitação vale
tanto quanto um que prova a proteção, porque impede que alguém
mais tarde leia "deny by default" e conclua mais do que está
escrito.

## Etapa 6+1 — o wiring (mesmo PR #51)

A Etapa 6 entregou o proxy testado isoladamente. **Nada no
caminho de produção o usava.** A Etapa 6+1 ligou
`exec.python`/`exec.node` nele: `start_network_proxy` sobe o
proxy por invocação, escreve `<workdir>/.frederico/proxy.port`,
e injeta `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` no
`SandboxConfig::extra_env` através de um `NetworkProxyGuard`
RAII, cujo drop derruba o proxy — sempre **depois** do
`collect_output`, senão o filho perde a saída de rede no meio da
execução.

O que devia ser um wiring de meia hora destravou **4 causas-raiz
empilhadas**, cada uma escondendo a próxima:

1. **`CREATE_UNICODE_ENVIRONMENT` faltava** no
   `CreateProcessAsUserW`. O env block UTF-16 que o código
   construía era lido como ANSI, e qualquer env block não-trivial
   falhava com `ERROR_INVALID_PARAMETER` (87).

2. **Um fallback silencioso reexecutava com `lpEnvironment =
   None`** quando (1) acontecia. O filho passava a herdar o
   ambiente **inteiro** do processo pai — credenciais incluídas —
   anulando o `EnvFilter` e a ameaça I1 do threat model, sem
   nenhum erro visível pro caller. Esta era uma regressão de
   segurança viva em produção, e ninguém teria motivo pra
   suspeitar: o teste passava, o processo rodava, a saída estava
   certa. Removido. Falha na construção do env controlado agora é
   erro duro que propaga.

3. **`SystemRoot`/`windir` fora do `EnvAllowlist::REQUIRED`.**
   Com o env block mínimo finalmente chegando ao filho, **todo**
   `socket.socket()` passou a falhar com `WSAEPROVIDERFAILEDINIT`
   (10106) — sem `SystemRoot`, o `WSAStartup` não resolve
   `%SystemRoot%\system32\mswsock.dll`. O sintoma era
   indistinguível de "o proxy está bloqueando", que é exatamente
   o resultado que se esperava ver. Antes de achar a causa foram
   descartados integrity level, restricted SIDs e privilege drop.

4. **`DbNetworkAuditSink` gravava o `run_id` errado** — o do
   momento em que o sink foi construído, não o da entry — e
   recebia o `Display` de `RunId` (`"RunId(<uuid>)"`, formato de
   log) em vez do uuid puro, então o `Uuid::parse_str` do outro
   lado falhava em silêncio.

### As duas lições que ficam

**Prova do fim não basta; é preciso prova do meio.** Os testes
originais de `exec.python` passavam pelo motivo errado: usavam um
host inexistente, então a falha de DNS era lida como "o proxy
bloqueou", e o assert aceitava qualquer erro de rede genérico.
Os testes foram reescritos para exigir primeiro que
`HTTP_PROXY` e `socket.socket()` funcionem de verdade no filho,
e só então aceitar o 502 do proxy contra um host que resolve. Um
teste que aceita o resultado certo por qualquer caminho não
prova o caminho.

**O ambiente do shell mascarou a causa.** A causa-raiz 3 ficou
invisível durante o diagnóstico porque o Git Bash normaliza
`SYSTEMROOT`/`WINDIR` para maiúsculas. Rodar a suíte no
PowerShell nativo foi o que expôs o problema. Desde então o
pré-flight desta fase roda em PowerShell.

Fecha com um teste que planta uma credencial falsa no ambiente
**real** do processo de teste e afirma sua ausência total no
filho (`crates/security/tests/env_credential_not_leaked.rs`) —
validado reintroduzindo `env=None` de propósito pra confirmar que
o teste pega a regressão.

## Etapa 7 — `exec.shell` (PR #52)

Mesmo padrão de sandbox das outras `exec.*`, com duas diferenças
de política: `risk_level: Critical` e denylist + allowlist de
comandos **sempre ativas**, checadas antes de qualquer spawn.

A honestidade sobre o alcance delas está no código e nos testes:
o match é substring literal, não um parser de shell, então
`rm -r -f` não casa `rm -rf`. Isso está fixado em
`denylist_hit_documents_split_flag_bypass`. A barreira real
contra dano ao host continua sendo o Jail (Mandatory Label\Low)
mais o Restricted Token; a denylist é defesa em profundidade
contra o caso comum, e é descrita como tal.

**Achado ao investigar o escopo de aprovação:** o cache de
aprovação por escopo (`OneTurn`/`OneSession` reusando uma decisão
anterior) **não existe em nenhuma linha de código**.
`RunExecutor::handle_tool_call` sempre chama `validate_tool_call`
com `approval: None`. A consequência é dupla e vale registrar
inteira: a garantia que `exec.shell` queria (`OneExecution`
sempre) não precisou de código nenhum, porque já é o
comportamento de toda tool com `requires_user_approval`; e
`exec.python`/`exec.node` **nunca** tiveram o `OneTurn` que a
spec deles descrevia. A spec descrevia intenção. O ADR-0034
ganhou nota de fechamento com as 3 divergências entre plano e
código.

No mesmo PR, a allowlist de rede deixou de ser hardcoded vazia e
passou a vir do perfil TOML do usuário ∩ projeto — e a
interseção expôs um **bug de fail-open**: a casca extraía só
`.network_allowlist` do `PermissionSet` mergeado e descartava
`.network`, o gate mestre. Um perfil com `network: false` e
allowlist não-vazia liberava os hosts assim mesmo. Fechado por
uma função pura (`effective_network_allowlist_hosts`) que zera a
allowlist incondicionalmente quando `network` é falso, com teste
de regressão.

## O DNS intercept, e por que ele não existe mais

Era a 3ª das pendências de rede. O plano do ADR-0033 previa
apontar o DNS da máquina pra um responder local
(`netsh interface ip set dns name=Loopback source=static
address=127.0.0.1`), fechando a exfiltração via `getaddrinfo`
direto, que o proxy não vê.

Foi construído: `frederico-security::dns_proxy`, um responder DNS
mínimo (RFC 1035 §4.1, `QTYPE=A`) sobre `UdpSocket`, wireado no
`start_network_proxy` junto com o `set_dns_intercept`.

E foi verificado à mão, com privilégio elevado real via UAC,
comparando a resolução de um host fora da allowlist antes,
durante e depois do `netsh`. O `netsh` confirmava a configuração
aplicada — `Servidores DNS Configurados Estaticamente:
127.0.0.1`. A resolução real continuava idêntica nos três
momentos, sempre pelo DNS do adaptador físico. Nenhum
`NXDOMAIN`, nenhuma consulta chegando no responder. **O Windows
não consulta a interface Loopback pra resolver DNS.** Reproduzido
duas vezes.

Ou seja: o mecanismo não protegia em nenhum cenário, nem como
Admin. O que ele fazia era exigir privilégio elevado e alterar a
configuração de rede da máquina do usuário à toa.

`dns_intercept.rs` e `dns_proxy.rs` foram **removidos por
inteiro** — não desativados, não deixados atrás de flag, não
marcados como "parcial". É a mesma regra que a Etapa 5+ já tinha
aplicado quando tirou `exec.python`/`exec.node` do catálogo até a
path safety existir de verdade: **capacidade incompleta é
capacidade indisponível**.

A consequência foi escrita onde dói: "deny by default" (ADR-0033
§D1) passa a valer **só** para código que respeita
`HTTP_PROXY`/`HTTPS_PROXY`. DNS exfiltration continua lacuna sem
mitigação, na mesma família do bypass por socket raw. Fechar de
verdade exige filtro no nível de processo (WFP/WDAC) — roadmap
Fase 8+.

Vale a distinção: isto **não é uma regressão**. O mecanismo nunca
funcionou; o que mudou é que agora alguém verificou. A tentação,
num caso desses, é manter o código — ele custou trabalho, e
remover parece admitir desperdício. Mas um mecanismo de segurança
que não protege é pior que a ausência dele, porque o
`SECURITY.md` teria continuado a listá-lo e alguém teria
continuado a contar com ele.

## O que fica registrado da fase

- Segurança não se revisa lendo código: **as duas falhas graves
  destas etapas — a herança silenciosa de ambiente e o DNS que
  nunca interceptou — passariam por qualquer revisão de
  diff.** Foram verificação fim-a-fim e privilégio real que as
  acharam.
- Teste que aceita o resultado esperado por qualquer caminho não
  prova o caminho. Prova do meio antes da prova do fim.
- Spec descreve intenção até que alguém confira. O `OneTurn` de
  `exec.python` viveu semanas numa spec sem existir no código.
- Remover é uma entrega. Duas vezes nesta fase (Etapa 5+ e Etapa
  7) o trabalho certo foi tirar capacidade do produto.

## Referências

- [ADR-0033](../../decisions/0033-sandbox-network-policy.md) — política de rede (§D1 corrigido, §D5 vira proposta não implementada, §D7 fechado).
- [ADR-0034](../../decisions/0034-fase-7-write-exec-approval-policy.md) — política de aprovação, com a nota de fechamento das 3 divergências.
- [`SECURITY.md`](../../../SECURITY.md) §"Rede" e §"`exec.shell` — denylist/allowlist são defesa em profundidade".
- [`security-threat-model.md`](../../architecture/security-threat-model.md) §"O que o sandbox NÃO protege".
- [`exec-tools-specification.md`](../../architecture/exec-tools-specification.md) — `exec.shell` como implementado.
- [`README.md`](./README.md) desta pasta — índice da fase e tabela das 7 lacunas nomeadas.
