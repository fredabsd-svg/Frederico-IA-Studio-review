<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 3 (Etapas 2 e 3)
-->

# `frederico-tool-registry`

Tool Registry, manifesto de ferramentas, validação 10-passos, jail,
`files.read` in-process e `PermissionSet` (Fase 3, Etapas 2 e 3).

## 1. O que este módulo faz

É a **fonte única da verdade** sobre ferramentas (spec
`tool-registry-specification.md` §"Tool Registry"). A Etapa 2 entrega
o manifesto, o registry, o jail, a validação 10-passos e a
`files.read` in-process. A Etapa 3 entrega o `PermissionSet` e a
interseção com o Passo 5 do `validate_tool_call`.

**Etapa 2:**

- O tipo `ToolManifest` e o builder fluente `ToolManifestBuilder` —
  o contrato do spec §"Contrato do manifesto" (22 campos: id,
  namespace, version, schemas, capabilities, risk_level, permissões,
  plataformas, modos de provedor, timeout, availability, etc.).
- A enum `JsonSchema` com validação via `jsonschema` crate
  (biblioteca canônica, mesma do TypeScript com `ajv`).
- A `ToolRegistry` — registro in-memory com `register`/`get`/`all`
  e a função `effective_tools(allowed_for_run)` (a interseção do
  spec §"Interseção de inventário por execução", filtrada por
  `availability` + `health` + allowlist).
- A `Jail` — o ponto único de normalização de caminhos. Rejeita
  `..` (path traversal), caminhos absolutos, UNC, letra de unidade
  diferente e symlinks apontando pra fora (a defesa contra a
  ameaça I3 do `security-threat-model.md`).
- `validate_tool_call` — a função que implementa os 10 passos do
  spec §"Validação antes de execução". Devolve `Approved` (pode
  executar), `ApprovalRequired` (precisa da UI) ou `Rejected`
  (erro estruturado, sem fallback silencioso).
- `ApprovalRequest` / `ApprovalDecision` / `ApprovalScope` — o
  modelo de aprovação que a UI da Etapa 6 consome.
- A trait `Tool` e `ToolResult` — a interface que as ferramentas
  concretas implementam.
- A `FilesReadTool` — a única ferramenta do catálogo inicial. Lê
  arquivo do workspace, com paginação via `max_bytes` (default
  1 MB, máximo 50 MB) e jail aplicado.

**Etapa 3 (permissões):**

- `PermissionSet` — 18 campos do spec `tool-permission-model.md`
  §"Contrato" (`file_read`, `file_create/modify/delete`,
  `terminal`, `python`, `node`, `git`, `github`, `web_browse`,
  `web_download`, `network`, `screen_capture`, `input_control`,
  `memory`, `credentials`, `documents`, `destructive_ops`).
  **Default é deny** (spec §"Invariantes").
- Enums com ordem canônica: `FileReadPermission` (None /
  WorkspaceOnly / WorkspacePlusApproved), `RuntimePermission`
  (None / ReadOnly / Sandboxed / Unrestricted),
  `GitPermission`, `GitHubPermission`, `MemoryPermission`,
  `DocumentPermission` — todos com `PartialOrd` pra `is_subset_of`.
- `PermissionSet::is_subset_of(&parent) -> bool` — invariante
  **"subagente ⊆ pai"** (Fase 6, modelado na Etapa 3). Booleanos:
  `!self.x || parent.x`; enums: `self <= parent` via
  `PartialOrd`; `file_read`: matriz de combinações.
- `PermissionSet::allow_all()` — helper da Etapa 4 (UI modal de
  "Permitir tudo").
- `ValidationContext::permissions` + `parent_permissions: Option<Box<PermissionSet>>`.
- `ValidationContext::check_subagent_invariant()` — Passo 5 do
  `validate_tool_call` valida o invariante subagente.

## 2. O que ele expõe

**Tipos centrais (Etapa 2):**

