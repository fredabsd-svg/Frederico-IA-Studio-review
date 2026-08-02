<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-01
Fase correspondente: Fase de Ligação (entre Fase 5 e Fase 6)
-->

# `frederico-app`

Camada de composição do Frederico IA Studio. Detém **o que é
montar o app** (catálogo de ferramentas, permissões iniciais,
resolvedor de jail por conversa, construção do `ChatOrchestrator`)
e **nada do que é rodar a UI**. A casca Tauri continua sendo a
casca — o `frederico-app` é o que ela importa.

## 1. O que este módulo faz

Centraliza a composição para que:

- a casca Tauri (`apps/desktop/src-tauri`) e os E2E da raiz
  (`tests/e2e/`, Etapa 5 da Fase de Ligação) consumam as
  **mesmas funções** de construção (regra do prompt da fase:
  "os testes usam a mesma função da casca");
- o modo servidor do §5.5 (VPS / headless) reaproveite
  `build_chat_orchestrator` sem fork — o crate é puro por
  construção (sem `tauri`, sem `windows`), e isso é a decisão
  registrada no ADR-0022 §D1.

A Etapa 1 da Fase de Ligação entrega:

- [`build_tool_registry(tools)`](../decisions/0022-jail-resolver-v1.md):
  itera sobre `tool.manifest()` e registra cada manifesto no
  `ToolRegistry`. Garante que **toda tool concreta tenha seu
  manifesto** (o método é obrigatório na trait `Tool`), eliminando
  a divergência "manifesto à mão vs. tool real" do §5.2.
- [`initial_permission_set()`](../decisions/0022-jail-resolver-v1.md):
  `PermissionSet` carregado da configuração fixa. Etapa 1:
  `file_read: WorkspaceOnly`, todo o resto deny (incluindo
  `documents: None`). `documents` é bumpado pra
  `DocumentPermission::Full` na Etapa 2, no mesmo commit do
  registro de `docs.generate`/`docs.inspect` (bump atômico
  do ADR-0020 §3 D3).
- [`JailResolver`](../decisions/0022-jail-resolver-v1.md) trait +
  `FileSystemJailResolver` (default): resolve jail por
  `ConversationId`, com `mkdir -p` idempotente e erro duro
  (sem fallback pra `temp_dir`).
- [`build_chat_orchestrator(parts)`](../decisions/0022-jail-resolver-v1.md):
  monta o `ChatOrchestrator` real (sem `PermissionSet::default()`
  hardcoded, sem `Jail::new(current_dir)`).

A Etapa 7 (modo desenvolvedor) substituirá o
`FileSystemJailResolver` por `SecurityJailResolver` via
`frederico-security` (Job Objects + AllowVolumeAccess). A troca
não exige mudanças no `ChatOrchestrator` nem no `RunExecutor` —
a interface do trait é estável.

## 2. O que ele expõe

**Composição (Etapa 1):**

- `pub fn build_tool_registry(tools: &[Arc<dyn Tool>]) -> ToolRegistry`
- `pub fn initial_permission_set() -> PermissionSet`
- `pub struct FileSystemJailResolver { workspaces_root: PathBuf }`
  com `impl JailResolver for FileSystemJailResolver`
- `pub trait JailResolver: Send + Sync { fn resolve(&self, conversation_id: &ConversationId) -> Jail; }`
- `pub fn build_chat_orchestrator(parts: ChatOrchestratorParts) -> ChatOrchestrator`
  (a struct `ChatOrchestratorParts` agrupa os 11 args que a
  Etapa 4.x.y espalhou no `new()` — entrada/saída coesa, mesmo
  construtor continua sendo 11 args internamente)

**Testes públicos (Etapa 1):**

- `build_tool_registry` registra todos os manifestos de tools
  não-vazias; `len() == tools.len()`.
- `initial_permission_set` tem `file_read == WorkspaceOnly`,
  `documents == DocumentPermission::None`, todo o resto deny.
- `FileSystemJailResolver::resolve(cid)` cria `workspaces/<cid>/`
  no `tempdir` (test fixture), aceita paths dentro, rejeita
  paths absolutos e `..` (regressão do §I3 do threat model).

## 3. De quem depende e quem depende dele

**Depende de:**

- `frederico-core` (tipos fundamentais, `ConversationId`).
- `frederico-tool-registry` (trait `Tool`, `ToolRegistry`,
  `ToolManifest`, `PermissionSet`, `FilesReadTool` — usado
  pela composição).
- `frederico-storage` (construção do `ChatOrchestrator` exige
  `Arc<Database>`).
- `frederico-security` (apenas o trait `Clock`; nenhuma
  dependência de DPAPI/Windows — `CredentialStore` é injetado
  pela casca via `parts`).
- `frederico-execution-engine` (`ChatOrchestrator`,
  `recovery::spawn_recover_stale_runs`).
