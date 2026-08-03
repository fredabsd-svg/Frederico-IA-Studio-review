//! Composição do Frederico IA Studio: catálogo de ferramentas,
//! permissões iniciais e (no commit 5 da Etapa 1 da Fase de
//! Ligação) o construtor do `ChatOrchestrator`.
//!
//! ## Por que um módulo `composition` separado
//!
//! Cada função de composição é **pura** (sem I/O de DB, sem
//! estado global, sem efeitos colaterais além do que é
//! explicitamente injetado). Isso permite:
//!
//! - **Testes determinísticos** — o test pode passar um `Vec`
//!   de tools e verificar o `ToolRegistry` resultante sem
//!   precisar de `Database`, `EventSink`, etc.
//! - **Reuso no modo servidor §5.5** (VPS / headless) — o
//!   `build_chat_orchestrator` recebe as dependências
//!   injetadas e roda em qualquer runtime `tokio` com
//!   acesso ao DB e à rede. Sem fork.
//! - **Mesma função para a casca e para os E2E** (regra do
//!   prompt da Fase de Ligação) — a Etapa 5 dos E2E
//!   `tests/e2e/` importa `frederico_app::build_chat_orchestrator`
//!   em vez de tentar `use` em `apps/desktop/src-tauri` (que
//!   é binário).
//!
//! Ver ADR-0022 §D1 e §D4.

use std::sync::Arc;

use frederico_tool_registry::{
    DocumentPermission, FileReadPermission, PermissionSet, Tool, ToolRegistry,
};

/// Constrói o `ToolRegistry` da casca + dos testes E2E.
///
/// Itera sobre `tool.manifest()` e registra cada manifesto no
/// `ToolRegistry`. **Garante que toda tool concreta tenha seu
/// manifesto** (o método `manifest()` é obrigatório na trait
/// `Tool`, e o `ToolRegistry::register` aceita `ToolManifest`
/// direto) — elimina a divergência "manifesto à mão vs. tool
/// real" do §5.2 do projeto anterior.
///
/// Não deduplica: se a mesma tool é passada duas vezes (por
/// engano, num vetor), o `ToolRegistry::register` substitui
/// o manifesto existente (comportamento do
/// `HashMap::insert` — ver
/// `crates/tool-registry/src/registry.rs`). Isso é
/// intencional: a Etapa 6 da Fase 3 (UI de configuração)
/// pode precisar ligar/desligar ferramentas mantendo
/// a versão atual do manifesto.
///
/// `tools` pode ser vazio — devolve um `ToolRegistry` vazio
/// (o que faz a Etapa 1 antes do `docs.generate` ser
/// registrado na Etapa 2).
#[must_use]
pub fn build_tool_registry(tools: &[Arc<dyn Tool>]) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for tool in tools {
        let manifest = tool.manifest().clone();
        registry.register(manifest);
    }
    registry
}

/// `PermissionSet` carregado de configuração fixa e explícita
/// na Etapa 1 da Fase de Ligação.
///
/// A Etapa 6 da Fase 3 (UI de configuração) substitui esta
/// função por uma que carrega do storage — até lá, **a casca
/// e o modo servidor §5.5 usam o mesmo conjunto fixo**, sem
/// `PermissionSet::default()` (que seria deny-all —
/// bloqueava o `files.read` no Passo 5 do
/// `validate_tool_call`).
///
/// ## Decisões da Etapa 1
///
/// - `file_read: WorkspaceOnly` — habilita o `files.read`
///   dentro do workspace da conversa (jail por conversa,
///   Etapa 1 commit 4a). `WorkspacePlusApproved` é
///   equivalente hoje, mas `WorkspaceOnly` deixa claro que
///   não há UI de aprovação ainda (Etapa 6 da Fase 3).
/// - Todo o resto **deny** — incluindo `documents: None`. O
///   `documents` é bumpado pra `DocumentPermission::Full`
///   **junto com o registro** de `docs.generate`/`docs.inspect`
///   na Etapa 2 (commit que fecha o catálogo de ferramentas
///   default), no mesmo commit (bump atômico do ADR-0020
///   §3 D3: capability + permissão atômicas).
/// - `destructive_ops: false` — exige aprovação explícita
///   mesmo quando o resto estiver liberado (regra do
///   `tool-permission-model.md`).
#[must_use]
pub fn initial_permission_set() -> PermissionSet {
    PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        // Etapa 1: documents = None (default deny). Bump atômico
        // pra DocumentPermission::Full entra no commit da Etapa 2
        // que registra docs.generate + docs.inspect.
        documents: DocumentPermission::None,
        // Demais campos idênticos ao `Default::default()` —
        // explicitados para tornar a decisão auditável.
        ..PermissionSet::default()
    }
}

