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

use frederico_model_catalog::{
    registry::list_summaries as registry_list_summaries, DefaultSpecialistRegistry,
    SpecialistDefinition, SpecialistId, SpecialistRegistry, SpecialistSummary,
};
use frederico_tool_registry::{
    DocumentPermission, FileReadPermission, PermissionSet, RuntimePermission, Tool, ToolRegistry,
};

// ============================================================================
// Especialistas: build_specialist_registry (Etapa 3 da Fase 6, ADR-0030)
// ============================================================================
//
// Constrói o `SpecialistRegistry` que o `ListSpecialists` Tauri
// command (Etapa 3) consome. **A Etapa 3 não invoca subagentes**
// — o registry só é consultado pelo UI pra listar disponíveis.
// A Etapa 4 (`SubagentRunner`) é quem consome o registry pra
// resolver `SpecialistId → ModelId + BudgetAllocation + allowed_tools`.
//
// **Por que `Arc<dyn SpecialistRegistry>` em vez de `DefaultSpecialistRegistry`
// direto:** mesmo princípio do `WorkerInvoker` (ADR-0024) e do
// `JailResolver` (ADR-0022) — trait no contrato, impl específica
// injetada. Permite mock em testes e registry de arquivo de
// projeto no futuro, sem mexer no `ListSpecialists`.
//
// **Por que recebe `Catalog`:** o `ListSpecialists` precisa
// resolver o `default_model` de cada especialista pra devolver as
// capabilities que o modelo tem (badge "tem tools", "tem visão"
// na UI). A resolução é `Catalog::find_model` — feito aqui uma
// vez pra não duplicar no comando.

/// Constrói o `Arc<dyn SpecialistRegistry>` default (bundled mais override) já pareado
/// com o `Catalog` do app pra resolver capabilities por `default_model`. Retorna também o
/// helper `list_summaries_with_catalog` que o Tauri command chama (devolve um vetor de
/// `SpecialistSummary` com capabilities resolvidas).
///
/// **Por que retorna uma struct e não só o `Arc<dyn>`:** o
/// Tauri command precisa das duas coisas — o registry (pra
/// `get`/`list`) e o `Catalog` (pra resolver capabilities por
/// `default_model`). Empacotar no mesmo `SpecialistBundle`
/// garante que os dois ficam sincronizados (mesma `Arc` do
/// `Catalog`, sem cópia).
pub struct SpecialistBundle {
    pub registry: Arc<dyn SpecialistRegistry>,
    pub catalog: Arc<frederico_model_catalog::Catalog>,
}

impl SpecialistBundle {
    /// Lista os summaries com capabilities do `default_model`
    /// resolvidas via catálogo. O `default_model` que não está
    /// no catálogo vira capability `[]` (lista vazia) — o
    /// `ListSpecialists` Tauri command inclui o especialista
    /// mesmo assim (o ID é válido) e a UI mostra o badge
    /// "modelo default não resolvido" baseado no tamanho do
    /// vetor.
    ///
    /// **Por que warning e não erro:** o registry já validou
    /// o ID; o `default_model` é só metadata. Hard-fail aqui
    /// quebraria o `ListSpecialists` se o catálogo evoluir
    /// e remover um modelo que um especialista bundled ainda
    /// referencia. Degradação declarada.
    pub fn list_summaries(&self) -> Vec<SpecialistSummary> {
        registry_list_summaries(&*self.registry, |def: &SpecialistDefinition| {
            resolve_default_model_capabilities(&self.catalog, def)
        })
    }

    /// Wrapper de conveniência pro Tauri command: pega um
    /// `SpecialistId` e devolve o `SpecialistDefinition` ou
    /// `RegistryError::UnknownSpecialist { valid }`. A UI da
    /// Etapa 6 (Modal Equipe) consome o `valid` pra renderizar
    /// a lista de disponíveis.
    pub fn get(
        &self,
        id: &SpecialistId,
    ) -> Result<&SpecialistDefinition, frederico_model_catalog::RegistryError> {
        self.registry.get(id)
    }

    /// Wrapper de conveniência: valida um ID (string crua, como
    /// vem do modelo) e devolve o `SpecialistId` canônico ou
    /// erro estruturado. O `SubagentRunner` da Etapa 4 vai
    /// chamar isso antes de delegar (defesa em profundidade —
    /// o `get` também checa, mas validar uma vez no boundary
    /// é mais barato).
    pub fn validate_id(
        &self,
        id: &str,
    ) -> Result<SpecialistId, frederico_model_catalog::RegistryError> {
        self.registry.validate_id(id)
    }
}

/// Constrói o `SpecialistBundle` default. Carrega o
/// `DefaultSpecialistRegistry` (bundled + override) e pareia
/// com o `Catalog` recebido.
///
/// **Por que recebe `Catalog` por `Arc` (não constrói um
/// novo):** a casca Tauri já constrói o `Catalog` no
/// `AppState` e passa pro `ChatOrchestrator`. Reusar a mesma
/// `Arc` garante que o `ListSpecialists` e o orchestrator
/// enxergam o mesmo catálogo (se a UI mudar o catálogo em
/// runtime — Etapa futura — o command e o orchestrator
/// atualizam juntos).
#[must_use]
pub fn build_specialist_registry(
    catalog: Arc<frederico_model_catalog::Catalog>,
) -> SpecialistBundle {
    let registry: Arc<dyn SpecialistRegistry> = Arc::new(DefaultSpecialistRegistry::load());
    SpecialistBundle { registry, catalog }
}

