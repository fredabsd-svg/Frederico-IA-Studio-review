# 0040 — GitHub: token no DPAPI, matriz de autorização e operações irreversíveis

## Contexto

A Fase 8 (ADR-0038 §D1) entrega `push` e criação de PR pelo app. É a primeira vez que o produto executa uma **operação destrutiva em serviço externo autenticado** por conta do agente. Todas as anteriores eram locais: escrita de arquivo sob Jail, execução sob sandbox, rede sob proxy com allowlist.

A diferença importa. Um `files.write` errado tem backup `.bak` e hashes no audit (ADR-0035). Um `git push --force` errado altera o repositório de outras pessoas, e não há `.bak` do GitHub. Um PR criado por engano notifica revisores e fica no histórico mesmo se fechado.

O ADR-0032 §D2 já citava o precedente: o `agent/githubAccess.js` do projeto anterior mostrou que "matriz de autorização estruturada" é necessária — permissão de GitHub não é um booleano.

## Decisões

### D1 — Token no Windows Credential Manager, nunca em arquivo nem em env

Mesma trilha do `WindowsCredentialStore` da Fase 2 Hardening 1: `CredWriteW`/`CredReadW`/`CredDeleteW`, `TargetName` no padrão `Frederico-IA-Studio:github:<conta>`. O token viaja como `SecretString` (`secrecy`), já dependência do workspace.

**Nunca no ambiente do processo.** A Fase 7 Etapa 6+1 provou por teste (`env_credential_not_leaked.rs`) que uma credencial no ambiente do pai vaza para o filho do sandbox quando o `EnvFilter` falha — e que essa falha pode ser silenciosa. Um token de GitHub com escopo de escrita no ambiente é a pior versão desse cenário. O `EnvAllowlist` não recebe entrada de GitHub.

### D2 — Autorização é matriz, não booleano

`PermissionSet` ganha um eixo `github` estruturado, não um `bool`:

| Dimensão | Forma | Fail-closed |
|---|---|---|
| Repositórios | lista explícita `owner/repo` | lista vazia = nenhum |
| Branches | padrão por repositório | vazio = nenhum |
| Operações | `read` / `push` / `create_pr` | ausente = negado |

A interseção segue a regra dos demais eixos (usuário ∩ projeto, fail-closed), e o `allow_all()` **não** vira curinga — pelo mesmo motivo que o `network_allowlist` da Fase 7 Etapa 7 manteve a lista vazia em `allow_all()`: não existe "todos os repositórios" que o sistema saiba interpretar sem inventar comportamento.

**Branch protegida é negação explícita.** `push` para `main`/`master` exige que o padrão a inclua nominalmente. Não há default permissivo.

### D3 — `--force` não existe na API

O `github-engine` **não expõe** push forçado, em nenhuma forma. Não é uma opção com aprovação reforçada: é ausência de API. Mesma regra que a Fase 7 aplicou duas vezes ("capacidade incompleta é capacidade indisponível") — aqui na variante "capacidade cuja falha é irreversível não entra pela porta da frente".

Quem precisa de force-push tem `exec.shell`, com o comando à vista, denylist e aprovação por invocação. A diferença entre as duas portas é que numa o usuário está lendo o comando e na outra o agente decidiu sozinho.

### D4 — Aprovação por operação, com o alvo no texto

`push` e `create_pr` exigem aprovação por invocação (`OneExecution`), e o pedido mostra **repositório, branch e contagem de commits** — não "o agente quer usar o GitHub". O ADR-0034 já estabeleceu que o pedido carrega o comando exato; aqui o equivalente é o alvo exato.

**Nota de realidade** herdada da Fase 7 Etapa 7: o cache de aprovação por escopo não existe em código, então toda tool com `requires_user_approval` já pede aprovação a cada invocação. `OneExecution` é o comportamento real de hoje sem código adicional — e o ADR-0038 §D4 registra a construção do cache como trabalho da Etapa 7, momento em que esta garantia precisará de código próprio para **não** ser afrouxada junto.

### D5 — E2E é noturno, com twin determinístico obrigatório

Criar PR de verdade exige rede, secret e serviço externo: `#[ignore]`, noturno, conforme REGRA §3.3. O twin determinístico roda em todo PR contra um servidor HTTP local que fala o subconjunto usado da API — provando o caminho de produção do `github-engine` sem tocar o GitHub.

O twin não é opcional nem "quando der": a REGRA §3.3 impede promover fase sem ele, e o ADR-0038 §D2 acrescenta que o noturno precisa de um run verde citável antes de a fase fechar.

## Alternativas descartadas

1. **OAuth device flow em vez de PAT.** Melhor experiência e escopo mais fino. Adiado, não rejeitado: exige registrar um GitHub App e manter um `client_id` do produto, decisão de produto que não cabe numa etapa de engenharia. O PAT resolve a Fase 8 com a trilha de credencial que já existe. Fica no roadmap.
2. **Permissão de GitHub como `bool`.** Rejeitado pelo precedente citado no ADR-0032 §D2 e pela assimetria de dano: "pode usar GitHub" autoriza tanto ler um repositório público quanto empurrar para o `main` de produção.
3. **Expor `--force` com aprovação reforçada.** Rejeitado pelo §D3. Aprovação protege contra o agente agir sozinho, não contra o usuário aprovar por hábito — e o dano aqui não tem desfazer.
4. **Token em variável de ambiente**, como muitas CLIs fazem. Rejeitado pelo §D1, com teste da Fase 7 como evidência de que o vazamento acontece em silêncio.
5. **Só noturno, sem twin.** Rejeitado pela REGRA §3.3 — e pela constatação de 2026-08-16 de que o noturno deste repositório nunca rodou verde em 12 tentativas.

## Consequências

- **Fica mais fácil:** auditar. Cada operação tem repositório, branch e decisão de autorização registrados, no mesmo espírito do `network_audit` da Fase 7.
- **Fica mais difícil:** usar o app como cliente de Git completo. Rebase interativo, force-push e reescrita de histórico ficam fora — deliberadamente.
- **Custo de configuração:** sem repositório na matriz, nada funciona. É fail-closed, e a UI precisa dizer isso com clareza, senão o usuário lê como bug. Risco real, registrado como item de UI da Etapa 6.
- **Dependência de secret no CI** para o noturno, que o ADR-0038 §D2 transformou em pré-condição de fechamento da fase.

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 1 da Fase 8.