/// Agrupa os argumentos do construtor do `ChatOrchestrator`
/// em um struct. Existe para que a casca Tauri e o modo
/// servidor §5.5 chamem `build_chat_orchestrator(parts)` em
/// vez de passar 12 args posicionais (decisão registrada na
/// conversa da Etapa 1: "campo na struct, não parâmetro
/// de `new()`").
///
/// **O `build_chat_orchestrator(parts)` é o ponto de entrada
/// único** para construir o `ChatOrchestrator` (Etapa 1
/// commit 5 da Fase de Ligação). Os campos refletem os 12
/// args do `ChatOrchestrator::new` (Etapa 4.x.y da Fase 3
/// + `permission_set` da Etapa 1).
#[allow(dead_code)] // Ativado quando o `build_chat_orchestrator` for consumido.
pub struct ChatOrchestratorParts {
    /// `Arc<ProviderMap>` com adapters pré-registrados.
    pub providers: Arc<frederico_provider_engine::ProviderMap>,
    /// `Arc<RunRegistry>` (registro de runs em andamento).
    pub runs: Arc<frederico_provider_engine::RunRegistry>,
    /// `Arc<dyn EventSink>` (sink de eventos, hoje
    /// `TauriEventSink` em produção).
    pub sink: Arc<dyn frederico_provider_engine::EventSink>,
    /// `Arc<Database>`.
    pub db: Arc<frederico_storage::Database>,
    /// `Arc<dyn Clock>` (SystemClock em produção,
    /// FakeClock em testes).
    pub clock: Arc<dyn frederico_security::Clock>,
    /// `Arc<Catalog>` (catálogo de modelos).
    pub catalog: Arc<frederico_model_catalog::Catalog>,
    /// `ToolRegistry` (catálogo de ferramentas). Construído
    /// via `build_tool_registry(tools)`.
    pub tool_registry: ToolRegistry,
    /// `Arc<dyn JailResolver>` (resolução de jail por
    /// conversa). `FileSystemJailResolver` em produção.
    pub jail_resolver: Arc<dyn frederico_tool_registry::JailResolver>,
    /// `Vec<Arc<dyn Tool>>` (tools concretas —
    /// `FilesReadTool` etc.). O `build_tool_registry` itera
    /// sobre as mesmas para registrar os manifestos.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Allowlist default de `ToolId` por execução.
    pub allowed_for_run: Vec<frederico_core::ToolId>,
    /// `PermissionSet` carregado de configuração fixa.
    /// Construído via `initial_permission_set()`.
    pub permission_set: PermissionSet,
    /// `Option<Arc<MemoryExtractorHandle>>` (Fase 4 Etapa 5).
    /// `None` desabilita classificação automática de
    /// memórias; `Some(h)` enfileira um `MemoryExtractionJob`
    /// após o run finalizar.
    pub memory_extractor: Option<Arc<frederico_memory::MemoryExtractorHandle>>,
}