/// Resolve as capabilities (como `Vec<String>`) do
/// `default_model` de um `SpecialistDefinition`. Se o modelo
/// não está no catálogo, devolve `vec![]` (capability_tags
/// vazia → UI mostra "modelo não resolvido"). O nome da
/// capability segue o shape do `Catalog::find_model` —
/// `tools`, `json_mode`, `parallel_tool_calls`, `vision`,
/// `prompt_caching`, `reasoning_content`.
///
/// **Heurística de provedor:** o `default_model` no TOML é
/// só o `ModelId` (ex.: `"gpt-4o"`). O `Catalog::find_model`
/// precisa de `(ProviderId, ModelId)`. Procuramos em todos
/// os provedores — o primeiro match vence. Pra Fase 6, o
/// `default_model` é resolvido em runtime quando o
/// subagente de fato roda (Etapa 4); aqui só precisamos
/// do `CapabilitySet` pro badge da UI. Match em qualquer
/// provedor é suficiente (capabilities são equivalentes
/// entre provedores que oferecem o mesmo modelo —
/// `gpt-4o` na openai e na openrouter tem o mesmo
/// `CapabilitySet`).
fn resolve_default_model_capabilities(
    catalog: &frederico_model_catalog::Catalog,
    def: &SpecialistDefinition,
) -> Vec<String> {
    let model_id = def.default_model.as_str();
    // Tenta match em todos os provedores — `find_model` exige
    // ProviderId específico, mas a Etapa 3 só precisa saber
    // **se** o modelo existe com capabilities (badge da UI).
    // Listamos todos e filtramos pelo ModelId.
    let descriptor = catalog
        .models()
        .iter()
        .find(|m| m.model.as_str() == model_id);
    match descriptor {
        Some(d) => {
            let mut caps: Vec<String> = d
                .capabilities
                .capabilities
                .iter()
                .map(|c| capability_to_string(*c))
                .collect();
            caps.sort();
            caps
        }
        None => {
            tracing::warn!(
                specialist.id = %def.id.as_str(),
                specialist.default_model = %model_id,
                "default_model do especialista não encontrado no catálogo. \
                 ListSpecialists vai devolver capability_tags vazia. \
                 Atualize o specialists.toml ou o catálogo."
            );
            Vec::new()
        }
    }
}

fn capability_to_string(c: frederico_model_catalog::Capability) -> String {
    use frederico_model_catalog::Capability;
    match c {
        Capability::Tools => "tools".into(),
        Capability::JsonMode => "json_mode".into(),
        Capability::ParallelToolCalls => "parallel_tool_calls".into(),
        Capability::Vision => "vision".into(),
        Capability::PromptCaching => "prompt_caching".into(),
        Capability::ReasoningContent => "reasoning_content".into(),
    }
}

// ============================================================================
// Memória: MemoryConfig + build_completion_provider + build_embedding_provider
//          + build_memory_extractor (Etapa 3 da Fase de Ligação)
// ============================================================================

/// Configuração da camada de memória (Fase 4). **Default sensato**:
/// classificador LLM habilitado com `gpt-4o-mini` via OpenRouter;
/// embeddings via `text-embedding-3-small` (1536 dim). A Etapa 6
/// da Fase 3 (UI de configuração) substitui por leitura de
/// storage; até lá, **a casca e o modo servidor §5.5 usam o
/// mesmo default**, sem `MemoryConfig::default()` ficar
/// desabilitado.
///
/// ## Degradação declarada (regra do PR #25 / memória cross-project)
///
/// `build_completion_provider` e `build_embedding_provider` recebem
/// `api_key: Option<SecretString>` da casca. Se a casca passou
/// `Some(key)` (DPAPI ou env var), os providers reais são
/// construídos. Se passou `None`, **warning é logado** e o
/// fallback `NoopCompletionProvider` / `NoopEmbeddingAdapter` é
/// usado — o classificador LLM vira noop, o retriever vira
/// lexical-only. **A UI mostra o diagnóstico** (a Etapa 6 da
/// Fase 3 vai adicionar o Settings panel; até lá, o log do
/// app é a única indicação).
///
/// A regra: **nunca substituição silenciosa** (memória do PR #25).
/// O sistema é explicitamente classificador/embedding real
/// **se** a key está disponível, e explicitamente noop/lexical
/// **se** não está. Sem farsa de "ligado e quebrado" (mesma
/// lição do PR #25 — defaults fail-open escondem o que nunca
/// funcionou).
pub struct MemoryConfig {
    /// Liga/desliga o classificador LLM pós-resposta. Default
    /// `true` (a Etapa 3 da Fase de Ligação). A Etapa 6 da
    /// Fase 3 (UI) vai expor isso no Settings.
    pub classifier_enabled: bool,
    /// Modelo do classificador. Default `openai/gpt-4o-mini`
    /// (OpenRouter gateway).
    pub classifier_model: String,
    /// Modelo de embedding. Default `openai/text-embedding-3-small`
    /// (OpenAI, 1536 dim).
    pub embedding_model: String,
    /// Dimensões do embedding (deve bater com `embedding_model`).
    pub embedding_dimensions: usize,
    /// Base URL do gateway. Default OpenRouter.
    pub base_url: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            classifier_enabled: true,
            classifier_model: "openai/gpt-4o-mini".into(),
            embedding_model: "openai/text-embedding-3-small".into(),
            embedding_dimensions: 1536,
            base_url: "https://openrouter.ai/api/v1".into(),
        }
    }
}

