# 0049 — A matriz de GitHub vira eixo do `PermissionSet`

## Contexto

O [ADR-0041](0041-github-auth-e-matriz-de-autorizacao.md) §D2 decidiu, na Etapa 1 da Fase 8, que autorização de GitHub é matriz e não escalar:

> `PermissionSet` ganha um eixo `github` estruturado, não um `bool`.

Isso não aconteceu. O `PermissionSet::github` continua sendo o enum `GitHubPermission` (`None`/`ReadOnly`/`Clone`/`Commit`/`Push`) criado na Fase 3 — uma escala linear, que é moralmente o mesmo problema que o §D2 rejeita: "pode até `Push`" autoriza empurrar para qualquer repositório e qualquer branch.

A Etapa 5 construiu a `MatrizAutorizacao` dentro do `github-engine`, onde ela **é aplicada de verdade** (estado do cliente, consultada antes de qualquer rede). O [ADR-0048](0048-superficie-de-ferramentas-de-marco-e-github.md) §D3 registrou as ferramentas `github.push` e `github.create_pr` — e a casca as deixou **fora do catálogo**, porque a matriz não tinha de onde vir.

Ou seja: existe motor com portão aplicado, existem ferramentas testadas, e falta o caminho que liga a configuração do usuário ao portão. É este ADR.

## Decisões

### D1 — A matriz entra no perfil como dado simples, não como o tipo do motor

O perfil TOML ganha um campo aditivo:

```toml
[[github_repos]]
repo = "owner/repo"
branches = ["main", "feature/*"]
operacoes = ["read", "push", "create_pr"]
```

O `PermissionSet` guarda `Vec<RegraGithubPerfil>` — `repo`, `branches` e `operacoes` como texto. A conversão para `MatrizAutorizacao` acontece na **composição**, não no perfil.

O precedente é literal e está na mesma struct: o `network_allowlist` é `Vec<String>` no `PermissionSet` e vira `NetworkAllowlist` tipada só quando o proxy é construído (Fase 7 Etapa 7). A razão é a mesma: manter o tipo que **decide** livre de serde e de formato de configuração, para que mudar o arquivo não force mudar o portão.

Campo aditivo, como o `network_allowlist` foi: perfil antigo continua parseando e cai em vazio — que nega tudo, exatamente o que já acontecia implicitamente antes do campo existir. Sem bump de `SCHEMA_VERSION`, porque o cache persistido guarda o TOML bruto e não o `PermissionSet` parseado.

### D2 — O merge é interseção nos três eixos, fail-closed

`efetivo = usuário ∩ projeto ∩ assistente`, e a interseção desce até dentro da regra:

- **Repositório** ausente de qualquer camada sai inteiro.
- **Branch** sobrevive só se **todas** as camadas que citam o repositório a citam.
- **Operação** idem.
- Regra que sobra sem branch ou sem operação é descartada — ela não autorizaria nada, e mantê-la produziria a mensagem de erro errada ("operação negada" em vez de "fora da matriz").

É a regra que o `MatrizAutorizacao::intersecao` já implementa e que o teste `intersecao_e_fail_closed` já trava. Este ADR não a inventa: ele a coloca no caminho do `PermissionSet`.

### D3 — O `GitHubPermission` escalar **sai**

Não fica convivendo com a matriz. Dois eixos para a mesma coisa é o defeito que produz a pergunta "qual dos dois vale?" — e a resposta certa nunca é "os dois".

O campo `github` do `PermissionSet` é removido junto com o enum. Perfis que o citam passam a ver o campo ignorado pelo parse (`serde` com `deny_unknown_fields` **não** está ligado nesta struct, e não passa a estar: recusar perfil antigo inteiro por uma chave obsoleta trocaria uma permissão a menos por um app que não abre).

**Nenhum perfil em uso perde capacidade por isso**, porque a capacidade nunca existiu: o campo não era lido por ferramenta nenhuma. É remoção de declaração morta, não de permissão viva.

### D4 — Sem matriz não-vazia, as ferramentas continuam fora do catálogo

O bump atômico do [ADR-0020](0020-fase-5-etapa-4-excelpro-inspect.md) §3 D3 continua valendo, e agora com as duas condições reais: **token no DPAPI** e **matriz não-vazia no perfil efetivo**. Faltando qualquer uma, `github.push` e `github.create_pr` não entram no catálogo nem na allowlist.

Matriz vazia com token presente **não** registra as ferramentas. Registrar produziria um catálogo que anuncia capacidade e recusa toda invocação, gastando uma ida à fila de aprovação para falhar — o que o ADR-0048 §D3 já rejeitou.

## Alternativas descartadas

1. **Manter o `GitHubPermission` escalar ao lado da matriz**, para compatibilidade. Rejeitado pelo §D3: dois eixos para a mesma decisão.
2. **Serializar a `MatrizAutorizacao` direto no perfil.** Rejeitado pelo §D1: acopla o tipo que decide ao formato do arquivo, e uma mudança de configuração passaria a mexer no portão.
3. **Ligar as ferramentas com matriz vazia e deixar o motor recusar.** Rejeitado pelo §D4 — é o comportamento que o ADR-0048 §D3 já recusou, pelo custo em aprovações inúteis.
4. **Curinga de repositório (`owner/*`).** Rejeitado: o ADR-0041 §D2 já registrou que `allow_all()` não vira curinga, porque não existe "todos os repositórios" que o sistema saiba interpretar sem inventar comportamento. O mesmo vale para "todos os repositórios de um dono".

## Consequências

- **Fica mais fácil:** usar GitHub pelo app. Era motor com porta trancada e chave inexistente.
- **Fica mais difícil:** ligar por engano. São duas condições independentes, e nenhuma tem default permissivo.
- **O `PermissionSet` muda de forma** — `github` sai, `github_repos` entra. Toca `permission.rs`, `permission_loader.rs` e os construtores da composição.
- **A UI precisa dizer que a lista vazia é o motivo** de as ferramentas não aparecerem. O ADR-0041 §Consequências já registrava esse risco ("o usuário lê como bug"); com a matriz no perfil, ele deixa de ser hipotético. Item de UI da Etapa 6.

## Histórico de revisão

- 2026-08-19 — versão inicial. Fecha o §D2 do ADR-0041, aberto desde a Etapa 1.