- `ToolManifest`, `ToolManifestBuilder`, `JsonSchema`.
- `RiskLevel` (Safe / Moderate / High / Critical).
- `ToolCategory` (Files / Exec / Web / GitHub / Memory / Docs / Brasil).
- `ProviderMode` (NativeTools / TextEmulation).
- `Platform` (Windows / Macos / Linux).
- `Availability` (Available / Disabled / Missing / Unhealthy).
- `WorkerHealth` (Ok / Degraded / Unhealthy).

**Permissões (Etapa 3):**

- `PermissionSet` (Default deny; `allow_all()`; `is_subset_of`).
- `FileReadPermission` (None / WorkspaceOnly / WorkspacePlusApproved).
- `RuntimePermission` (None / ReadOnly / Sandboxed / Unrestricted).
- `TerminalMode` (None / RequireApproval / Denylist / Allowlist).
- `GitPermission`, `GitHubPermission`, `MemoryPermission`,
  `DocumentPermission`.

**Registro (Etapa 2):**

- `ToolRegistry` com `new`, `register`, `get`, `all`, `len`,
  `is_empty`, `set_health`, `health`, `effective_tools`.

**Workspace + Jail (Etapa 2):**

- `Workspace` trait (raiz do workspace).
- `Jail` com `new`, `root`, `resolve`, `resolve_allowing_nonexistent`.

**Validação (Etapas 2 e 3):**

- `validate_tool_call(ctx, call) -> ValidationOutcome`.
- `ToolCall`, `ValidationContext`, `ValidationOutcome` (Approved |
  ApprovalRequired | Rejected).

**Aprovação (Etapa 2):**

- `ApprovalRequest`, `ApprovalDecision`, `ApprovalScope` (Once / Run /
  Project).

**Ferramentas (Etapa 2):**

- `Tool` trait, `ToolResult`.
- `FilesReadTool` (in-process, com jail aplicado no `execute`).

**Erros (Etapa 2):**

- `ToolError` (com `code: ToolErrorCode` + `tool_id` + `message` +
  `details`).
- `ToolErrorCode` com 11 variantes canônicas (`TOOL_NOT_FOUND`,
  `TOOL_VERSION_MISMATCH`, `TOOL_UNAVAILABLE`,
  `TOOL_NOT_IN_INVENTORY`, `TOOL_PERMISSION_DENIED`,
  `TOOL_SCHEMA_INVALID`, `TOOL_JAIL_VIOLATION`,
  `TOOL_LIMITS_EXCEEDED`, `TOOL_APPROVAL_REQUIRED`,
  `TOOL_AUDIT_FAILED`, `TOOL_EXECUTION_FAILED`).

## 3. De quem depende e quem depende dele

**Depende de:**

- `frederico-core` — `ToolId` (Etapa 1), `AssistantId` (Etapa 1),
  `RunId` (não usado ainda).
- `frederico-agent-engine` — não é usado ainda (a integração com
  a máquina de estados é Etapa 4). Listado como dependência
  preditiva.
- `serde`, `serde_json`, `chrono`, `thiserror`, `tracing` —
  utilitários.
- `jsonschema = "0.18"` (default-features = false) — a biblioteca
  canônica de validação de JSON Schema em Rust. Spec §7.1.

**Quem depende dele (hoje):**

- Ninguém ainda. A Etapa 4 (integração) vai fazer
  `frederico-provider-engine` depender dele para o `RunExecutor`
  construir a interseção de inventário (`effective_tools`) que
  vira o `tools:` enviado ao provedor.

**Quem vai depender dele (próximas etapas):**

- `frederico-agent-engine` (Etapa 4) — o executor traduz
  `ApprovalRequired` em `RunState::WaitingUserApproval`.
- Casca Tauri (Etapa 6) — a UI consome `ApprovalRequest` e
  `PermissionSet` no modal de aprovação.
- Casca Tauri (Etapa 6) — `ToolManifest.all()` lista ferramentas
  no painel de configuração.

## 4. Decisões não óbvias e armadilhas conhecidas

