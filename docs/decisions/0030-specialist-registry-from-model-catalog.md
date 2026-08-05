# 0030 — `SpecialistRegistry` carrega definições do `model-catalog` + `PermissionSet` real do assistant/project/user (Etapa 3 da Fase 6)

## Contexto

O `PROMPT MESTRE` §9.1 define o registro explícito de especialistas:

> O modelo principal só pode delegar para IDs existentes; nunca para nomes inventados.

E o §9.2 define o **zero fallback silencioso**:

> Erro estruturado, lista de válidos, sem substituição.

A Fase 3 (Etapa 3) implementou o `PermissionSet` no `frederico-tool-registry` (ADR-0014, hoje no `crates/tool-registry/src/permission.rs`) com 18 campos, 5 enums auxiliares com `PartialOrd` pra invariante subagente, e a função `is_subset_of` que prova `perm(filho) ⊆ perm(pai)` em teste. A regra "subagente nunca tem mais permissões que o pai" está modelada.

A Fase 3 (Etapa 4) **não** ligou o `PermissionSet` real do `assistant`/`project`/`user` antes do `validate_tool_call`. O `validate.rs` da Fase 3 Etapa 3 consome `ValidationContext { permissions: PermissionSet, parent_permissions: Option<Box<PermissionSet>> }` mas o `ChatOrchestrator` (Etapa 4 da Fase 3) passa `PermissionSet::default()` (deny-all) como `permissions` — o que **bloqueia qualquer chamada de tool** se o Passo 5 do `validate_tool_call` for estrito. O detalhe técnico está documentado em `docs/modules/tool-registry.md §3` ("a Etapa 4 carrega o `PermissionSet` real do `assistant`/`project`/`user` antes de validar — pendente").

A Etapa 3 da Fase 6 é o que liga isso. O `SpecialistDefinition` (do spec `subagent-architecture.md` stub) é a peça que carrega o catálogo de quem-pode-fazer-o-quê; o `PermissionSet` é o que aplica as permissões. Os dois precisam ser alimentados pela **mesma fonte de verdade** — o `model-catalog` (Fase 2 Etapa 1) e o **sistema de profiles** (assistant/project/user) que já existe parcialmente.

Hoje, o `model-catalog` tem o conceito de "modelo com capabilities" (`ModelDescriptor { id, name, provider, capabilities, cost, ... }`). O `SpecialistDefinition` é a peça **user-facing** do mesmo conceito: "qual modelo + quais ferramentas + quais permissões + qual orçamento + qual timeout". O registro é o que o usuário vê no Modo Equipe; o catálogo é o que o executor consulta pra resolver o modelo.

## Decisões

### D1 — `SpecialistDefinition` carrega do `model-catalog` (não arquivo separado, não hardcoded)

Nova struct em `crates/model-catalog/src/specialist.rs`:

```rust
pub struct SpecialistDefinition {
    pub id: SpecialistId,                    // ex.: "revisor", "pesquisador", "testador"
    pub name: String,                        // "Revisor de Código"
    pub description: String,                 // "Revisa o diff e aponta problemas..."
    pub purpose: String,                     // "Revisão de PR antes de merge"

    pub default_model: ModelId,              // resolve via model-catalog
    pub allowed_model_capabilities: Vec<String>,  // ["code", "long-context", "tools"]

    pub allowed_tools: Vec<ToolId>,
    pub denied_tools: Vec<ToolId>,           // tem precedência sobre allowed_tools

    pub max_steps: u32,                      // sub-budget de steps
    pub timeout_ms: u32,                     // timeout do subagente (não compartilhado com pai)
    pub token_budget: Option<u64>,
    pub cost_budget: Option<Budget>,         // alocação (ADR-0027 D5)
}
```

O `model-catalog` é estendido com `pub fn specialists() -> &[SpecialistDefinition]` que lê de `frederico://specialists/default.toml` (bundled no binário) e merge com `~/.config/frederico/specialists.toml` (override do usuário). A lista bundled é o conjunto **mínimo** necessário para a Fase 6:

- `revisor` — lê código, emite diff de revisão
- `pesquisador` — busca em memória + web_browse
- `testador` — roda testes via terminal sandboxes
- `validador` — checa invariantes (ex.: "esse JSON deve parsear")
- `sumador` — sumariza o output de outros especialistas
- `arquiteto` — projeta estrutura antes de implementar
- `crítico` — aponta fraquezas no plano
- `executor` — implementa o plano

Por que **8 especialistas** (e não 4, não 16): é o conjunto que cobre o caso de uso real do §9 sem inflar. Cada um tem propósito distinto; o usuário pode desabilitar os que não quer (override em `~/.config/frederico/specialists.toml`).

O usuário pode **adicionar** novos via arquivo de config (sem mexer no binário), mas **não pode invadir IDs que não existem** — o `SpecialistRegistry::get(id)` retorna erro com a lista de válidos (§9.2 zero fallback silencioso).

### D2 — `SpecialistRegistry` é a interface; `ModelCatalog` é a fonte primária

Nova trait em `crates/model-catalog/src/registry.rs`:

```rust
#[async_trait]
pub trait SpecialistRegistry: Send + Sync {
    async fn get(&self, id: &SpecialistId) -> Result<&SpecialistDefinition, RegistryError>;
    async fn list(&self) -> Result<Vec<&SpecialistDefinition>, RegistryError>;
    async fn validate_id(&self, id: &str) -> Result<SpecialistId, RegistryError>;
}
```

Implementação bundled: `DefaultSpecialistRegistry` que carrega de `model-catalog` + override do usuário. A trait permite extensões futuras (registry carregado de arquivo de projeto, registry carregado de servidor) sem mexer no `SubagentRunner`.

