<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-10
Fase correspondente: 1-3, 7
-->

# Modelo de Ameaça de Segurança

Análise STRIDE enxuta dos principais componentes. Para cada ameaça, referência à contramedida e ao teste que a prova. Decisões específicas de segurança (ex: qual provider de credencial) vivem em ADRs futuros.

## Atores

- **Usuário** — humano dono da máquina. Confiável, mas pode ser enganado por prompt injection via documento anexo.
- **Modelo de IA** — terceiro parcialmente confiável. Pode alucinar, ser enganado, ou ter comportamento adversarial via conteúdo recuperado.
- **Subagente** — menor privilégio que o agente pai; mesma classe de risco.
- **Worker** — software confiável, mas roda com permissões amplas; precisa de isolamento.
- **Rede externa** — hostil. Internet é zona não confiável.

## Ativos

- Credenciais de API (provedores, GitHub, etc.)
- Dados pessoais do usuário (LGPD)
- Arquivos do workspace
- Banco SQLite (histórico, memórias, execuções, checkpoints)
- Tokens de sessão / curta duração
- Documentos gerados (PDFs, planilhas)
- Memória semântica (vetores)
- Logs e diagnóstico
- Pacotes binários dos workers (integridade da distribuição)

## Ameaças (top, não exaustivo)

| ID | Categoria | Ameaça | Contramedida | Teste (referência) |
|---|---|---|---|---|
| S1 | Spoofing | Worker falso se passando por worker legítimo | Assinatura digital do binário + token de curta duração no IPC | Inicia worker adulterado → handshake falha |
| S2 | Spoofing | Tool manifest adulterado no banco | Hash do manifesto verificado no boot; alteração fora de migração numerada é defeito | Editar manifesto em SQLite → app recusa subir |
| T1 | Tampering | Credencial vaza em log | Filtro de logging + varredura automatizada em CI | Forçar print de credencial em código de teste → teste falha |
| T2 | Tampering | Tool manifest divergente da realidade do worker | Handshake com schema versionado; worker reenvia manifesto a cada boot | Mutar manifesto em memória no worker → reconciliação detecta |
| T3 | Tampering | Checkpoint adulterado para "completar" uma execução | Hash do checkpoint assinado; leitura verifica antes de restaurar | Modificar byte em checkpoint → app recusa restaurar |
| R1 | Repudiation | Execução sem log de auditoria | Toda execução e tool_call emite evento de auditoria com timestamp e ator | Encerrar app mid-tool_call → log contém a invocação |
| I1 | Information Disclosure | Sandbox herda env do processo pai | Env é zerado e reconstruído por allowlist (`PROMPT MESTRE` §22.5) | Injetar `OPENAI_API_KEY` no env do sandbox → filho não vê |
| I2 | Information Disclosure | URL maliciosa via tool call aponta para IP interno | SSRF guard: bloqueia ranges privados e link-local; valida redirecionamentos | Tentar `http://127.0.0.1`, `http://10.0.0.1`, `http://169.254.169.254` → bloqueado |
| I3 | Information Disclosure | Path traversal via tool `files.read` | Normalização de caminhos + jail obrigatório | `../../../etc/passwd` → rejeitado |
| I4 | Information Disclosure | Memória semântica vaza entre projetos | Filtro por escopo no retrieval (`PROMPT MESTRE` §10.6) | Memória do projeto A em conversa do projeto B → não recuperada |
| D1 | Denial of Service | Worker travado em loop | `timeout_ms` no manifesto + watchdog | Worker que nunca responde → morto em `timeout_ms` |
| D2 | Denial of Service | Modelo em loop emitindo tool_calls inúteis | `max_steps`, `max_cost`, `max_tokens` no `Budget` | Run que estoura `max_steps` → falha estruturada |
| D3 | Denial of Service | PDF/Documento gigante enviado para o contexto | Paginação de leitura com limite; contagem de tokens explícita | Arquivo de 1 GB como anexo → app recusa antes de carregar |
| E1 | Elevation of Privilege | Subagente invoca ferramenta que o pai não tem | Interseção por execução inclui camada do pai | Subagente tenta `exec.shell` sem permissão do pai → bloqueado |
| E2 | Elevation of Privilege | Memória armazenada altera system prompt | Memória é **dado**, não instrução (`PROMPT MESTRE` §10.10) | Injetar "ignore all previous instructions" em memória → modelo trata como conteúdo |
| E3 | Elevation of Privilege | Documento anexo instrui modelo a vazar credencial | Conteúdo recuperado entra como dado, não como instrução; ferramentas perigosas exigem aprovação | PDF com payload malicioso anexado → execução não vaza credencial |
| P1 | Prompt Injection | Página web aberta via `web.open` contém instrução para o modelo | Mesmo tratamento: conteúdo é dado, não instrução; LLMs são explicitamente instruídos a tratar conteúdo recuperado como não-confiável | Página HTML com `<meta name="instructions">` → execução não segue |