/// Constrói o `Arc<dyn CompletionProvider>` pro classificador de
/// memória. Se `api_key` é `Some`, constrói
/// `OpenRouterCompletionProvider`. Se `None`, loga warning
/// explícito e cai pra `NoopCompletionProvider` (degradação
/// declarada — classificador vira noop).
///
/// **Por que a casca passa a key, não busca ela mesma:** o
/// `frederico-app` é puro (sem dependência Windows, ADR-0003 +
/// `scripts/check-core-purity.ps1`). O `CredentialStore` do
/// DPAPI vive na casca (`apps/desktop/src-tauri`). A casca
/// busca, o `app` recebe. Mesma divisão do resto da
/// composição.
#[must_use]
pub fn build_completion_provider(
    cfg: &MemoryConfig,
    api_key: Option<secrecy::SecretString>,
) -> Arc<dyn frederico_memory::classifier::CompletionProvider> {
    match api_key {
        Some(key) => Arc::new(
            frederico_memory::classifier::OpenRouterCompletionProvider::new(key)
                .with_model(&cfg.classifier_model)
                .with_base_url(&cfg.base_url),
        ),
        None => {
            tracing::warn!(
                memory.classifier = "openrouter",
                classifier_model = %cfg.classifier_model,
                "OpenRouter API key ausente (DPAPI + env var): \
                 classificador de memoria opera em modo Noop. \
                 Captura automatica desabilitada. \
                 Retrieval fica lexical-only (sem semantica)."
            );
            Arc::new(frederico_memory::classifier::NoopCompletionProvider)
        }
    }
}

/// Constrói o `Arc<dyn EmbeddingProvider>` pro retriever híbrido.
/// Se `api_key` é `Some`, constrói `OpenRouterEmbeddingAdapter`.
/// Se `None`, loga warning e cai pra `NoopEmbeddingAdapter`
/// (retriever vira lexical-only — `HybridRetriever` já trata
/// transparentemente, ver `docs/modules/memory.md`).
#[must_use]
pub fn build_embedding_provider(
    cfg: &MemoryConfig,
    api_key: Option<secrecy::SecretString>,
) -> Arc<dyn frederico_memory::embedding::EmbeddingProvider> {
    match api_key {
        Some(key) => Arc::new(
            frederico_memory::embedding::OpenRouterEmbeddingAdapter::new(key)
                .with_model(&cfg.embedding_model, cfg.embedding_dimensions)
                .with_base_url(&cfg.base_url),
        ),
        None => {
            tracing::warn!(
                memory.embedding = "openrouter",
                embedding_model = %cfg.embedding_model,
                "OpenRouter API key ausente (DPAPI + env var): \
                 retriever opera em modo Noop (lexical-only). \
                 Buscas por similaridade semantica desabilitadas."
            );
            Arc::new(frederico_memory::embedding::NoopEmbeddingAdapter)
        }
    }
}

/// Constrói o `MemoryExtractor` da casca. Se `cfg.classifier_enabled`
/// é `false`, retorna `None` (memória desabilitada). Senão, monta
/// o `LlmMemoryClassifier` com o completion provider real (ou
/// noop) e inicia o worker em background via `tokio::spawn`.
///
/// O extractor roda em background e processa jobs do canal
/// `mpsc` (256, sem `tokio::time::interval` — ADR-0014 §1). É
/// o portão de **captura automática** da memória: cada run que
/// termina enfileira um `MemoryExtractionJob` (Fase 4 Etapa 5),
/// o worker classifica via LLM, e persiste via `MemoryRepo`.
///
/// **Retorna `Some(Arc<MemoryExtractorHandle>)` para caller
/// passar pro [`ChatOrchestratorParts::memory_extractor`]** —
/// o `ChatOrchestrator` enfileira o job automaticamente quando
/// o run termina (regra do ADR-0012 §1 — fora do caminho
/// crítico).
#[must_use]
pub fn build_memory_extractor(
    db: &frederico_storage::Database,
    cfg: &MemoryConfig,
    api_key: Option<secrecy::SecretString>,
) -> Option<Arc<frederico_memory::worker::MemoryExtractorHandle>> {
    if !cfg.classifier_enabled {
        tracing::info!(
            memory.classifier = "disabled",
            "classificador de memoria desabilitado por MemoryConfig::classifier_enabled"
        );
        return None;
    }
    let completion = build_completion_provider(cfg, api_key);
    let classifier: Arc<dyn frederico_memory::classifier::MemoryClassifier> = Arc::new(
        frederico_memory::classifier::LlmMemoryClassifier::new(completion),
    );
    let extractor = frederico_memory::worker::MemoryExtractor::start(db.pool(), classifier);
    Some(Arc::new(extractor.handle()))
}

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

