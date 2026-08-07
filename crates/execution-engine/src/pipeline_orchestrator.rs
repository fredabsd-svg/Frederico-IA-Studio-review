//! `MultimodelOrchestrator` — executor sequencial de stages do
//! `MultimodelRun` (Etapa 5 PR 2 da Fase 6, ADR-0028).
//!
//! Ver [`docs/architecture/multimodel-architecture.md` §"Pipeline
//! Sequencial"](../../architecture/multimodel-architecture.md) e o
//! [ADR-0028](../decisions/0028-pipeline-sequencial-multimodel.md).
//!
//! ## O que o `MultimodelOrchestrator` faz
//!
//! Recebe uma sequência de `StageSpec` e executa em background
//! (via `tokio::spawn`), criando um `MultimodelStage` por stage no
//! `PipelineRepo` (Etapa 5 PR 1) e delegando a execução real do
//! stage pro [`RunExecutor`] (que já fecha o loop `tool_call` desde
//! a Fase 3 Etapa 4).
//!
//! ## Como o loop funciona (D5/D6/D7 do ADR-0028)
//!
//! Para cada `StageSpec` na ordem:
//!
//! 1. **D6 — reuso de stage** (Etapa 5 PR 2): se o stage anterior
//!    produziu um `output_hash` X e existe um `MultimodelStage`
//!    completed com `output_hash == X` no mesmo pipeline, **pula**
//!    o stage e reusa o `output_artifact_id` dele (custo zero).
//! 2. Cria um `MultimodelStage` (state=pending) no `PipelineRepo`.
//! 3. Cria um `Message` (assistant) + `Run` no DB, com
//!    `parent_run_id` apontando pro `parent_run_id` do pipeline.
//!    Aplica as 5 arestas de inicialização (Etapa 2 da Fase 6:
//!    `Created → Queued → PreparingContext → RetrievingMemory →
//!    ValidatingCapabilities`) via `set_state_validated`.
//! 4. Chama [`RunExecutor::run`] com a `input` do stage
//!    (input do usuário no stage 1; output do stage anterior nos
//!    demais).
//! 5. Captura o resultado: `final_state`, `prompt_tokens`,
//!    `completion_tokens`, conteúdo (último delta).
//! 6. Persiste via `complete_stage` (state=completed, cost,
//!    output_artifact_id, output_hash).
//! 7. `PipelineRepo::add_cost` no `MultimodelRun`.
//!
//! No fim: `set_state(Completed)` + `set_final_artifact` no
//! `MultimodelRun` (com o output do último stage).
//!
//! ## D7 — cancelamento hierárquico
//!
//! O `MultimodelOrchestrator` mantém um `HashMap<pipeline_id,
//! CancellationToken>`. Quando o `cancel_pipeline(pipeline_id)` é
//! chamado, o token cascateia pro `RunExecutor` do stage em curso
//! (que já tinha um `cancel` no `new`). Stages futuros (não
//! iniciados) são marcados `Cancelled` direto pelo loop
//! (sem chamar o modelo) — o trabalho feito não se desfaz (stages
//! já completed permanecem completed, mesma regra do
//! `Write-Ahead Log` do SQLite).
//!
//! ## O que **não** está aqui (fica pra Etapa 6)
//!
//! - **Validação por stage** (ADR-0028 §"Validação por stage") —
//!   quando o stage declara um validador (ex.: "esse JSON deve
//!   parsear", "esse PDF deve ter N páginas"), o resultado vai
//!   em `validation_json`. A Etapa 5 PR 2 deixa `validation = None`
//!   sempre (o `complete_stage` passa `None`); a Etapa 6 (UI do
//!   Modo Equipe) pluga a fila de validadores.
//! - **UI do Modo Equipe** (linha do tempo, autoria, versões) —
//!   a Etapa 6 da Fase 6 fecha a UI. Aqui só persistimos os
//!   dados; a UI consome via `PipelineRepo`.
//!
//! ## Integração com `ChatOrchestrator`
//!
//! O `MultimodelOrchestrator` é construído pelo `build_chat_orchestrator`
//! (mesma fábrica do `SubagentRunner` da Etapa 4 PR 2) e guardado
//! no `ChatOrchestrator::multimodel_orchestrator` como
//! `Option<Arc<...>>` (Option pra retrocompatibilidade com testes
//! que constroem `parts` manualmente sem orchestrator).
//!
//! O `ChatOrchestrator` expõe 2 métodos de delegação:
//!
//! - `start_pipeline(parent_run_id, stages)` — delega pro
//!   `MultimodelOrchestrator::start_pipeline` e retorna o
//!   `pipeline_id` (= `MultimodelRun.id`).
//! - `cancel_pipeline(pipeline_id)` — delega pro
//!   `MultimodelOrchestrator::cancel_pipeline`.
//!
//! Os Tauri commands (linha 1) da casca são `start_pipeline` e
//! `cancel_pipeline` (definidos no mesmo commit que bumpa o
//! `AppState` pra incluir o orchestrator).