## Credenciais (`PROMPT MESTRE` §25.1)

- **Onde**: Windows Credential Manager (DPAPI), vinculadas ao usuário Windows.
- **Onde nunca**: `.env`, SQLite em texto puro, logs, JSON, frontend, memória semântica.
- **Quem acessa**: apenas o adapter do provedor, no momento de uso.
- **Quem nunca acessa**: workers (eles recebem tarefa, não credencial); frontend (apenas referência opaca, ex: "openai-configured").
- **Teste automático obrigatório**: nenhuma variável de ambiente de processo do sandbox contém valor de credencial cadastrada (`PROMPT MESTRE` §22.5).

## Sandbox (`PROMPT MESTRE` §22)

Aprofundado em [`docs/architecture/windows-sandbox-design.md`](./windows-sandbox-design.md). Decisões concretas estão em [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md), [ADR-0033](../decisions/0033-sandbox-network-policy.md), [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md). Resumo:

- **3 camadas combinadas** por tipo de execução: Jail (path safety, Fase 6 Etapa 5.X) + Job Object (tree-kill, limites de memória) + Restricted Token (drop de 6 privilégios elevados) + env zeroed por allowlist.
- **AppContainer adiado** para Fase 8+ (quebra rotinas comuns de Python/Node; ADR-0031 D6).
- **Rede desligada por padrão**; quando habilitada, passa por proxy local do app (ADR-0033) com allowlist (deny-by-default) e registro de URLs visível ao usuário (entrada em `DbAuditSink`, `kind: 'network_access'`).
- **Limites**: memória por processo (2 GB) e total da árvore (4 GB), wall-clock (60s default, configurável), tree-kill via `KILL_ON_JOB_CLOSE` do Job Object (mesmo em `kill -9` do app).
- **Diretório temporário por execução** dentro do workspace, limpo após fim.
- **Comandos aprovados são exibidos ao usuário exatamente como serão executados, sem abreviação** (`PROMPT MESTRE` §22.5 final, ADR-0034 D5). A invariante tem teste em `crates/e2e/tests/e2e_approval_display.rs::approved_command_matches_actual_invocation` (byte-a-byte).
- **Default deny** para ferramentas perigosas (D1 do ADR-0034): `files.write`, `files.edit`, `exec.python`, `exec.node`, `exec.shell` nascem com `PermissionSet` em `None`. Usuário liga explicitamente.
- **Regra de "teste de negação"** (do prompt do user, 2026-08-08): toda etapa da Fase 7 entrega **pelo menos um teste que prova o que o sandbox bloqueia**, não só o que ele permite.

### O que o sandbox **NÃO** protege (a parte "NÃO protege" da honestidade)

A REGRA 1.1 e a honestidade do `SECURITY.md` exigem que o documento diga o que o mecanismo **não** cobre. Isolamento local sem contêiner é, por natureza, mais fraco que `runc`/`runsc`/WSL. As lacunas explícitas da Fase 7:

| Cenário | Protege? | Por quê / Mitigação |
|---|---|---|
| Filho lê arquivo fora do workspace via `open()` | **sim** | Jail (Fase 6 Etapa 5.X, PR #25) + filesystem do OS |
| Filho cria neto que sobrevive ao `kill -9` do app | **sim** | Job Object com `KILL_ON_JOB_CLOSE` (ADR-0031 D3, ADR-0036 D2) |
| Filho herda privilégio de admin | **não** | Restricted Token descarta 6 privilégios elevados, mas outros (SeNetworkLogonRight, SeInteractiveLogonRight, etc.) permanecem. Defesa contra elevação total requer AppContainer (Fase 8+) ou conta de usuário separada (Fase 8+). |
| Filho lê credencial em cache de DLL/TLS | **parcialmente** | Env filter zera env vars (ADR-0031 D5, ADR-0036 D5), mas a credencial pode estar em (a) estrutura de adapter em memória, (b) TLS handshake cache, (c) `~/.netrc` do usuário. Defesa em profundidade: ferramentas que lidam com credencial **não passam** pelo sandbox (worker de provider, Fase 2). |
| Filho conecta direto via `socket.socket(AF_INET, SOCK_STREAM)` raw, ignorando `HTTPS_PROXY` | **não** | Sem firewall no nível de processo (WDAC é roadmap de Fase 8+). O filho pode chamar `connect()` direto, bypassando o proxy. **Lacuna documentada.** |
| Filho tenta HTTP/3 (QUIC) | **não** | Proxy é TCP+TLS (ADR-0033). QUIC bypassa. **Lacuna documentada.** |
| Filho exfiltra via DNS | **sim** | `netsh dns set` força DNS via proxy, que valida hostname **antes** de resolver (ADR-0033 D5) |
| Filho lê `SAM` (Windows Security Account Manager) | **sim** | `SeBackup` removido pelo Restricted Token (ADR-0036 D4) |
| Filho faz `rm -rf /` no host | **dentro do sandbox: sim (afeta apenas workdir)** | `exec.shell` com `Denylist` proíbe comandos destrutivos (Etapa 6); `exec.python` requer aprovação (ADR-0034) |
| Filho **lê** arquivo Medium-labeled fora do workdir (read-up) | **não** | Mandatory Label\Low (Etapa 5+ da Fase 7) só bloqueia **write-up** (`NO_WRITE_UP`). Child (Low) **consegue ler** paths Medium-labeled (default do filesystem): `%LOCALAPPDATA%\studio\frederico\ia\data\frederico.db`, `%USERPROFILE%\Documents`, `%APPDATA%`, etc. DACL permite o user ler seus próprios arquivos; Mandatory Label só nega **escrita** se `token_integrity < object_label`. **Lacuna documentada** — Etapa 5+ (Fase 7) + roadmap Fase 8 (SID restritivo próprio + DACL custom fecha). |
| Usuário malicioso bypassa o sandbox | **não** (esperado) | Sandbox é defesa contra o **filho**, não contra o usuário. Usuário admin pode matar o app, debugar, editar config. **Esperado e documentado.** |

**A frase que vale para o usuário é literal:** *"O sandbox da Fase 7 é defesa em profundidade contra as ameaças I1, I2, I3, e a classe 'filho malicioso/invadido' das ameaças STRIDE. Não é sandbox de contêiner (Docker/runc), não é VPN, não é firewall. Lacunas explícitas: bypass de proxy via socket raw, HTTP/3, certificate pinning bypass, privilégios SeNetworkLogonRight, **read-up de paths Medium-labeled** (Mandatory Label só bloqueia write-up; fechamento exige SID restritivo próprio, Fase 8+). Mitigações estão em roadmap de Fase 8+."*

## LGPD (`PROMPT MESTRE` §25.4)

- **Exportação de todos os dados do usuário** (formato portável, JSON + binários).
- **Exclusão total** (`purge`) de conta local.
- **Controle granular de telemetria** (opt-out por padrão).
- **Processamento local por padrão**; nada de telemetria obrigatória.
- **Logs sem segredos** (T1 acima é a rede de segurança).

## Testes

- Cobertura de cada linha da tabela acima é obrigatória.
- Suítes `tests/security/` e `tests/recovery/` (`PROMPT MESTRE` §28).
- Máquina limpa (`PROMPT MESTRE` §28.5) é a verificação final de que as contramedidas funcionam sem dependência de ambiente pré-instalado.

### O que já está coberto por teste (verificado em 2026-07-29)

Promoção do Estado para `parcialmente implementado` (`REGRAS §1.13`). As linhas abaixo são as que têm contramedida implementada **e** teste que a prova; as demais da tabela continuam sendo especificação.

| ID | Onde a contramedida vive | Teste que prova |
|---|---|---|
| T1 | `frederico_provider_engine::sanitize` (regex bloqueando `Authorization`, `api_key`, `Bearer `, `sk-`, …) | `crates/provider-engine/tests/fixtures_sanitize.rs::every_fixture_passes_sanitization` |
| I3 | `Jail::resolve` — ponto único de normalização de caminho (`crates/tool-registry/src/workspace.rs`) | `reject_parent_dir_component`, `reject_nested_parent_dir`, `reject_absolute_windows_path`, `reject_unc_path`, `reject_symlink_pointing_outside` |
| I4 | Filtro por escopo no retrieval (`frederico-memory`) | `crates/memory/tests/evaluation.rs` — gate `max_cross_scope_leak = 0` no `config/eval.toml` |
| R1 | Tabela `tool_audit` append-only (migration `0005_tool_audit.sql`) + `DbAuditSink` | `crates/execution-engine/tests/audit.rs::audit_records_files_read_execution` |
| D2 | `BudgetEnforcer` com `max_steps` (`crates/agent-engine/src/budget.rs`) | suíte do `frederico-execution-engine` (teste de budget) |
| E2 | `determine_real_origin` — conteúdo externo vira `pending_review`, nunca instrução | cenários de prompt injection e malicious memory no `crates/memory/tests/fixtures/gold_set.jsonl` |

**Limite explícito desta verificação:** foram conferidas as seis linhas acima contra o código. As demais (S1, S2, T2, T3, I1, I2, D1, D3, E1, E3, P1) **não** foram verificadas e permanecem especificação — várias dependem do sandbox (Fase 7) e dos workers sidecar (Fase 5), que não existem.

**Cobertura planejada pela Fase 7 (Etapa 1, 2026-08-08, sem código ainda):**

| ID | Etapa da Fase 7 | Teste planejado |
|---|---|---|
| I1 (env leak) | Etapa 2 | `crates/security/tests/env_isolation.rs::child_env_does_not_contain_parent_secrets` (fecha a única ameaça com teste de regressão que ainda depende da Fase 7) |
| I2 (SSRF) | Etapa 7 | `crates/e2e/tests/e2e_network_proxy.rs::child_request_to_169_254_169_254_is_denied` (proxy local + allowlist) |
| I3 (path traversal) | Etapa 5 (file ops) | já coberto pelo `Jail` (Fase 6); ganha mais 3 testes com `files.write`/`files.edit` recusando path traversal |
| D1 (worker travado) | Etapa 2 (Job Object) | `crates/security/tests/tree_kill.rs::child_survives_parent_kill9` |
| E1 (subagente escala privilégio) | Etapa 4 (subagente sob sandbox) | `crates/e2e/tests/e2e_subagent_approval.rs::subagent_requires_own_approval_even_if_parent_approved` |
| E3 (documento malicioso vaza credencial) | Etapa 5 (file ops + audit) | `crates/e2e/tests/e2e_audit_logging.rs::file_write_audit_contains_hashes_not_content` (audit log sem vazar conteúdo) |
| P1 (página web com prompt injection) | Etapa 7 (proxy + audit) | `crates/e2e/tests/e2e_network_proxy.rs::audit_records_network_access` (log de URLs acessadas, fecha a auditoria pós-incidente) |

A Etapa 1 da Fase 7 planejou esses testes (vide [`windows-sandbox-design.md`](./windows-sandbox-design.md) §"Mapa de E2E planejado por etapa"). A Etapa 2 em diante implementa e atualiza o `status.md` com `path::fn_name` real — gate `check-e2e-gate.ps1` valida consistência.

## Não-objetivos

- Hardening de kernel/Windows em si.
- Anti-DDoS de rede (não temos servidor público).
- Certificação FIPS / Common Criteria.
- Sandboxing de GPU (sem uso sensível na v1).
- Análise estática de modelos adversariais.

## Decisões

Nenhuma nova nesta versão. Decisões a tomar na Fase 3 (com ADR próprio):

- Provider de credencial alternativo em ambientes sem Windows Credential Manager (cenário: máquina corporativa sem DPAPI?).
- Política de allowlist de rede no sandbox: o que é "trusted"?
- Política de retenção de logs: quanto tempo guardar antes de anonimizar / descartar?

## Referências

- `PROMPT MESTRE` §22 (execução local), §22.5 (segredos e rede), §25 (segurança e privacidade), §28.4 (testes de recuperação)
- [`process-architecture.md`](./process-architecture.md)
- [`tool-permission-model.md`](./tool-permission-model.md)
- [`agent-state-machine.md`](./agent-state-machine.md) — integridade dos checkpoints
- [`testing-strategy.md`](./testing-strategy.md)