/// `PermissionSet` para casca com `exec_deps` Some (sandbox
/// + runtimes Python/Node disponíveis). **Bump atômico** do
///   `python/node: None → Sandboxed` (Etapa 4/5+ da Fase 7,
///   ADR-0036).
///
/// A casca Tauri deve chamar esta função **somente se** o
/// [`ExecDeps`] foi construído com sucesso (sandbox + runtimes
/// prontos). Se o subsistema exec está indisponível,
/// [`initial_permission_set`] é o caminho certo —
/// `exec.python`/`exec.node` não entram no `ToolRegistry` (o
/// modelo não as vê), e o `runtime` fica em `None` (deny) pra
/// refletir a indisponibilidade honestamente. **Bump capability
/// + permission atômicas** — sem meia-medida.
///
/// **Etapa 5+ (2026-08-10):** reativado. A Etapa 5+ fechou a
/// path safety enforcement (Mandatory Label\Low no workdir +
/// TokenIntegrityLevel=Low no child), permitindo bumpar
/// `runtime` de `None` pra `Sandboxed` com segurança.
#[must_use]
pub fn initial_permission_set_for_exec() -> PermissionSet {
    PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        documents: DocumentPermission::None,
        // Etapa 4/5+ da Fase 7: runtime = Sandboxed (BUMP
        // atômico — vai junto com o registro de exec.python +
        // exec.node no `build_default_tools`). `Sandboxed`
        // (não `Unrestricted`) reflete o fato de o child
        // rodar sob SecurityJailResolver (path safety +
        // restricted token).
        python: RuntimePermission::Sandboxed,
        node: RuntimePermission::Sandboxed,
        ..PermissionSet::default()
    }
}

/// `PermissionSet` para casca com **ambos** os subsistemas
/// disponíveis: document-worker (Etapa 2.A) **e** exec (Etapa
/// 5+). **Bump atômico combinado** de `documents: None → Full`
/// (ADR-0023) **e** `python/node: None → Sandboxed` (Etapa 5+).
///
/// Esta função é a usada pela casca em produção (Etapa 5+)
/// — tanto o `document-worker` quanto o `exec.python`/`exec.node`
/// são dependências resolvidas no startup, então o
/// `PermissionSet` carrega ambos os bumps. É a única forma
/// de manter a regra **bump atômico capability + permission**
/// (ADR-0020 §3 D3) sem meia-medida: ou o tool está no
/// `ToolRegistry` + na allowlist + com permissão bumpada, ou
/// em nenhum dos três.
#[must_use]
pub fn initial_permission_set_for_capable_launcher_and_exec() -> PermissionSet {
    PermissionSet {
        file_read: FileReadPermission::WorkspaceOnly,
        // Etapa 2.A: documents = Full (BUMP atômico — vai
        // junto com o registro de docs.generate + docs.inspect
        // no `build_default_tools`). ADR-0020 §3 D3.
        documents: DocumentPermission::Full,
        // Etapa 5+ da Fase 7: python/node = Sandboxed. Ver
        // `initial_permission_set_for_exec` acima.
        python: RuntimePermission::Sandboxed,
        node: RuntimePermission::Sandboxed,
        ..PermissionSet::default()
    }
}

/// Constrói o `PermissionSet` **real** da cadeia
/// `user ∩ project ∩ assistant` via
/// `PermissionLoader::load_effective_permission_set`.
///
/// **Etapa 3 da Fase 6, PR 2, ADR-0030 §D3** — fecha a
/// pendência deixada pela Etapa 3 da Fase 3 Etapa 4
/// (`docs/modules/tool-registry.md §3`: "a Etapa 4 carrega
/// o `PermissionSet` real do `assistant` / `project` /
/// `user` antes de validar — pendente").
///
/// **Fail-closed (decisão de 2026-08-06 no PR 2):** o
/// `PermissionLoader` cai pro `PermissionSet::default()`
/// (deny all) quando um layer está ausente ou tem TOML
/// inválido. A `PermissionSet::merge3` é o "mais restritivo
/// vence" — campo ausente num layer **nunca herda** `true`
/// de outro layer.
///
/// **Por que um `PermissionLoader` compartilhado** (em vez
/// de construir um novo a cada chamada): o loader tem cache
/// em memória chaveado por `(path, content_hash)`. Reusar
/// a mesma instância entre runs do mesmo `AppState`
/// aproveita o cache (cada load = 1 read de disco + 1
/// parse, no pior caso). A casca Tauri guarda o loader no
/// `AppState` e o `ChatOrchestrator` consome via
/// `Arc<PermissionLoader>`.
///
/// **Por que recebe os paths, não o `PermissionLoader`:**
/// o `load_effective_permission_set` do loader é o que
/// faz o trabalho (read + parse + merge). Esta função
/// é só o **wrapper** que resolve os paths default
/// (`~/.config/...`, `./.frederico/...`) e chama o
/// loader. Tests podem passar paths custom (TempDir
/// + perfis escritos manualmente).
#[must_use]
pub fn build_default_permission_set(
    loader: &frederico_tool_registry::PermissionLoader,
    user: &std::path::Path,
    project: &std::path::Path,
    assistant: &std::path::Path,
) -> PermissionSet {
    loader.load_effective_permission_set(user, project, assistant)
}

/// Dependências do subsistema `exec.*` (Etapa 4 da Fase 7).
/// Passadas pra [`build_default_tools`] quando os runtimes
/// portáteis (Python + Node) e o sandbox estão disponíveis.
///
/// **Por que um struct em vez de args posicionais:** a
/// `build_default_tools` já tem 2 níveis de opcionalidade
/// (`invoker` + `exec_deps`); args posicionais tornariam a
/// assinatura ilegível. O struct agrupa os 3 deps do
/// subsistema exec e documenta a relação entre eles.
#[derive(Clone)]
pub struct ExecDeps {
    /// `SecurityJailResolver` (Etapa 2 da Fase 7). Em Windows
    /// cria o Job Object per-invocation e aplica o env filter;
    /// em Linux retorna `SpawnError::Unsupported` (degradação
    /// declarada — mesma regra da Etapa 2 v1).
    pub resolver: Arc<frederico_security::jail::SecurityJailResolver>,
    /// `RuntimeRegistry` (Etapa 3 da Fase 7). Hard-coda
    /// Python 3.12.4 + Node 20.16.0; o `bootstrap_all` é
    /// responsabilidade da casca (background task, com timeout).
    pub runtimes: Arc<frederico_runtimes::RuntimeRegistry>,
    /// Audit sink (Etapa 1 da Fase 3). v1 da Etapa 4 usa
    /// `NoopAuditSink`; o `DbAuditSink` (escreve em `tool_audit`)
    /// entra na Etapa 5+ (a Etapa 5 da Fase 3 fecha o Passo 10
    /// do `validate_tool_call`, que é o lugar natural pra gravar
    /// — o `Tool::execute` em si não tem `run_id`).
    pub audit: Arc<dyn frederico_tool_registry::AuditSink>,
}