- **A Etapa 2 só tem `files.read`.** O spec §7.11 lista
  `files.read / files.write / files.list / files.edit`,
  `exec.python / exec.node / exec.shell`, `web.search / web.open`,
  `brasil.cnpj`, `github.clone / commit / push / pull_request`,
  `memory.save / memory.search`, `docs.generate / docs.inspect`.
  A Etapa 2 entrega **uma** ferramenta profunda em vez de várias
  rasas: a regra "uma ferramenta profunda vale mais que duas
  rasas" (decisão registrada na conversa da Etapa 1).
  `files.write`/`files.list`/`files.edit` entram na Etapa 4.
- **`files.read` é in-process, sem worker sidecar.** O backend
  sandboxed (Fase 5) substitui a implementação sem trocar o
  manifesto. A interface pública (`Tool::execute`, `ToolResult`,
  schema do manifesto) é estável.
- **Jail é o ponto único de normalização.** O spec do
  `software-architecture.md` §"Invariantes" diz
  "Nenhum caminho de arquivo é construído concatenando strings em
  mais de um lugar. Tudo passa por `AppPaths::resolve(LogicalPath)`.
  O `Jail::resolve` é esse ponto. Ferramentas concretas
  (`FilesReadTool::execute`) re-aplicam o jail **defesa em
  profundidade** — o validador já rodou, mas o `execute` pode ser
  chamado direto em testes, e o segundo jail é barato.
- **`LimitsExceeded` existe mas não é exercitado pela Etapa 2.**
  O schema do `files.read` já tem `maximum: 52428800` (50 MB) e
  o teste documenta que o **schema é o gate primário** de
  limites. A Etapa 4 reintroduz a checagem de `LimitsExceeded` em
  runtime se houver limites que não cabem no schema (timeoutMs,
  contadores). O `ToolErrorCode::LimitsExceeded` continua no enum.
- **`PermissionSet` modela o **default deny** do spec §"Invariantes":
  "Default é deny: ferramenta perigosa nasce desligada; ligar é
  decisão consciente." A Etapa 4 (integração) carrega o
  `PermissionSet` real do `assistant`/`project`/`user` antes de
  validar. A Etapa 3 modela o tipo, a interseção hierárquica
  (subagente ⊆ pai) e a checagem do Passo 5 do `validate_tool_call`
  (rejeição por `file_read == None`).
- **`FileReadPermission::WorkspacePlusApproved` ainda não tem
  comportamento especial na Etapa 3.** A Etapa 2 (Jail estrito)
  garante que paths fora do workspace são rejeitados antes do
  Passo 5. A Etapa 6 (UI) consome `WorkspacePlusApproved` para
  mostrar o modal de aprovação quando o usuário tenta ler pasta
  autorizada do PC.
- **`PermissionSet::is_subset_of` cobre os 18 campos com regras
  explícitas.** Booleanos: `!self.x || parent.x`. Enums: ordem
  do `PartialOrd` (None < ... < Unrestricted). `file_read`:
  matriz (None é sempre OK; WorkspaceOnly ≤ WorkspaceOnly ou
  WorkspacePlusApproved; WorkspacePlusApproved ≤
  WorkspacePlusApproved). `terminal.mode`: ordem simples
  (None < RequireApproval < Denylist/Allowlist). A Etapa 6
  introduz a checagem fina de patterns (denylist/allowlist)
  se necessário.
- **`TerminalPermission` struct vs `TerminalMode` enum.** O spec
  original listava `TerminalPermission { None | RequireApproval
  | Denylist(Vec<String>) | Allowlist(Vec<String>) }`. A Etapa
  3 modela só o enum `TerminalMode` (sem patterns). A Etapa 4
  (ou um hardeings) reintroduz o struct com `Vec<String>` quando
  o `Jail::resolve` for estendido para filtrar por patterns.
- **Lint `#![warn(missing_docs)]`** (em vez de `deny`) na Etapa 2.
  O crate tem 84 warnings de docs em builders/impl helpers. É
  trabalho de um hardeings; registrado como pendência. A Etapa 3
  e o primeiro hardeings da Fase 3 promovem pra `deny`.
