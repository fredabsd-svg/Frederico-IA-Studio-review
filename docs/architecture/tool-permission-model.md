<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 3
-->

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
