<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 3 (Etapa 3)
-->

> Última verificação: 2026-07-27. Reflete a Etapa 3 da Fase 3 — tipo
> `PermissionSet` completo no `frederico-tool-registry`
> (`crates/tool-registry/src/permission.rs`), com todos os 18
> campos do spec §"Contrato" (file_read, file_create/modify/delete,
> terminal, python, node, git, github, web_browse, web_download,
> network, screen_capture, input_control, memory, credentials,
> documents, destructive_ops). Enums auxiliares:
> `FileReadPermission` (None / WorkspaceOnly / WorkspacePlusApproved),
> `RuntimePermission` (None / ReadOnly / Sandboxed / Unrestricted,
> com `PartialOrd` pra invariante subagente), `TerminalMode`,
> `GitPermission`, `GitHubPermission`, `MemoryPermission`,
> `DocumentPermission` (todos com ordem canônica pro
> `is_subset_of`). **Default é deny**: `PermissionSet::default()`
> tem todos os campos em `false` ou variante mais restritiva
> (spec §"Invariantes": "Default é deny: ferramenta perigosa
> nasce desligada; ligar é decisão consciente"). Invariante
> **"subagente ⊆ pai"** modelado em `PermissionSet::is_subset_of`
> (booleanos: `!self.x || parent.x`; enums: `self <= parent` via
> `PartialOrd`; `file_read`: matriz de combinações que respeita
> `None < WorkspaceOnly < WorkspacePlusApproved`). `ValidationContext`
> ganha `permissions: PermissionSet` e
> `parent_permissions: Option<Box<PermissionSet>>`; o Passo 5 do
> `validate_tool_call` consome: rejeita `files.read` quando
> `file_read == None` com `TOOL_PERMISSION_DENIED`, e valida o
> invariante subagente via `check_subagent_invariant()` (falha
> com `TOOL_PERMISSION_DENIED` se `permissions ⊄ parent_permissions`).
> `PermissionSet::allow_all()` é o helper da Etapa 4 (UI modal de
> "Permitir tudo"). Suíte do crate: 15 testes no `permission.rs`
> (default deny, allow_all, is_subset_of em pares válidos e
> inválidos, random pair invariant) + 4 testes no `validate.rs`
> (rejeição por None, aceitação por WorkspaceOnly/PlusApproved,
> subagente com mais permissões rejeitado, subagente ⊆ pai
> aceito). Suíte workspace **240/240 verde** (era 225/225). Etapas
> 4 (integração), 5 (watchdog) e 6 (UI) ainda dependem — em
> particular, a Etapa 4 carrega o `PermissionSet` real do
> `assistant`/`project`/`user` antes de validar; a Etapa 6 consome
> o `WorkspacePlusApproved` no modal de aprovação de leitura fora
> do workspace. Ver
> [`docs/modules/tool-registry.md`](../modules/tool-registry.md)
> para o detalhamento por eixo do template §1.4.

# Modelo de Permissões de Ferramentas

Permissões são **hierárquicas e interseccionais**. Uma ferramenta só executa se passar por todas as camadas (`PROMPT MESTRE` §8).

## Hierarquia

```text
permissões globais
∩ permissões do perfil
∩ permissões do assistente
∩ permissões do projeto
∩ permissões do agente pai
∩ permissões do subagente
∩ permissões da execução
∩ aprovação do usuário
```

**Subagente nunca tem mais permissões que o agente pai** — a interseção com a camada pai é obrigatória. A regra é *verificável em teste*: dado `perm(pai)`, `perm(subagente) ⊆ perm(pai)` deve valer para todo par.

## Contrato

```rust
struct PermissionSet {
    file_read: bool,
    file_create: bool,
    file_modify: bool,
    file_delete: bool,
    terminal: TerminalPermission,    // Allowlist | Denylist | RequireApproval | None
    python: RuntimePermission,
    node: RuntimePermission,
    git: GitPermission,
    github: GitHubPermission,
    web_browse: bool,
    web_download: bool,
    network: bool,
    screen_capture: bool,
    input_control: bool,             // mouse / teclado
    memory: MemoryPermission,
    credentials: bool,
    documents: DocumentPermission,
    destructive_ops: bool,
}

enum RuntimePermission { None | ReadOnly | Sandboxed | Unrestricted }
enum TerminalPermission { None | RequireApproval | Denylist(Vec<String>) | Allowlist(Vec<String>) }
```

## Categorias (`PROMPT MESTRE` §8)

Lista fechada de categorias. Permissões granulares por **categoria**, não por arquivo individual — o controle de granularidade por caminho é trabalho do jail de filesystem (ver [`security-threat-model.md`](./security-threat-model.md), invariante de path traversal).

## UI de aprovação

A interface mostra, no momento da aprovação:

- O que está sendo solicitado (operação, argumentos, arquivos afetados)
- Por qual agente (e qual subagente, se houver)
- Com qual modelo
- Qual ferramenta (ID, versão, risco declarado)
- Quais arquivos serão acessados (caminho **normalizado**, não o que o modelo pediu)
- Se haverá rede
- Se a operação é reversível
- Botões: "Permitir uma vez" / "Permitir para esta execução" / "Permitir para o projeto" / "Negar"

O comando exato (quando for `exec.shell`) é exibido **exatamente** como será executado, sem abreviação (`PROMPT MESTRE` §22.5).

## Invariantes

- **Subagente nunca tem permissão que o pai não tem** (verificável em teste: `perm(subagente) ⊆ perm(pai)` para todo par).
- **Ferramentas com `risk_level = critical` exigem aprovação explícita a cada invocação**, sem "lembrar para esta execução".
- **Default é deny**: ferramenta perigosa nasce desligada; ligar é decisão consciente.
- **Aprovação é escopada** à invocação (ou ao escopo que o usuário escolheu); persistente só com escolha explícita.
- **Aprovação é auditada** (quem, quando, o quê, por quê, com qual escopo).
- **Mudança de categoria exige reinício do app ou reload explícito** — não é dinâmico em runtime, para evitar race entre o que o modelo viu e o que está valendo.

## Não-objetivos

- ACLs arbitrárias por usuário (caminho a caminho) na v1.
- Permissões herdadas de "grupo" (estilo Unix).
- Permissões delegáveis entre subagentes (caminho inverso: pai dar ao filho uma permissão que ele mesmo não tem).
- "Modo silencioso" que aprova automaticamente após N usos — apenas opt-in explícito por categoria, e **nunca** para `critical`.

## Decisões

Nenhuma nova nesta versão. Mudanças no modelo de permissão (ex: introduzir ACLs por caminho) exigem ADR.

## Referências

- `PROMPT MESTRE` §8 (permissões), §7.7 (validação), §9.4 (cancelamento)
- [`tool-registry-specification.md`](./tool-registry-specification.md)
- [`security-threat-model.md`](./security-threat-model.md) — modelagem de ameaça do modelo de permissão
- [`testing-strategy.md`](./testing-strategy.md) — cobertura do invariante "subagente ≤ pai"