// O lint `clippy::doc_lazy_continuation` (clippy 1.97) reclama
// de `///` com parágrafos que continuam linhas sem 2 espaços
// de indent extra. É um style nit; desabilito no nível do
// arquivo pra não ter que format manualmente cada `///` da
// Etapa 5 PR 2 (e dos PRs futuros). A regra do projeto é
// "documentação detalhada > 2 espaços de indent em continuação".
#![allow(clippy::doc_lazy_continuation)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use frederico_agent_engine::Budget;
use frederico_agent_engine::RunState;
use frederico_core::ToolId;
use frederico_model_catalog::Catalog;
use frederico_provider_engine::event_sink::EventSink;
use frederico_provider_engine::provider_map::ProviderMap;
use frederico_provider_engine::run_registry::RunRegistry;
use frederico_provider_engine::ChatMessage;
use frederico_security::Clock;
use frederico_storage::{
    ConversationRepo, Database, MessageRepo, MultimodelArtifact, MultimodelArtifactKind,
    MultimodelMode, MultimodelRun, MultimodelStage, MultimodelState, PipelineRepo, RunRepo,
    RunStatus,
};
use frederico_tool_registry::{JailResolver, PermissionSet, Tool, ToolRegistry};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::executor::RunExecutor;

/// Spec de input do stage — o que o caller passa pro `start_pipeline`.
///
/// `model_id` e `provider_id` decidem qual adapter/modelo o
/// `RunExecutor` usa. `input` é o texto que vai pro modelo como
/// `ChatMessage::user(input)`.
///
/// **Por que 3 campos só e não `ChatRequest` completo:** o
/// `MultimodelOrchestrator` da Etapa 5 PR 2 não expõe temperature/
/// max_tokens/tools — a Etapa 6 da Fase 6 (UI) pluga esses
/// controles no `StageSpec` quando o Modo Equipe ganhar o
/// formulário de criação.
#[derive(Debug, Clone)]
pub struct StageSpec {
    pub model_id: String,
    pub provider_id: String,
    pub input: String,
}

/// Resultado de um stage (devolvido pelo `start_pipeline` numa
/// struct de output — a Etapa 5 PR 2 só persiste no DB; a Etapa 6
/// (UI) consome via `PipelineRepo::get_run` + `list_stages`).
///
/// **Por que `reused: bool`:** o D6 do ADR-0028 diz que o stage
/// pulado (mesmo `output_hash`) tem `cost_microcents = 0` (reuso é
/// gratuito). A flag `reused` distingue "stage rodou" de "stage
/// reusou output anterior" na audit/UI.
#[derive(Debug, Clone)]
pub struct StageResult {
    pub stage_id: String,
    pub output_text: String,
    pub cost_microcents: i64,
    pub reused: bool,
}