- `frederico-provider-engine` (`ProviderMap`, `RunRegistry`,
  `EventSink`).
- `frederico-model-catalog` (`Catalog`).
- `frederico-memory` (`MemoryExtractorHandle`).
- `frederico-diagnostics` (init de logs — só se a casca
  delegar; na Etapa 1 a casca mantém seu próprio init).
- `tokio`, `tracing`, `thiserror`, `async-trait`,
  `std::path::PathBuf` (utilitários).

**Quem depende dele (Etapa 1):**

- `apps/desktop/src-tauri` (casca Tauri) — usa
  `build_tool_registry`, `initial_permission_set`,
  `FileSystemJailResolver::new(workspaces_root)`,
  `build_chat_orchestrator`.

**Quem vai depender dele (Etapas seguintes):**

- `tests/e2e/` na raiz (Etapa 5 da Fase de Ligação) —
  importam as mesmas funções que a casca.
- Modo servidor §5.5 — `build_chat_orchestrator` é o ponto
  de entrada headless.

## 4. Decisões não óbvias e armadilhas conhecidas

- **Puro por construção** (ADR-0022 §D1): sem `tauri`, sem
  `windows`. Se alguém "simplificar" adicionando `tauri` ao
  `Cargo.toml` deste crate, o gate
  `scripts/check-core-purity.ps1` quebra o build. A
  pureza é verificada — não é promessa.
- **Erro duro no `mkdir`**: `FileSystemJailResolver::resolve`
  propaga `std::io::Error` se o `create_dir_all` falhar. **Não**
  tem fallback pra `temp_dir` (análise da conversa da Etapa 1:
  degradação silenciosa num caminho de isolamento é o tipo
  de bug que a Fase de Ligação existe pra eliminar).
- **Sem I/O no `build_*`**: `build_tool_registry` e
  `initial_permission_set` são funções puras. `build_chat_orchestrator`
  é onde a I/O de inicialização (carregar providers, criar
  `EventSink`) acontece — mas isolada nessa função pra
  permitir testes determinísticos das outras.
- **Convenção de nome do workspace**: `workspaces_root.join(cid.to_string())`.
  `cid.to_string()` é o UUID em formato hyphenated, mesmo
  formato do `frederico_core::ConversationId::0`
  (`uuid::Uuid::to_string`). Compatível com `ConversationRepo::delete`
  que não mexe no filesystem hoje (a Etapa 1 não precisa
  deletar workspace; isso é trabalho da Etapa 6 com a UI de
  configuração).
- **Reuso do `Workspace` trait do `frederico-tool-registry`**:
  o `JailResolver` devolve um `Jail` (que usa o `Workspace`
  internamente). Não há trait novo de "resolvedor de jail" no
  `tool-registry` — o `JailResolver` mora no `frederico-app`
  porque é decisão de composição, não de toolkit.
- **`#[non_exhaustive]` no `ToolContext`**: ver ADR-0022 §D3.
  Acrescentar campo no contexto não é nova quebra.

## 5. Como testá-lo isoladamente

```pwsh
# Suíte do crate (unit tests, ~5 testes na Etapa 1)
cargo test -p frederico-app

# Cobertura por área (Etapa 1):
#   - registry.rs: build_tool_registry com tools vazias e com
#     FilesReadTool; len() == tools.len(); manifestos batem
#     com tool.manifest()
#   - permissions.rs: initial_permission_set tem file_read ==
#     WorkspaceOnly e documents == None; default() vs.
#     initial_permission_set() divergem nos campos ligados
#   - jail.rs: FileSystemJailResolver resolve cria dir;
#     paths dentro aceitos; paths absolutos e ".." rejeitados
#     (regressão §I3); mkdir falha → erro duro propagado

# Verificação completa
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
```

## 6. O que ele **não** faz

- **Não conhece a UI**. Não tem nenhum tipo de Tauri, nenhum
  evento de janela, nenhum `Window::emit`. É composição pura.
- **Não conhece Windows**. Não usa `windows` crate, não chama
  DPAPI, não lê `Credential Manager`. A casca passa um
  `Arc<dyn CredentialStore>` já construído via `parts`.
- **Não persiste configuração**. `initial_permission_set()` é
  fixo por enquanto; a UI de configuração (Fase 3, Etapa 6) é
  trabalho de fase futura.
- **Não substitui o `frederico-security`**. O `JailResolver` da
  Etapa 1 é a versão mínima correta (filesystem + jail); o
  resolvedor definitivo (Job Objects + AllowVolumeAccess) é
  Etapa 7.
- **Não tem healthcheck**. O `WorkerHealth` no `ToolRegistry`
  é default `Ok` — o `set_health` continua sendo trabalho da
  Etapa 4 ou de um hardeings (registrado como pendência
  herdada do `tool-registry`).
- **Não tem hot-reload**. `build_tool_registry` roda uma vez
  no startup; reload de manifesto é trabalho de Etapa futura.
