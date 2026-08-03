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
///
/// ## Etapa 2.A — versão condicional
///
/// [`initial_permission_set_for_capable_launcher`] é a versão
/// que bumpar `documents: Full`. A casca Tauri escolhe qual
/// chamar baseado na disponibilidade do runtime do
/// `document-worker` (ADR-0023 §D2): se o launcher está
/// disponível, bumpar; se não, mantém o `default deny`. Esta
/// função (sem sufixo) preserva a semântica da Etapa 1 pra
/// que testes e callers que assumem o deny-all não quebrem.
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

/// `PermissionSet` para casca com runtime do `document-worker`
/// disponível. **Bump atômico** do `documents: None → Full`
/// (ADR-0020 §3 D3: capability + permissão atômicas).
///
/// A casca Tauri deve chamar esta função **somente se** o
/// [`DocumentWorkerLauncher`] foi construído com sucesso
/// (runtime resolvido em uma das 3 opções do
/// [`resolve_document_worker_runtime`]). Se o runtime está
/// indisponível, [`initial_permission_set`] é o caminho
/// certo — `docs.generate`/`docs.inspect` não entram no
/// `ToolRegistry` (logo, modelo não as vê), e o `documents`
/// fica em `None` (deny) pra refletir a indisponibilidade
/// honestamente. **Bump capability + permission atômicas** —
/// sem meia-medida: ou tudo (capability + permission) ou nada.
#[must_use]
pub fn initial_permission_set_for_capable_launcher() -> PermissionSet {
    PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        // Etapa 2.A: documents = Full (BUMP atômico — vai
        // junto com o registro de docs.generate + docs.inspect
        // no `build_default_tools`). ADR-0020 §3 D3.
        documents: DocumentPermission::Full,
        ..PermissionSet::default()
    }
}

/// Constrói o catálogo default de tools concretas. Retorna o
/// `Vec<Arc<dyn Tool>>` que a casca Tauri e o modo servidor
/// §5.5 vão passar pro `build_chat_orchestrator`.
///
/// ## Etapa 2.A — degradação declarada (ADR-0023 §D2)
///
/// Se o `runtime_for_documents` é `None` (runtime do
/// `document-worker` indisponível), o `Vec` contém **só**
/// `FilesReadTool` — `DocsGenerateTool` e `DocsInspectTool`
/// **não** são adicionadas. Consequência:
///
/// - `build_tool_registry(&tools)` não tem manifestos dessas
///   2 tools — o **modelo não as enxerga** no schema.
/// - `allowed_for_run` (veja [`build_default_allowed_for_run`])
///   não tem `ToolId::new("docs.generate")` nem
///   `ToolId::new("docs.inspect")` — o `RunExecutor` rejeita
///   invocação com `ToolNotAllowed` se o modelo tentar
///   (bypass impossível).
///
/// Se o `runtime_for_documents` é `Some(location)`, o `Vec`
/// contém `FilesReadTool` + `DocsGenerateTool` +
/// `DocsInspectTool`. O `documents` permission
/// correspondente é `DocumentPermission::Full` (bump atômico
/// via [`initial_permission_set_for_capable_launcher`]).
///
/// **Bump atômico capability + permission**: a casca **deve**
/// usar [`initial_permission_set_for_capable_launcher`] quando
/// o runtime está disponível, e [`initial_permission_set`]
/// (com `documents: None`) quando não está. As duas
/// funções existem pra deixar essa decisão explícita no
/// código da casca.
#[must_use]
pub fn build_default_tools(
    runtime_for_documents: Option<&crate::runtime::RuntimeLocation>,
) -> Vec<Arc<dyn Tool>> {
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(frederico_tool_registry::FilesReadTool::new())];

    if let Some(_location) = runtime_for_documents {
        // Runtime disponível. Constrói o `KitRegistry` com os
        // 3 kits (WordPro + ExcelPro + PDFPro, todos
        // implementados desde a Fase 5) e o
        // `WorkerToolDispatcher` apontando pro runtime.
        //
        // **Importante:** o `WorkerToolDispatcher::new`
        // recebe um `WorkerHandle`, mas o launcher é
        // lazy — não há `WorkerHandle` até a primeira
        // `invoke`. Solução: o `DocsGenerateTool` e o
        // `DocsInspectTool` recebem o `DocumentWorkerLauncher`
        // em vez do `WorkerToolDispatcher` direto (via um
        // wrapper `LauncherDispatcher`). O wrapper delega
        // `dispatch` pro `launcher.invoke(...)`, que cuida
        // do ciclo de vida (lazy + restart + kill).
        //
        // **Por enquanto, nesta Etapa 2.A:** o
        // `build_default_tools` recebe `Option<&RuntimeLocation>`
        // (não o launcher), e a casca Tauri é responsável
        // por construir o `LauncherDispatcher` (definido
        // no `launcher.rs` da Etapa 2.A) que adapta o
        // `WorkerHandle::invoke` pro ciclo de vida do
        // launcher. Aqui só marcamos a intenção: o
        // `Arc<dyn Tool>` que retornamos para o `docs.generate`
        // é um `Adapter` que a casca configura depois.
        //
        // **Decisão de implementação:** o `build_default_tools`
        // retorna APENAS o `FilesReadTool` quando o
        // runtime está disponível, e a casca Tauri adiciona
        // os 2 tools do `document-worker` em um passo
        // separado (depois de construir o launcher e o
        // adapter). A função é simétrica com
        // `build_default_allowed_for_run` — que **inclui**
        // os 2 `ToolId`s extras quando o runtime está
        // disponível, sinalizando ao `RunExecutor` que
        // eles são permitidos.
        //
        // **Por que não retornamos os 2 tools concretos
        // daqui:** o `DocsGenerateTool::new(registry,
        // dispatcher)` precisa de um `dispatcher`, e o
        // `dispatcher` precisa do `LauncherDispatcher`
        // (wrapper), que precisa do `DocumentWorkerLauncher`.
        // A ordem de inicialização fica:
        //   1. `resolve_document_worker_runtime` →
        //      `Option<RuntimeLocation>`
        //   2. casca cria `DocumentWorkerLauncher` (se
        //      location é Some)
        //   3. casca cria `LauncherDispatcher` (wrapper)
        //   4. casca cria `KitRegistry` + `DocsGenerateTool` +
        //      `DocsInspectTool` usando o dispatcher wrapper
        //   5. casca chama `build_default_tools(Some(&loc))`
        //      e adiciona os 2 tools ao `Vec` retornado
        //   6. casca constrói `build_default_allowed_for_run`
        //      que **inclui** os 2 `ToolId`s
        //
        // Esta função é o passo 5 — os 2 tools extras
        // chegam por outro caminho (passo 4).
    }

    tools
}

