<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-18
Fase correspondente: 8 (Etapa 5)
-->

# `frederico-github-engine`

`push`, criação de PR e a matriz que autoriza os dois.

Spec: [`github-integration-architecture.md`](../architecture/github-integration-architecture.md).
Decisão: [ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md).

## 1. Por que este módulo é diferente

É a **primeira operação destrutiva em serviço externo** que o produto
executa por conta do agente. As anteriores eram locais e reversíveis
por construção: `files.write` tem `.bak` e hashes na auditoria;
`exec.*` roda em sandbox que morre com o run.

Aqui não há desfazer. Um `push` errado altera o repositório de outras
pessoas; um PR criado por engano notifica revisores e fica no
histórico mesmo depois de fechado.

## 2. O que existe hoje

| API | O que faz |
|---|---|
| `MatrizAutorizacao::autoriza` | repositório × branch × operação, fail-closed |
| `MatrizAutorizacao::intersecao` | `usuário ∩ projeto`, só restringe |
| `GithubEngine::criar_pr` | REST, com `base_url` injetável para o twin |
| `GithubEngine::push` | `git2` sobre o remoto configurado, refspec sem force, com o remoto conferido contra a matriz |

## 3. A matriz é o portão, e ela é estado do cliente

`GithubEngine` guarda a matriz e a consulta **antes de qualquer
rede**, em toda operação. Ela não é parâmetro de chamada de propósito:
se a autorização viajasse por argumento, bastaria um caminho de código
esquecer de passá-la.

Isso é diferente do `PermissionSet::git` da Etapa 3, que é declaração
sem portão (o `validate_tool_call` não lê permissão por categoria).
Aqui a autorização é **aplicada**, no próprio motor.

Três dimensões, todas fail-closed:

| Dimensão | Vazio significa |
|---|---|
| Repositórios (`owner/repo`) | nenhum |
| Branches (nome exato ou `prefixo*`) | nenhuma |
| Operações (`read`/`push`/`create_pr`) | negado |

**Curinga não alcança `main` nem `master`.** Quem quer empurrar para a
branch principal escreve o nome dela, e a escrita é o consentimento —
não um `*` digitado para liberar branches de trabalho que passou a
cobrir produção sem ninguém perceber. Fixado em
`curinga_nao_alcanca_branch_protegida`.

## 3.1 O remoto é conferido contra a matriz (ADR-0048 §D4)

A matriz autoriza `owner/repo`, mas o `git2` empurra para onde o
remoto apontar. Sem conferência, um remoto trocado empurraria para
outro lugar carregando a autorização do repositório certo — e
`.git/config` fica no workspace, onde o agente escreve.

O `push` compara a URL do remoto com o `owner/repo` autorizado e
recusa se não baterem. São aceitas as formas que o GitHub publica
(`https://`, `git@`, `ssh://git@`, com ou sem `.git` e barra final);
**qualquer outro host é recusado**, inclusive um com o mesmo caminho
(`https://gitlab.com/owner/repo`) ou um sufixo enganoso
(`github.com.attacker.example`).

Isto custou o twin do `push`: um repositório bare local nunca é
`github.com`, então ele passou a ser corretamente recusado. A mecânica
do push virou função privada, exercitada por teste de unidade que a
alcança **sem** abrir porta que contorne a política; o antigo twin
virou a negação `push_recusa_remoto_que_nao_e_o_repositorio_autorizado`.

## 4. Force-push é ausência de API

Não é opção com aprovação reforçada (ADR-0041 §D3). O refspec é
montado no código, literal, sem prefixo `+`, e não há parâmetro que o
produza. O teste `github_has_no_force_push_api` varre o fonte e falha
se `force`, `+refs/` ou um refspec vindo de fora aparecerem.

Quem precisa de force-push tem `exec.shell`, com o comando à vista,
denylist e aprovação por invocação. A diferença entre as duas portas é
que numa o usuário está lendo o comando e na outra o agente decidiu
sozinho.