/// Constrói o catálogo default de tools concretas. Retorna o
/// `Vec<Arc<dyn Tool>>` que a casca Tauri e o modo servidor
/// §5.5 vão passar pro `build_chat_orchestrator`.
///
/// ## Composição por subsistema
///
/// - `files.read` (in-process, sem deps) — sempre presente.
/// - `docs.generate` + `docs.inspect` (worker sidecar) — se
///   `invoker` é `Some` (Etapa 2.A da fase-ligação, ADR-0023).
/// - `exec.python` + `exec.node` (Etapa 4 da Fase 7) — se
///   `exec_deps` é `Some` (sandbox + runtimes disponíveis).
///
/// ## Degradação declarada (regra do PR #25 / memória cross-project)
///
/// Cada subsistema é **fail-soft**: se a dep não está
/// disponível, as tools daquele subsistema **não** entram no
/// `Vec`. Consequência: o modelo não as vê no schema (não
/// pode tentar invocar algo que não existe); o `RunExecutor`
/// rejeita com `ToolNotAllowed` se aparecer no tool_call
/// (defesa em profundidade). O log da casca mostra qual
/// subsistema está indisponível — nunca substituição silenciosa.
///
/// ## Bump atômico capability + permission
///
/// - `invoker.is_some()` → `initial_permission_set_for_capable_launcher`
///   (bumpa `documents: None → Full`).
/// - `exec_deps.is_some()` → `initial_permission_set_for_exec`
///   (bumpa `python/node: None → Sandboxed` no `PermissionSet`,
///   Etapa 4 da Fase 7).
/// - Ambos `Some` → `initial_permission_set_for_capable_launcher_and_exec`
///   (combina os dois bumps).
///
/// A casca é quem passa o `PermissionSet` correto (regra de
/// consistência: a mesma `Option<...>` vai pra
/// `build_default_tools` e `build_default_allowed_for_run`).
#[must_use]
pub fn build_default_tools(
    invoker: Option<Arc<dyn frederico_core::WorkerInvoker>>,
    exec_deps: Option<ExecDeps>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(frederico_tool_registry::FilesReadTool::new()),
        // `files.list` (Etapa 5 do Phase 7, ADR-0035): in-process,
        // Safe, sem runtime/invoker — sempre disponível. Mesmo
        // padrão do `files.read`: o `Jail` vem do `ToolContext`
        // por chamada.
        Arc::new(frederico_tool_registry::FilesListTool::new()),
        // `files.write` (Etapa 5 do Phase 7, ADR-0035): in-process,
        // Moderate, **requer aprovação do usuário** (Passo 9 do
        // `validate_tool_call`). Atômico (temp + rename no mesmo
        // dir + fsync), backup `.bak` em overwrite, audit com
        // `before_sha256`/`after_sha256` (D6). Sempre disponível —
        // não depende de runtime/invoker (escrita é in-process).
        Arc::new(frederico_tool_registry::FilesWriteTool::new()),
        // `files.edit` (Etapa 5 do Phase 7, ADR-0035 D4): in-process,
        // Moderate, **requer aprovação do usuário** (Passo 9 do
        // `validate_tool_call`). Find/replace literal, atômico
        // (reusa o protocolo de files.write), backup `.bak`,
        // recusa se `expected_sha256` não bate (defesa contra
        // race read-modify-write — o modelo não corrompe
        // arquivo silenciosamente). Preserva indentação do
        // `find` na linha. Sempre disponível — não depende de
        // runtime/invoker.
        Arc::new(frederico_tool_registry::FilesEditTool::new()),
    ];

    // --- Subsistema documentos (Etapa 2.A) ------------------------
    if let Some(invoker) = invoker {
        // Runtime disponível. Constrói o `KitRegistry` com os
        // 3 kits (WordPro + ExcelPro + PDFPro, todos
        // implementados desde a Fase 5) + o
        // `WorkerToolDispatcher` apontando pro `invoker` (que
        // pode ser `WorkerHandle` real ou `DocumentWorkerLauncher`
        // lazy, indistinguíveis pelo trait).
        //
        // **Bump atômico do capability + permission** (ADR-0020
        // §3 D3): a allowlist do `build_default_allowed_for_run`
        // também inclui os 2 `ToolId`s novos. A casca chama
        // ambas as funções com a **mesma** `Option` — quando
        // o invoker é `Some`, os 2 tools aparecem no
        // `ToolRegistry` E na allowlist; quando é `None`,
        // nenhum dos dois aparece. **Bump atômico.**
        //
        // **Por que `Arc<dyn WorkerInvoker>` em vez do
        // `LauncherDispatcher` wrapper (como o comentário
        // anterior sugeria):** a Etapa 2.B introduziu o
        // trait `WorkerInvoker` no `core` (ADR-0024). O
        // `WorkerHandle` (Fase 5) e o `DocumentWorkerLauncher`
        // (Etapa 2.A) **ambos** implementam o trait. O
        // wrapper `LauncherDispatcher` que o comentário
        // anterior previa não é mais necessário — o trait
        // faz o papel. Construção mais simples, sem
        // redundância.
        let wordpro = Arc::new(frederico_document_kits::WordProKit::new(invoker.clone()));
        let excelpro = Arc::new(frederico_document_kits::ExcelProKit::new(invoker.clone()));
        let pdfpro = Arc::new(frederico_document_kits::PdfProKit::new(invoker.clone()));

        let mut registry = frederico_document_kits::KitRegistry::new();
        registry.register(wordpro);
        registry.register(excelpro);
        registry.register(pdfpro);
        let registry = Arc::new(registry);

        // **Bump atômico do path safety (Fase de Ligação Etapa
        // 5.X — patch-allowed-paths):** a Etapa 3 da Fase 5
        // deixou o `WorkerToolDispatcher::allowed_paths` vazio
        // e a barreira de path safety desligada. A correção
        // (commit `feat: dispatcher recebe allowlist por
        // chamada; docs.generate valida contra o jail da
        // conversa`) mudou a API: a allowlist é passada por
        // chamada a partir do `ctx.jail.root_canonical()` no
        // `Tool::execute` do `docs.generate`. Aqui só
        // construímos o dispatcher — sem allowlist.
        let dispatcher = frederico_tool_registry::WorkerToolDispatcher::new(invoker);

        tools.push(Arc::new(frederico_document_kits::DocsGenerateTool::new(
            registry.clone(),
            dispatcher.clone(),
        )));
        tools.push(Arc::new(frederico_document_kits::DocsInspectTool::new(
            dispatcher,
        )));
    }

    // --- Subsistema exec (Etapa 4 da Fase 7) ---------------------
    //
    // Bump atômico: se `exec_deps` é `Some`, o `Vec` recebe
    // `exec.python` + `exec.node` E a allowlist do
    // `build_default_allowed_for_run` inclui os 2 `ToolId`s.
    // A casca chama ambas as funções com a **mesma** `Option`
    // (mesma regra do `invoker`).
    if let Some(exec) = exec_deps {
        // 3 deps compartilhados entre Python e Node via
        // `FilesExecToolBase`. Constrói via
        // `build_default_exec_tools` que esconde o detalhe.
        let exec_tools = frederico_tool_registry::exec::build_default_exec_tools(
            exec.resolver,
            exec.runtimes,
            exec.audit,
        );
        tools.extend(exec_tools);
    }

    tools
}