O `SubagentRunner` (ADR-0027) consome `Arc<dyn SpecialistRegistry>` — não conhece a fonte, só a interface. Mesmo princípio do `WorkerInvoker` (PR #23, ADR-0024): **trait no `core`, implementações específicas em outros crates**.

E2E em `crates/e2e/tests/e2e_specialist_registry_e2e.rs::registry_loads_specialists_from_catalog`:
- Lança `build_chat_orchestrator` com catálogo default.
- Chama `SpecialistRegistry::get(&"revisor".into())` e `::get(&"inexistente".into())`.
- Assert: o primeiro retorna `Ok(SpecialistDefinition { id: "revisor", ... })`; o segundo retorna `Err(RegistryError::UnknownSpecialist { requested: "inexistente", valid: [...] })` com a lista dos 8 bundled.

### D3 — `PermissionSet` real carrega de assistant/project/user antes do `validate_tool_call`

A pendência da Fase 3 Etapa 4 (hoje em `docs/modules/tool-registry.md §3`):

> A Etapa 4 carrega o `PermissionSet` real do `assistant`/`project`/`user` antes de validar — pendente.

A Etapa 3 da Fase 6 fecha isso. Cadeia de resolução:

1. **Profile do usuário** (`~/.config/frederico/profiles/default.toml`): `PermissionSet` base. Default é deny-all.
2. **Profile do projeto** (`./.frederico/project.toml`): merge sobre o do usuário. Default é "herdar do usuário".
3. **Profile do assistant** (`./.frederico/assistants/<id>.toml`): merge sobre o do projeto. Default é "herdar do projeto".
4. **Profile do subagente** (`SpecialistDefinition.allowed_tools ∩ parent_permissions`): interseção, não união (regra do §8). Deny explicito tem precedência.

A interseção **total** (perfil do usuário ∩ perfil do projeto ∩ perfil do assistant ∩ `parent_permissions` ∩ `SpecialistDefinition.allowed_tools` − `denied_tools`) é o `effective_permission_set` do run. Carregado **uma vez** no início do run, cacheado, e passado pro `ValidationContext` em todo `validate_tool_call`.

**Invariante testável (no caminho real, Etapa 4):** `effective_permission_set ⊆ parent_permission_set`. Mesmo padrão do `PermissionSet::is_subset_of` da Fase 3, mas agora exercitado no caminho de produção.

E2E em `crates/e2e/tests/e2e_specialist_registry_e2e.rs::permission_set_inherited_from_assistant_project_user`:
- Cria profiles em temp dir: usuário com `terminal: None`, projeto com `terminal: Denylist(["rm"])`, assistant com `terminal: Allowlist(["cargo", "npm"])`.
- Lança `build_chat_orchestrator` apontando pros profiles.
- Assert: `effective_permission_set.terminal == Allowlist(["cargo", "npm"])` (interseção, não união).
- Subagente com `terminal: Allowlist(["cargo"])` herda do pai e tem `Allowlist(["cargo"])` (estreita, não alarga).

### D4 — Erro "specialist not found" com lista de válidos

A regra do §9.2 (zero fallback silencioso) é mecânica: o `SpecialistRegistry::get` retorna erro estruturado, **sempre** com a lista de IDs válidos:

```rust
pub enum RegistryError {
    UnknownSpecialist {
        requested: String,
        valid: Vec<SpecialistId>,   // <-- sempre presente
    },
    PermissionDenied {
        specialist: SpecialistId,
        required: PermissionSet,
        available: PermissionSet,
    },
    ConfigurationError {
        path: PathBuf,
        cause: String,
    },
}
```

A UI do Modo Equipe (Etapa 6 da Fase 6) renderiza o erro como modal:

> "Subagente 'revisor-final' não encontrado. Os subagentes disponíveis são: revisor, pesquisador, testador, validador, sumador, arquiteto, crítico, executor. Cancele a operação ou escolha um dos disponíveis."

E2E em `::specialist_unknown_id_returns_structured_error`:
- `SubagentRunner::try_spawn(parent, "revisor-final", allocation)` retorna `Err(SubagentError::Registry(RegistryError::UnknownSpecialist { requested: "revisor-final", valid: [...] }))`.
- O `RunState` do pai **não** transiciona para `Failed` — o erro é estruturado e devolvido pro modelo, que decide.
- O `RunEvent` do pai registra `kind = RejectedInvalid` com `payload = { requested: "revisor-final", valid: [...] }`.

### D5 — UI helper de seleção (Etapa 3) sem invocação

A Etapa 3 da Fase 6 entrega o `SpecialistRegistry` consumível pelo backend, **sem UI completa do Modo Equipe**. O que entra na Etapa 3:

- Comando Tauri `ListSpecialists` (em `crates/app/src/commands/`) que retorna `Vec<SpecialistSummary>` (id + name + description, sem campos sensíveis).
- Componente React `<SpecialistPicker>` em `apps/desktop/src/components/team-mode/SpecialistPicker.tsx` — dropdown que lista os 8 bundled + filtragem por capability. Renderiza, mas **não** dispara spawn.
- Suíte de testes do componente (Vitest + Testing Library) que valida renderização, filtragem, e estado disabled quando o registry falha.

A UI completa do Modo Equipe (sidebar com progresso, custo, erros, dependências) é a **Etapa 6** da Fase 6. A Etapa 3 entrega o **backend consumível** + o **componente base** que a Etapa 6 consome.

## Consequências

- `crates/model-catalog/src/specialist.rs` (novo): `SpecialistDefinition` + `SpecialistId` + `SpecialistSummary`. ~150 linhas.
- `crates/model-catalog/src/registry.rs` (novo): trait `SpecialistRegistry` + `DefaultSpecialistRegistry`. ~200 linhas.
- `crates/model-catalog/src/lib.rs` ganha `pub fn specialists() -> &[SpecialistDefinition]` e `pub fn registry() -> Arc<dyn SpecialistRegistry>`. Bump atômico.
- `crates/model-catalog/frederico://specialists/default.toml` (novo, bundled): os 8 especialistas default, cada um com `default_model` apontando pra um modelo real do `model-catalog` (ex.: `revisor → gpt-4o`, `pesquisador → gpt-4o-mini`, `sumador → gpt-4o-mini`).
- `crates/execution-engine/src/subagent_runner.rs` (novo, do ADR-0027) consome `Arc<dyn SpecialistRegistry>` no construtor. O `build_chat_orchestrator` da `crates/app/src/composition.rs` injeta o `DefaultSpecialistRegistry` por default.
- `crates/tool-registry/src/permission_loader.rs` (novo): carrega o `PermissionSet` da cadeia assistant/project/user, faz a interseção, retorna o `effective_permission_set`. ~200 linhas. Bump atômico com o `PermissionSet::merge` que ganha método novo.
- `crates/storage/migrations/0028_profiles.sql` (novo, Etapa 3): tabela `permission_profiles` (path, content_toml_hash, parsed_at, is_user_override). Cache da parse pra não reler TOML a cada run.
- `docs/modules/model-catalog.md` ganha §"Specialists" + bump no carimbo.
- `docs/modules/tool-registry.md §3` perde a frase "a Etapa 4 carrega o `PermissionSet` real do `assistant`/`project`/`user` antes de validar — pendente" (a pendência fechou).
- E2E em `crates/e2e/tests/e2e_specialist_registry_e2e.rs` (Etapa 3): 4 testes, todos consumindo `build_chat_orchestrator`:
  - `registry_loads_specialists_from_catalog`
  - `permission_set_inherited_from_assistant_project_user`
  - `specialist_unknown_id_returns_structured_error`
  - `effective_permission_set_is_subset_of_parent` (o invariante, no caminho real)

## Alternativas consideradas

1. **Hardcoded em código Rust** (`pub fn default_specialists() -> Vec<SpecialistDefinition>`). Rejeitado porque (a) o usuário não pode customizar sem recompilar, (b) o Modo Equipe precisa ler de uma fonte carregável em runtime, (c) a regra §9.1 fala de "registro", não de "constante".
2. **Arquivo de config único** (`~/.config/frederico/specialists.toml`, sem bundled). Rejeitado porque (a) o usuário começa sem o arquivo e não tem nenhum subagente, (b) o Modo Equipe fica vazio no primeiro launch, (c) o bundled é o "default razoável" e o override é a customização.
3. **Servidor de registry** (subagentes vêm de uma API). Rejeitado porque (a) Frederico é desktop-first, sem dependência de servidor (mesma razão do `PROMPT MESTRE` §15 — "sincronização entre dispositivos é não-objetivo"), (b) adiciona ponto único de falha.
4. **Permissões por subagente, sem cadeia assistant/project/user** (cada `SpecialistDefinition` carrega seu `PermissionSet` direto). Rejeitado porque (a) perde a granularidade do `PROMPT MESTRE` §8 (permissões globais ∩ perfil ∩ assistant ∩ projeto ∩ agente pai ∩ subagente ∩ execução ∩ aprovação), (b) o `PermissionSet::is_subset_of` da Fase 3 já modela a interseção, é trabalho de Etapa 3 exercitar.
5. **Etapa 3 com UI completa do Modo Equipe**. Rejeitado porque (a) UI do Modo Equipe é trabalho coerente com a UI do Pipeline (Etapa 6), (b) misturar UI de subagente com UI de pipeline no mesmo PR viola a regra de PRs pequenas.

## Pendências

- **UI completa do Modo Equipe** (Etapa 6 da Fase 6): sidebar com especialista, modelo, objetivo, dependências, ferramentas, progresso, custo, resultado, erros. Esta ADR entrega o backend consumível + o componente base; a Etapa 6 fecha a UI.
- **Customização de `allowed_tools` por chamada**: hoje, o `SpecialistDefinition.allowed_tools` é fixo. Permitir que o pai passe um `allowed_tools_override` por spawn é trabalho de fase futura (Fase 8 — Copiloto, refino).
- **Migração de `PermissionSet` parseado de TOML pra binário**: performance. Default é parse-once-com-cache, mas se o TOML for grande ou se o cache invalidar, o parse pode ficar caro. Bench fica pra depois.
- **Versionamento de `default.toml`**: o `default.toml` bundled tem `version: "1"`. Mudanças incompatíveis exigem migração (campo novo = adicionar, não renomear). Política fica pra fase futura.
- **Profile sync entre devices** (não-objetivo explícito do `PROMPT MESTRE` §15): profiles são locais. Sem nuvem.

## Histórico de revisão

- 2026-08-05 — versão inicial. Decisão da Etapa 1 da Fase 6. Pendência da Fase 3 Etapa 4 ("PermissionSet real do assistant/project/user antes de validar") entra como D3 desta ADR — é a mesma peça de trabalho, e isolar em ADR separado inflaria a contagem sem ganho.