/// Constrói o `ChatOrchestrator` a partir de `parts`.
///
/// Esta é a **única** forma de construir o `ChatOrchestrator`
/// na Fase de Ligação (Etapa 1 commit 5). A casca Tauri e o
/// modo servidor §5.5 chamam esta função em vez de passar 12
/// args posicionais pro `ChatOrchestrator::new` direto
/// (decisão registrada na conversa da Etapa 1: "campo na
/// struct, não parâmetro de `new()`"). **Mesma função para
/// a casca e para os E2E** (regra do prompt da Fase de
/// Ligação) — os testes E2E da Etapa 5 (`tests/e2e/`) usam
/// esta função, garantindo que a forma do contrato é a
/// mesma em todos os pontos.
///
/// Ver ADR-0022 §D4.
#[must_use]
pub fn build_chat_orchestrator(
    parts: ChatOrchestratorParts,
) -> frederico_execution_engine::orchestrator::ChatOrchestrator {
    frederico_execution_engine::orchestrator::ChatOrchestrator::new(
        parts.providers,
        parts.runs,
        parts.sink,
        parts.db,
        parts.clock,
        parts.catalog,
        parts.tool_registry,
        parts.jail_resolver,
        parts.tools,
        parts.allowed_for_run,
        parts.permission_set,
        parts.memory_extractor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: tool `files.read` real (a única do catálogo
    /// default da Etapa 1). Útil para verificar que o
    /// `build_tool_registry` registra o manifesto correto.
    fn sample_tool() -> Arc<dyn Tool> {
        Arc::new(frederico_tool_registry::FilesReadTool::new())
    }

    #[test]
    fn build_tool_registry_empty_returns_empty() {
        let registry = build_tool_registry(&[]);
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn build_tool_registry_with_one_tool_registers_manifest() {
        let tool = sample_tool();
        let registry = build_tool_registry(&[tool]);
        assert_eq!(registry.len(), 1);
        let manifest = registry
            .get(&frederico_core::ToolId::new("files.read"))
            .expect("manifesto de files.read registrado");
        assert_eq!(manifest.id, frederico_core::ToolId::new("files.read"));
    }

    #[test]
    fn build_tool_registry_with_same_tool_twice_does_not_dedupe() {
        // `ToolRegistry::register` faz `HashMap::insert` —
        // substitui em vez de duplicar. O `len()` reflete o
        // número de `ToolId`s distintos, não o número de
        // tools passadas.
        let tool = sample_tool();
        let registry = build_tool_registry(&[tool.clone(), tool]);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn initial_permission_set_enables_files_read_only() {
        let p = initial_permission_set();
        // `files.read` habilitado dentro do workspace.
        assert_eq!(p.file_read, FileReadPermission::WorkspaceOnly);
        // Todo o resto deny.
        assert!(!p.file_create);
        assert!(!p.file_modify);
        assert!(!p.file_delete);
        assert_eq!(p.documents, DocumentPermission::None);
        assert!(!p.credentials);
        assert!(!p.web_browse);
        assert!(!p.web_download);
        assert!(!p.network);
        assert!(!p.screen_capture);
        assert!(!p.input_control);
        assert!(!p.destructive_ops);
    }

    #[test]
    fn initial_permission_set_differs_from_default() {
        // O `PermissionSet::default()` é tudo-deny (regra do
        // `tool-permission-model.md` §"Invariantes"). O
        // `initial_permission_set()` da Etapa 1 difere apenas
        // em `file_read` (WorkspaceOnly vs. None) e em
        // `documents` (None em ambos, mas explícito). Esta
        // diferença é o que evita o deny-all hardcoded que
        // bloqueava o `files.read` no Passo 5 do
        // `validate_tool_call` antes desta fase.
        let p = initial_permission_set();
        let d = PermissionSet::default();
        assert_ne!(p.file_read, d.file_read);
        // Outros campos idênticos.
        assert_eq!(p.file_create, d.file_create);
        assert_eq!(p.documents, d.documents);
    }
}