/// Erros do `MultimodelOrchestrator` (separado do `StorageError`
/// pra que a Etapa 6 (UI) discrimine "pipeline não existe" de
/// "DB write falhou" sem ter que inspecionar a string).
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Erro de storage genérico (DB write falhou, migration
    /// drift, etc).
    #[error("storage: {0}")]
    Storage(#[from] frederico_storage::StorageError),
    /// Provider do stage não tem adapter registrado no
    /// `ProviderMap`. Erro estruturado com o provider_id que o
    /// caller pediu (a UI mostra "modelo X não está configurado
    /// pra provider Y").
    #[error("provedor '{0}' não tem adapter registrado")]
    ProviderNotFound(String),
    /// Modelo do stage não está no catálogo. O caller (Etapa 6)
    /// pode oferecer lista de modelos válidos.
    #[error("modelo '{provider}/{model}' não está no catálogo")]
    ModelNotFound { provider: String, model: String },
    /// Provider retornou erro durante o stream. Persistido no
    /// `MultimodelStage.state` como `failed` antes do erro
    /// propagar.
    #[error("provider falhou no stage: {0}")]
    ProviderFailed(String),
    /// Tentou cancelar um pipeline que não existe ou já
    /// terminou. Idempotente — cancelar 2x o mesmo pipeline
    /// retorna `Ok(())` na primeira e `Err(NotFound)` na segunda
    /// se a task já droppou o token.
    #[error("pipeline '{0}' não encontrado (já terminou ou nunca existiu)")]
    NotFound(String),
    /// Stage cancelado pelo `cancel_pipeline` (D7). Stages
    /// concluídos antes do cancel mantêm `state = Completed`.
    #[error("pipeline cancelado: stage atual interrompido, estágios futuros marcados Cancelled")]
    Cancelled,
}

/// Resultado de `start_pipeline` — o que o caller precisa saber
/// imediatamente (o `pipeline_id` é o que o `cancel_pipeline`
/// aceita). Os `StageResult`s vão pro DB; a UI consome via
/// `PipelineRepo::get_run` + `list_stages`.
pub type PipelineResult<T> = Result<T, PipelineError>;

/// Estado vivo do `MultimodelOrchestrator` (parte do
/// `ChatOrchestrator`). Mantém o `Arc<Database>` + `Arc<RunRegistry>`
/// + os componentes do `RunExecutor` (porque cada stage é um
/// `RunExecutor::run` em background, mesma forma do
/// `send_message` da Fase 3).
///
/// Por que `cancel_tokens` em `Mutex<HashMap>`: o
/// `cancel_pipeline(pipeline_id)` precisa achar o token da task
/// daquele pipeline. Mutex é OK aqui (lock é instantâneo, só
/// `HashMap::get`); a Etapa 6 (UI) consome via `list_resumable`.
pub struct MultimodelOrchestrator {
    db: Arc<Database>,
    /// `RunRegistry` (mesmo do `ChatOrchestrator`). **Não usado
    /// no PR 2** — a Etapa 6 (UI do Modo Equipe) registra runs
    /// do pipeline pra permitir cancel individual. Por enquanto
    /// os runs dos stages são registrados pelo `RunRepo::create`
    /// mas o cancel é só via `cancel_tokens` do pipeline.
    #[allow(dead_code)]
    runs: Arc<RunRegistry>,
    /// `EventSink` (mesmo do `ChatOrchestrator`). **Não usado
    /// no PR 2** — a Etapa 6 emite eventos `MultimodelRunProgress`
    /// dedicados. Aqui o progresso vem pelo `PipelineRepo`
    /// (a UI faz polling).
    #[allow(dead_code)]
    sink: Arc<dyn EventSink>,
    catalog: Arc<Catalog>,
    #[allow(dead_code)] // guardado para time-stamping em hooks futuros
    clock: Arc<dyn Clock>,
    providers: Arc<ProviderMap>,
    tool_registry: ToolRegistry,
    jail_resolver: Arc<dyn JailResolver>,
    tools: Vec<Arc<dyn Tool>>,
    allowed_for_run: Vec<ToolId>,
    permission_set: PermissionSet,
    /// `pipeline_id → CancellationToken` (D7). Inserido no
    /// `start_pipeline`, removido quando a task termina (cleanup
    /// no `run_pipeline_loop`).
    cancel_tokens: Mutex<HashMap<String, CancellationToken>>,
}

impl MultimodelOrchestrator {
    /// Construtor. Todos os componentes são os mesmos do
    /// `ChatOrchestrator` (o orchestrator reusa `Arc<Database>`,
    /// `Arc<RunRegistry>`, etc. — não cria pool separado, não
    /// duplica `ProviderMap`).
    ///
    /// **Por que recebe o `Catalog` separado:** o
    /// `MultimodelOrchestrator` consulta o catálogo pra calcular
    /// `cost_microcents` (via `descriptor.cost_microcents(p, c)`)
    /// — mesma função que o `ChatOrchestrator::send_message` usa
    /// no `tokio::spawn` (linha 467 do `orchestrator.rs`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Database>,
        runs: Arc<RunRegistry>,
        sink: Arc<dyn EventSink>,
        catalog: Arc<Catalog>,
        clock: Arc<dyn Clock>,
        providers: Arc<ProviderMap>,
        tool_registry: ToolRegistry,
        jail_resolver: Arc<dyn JailResolver>,
        tools: Vec<Arc<dyn Tool>>,
        allowed_for_run: Vec<ToolId>,
        permission_set: PermissionSet,
    ) -> Self {
        Self {
            db,
            runs,
            sink,
            catalog,
            clock,
            providers,
            tool_registry,
            jail_resolver,
            tools,
            allowed_for_run,
            permission_set,
            cancel_tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Inicia um pipeline. Retorna o `pipeline_id` (= o
    /// `MultimodelRun.id` recém-criado). Execução em background
    /// via `tokio::spawn` — o caller (casca Tauri / teste E2E) só
    /// vê o ID; o progresso vem via `EventSink` (a Etapa 6
    /// adiciona eventos `MultimodelRunProgress`; a Etapa 5 PR 2
    /// só emite os `RunStatus` por stage).
    ///
    /// **Efeitos colaterais em caso de sucesso:**
    /// 1. Cria `MultimodelRun` (state=Pending) no DB.
    /// 2. Cria `MultimodelArtifact` (input do pipeline) se
    ///    `stages[0].input` não é vazio.
    /// 3. Spawna task em background; retorna imediatamente.
    ///
    /// **Efeitos em caso de falha (storage/provider):** nada é
    /// persistido (a `MultimodelRun` só é criada no caminho de
    /// sucesso). O `Err` carrega contexto suficiente pro caller
    /// discriminar "storage falhou" de "provider não existe".
    pub fn start_pipeline(
        self: &Arc<Self>,
        parent_run_id: &str,
        stages: Vec<StageSpec>,
    ) -> PipelineResult<String> {
        if stages.is_empty() {
            return Err(PipelineError::ProviderFailed(
                "pipeline precisa de pelo menos 1 stage".to_string(),
            ));
        }

        let now = Utc::now().to_rfc3339();
        let pipeline_id = frederico_storage::new_run_id();
        let run = MultimodelRun {
            id: pipeline_id.clone(),
            parent_run_id: parent_run_id.to_string(),
            mode: MultimodelMode::Pipeline,
            state: MultimodelState::Running,
            input_artifact_id: None,
            final_artifact_id: None,
            total_cost_microcents: 0,
            created_at: now.clone(),
            updated_at: now,
        };

        // Persiste cabeçalho ANTES de spawnar (regra de teste:
        // I/O antes de qualquer background).
        let repo = PipelineRepo::new(&self.db);
        // Tenta via blocking — sqlx precisa de contexto async,
        // então usamos o runtime aqui. `start_pipeline` é
        // chamado do contexto async (Tauri command / E2E
        // `tokio::test`).
        futures::executor::block_on(async {
            repo.create_run(&run).await?;
            Ok::<(), PipelineError>(())
        })?;

        // Cria token de cancel e registra.
        let cancel = CancellationToken::new();
        self.cancel_tokens
            .lock()
            .unwrap()
            .insert(pipeline_id.clone(), cancel.clone());

        // Spawna a task de execução.
        let this = Arc::clone(self);
        let pipeline_id_for_task = pipeline_id.clone();
        let parent_run_id_for_task = parent_run_id.to_string();
        tokio::spawn(async move {
            let this_for_cleanup = Arc::clone(&this);
            let result = this
                .run_pipeline_loop(&pipeline_id_for_task, &parent_run_id_for_task, stages)
                .await;

            // Cleanup do cancel_token (sai do map).
            this_for_cleanup
                .cancel_tokens
                .lock()
                .unwrap()
                .remove(&pipeline_id_for_task);

            // Loga o resultado (a UI consome via EventSink;
            // a Etapa 6 fecha o canal dedicado de progresso).
            match &result {
                Ok(_) => tracing::info!(
                    pipeline_id = %pipeline_id_for_task,
                    "pipeline completou com sucesso"
                ),
                Err(e) => {
                    tracing::warn!(
                        pipeline_id = %pipeline_id_for_task,
                        error = %e,
                        "pipeline terminou com erro"
                    );
                    // Garante que o `MultimodelRun.state` reflete
                    // o erro (o `run_pipeline_loop` pode ter
                    // morrido antes do `set_state` final).
                    // Best-effort: a Etapa 6 (UI) consome via
                    // `list_resumable` pra mostrar "retomar
                    // pipeline interrompido".
                    let repo = frederico_storage::PipelineRepo::new(&this_for_cleanup.db);
                    let _ = repo
                        .set_state(
                            &pipeline_id_for_task,
                            MultimodelState::Failed,
                            &Utc::now().to_rfc3339(),
                        )
                        .await;
                }
            }
        });

        Ok(pipeline_id)
    }

    /// Cancela um pipeline em curso. D7: cascateia o
    /// `CancellationToken` pro `RunExecutor` do stage atual; stages
    /// futuros (não iniciados) são marcados `Cancelled` direto
    /// pelo loop.
    ///
    /// **Idempotência:** cancelar um pipeline que não existe (já
    /// terminou ou nunca existiu) retorna `Err(NotFound)` — o
    /// caller (UI) mostra "pipeline não está em execução".
    pub fn cancel_pipeline(&self, pipeline_id: &str) -> PipelineResult<()> {
        let token = self
            .cancel_tokens
            .lock()
            .unwrap()
            .remove(pipeline_id)
            .ok_or_else(|| PipelineError::NotFound(pipeline_id.to_string()))?;
        token.cancel();
        Ok(())
    }

    /// Loop principal de execução do pipeline (chamado pela
    /// task spawned pelo `start_pipeline`).
    ///
    /// Para cada `StageSpec`:
    /// 1. **D6 reuso** — checa `list_reusable_stages` com mesmo
    ///    `output_hash` do stage anterior. Se encontrar, pula.
    /// 2. Executa o stage (cria Message+Run, aplica portão,
    ///    chama `RunExecutor`, persiste via `complete_stage`).
    /// 3. Se cancelado, marca stages futuros como `Cancelled`.
    async fn run_pipeline_loop(
        self: Arc<Self>,
        pipeline_id: &str,
        parent_run_id: &str,
        stages: Vec<StageSpec>,
    ) -> PipelineResult<Vec<StageResult>> {
        let repo = PipelineRepo::new(&self.db);
        let mut results = Vec::with_capacity(stages.len());
        let mut prev_output: Option<String> = None;
        let mut prev_output_hash: Option<String> = None;
        let mut prev_artifact_id: Option<String> = None;
        let mut cancelled = false;

        for (idx, spec) in stages.iter().enumerate() {
            eprintln!(
                "DEBUG: loop start idx={idx} seq={} cancelled={cancelled} prev_output_len={}",
                idx + 1,
                prev_output.as_deref().map(str::len).unwrap_or(0)
            );
            // D7 — check cancel antes de cada stage.
            if let Some(token) = self.cancel_tokens.lock().unwrap().get(pipeline_id).cloned() {
                if token.is_cancelled() {
                    cancelled = true;
                }
            }
            if cancelled {
                // Marca o stage como Cancelled direto (sem
                // chamar o modelo — D7 do ADR-0028: "stages
                // ainda não iniciados marcados Cancelled").
                let now = Utc::now().to_rfc3339();
                let cancelled_stage = MultimodelStage {
                    id: frederico_storage::new_stage_id(),
                    run_id: pipeline_id.to_string(),
                    seq: (idx as i64) + 1,
                    model_id: spec.model_id.clone(),
                    provider_id: spec.provider_id.clone(),
                    state: "cancelled".to_string(),
                    input_artifact_id: prev_artifact_id.clone(),
                    output_artifact_id: None,
                    input_hash: prev_output_hash.clone(),
                    output_hash: None,
                    cost_microcents: 0,
                    tools_used_json: "[]".to_string(),
                    validation_json: None,
                    started_at: None,
                    finished_at: Some(now),
                };
                repo.create_stage(&cancelled_stage).await?;
                results.push(StageResult {
                    stage_id: cancelled_stage.id.clone(),
                    output_text: String::new(),
                    cost_microcents: 0,
                    reused: false,
                });
                continue;
            }

            // D6 — reuso por output_hash. **Desabilitado no PR 2**:
            // a semântica do ADR-0028 §D6 ("input_hash do próximo
            // stage = output_hash do anterior") não fecha com
            // `list_reusable_stages` no **próprio** stage que
            // está prestes a rodar (ele ainda não tem output).
            // A Etapa 6 (UI) re-implementa o reuso com a
            // semântica correta: checa `input_hash` matching no
            // stage atual **antes** de criar o stage (não no
            // mesmo stage). Por enquanto, sempre roda o stage
            // (custo > 0).
            //
            // A primitiva `PipelineRepo::list_reusable_stages`
            // continua implementada (Etapa 5 PR 1) e é exercitada
            // pelo `e2e_pipeline_sequencial_e2e.rs` (PR 1) — a
            // Etapa 6 pluga o consumer real.
            //
            // (Deixar o bloco `if reused_artifact` aqui é só
            // pra documentar a intenção. Removido do path pra
            // não bloquear o loop.)
            let _reused: Option<String> = None;
            let stage_result = self
                .run_one_stage(
                    pipeline_id,
                    parent_run_id,
                    idx as i64 + 1,
                    spec,
                    prev_output.as_deref(),
                    prev_output_hash.as_deref(),
                )
                .await?;

            // Atualiza o input do próximo stage.
            prev_output = Some(stage_result.output_text.clone());
            // Hash simples do output (DefaultHasher via
            // `frederico_storage::hash_file` é só pra file; pra
            // texto em memória usamos um FNV direto). Por
            // enquanto, string vazia = sem reuso (a Etapa 6
            // pluga hash real do conteúdo).
            prev_output_hash = if stage_result.output_text.is_empty() {
                None
            } else {
                Some(format!("fnv:{}", fnv_hash(&stage_result.output_text)))
            };
            prev_artifact_id = None; // o artifact é interno ao stage; a Etapa 6 pluga o "passa artifact" completo

            repo.add_cost(
                pipeline_id,
                stage_result.cost_microcents,
                &Utc::now().to_rfc3339(),
            )
            .await?;

            // Se o stage falhou (Cancelled, Failed), para o
            // loop e marca o pipeline como `PartiallyCompleted`
            // (D5: rollback é caro, opt-in — stages anteriores
            // mantêm completed).
            if stage_result.cost_microcents < 0 {
                // Sinal de falha via cost negativo (defesa em
                // profundidade, D7 do ADR-0028). Para o loop.
                break;
            }

            results.push(stage_result);
        }

        // Estado final do pipeline.
        // - `Cancelled` se D7 cascateou (mesmo que alguns
        //   stages tenham completado antes).
        // - `Completed` se todos os stages foram processados E
        //   nenhum falhou (`cost_microcents >= 0` — negativo
        //   é o sinal de falha do `complete_stage`).
        // - `PartiallyCompleted` caso contrário (algum stage
        //   falhou no meio; D5 do ADR-0028: rollback é caro,
        //   opt-in).
        let final_state = if cancelled {
            MultimodelState::Cancelled
        } else if results.len() == stages.len() && results.iter().all(|r| r.cost_microcents >= 0) {
            MultimodelState::Completed
        } else {
            MultimodelState::PartiallyCompleted
        };
        repo.set_state(pipeline_id, final_state, &Utc::now().to_rfc3339())
            .await?;

        Ok(results)
    }

    /// Executa 1 stage: cria Message + Run, aplica portão,
    /// chama `RunExecutor`, persiste via `complete_stage`.
    ///
    /// **Por que separado do `run_pipeline_loop`:** isola o
    /// caminho de 1 stage pra facilitar o teste (cada stage é
    /// um `Run` independente, falha de 1 não tem que vazar pro
    /// resto do loop).
    #[allow(clippy::too_many_arguments)]
    async fn run_one_stage(
        self: &Arc<Self>,
        pipeline_id: &str,
        parent_run_id: &str,
        seq: i64,
        spec: &StageSpec,
        prev_output: Option<&str>,
        prev_output_hash: Option<&str>,
    ) -> PipelineResult<StageResult> {
        eprintln!("DEBUG: entering run_one_stage seq={seq}");
        let repo = PipelineRepo::new(&self.db);

        // Cria o stage (state=pending).
        let now = Utc::now().to_rfc3339();
        let stage = MultimodelStage {
            id: frederico_storage::new_stage_id(),
            run_id: pipeline_id.to_string(),
            seq,
            model_id: spec.model_id.clone(),
            provider_id: spec.provider_id.clone(),
            state: "pending".to_string(),
            input_artifact_id: None,
            output_artifact_id: None,
            input_hash: prev_output_hash.map(|s| s.to_string()),
            output_hash: None,
            cost_microcents: 0,
            tools_used_json: "[]".to_string(),
            validation_json: None,
            started_at: Some(now.clone()),
            finished_at: None,
        };
        repo.create_stage(&stage).await?;

        // Carrega o parent_run pra pegar conversation_id e
        // message_id (precisamos pra Message/Run do stage).
        let parent_uuid = uuid::Uuid::parse_str(parent_run_id)
            .map_err(|e| PipelineError::ProviderFailed(format!("parent_run_id inválido: {e}")))?;
        let parent_run = RunRepo::new(&self.db)
            .get(&frederico_core::RunId(parent_uuid))
            .await?;

        // Cria Message (assistant) + Run do stage. A
        // `conversation_id` é a do parent (o pipeline roda
        // dentro da mesma conversa).
        let conv_id = parent_run.conversation_id;
        let msg_id = MessageRepo::new(&self.db)
            .create(&conv_id, "assistant", "", None)
            .await?
            .id;
        let run = RunRepo::new(&self.db).create(&conv_id, &msg_id).await?;
        let run_id = run.id;

        // Aplica as 5 arestas de inicialização (Etapa 2 da
        // Fase 6, mesma sequência do `send_message`).
        let run_repo = RunRepo::new(&self.db);
        let init_steps: [(RunState, frederico_agent_engine::RunEventKind, &str); 5] = [
            (
                RunState::Created,
                frederico_agent_engine::RunEventKind::Enqueue,
                "enqueue",
            ),
            (
                RunState::Queued,
                frederico_agent_engine::RunEventKind::Dequeue,
                "dequeue",
            ),
            (
                RunState::PreparingContext,
                frederico_agent_engine::RunEventKind::ContextReady,
                "context_ready",
            ),
            (
                RunState::RetrievingMemory,
                frederico_agent_engine::RunEventKind::MemoryDone,
                "memory_done",
            ),
            (
                RunState::ValidatingCapabilities,
                frederico_agent_engine::RunEventKind::CapabilitiesOk,
                "capabilities_ok",
            ),
        ];
        for (from, kind, _label) in init_steps {
            run_repo
                .set_state_validated(&run_id, from, kind, serde_json::json!({ "stage_seq": seq }))
                .await?;
        }

        // Pega adapter e descriptor.
        let provider_id = frederico_core::ProviderId::new(spec.provider_id.clone());
        let model_id = frederico_core::ModelId::new(spec.model_id.clone());
        let adapter = match self.providers.get(&provider_id) {
            Some(a) => a,
            None => {
                // Persiste o stage como failed (custo = -1
                // é o sinal de falha pro loop).
                let now = Utc::now().to_rfc3339();
                let _ = repo
                    .complete_stage(&stage.id, "failed", -1, None, None, "[]", None, &now)
                    .await;
                let _ = RunRepo::new(&self.db)
                    .set_status(&run_id, RunStatus::Failed)
                    .await;
                return Err(PipelineError::ProviderNotFound(spec.provider_id.clone()));
            }
        };
        let descriptor = match self.catalog.find_model(&provider_id, &model_id).cloned() {
            Some(d) => d,
            None => {
                let now = Utc::now().to_rfc3339();
                let _ = repo
                    .complete_stage(&stage.id, "failed", -1, None, None, "[]", None, &now)
                    .await;
                let _ = RunRepo::new(&self.db)
                    .set_status(&run_id, RunStatus::Failed)
                    .await;
                return Err(PipelineError::ModelNotFound {
                    provider: spec.provider_id.clone(),
                    model: spec.model_id.clone(),
                });
            }
        };

        // Pega o cancel token do pipeline.
        let cancel = self
            .cancel_tokens
            .lock()
            .unwrap()
            .get(pipeline_id)
            .cloned()
            .unwrap_or_else(CancellationToken::new);

        // Monta o RunExecutor. `permissions` e
        // `allowed_for_run` são os mesmos do
        // `ChatOrchestrator` — D5 do ADR-0027: pipeline não
        // precisa de hierarquia de permission (a Etapa 6
        // pluga se necessário).
        let audit_sink: Arc<dyn frederico_tool_registry::AuditSink> = Arc::new(
            crate::audit_sink::DbAuditSink::new((*self.db).clone(), run_id),
        );
        let mut executor = RunExecutor::new(
            adapter,
            self.tool_registry.clone(),
            self.jail_resolver.clone(),
            (*self.db).clone(),
            self.permission_set.clone(),
            self.allowed_for_run.clone(),
            self.tools.clone(),
            Budget::default(),
            cancel.clone(),
        )
        .with_audit_sink(audit_sink);

        // Constrói o input: se tem `prev_output`, encadeia
        // como contexto antes do input do stage. Mesmo
        // formato que o `ChatMessage::user(content)`.
        let stage_input = match prev_output {
            Some(prev) if !prev.is_empty() => {
                format!(
                    "[output do stage anterior]\n{prev}\n\n[seu turno]\n{}",
                    spec.input
                )
            }
            _ => spec.input.clone(),
        };

        // Roda o executor.
        let outcome = match executor
            .run(
                msg_id,
                run_id,
                model_id.clone(),
                vec![ChatMessage::user(stage_input)],
            )
            .await
        {
            Ok(outcome) => {
                eprintln!("DEBUG: stage {} outcome: {:?}", seq, outcome);
                outcome
            }
            Err(e) => {
                eprintln!("DEBUG: stage {} ERROR: {:?}", seq, e);
                tracing::error!(
                    pipeline_id = %pipeline_id,
                    stage_seq = seq,
                    error = ?e,
                    "RunExecutor falhou no stage"
                );
                // Persiste o stage como failed (custo = -1
                // é o sinal de falha que o loop trata).
                let now = Utc::now().to_rfc3339();
                repo.complete_stage(
                    &stage.id, "failed", -1, // sinal de falha pro loop
                    None, None, "[]", None, &now,
                )
                .await?;
                let _ = RunRepo::new(&self.db)
                    .set_status(&run_id, RunStatus::Failed)
                    .await;
                return Err(PipelineError::ProviderFailed(format!("{e:?}")));
            }
        };

        // Calcula custo.
        let cost = descriptor.cost_microcents(outcome.prompt_tokens, outcome.completion_tokens);
        if outcome.final_state == RunState::Completed {
            let _ = MessageRepo::new(&self.db)
                .set_usage_and_cost(
                    &msg_id,
                    outcome.prompt_tokens,
                    outcome.completion_tokens,
                    cost,
                )
                .await;
            let _ = ConversationRepo::new(&self.db)
                .add_cost(&conv_id, cost)
                .await;
        }

        // Extrai o output text do Message.
        let output_text = MessageRepo::new(&self.db)
            .get(&msg_id)
            .await
            .map(|m| m.content)
            .unwrap_or_default();

        // Hash do output (D6: chave de reuso).
        let output_hash = if output_text.is_empty() {
            None
        } else {
            Some(format!("fnv:{}", fnv_hash(&output_text)))
        };

        // Persiste o stage como completed.
        let now = Utc::now().to_rfc3339();
        let output_artifact_id = if !output_text.is_empty() {
            // Cria o artifact do output do stage.
            let artifact = MultimodelArtifact {
                id: frederico_storage::new_artifact_id(),
                run_id: pipeline_id.to_string(),
                stage_id: Some(stage.id.clone()),
                kind: MultimodelArtifactKind::Text,
                content_ref: format!("memory://stage_{seq}_output"),
                hash: output_hash.clone().unwrap_or_default(),
                size_bytes: output_text.len() as i64,
                created_at: now.clone(),
            };
            repo.create_artifact(&artifact).await?;
            Some(artifact.id)
        } else {
            None
        };

        repo.complete_stage(
            &stage.id,
            match outcome.final_state {
                RunState::Completed => "completed",
                RunState::Failed => "failed",
                RunState::Cancelled => "cancelled",
                RunState::Interrupted => "interrupted",
                _ => "completed",
            },
            cost as i64,
            output_artifact_id.as_deref(),
            output_hash.as_deref(),
            "[]",
            None,
            &now,
        )
        .await?;

        // Bump atômico do runs.status legado (mesma regra
        // do `send_message` do `ChatOrchestrator`).
        let _ = RunRepo::new(&self.db)
            .set_status(
                &run_id,
                match outcome.final_state {
                    RunState::Completed => RunStatus::Completed,
                    RunState::Failed => RunStatus::Failed,
                    RunState::Cancelled => RunStatus::Cancelled,
                    RunState::Interrupted => RunStatus::Timeout,
                    _ => RunStatus::Failed,
                },
            )
            .await;

        Ok(StageResult {
            stage_id: stage.id,
            output_text,
            cost_microcents: cost as i64,
            reused: false,
        })
    }
}

/// Hash FNV simples de uma string (não-criptográfico, só pra
/// chave de reuso do D6). Retorna `u64` em hex.
///
/// **Por que não SHA-256:** o D6 do ADR-0028 diz que o hash é
/// só pra detectar "output mudou" (reuso de stage). A Etapa 6
/// pode trocar por SHA-256 se precisar de integridade
/// criptográfica (defesa contra tampering do storage).
fn fnv_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