## 5. O token

Chega como `SecretString`, de quem já o leu do Windows Credential
Manager (`ServiceCredentialStore`, Etapa 2). O crate **não** o guarda
em arquivo e **nunca** o coloca no ambiente do processo — a Fase 7
provou por teste (`env_credential_not_leaked.rs`) que credencial no
ambiente do pai vaza para o filho do sandbox, e que a falha pode ser
silenciosa.

No `push`, o token vai pelo callback de credencial do `git2`
(`userpass_plaintext` com usuário `x-access-token`), que é o que o
GitHub aceita em HTTPS.

## 6. Testes

`crates/github-engine/tests/matriz_e_pr.rs`, 12 em todo PR + 1 noturno:

| Teste | Prova |
|---|---|
| `github_create_pr_against_local_stub` | **twin** — caminho de produção contra socket HTTP local, conferindo request e resposta |
| `github_create_pr_against_real_service` | **noturno** (`#[ignore]`) — PR de verdade no GitHub |
| `github_rejects_repo_outside_matrix` | **negação** — recusa antes da rede (o `base_url` aponta para porta inválida; se autorizasse depois, falharia por timeout) |
| `github_has_no_force_push_api` | **negação estrutural** — varre o fonte |
| `curinga_nao_alcanca_branch_protegida` | **negação** — `*` não cobre `main`/`master`, com controle positivo |
| `matriz_vazia_nega_tudo` | **negação** — o default |
| `operacao_ausente_e_negada` | **negação** |
| `repo_mal_formado_e_recusado` | **negação** — 6 formas inválidas |
| `intersecao_e_fail_closed` | interseção só restringe, em repositório, branch e operação |
| `recusa_do_github_traz_a_mensagem_do_servico` | erro do serviço vira causa nomeada |
| `push_chega_ao_remoto_pelo_caminho_de_producao` | twin do push contra repositório bare local |
| `push_para_branch_nao_autorizada_e_recusado` | **negação** — e nada chega ao remoto |
| `push_de_branch_inexistente_falha_antes_da_rede` | **negação** |

## 7. O caminho até o perfil, e o que ainda não existe

- **A matriz chegou ao perfil em 2026-08-19**
  ([ADR-0049](../decisions/0049-matriz-de-github-no-permission-set.md)),
  e com ela as ferramentas passaram a poder ligar. São **duas
  condições independentes** (§D4): token no cofre **e** matriz
  não-vazia no perfil efetivo (`usuário ∩ projeto`). Faltando
  qualquer uma, `github.push` e `github.create_pr` ficam fora do
  catálogo e da allowlist. Matriz vazia com token presente **não**
  liga — registrar anunciaria capacidade e recusaria toda invocação.
- **Multi-conta não está resolvido.** A casca lê a primeira conta
  cadastrada no serviço `github`. Escolher aqui uma regra silenciosa
  ("a mais recente", "a alfabética") criaria comportamento que
  ninguém pediu e que o usuário não consegue prever; a escolha é de
  UI, na Etapa 6.
- **Auditoria própria.** O spec pede repositório, branch, decisão e
  resultado gravados no espírito do `network_audit`. Não existe.
- **O callback de credencial não é exercitado em todo PR.** O twin do
  push usa repositório bare local, cujo transporte não autentica. Quem
  exercita o callback é o noturno.

## 8. Pureza e dependências

`unsafe_code = "forbid"`. `frederico-core`, `git2`, `reqwest`,
`secrecy`. Sem `tauri`, sem `windows` — o `check-core-purity.ps1`
cobra.

O `git2` aqui é o mesmo do `git-engine` (ADR-0047), e pelo mesmo
motivo: `push` fala o protocolo Git, e `Command::new("git")`
contornaria o sandbox inteiro da Fase 7 (ADR-0040 §D1).