- **`#![allow(missing_docs)]` na Etapa 2** (decisão de escopo
  pra fechar a Etapa 2 com `clippy -D warnings`). Pendência de
  hardeings.
- **`ValidationOutcome::Approved.manifest` e `Rejected` usam
  `Box<ToolManifest>` / `Box<ToolError>`.** Clippy
  `large_enum_variant` — variantes grandes são `Box`-ed para
  manter o enum raso.
- **O `PermissionSet` da Etapa 3 não tem `Default` implícito.**
  `Default::default()` retorna "tudo deny", que é o que
  `ValidationContext::new()` consome. A Etapa 4 carrega o
  `PermissionSet` real do `assistant`/`project`/`user` antes
  de validar (camadas do spec `tool-permission-model.md`
  §"Hierarquia").

## 5. Como testá-lo isoladamente

```pwsh
# Suíte do crate (unit tests, 56 testes)
cargo test -p frederico-tool-registry

# Cobertura por área:
#   - manifest.rs: builder, JsonSchema, enums (7 testes)
#   - registry.rs: register, get, all, effective_tools (5 testes)
#   - workspace.rs: resolve, jail (8 testes incluindo symlink)
#   - permission.rs: default deny, allow_all, is_subset_of, random
#     pair invariant (15 testes)
#   - validate.rs: 10 passos + subagente invariant (14 testes)
#   - tools/files_read.rs: happy path, jail, paginação (10 testes)
#   - approval.rs: builder (2 testes)
#   - error.rs: error codes, builder (2 testes)

# Verificação completa
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
```

## 6. O que ele **não** faz

- **Não conhece provedores, modelos, conversas, runs.** O
  `ToolRegistry` é agnóstico de onde a chamada veio. A
  integração com o `provider-engine` (converter `ToolManifest` no
  `tools:` do provedor, validar com o que o modelo "viu") é da
  Etapa 4.
- **Não executa ferramentas de verdade.** Bem, `Tool::execute` é
  o contrato, e `FilesReadTool::execute` implementa. Mas o
  `ToolRegistry` em si não chama — quem chama é o executor da
  Etapa 4.
- **Não carrega o `PermissionSet` real.** A Etapa 3 entrega o
  **tipo**; a Etapa 4 carrega o real (das camadas global /
  perfil / assistente / projeto / agente pai / subagente /
  execução / aprovação do usuário, do spec §"Hierarquia").
- **Não tem healthcheck ativo dos workers.** O `WorkerHealth`
  existe como tipo, mas é default `Ok` na Etapa 2 (todas as
  ferramentas são in-process). Etapa 5.
- **Não persiste nada em disco.** Estado do registry é
  in-memory. A Etapa 4 (ou um hardeings) introduz persistência do
  manifesto via migração numerada, pra que ferramentas possam ser
  habilitadas/desabilitadas sem re-deploy.
- **Não tem `ProviderToolAdapter`** (Etapa 4) — converter
  `ToolManifest` no formato `tools:` de cada provedor.
- **Não tem `Tool` registry para workers sidecar** (Fase 5+).
- **Não tem hot-reload de manifesto.** `register` substitui
  in-memory; a Etapa 4 define o ciclo de migração.
- **Não tem UI de configuração.** A Etapa 6 consome
  `ToolRegistry.all()` pra listar e
  `ToolManifest::set_availability` (ainda não existe) pra
  habilitar/desabilitar.
- **Não tem `ToolError` com i18n.** A Etapa 4 (integração com
  `ChatOrchestrator`) consome `ToolError` e formata a
  `ProviderErrorView` em PT-BR com ação (spec
  `chat-and-providers.md`).
- **Não tem `FileReadPermission::WorkspacePlusApproved` ativo.**
  A Etapa 3 modela o tipo, mas o comportamento "leitura fora do
  workspace via aprovação" é da Etapa 6 (UI) e Etapa 7 (modo
  desenvolvedor, que define quais pastas são "autorizadas do
  PC").