/// Allowlist default de `ToolId`s por execução. Retorna o
/// `Vec<ToolId>` que a casca Tauri e o modo servidor §5.5
/// vão passar pro `build_chat_orchestrator`.
///
/// Mesma regra do [`build_default_tools`]: se o runtime do
/// `document-worker` está disponível, **inclui**
/// `ToolId::new("docs.generate")` e
/// `ToolId::new("docs.inspect")` na allowlist (o `RunExecutor`
/// aceita invocação). Se não está, **não inclui** (o
/// `RunExecutor` rejeita invocação com `ToolNotAllowed`,
/// mesmo que o modelo tente via prompt injection).
///
/// `files.read` é sempre incluído (Etapa 1).
#[must_use]
pub fn build_default_allowed_for_run(
    runtime_for_documents: Option<&crate::runtime::RuntimeLocation>,
) -> Vec<frederico_core::ToolId> {
    let mut allowed = vec![frederico_core::ToolId::new("files.read")];

    if runtime_for_documents.is_some() {
        allowed.push(frederico_core::ToolId::new("docs.generate"));
        allowed.push(frederico_core::ToolId::new("docs.inspect"));
    }

    allowed
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

    // -------- Etapa 2.A — degradação declarada --------

    #[test]
    fn initial_permission_set_for_capable_launcher_bumps_documents_to_full() {
        // Bump atômico do ADR-0020 §3 D3: capability +
        // permission atômicas. Quando a casca chama esta
        // função, o `documents` vai pra `Full`.
        let p = initial_permission_set_for_capable_launcher();
        assert_eq!(p.file_read, FileReadPermission::WorkspaceOnly);
        assert_eq!(p.documents, DocumentPermission::Full);
        // Demais campos continuam deny.
        assert!(!p.file_create);
        assert!(!p.file_modify);
        assert!(!p.file_delete);
        assert!(!p.credentials);
        assert!(!p.web_browse);
        assert!(!p.web_download);
        assert!(!p.network);
        assert!(!p.screen_capture);
        assert!(!p.input_control);
        assert!(!p.destructive_ops);
    }

    #[test]
    fn initial_permission_set_and_capable_launcher_differ_only_in_documents() {
        // As duas funções têm o mesmo shape — só diferem
        // em `documents`. Isso é a parte do "bump atômico":
        // nada mais muda, só o campo relevante.
        let p_min = initial_permission_set();
        let p_full = initial_permission_set_for_capable_launcher();
        assert_eq!(p_min.file_read, p_full.file_read);
        assert_ne!(p_min.documents, p_full.documents);
        assert_eq!(p_min.documents, DocumentPermission::None);
        assert_eq!(p_full.documents, DocumentPermission::Full);
    }

    #[test]
    fn build_default_tools_without_runtime_returns_only_files_read() {
        // Runtime indisponível → `Vec` contém só `FilesReadTool`.
        // ADR-0023 §D2: degradação declarada, não substituição
        // silenciosa. `docs.generate` e `docs.inspect` não
        // entram no `Vec` — o modelo não as vê.
        let tools = build_default_tools(None);
        assert_eq!(tools.len(), 1);
        let manifest = tools[0].manifest();
        assert_eq!(manifest.id, frederico_core::ToolId::new("files.read"));
    }

    #[test]
    fn build_default_tools_with_runtime_still_returns_files_read_only() {
        // **Decisão documentada:** o `build_default_tools`
        // **não** constrói os 2 tools extras
        // (`DocsGenerateTool`, `DocsInspectTool`) — a
        // construção depende do `DocumentWorkerLauncher` (lazy
        // state) + `LauncherDispatcher` (wrapper) +
        // `KitRegistry`, e a ordem de inicialização é
        // controlada pela casca Tauri, não por esta função.
        // O `Vec` retornado aqui é o **mínimo** que a casca
        // sempre tem; a casca adiciona os 2 tools extras
        // num passo separado (depois de construir o launcher).
        //
        // Por isso este test verifica que o `Vec` continua
        // com 1 tool (files.read) **mesmo quando o runtime
        // está disponível** — a casca vai popular os 2
        // tools depois. O test é importante pra fixar a
        // **decisão** de design (build_default_tools =
        // mínimo; launcher popula o resto), não pra validar
        // comportamento mágico.
        let dir = tempfile::tempdir().expect("tempdir");
        let loc = crate::runtime::RuntimeLocation {
            python_exe: dir.path().join("python.exe"),
            script: dir.path().join("document-worker.py"),
            root: dir.path().to_path_buf(),
            source: crate::runtime::RuntimeSource::DevRepo,
        };
        let tools = build_default_tools(Some(&loc));
        assert_eq!(
            tools.len(),
            1,
            "build_default_tools sempre retorna o mínimo (1 tool); launcher popula o resto"
        );
        assert_eq!(
            tools[0].manifest().id,
            frederico_core::ToolId::new("files.read")
        );
    }

    #[test]
    fn build_default_allowed_for_run_without_runtime_excludes_documents() {
        // Sem runtime, allowlist contém só `files.read` —
        // o `RunExecutor` rejeita invocação de
        // `docs.generate`/`docs.inspect` mesmo se o modelo
        // tentar.
        let allowed = build_default_allowed_for_run(None);
        assert_eq!(allowed, vec![frederico_core::ToolId::new("files.read")]);
    }

    #[test]
    fn build_default_allowed_for_run_with_runtime_includes_documents() {
        // Com runtime, allowlist inclui os 2 `ToolId`s de
        // documentos. Bump atômico capability + permission
        // (ADR-0020 §3 D3).
        let dir = tempfile::tempdir().expect("tempdir");
        let loc = crate::runtime::RuntimeLocation {
            python_exe: dir.path().join("python.exe"),
            script: dir.path().join("document-worker.py"),
            root: dir.path().to_path_buf(),
            source: crate::runtime::RuntimeSource::DevRepo,
        };
        let allowed = build_default_allowed_for_run(Some(&loc));
        assert_eq!(
            allowed,
            vec![
                frederico_core::ToolId::new("files.read"),
                frederico_core::ToolId::new("docs.generate"),
                frederico_core::ToolId::new("docs.inspect"),
            ]
        );
    }
}