/// Allowlist default de `ToolId`s por execução. Retorna o
/// `Vec<ToolId>` que a casca Tauri e o modo servidor §5.5
/// vão passar pro `build_chat_orchestrator`.
///
/// Mesma regra do [`build_default_tools`]:
///
/// - `invoker.is_some()` → inclui `docs.generate` + `docs.inspect`
///   na allowlist (o `RunExecutor` aceita invocação).
/// - `exec_deps.is_some()` → inclui `exec.python` + `exec.node`
///   na allowlist (Etapa 4 da Fase 7).
/// - Qualquer `None` → **não** inclui o `ToolId` correspondente
///   (o `RunExecutor` rejeita invocação com `ToolNotAllowed`,
///   mesmo que o modelo tente via prompt injection — defesa
///   em profundidade).
///
/// `files.read` é sempre incluído (Etapa 1).
///
/// **Bump atômico:** a casca **deve** passar a **mesma**
/// `Option` que passou pra `build_default_tools` (regra de
/// consistência: `invoker` Some → `documents: Full` no
/// `PermissionSet`; `exec_deps` Some → `python/node: Sandboxed`
/// no `PermissionSet` — bumps atômicos).
#[must_use]
pub fn build_default_allowed_for_run(
    invoker: Option<Arc<dyn frederico_core::WorkerInvoker>>,
    exec_deps: Option<&ExecDeps>,
) -> Vec<frederico_core::ToolId> {
    let mut allowed = vec![
        frederico_core::ToolId::new("files.read"),
        // `files.list` (Etapa 5 do Phase 7) sempre incluído
        // (read-only, mesma família do `files.read`).
        frederico_core::ToolId::new("files.list"),
        // `files.write` (Etapa 5 do Phase 7) sempre incluído —
        // a aprovação é por invocação (Passo 9 do validador),
        // não por estar fora/não na allowlist. Sem `files.write`
        // na allowlist, o `RunExecutor` rejeita invocação mesmo
        // se o modelo tentar (defesa em profundidade contra
        // prompt injection).
        frederico_core::ToolId::new("files.write"),
        // `files.edit` (Etapa 5 do Phase 7) — mesma regra de
        // `files.write` (aprovação por invocação, allowlist
        // sempre inclui).
        frederico_core::ToolId::new("files.edit"),
    ];

    if invoker.is_some() {
        allowed.push(frederico_core::ToolId::new("docs.generate"));
        allowed.push(frederico_core::ToolId::new("docs.inspect"));
    }

    if exec_deps.is_some() {
        // Etapa 4/5+ da Fase 7: o `PermissionSet::python` e
        // `PermissionSet::node` devem ser `Sandboxed` quando o
        // subsistema exec está disponível. A casca cuida do
        // bump atômico via `initial_permission_set_for_exec`
        // (Etapa 5+) ou `initial_permission_set_for_capable_launcher_and_exec`
        // (quando o document-worker também está disponível).
        allowed.push(frederico_core::ToolId::new("exec.python"));
        allowed.push(frederico_core::ToolId::new("exec.node"));
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
    /// `Arc<dyn SpecialistRegistry>` (Etapa 4 da Fase 6, ADR-0030).
    /// Consumido pelo `SubagentRunner` no caminho de
    /// produção. A casca Tauri constrói via
    /// `build_specialist_registry` (mesma fábrica que o
    /// `ListSpecialists` Tauri command usa).
    pub specialist_registry: std::sync::Arc<dyn frederico_model_catalog::SpecialistRegistry>,
    /// `Arc<PermissionLoader>` (Etapa 3 PR 2). Consumido pelo
    /// `SubagentRunner` pra carregar o `PermissionSet` efetivo
    /// (`merge3(user, project, assistant)`) do pai. A casca
    /// Tauri constrói via `PermissionLoader::new()` e o
    /// guarda no `AppState`.
    pub permission_loader: std::sync::Arc<frederico_tool_registry::PermissionLoader>,
    /// `Option<Arc<MultimodelOrchestrator>>` (Etapa 5 PR 2,
    /// ADR-0028). Quando `Some`, o `ChatOrchestrator` expõe
    /// `start_pipeline` + `cancel_pipeline` que delegam pro
    /// runner. `None` desabilita pipeline (modo legado —
    /// testes que não precisam de pipeline podem omitir).
    /// A casca Tauri sempre passa `Some` (a Etapa 6 da Fase 6
    /// fecha a UI que consome).
    pub multimodel_orchestrator: Option<
        std::sync::Arc<frederico_execution_engine::pipeline_orchestrator::MultimodelOrchestrator>,
    >,
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
        parts.specialist_registry,
        parts.permission_loader,
        parts.multimodel_orchestrator,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_process_architecture::FakeWorkerConfig;

    /// Helper: cria um `Arc<dyn WorkerInvoker>` real apontando
    /// pra um `FakeWorker` in-process. Não toca em disco, não
    /// spawna Python — só exercita o contrato do trait
    /// `WorkerInvoker` (ADR-0024). O `_manager` segura o
    /// `WorkerManager` vivo (o `WorkerHandle` é um `Arc`
    /// interno; sem o manager, o handle morre quando o handle
    /// é droppado — manter no escopo do test).
    async fn fake_invoker() -> Arc<dyn frederico_core::WorkerInvoker> {
        let (_manager, handle) = frederico_process_architecture::WorkerManager::spawn_in_process(
            FakeWorkerConfig::default(),
            frederico_process_architecture::WorkerSpawnConfig::default(),
        )
        .await
        .expect("spawn fake");
        Arc::new(handle)
    }

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
    fn build_default_tools_without_runtime_returns_files_read_and_files_list() {
        // Runtime indisponível e sem `exec_deps` → `Vec` contém só
        // tools in-process que não dependem de runtime:
        // `FilesReadTool` + `FilesListTool` + `FilesWriteTool` +
        // `FilesEditTool` (Etapa 5 do Phase 7, ADR-0035). ADR-0023
        // §D2: degradação declarada, não substituição silenciosa.
        // `docs.generate`/`docs.inspect` (precisam de invoker) e
        // `exec.python`/`exec.node` (precisam de exec_deps) NÃO
        // entram — o modelo não as vê. Mesma regra do exec:
        // sem `exec_deps`, `exec.*` não entram.
        let tools = build_default_tools(None, None);
        assert_eq!(tools.len(), 4);
        let ids: Vec<frederico_core::ToolId> =
            tools.iter().map(|t| t.manifest().id.clone()).collect();
        assert!(ids.contains(&frederico_core::ToolId::new("files.read")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.list")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.write")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.edit")));
        // Sem exec_deps, `exec.*` NÃO aparecem.
        assert!(!ids.contains(&frederico_core::ToolId::new("exec.python")));
        assert!(!ids.contains(&frederico_core::ToolId::new("exec.node")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_default_tools_with_invoker_returns_four_tools() {
        // Com invoker, o `Vec` contém **4** tools:
        // `FilesReadTool` + `FilesListTool` (in-process, sempre) +
        // `DocsGenerateTool` + `DocsInspectTool`. É o **bump
        // atômico do ADR-0020 §3 D3** (capability + permission
        // atômicas) — quando o invoker é `Some`, as 2 tools do
        // `document-worker` entram no schema do modelo (juntas
        // com a permissão `documents: Full` em
        // `initial_permission_set_for_capable_launcher`).
        //
        // O `Arc<dyn WorkerInvoker>` aqui é o **contrato
        // genérico** (ADR-0024) — o test usa um `FakeWorker`
        // in-process (sem Python, sem disco) só pra satisfazer
        // o trait. A integração com o `DocumentWorkerLauncher`
        // lazy é responsabilidade da casca Tauri.
        let invoker = fake_invoker().await;
        let tools = build_default_tools(Some(invoker), None);
        assert_eq!(
            tools.len(),
            6,
            "Esperado 6 tools: FilesReadTool + FilesListTool + FilesWriteTool + FilesEditTool + DocsGenerateTool + DocsInspectTool"
        );
        let ids: Vec<frederico_core::ToolId> =
            tools.iter().map(|t| t.manifest().id.clone()).collect();
        assert!(ids.contains(&frederico_core::ToolId::new("files.read")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.list")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.write")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.edit")));
        assert!(ids.contains(&frederico_core::ToolId::new("docs.generate")));
        assert!(ids.contains(&frederico_core::ToolId::new("docs.inspect")));
    }

    // O teste `build_default_tools_with_exec_deps_*` é da
    // Etapa 4 da Fase 7 e vive no PR da Etapa 4. Este branch
    // (`fase-7-etapa-5-files-write-edit`) parte de `da9e98f2`
    // (Etapa 3 merged, Etapa 4 ainda em PR). Quando este PR for
    // mergeado em `main`, o `git rebase` da Etapa 5 sobre a
    // Etapa 4 traz o teste de volta (a versão 2-arg de
    // `build_default_tools`). O rebase resolve o
    // `<<<<<<< Updated upstream` automaticamente.

    #[test]
    fn build_default_tools_with_exec_deps_returns_files_read_plus_exec() {
        // Etapa 4 da Fase 7: com `exec_deps` Some, o `Vec`
        // contém `FilesReadTool` + `exec.python` + `exec.node`
        // (3 tools, sem docs). O `exec_deps` é construído via
        // `RuntimeConfig` apontando pra um `tempfile::TempDir`
        // (instalação isolada). O bootstrap **não** é chamado
        // aqui — o test verifica apenas o shape do `Vec`
        // retornado.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let runtime_cfg = frederico_runtimes::RuntimeConfig {
            install_root: tmp.path().to_path_buf(),
            keep_n_versions: 1,
            allow_download: false, // test não baixa nada
            mirror_url: None,
            download_timeout: std::time::Duration::from_secs(1),
        };
        let runtimes = Arc::new(
            frederico_runtimes::RuntimeRegistry::new(runtime_cfg)
                .expect("RuntimeRegistry::new em tempdir"),
        );
        let resolver = frederico_security::jail::SecurityJailResolver::new(
            frederico_security::jail::SecurityJailConfig::secure_default(),
        )
        .expect("SecurityJailResolver::new");
        // `new()` já retorna `Arc<SecurityJailResolver>` —
        // não envolver em outro `Arc::new` (causaria
        // `Arc<Arc<...>>` e quebraria a invariante de
        // ownership do `ExecDeps`).
        let audit: Arc<dyn frederico_tool_registry::AuditSink> =
            Arc::new(frederico_tool_registry::NoopAuditSink);
        let exec_deps = ExecDeps {
            resolver,
            runtimes,
            audit,
        };

        let tools = build_default_tools(None, Some(exec_deps));
        assert_eq!(
            tools.len(),
            6,
            "Esperado 6 tools: files.read + files.list + files.write + files.edit + exec.python + exec.node"
        );
        let ids: Vec<frederico_core::ToolId> =
            tools.iter().map(|t| t.manifest().id.clone()).collect();
        assert!(ids.contains(&frederico_core::ToolId::new("files.read")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.list")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.write")));
        assert!(ids.contains(&frederico_core::ToolId::new("files.edit")));
        assert!(ids.contains(&frederico_core::ToolId::new("exec.python")));
        assert!(ids.contains(&frederico_core::ToolId::new("exec.node")));
        // Sem invoker, docs.generate e docs.inspect NÃO aparecem.
        assert!(!ids.contains(&frederico_core::ToolId::new("docs.generate")));
        assert!(!ids.contains(&frederico_core::ToolId::new("docs.inspect")));
    }

    #[test]
    fn build_default_allowed_for_run_without_runtime_excludes_documents() {
        // Sem runtime e sem exec_deps, allowlist contém os tools
        // in-process que sempre existem: `files.read` + `files.list`
        // + `files.write` + `files.edit` (Etapa 5 do Phase 7,
        // ADR-0035). O `RunExecutor` rejeita invocação de `docs.*`
        // e `exec.*` mesmo se o modelo tentar (defesa em
        // profundidade contra prompt injection).
        let allowed = build_default_allowed_for_run(None, None);
        assert_eq!(
            allowed,
            vec![
                frederico_core::ToolId::new("files.read"),
                frederico_core::ToolId::new("files.list"),
                frederico_core::ToolId::new("files.write"),
                frederico_core::ToolId::new("files.edit"),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_default_allowed_for_run_with_invoker_includes_documents() {
        // Com invoker, allowlist inclui os 2 `ToolId`s de
        // documentos. **Bump atômico capability + permission**
        // (ADR-0020 §3 D3): mesma `Option<Arc<dyn WorkerInvoker>>`
        // passada pra `build_default_tools` e
        // `build_default_allowed_for_run` — quando `Some`, os
        // 2 `ToolId`s aparecem em ambos; quando `None`, em
        // nenhum. A casca Tauri é quem garante a simetria.
        let invoker = fake_invoker().await;
        let allowed = build_default_allowed_for_run(Some(invoker), None);
        assert!(allowed.contains(&frederico_core::ToolId::new("files.read")));
        assert!(allowed.contains(&frederico_core::ToolId::new("files.list")));
        assert!(allowed.contains(&frederico_core::ToolId::new("files.write")));
        assert!(allowed.contains(&frederico_core::ToolId::new("files.edit")));
        assert!(allowed.contains(&frederico_core::ToolId::new("docs.generate")));
        assert!(allowed.contains(&frederico_core::ToolId::new("docs.inspect")));
    }
}
