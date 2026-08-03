//! `RunExecutor` — fecha o loop `tool_call` do Frederico IA Studio
//! (Fase 3, Etapa 4).
//!
//! ## O que o executor faz
//!
//! 1. Constrói um [`ChatRequest`] com `tools:` (vindo do
//!    [`ToolRegistry`] filtrado por `allowed_for_run`).
//! 2. Chama [`ProviderAdapter::stream`] e consome o
//!    [`futures::stream::BoxStream`].
//! 3. Para cada [`StreamEvent`]:
//!    - **Journal-then-emit** (spec `chat-and-providers.md`): persiste
//!      em `message_events` **antes** de agir.
//!    - `Delta` → acumula texto + atualiza `messages.content`.
//!    - `Usage` → atualiza contadores de tokens.
//!    - `ToolCall` → guarda pro próximo round (o provider manda
//!      `Done { stop_reason: ToolCalls }` na sequência).
//!    - `Done` → sai do loop de stream.
//!    - `Error` / `Cancelled` → finaliza imediatamente.
//! 4. Se tinha `ToolCall` no round:
//!    - [`validate_tool_call`] (10 passos do tool-registry).
//!    - Se `Approved` → executa via [`Tool::execute`] → emite
//!      [`StreamEvent::ToolResult`]`{ ok: true, output }`.
//!    - Se `Rejected` → emite `ToolResult` com erro estruturado
//!      (`TOOL_NOT_FOUND` etc. — o modelo recebe o erro e pode
//!      tentar de novo ou encerrar).
//!    - Se `ApprovalRequired` → erro (a UI da Etapa 6 consome o
//!      `ApprovalRequest`).
//!    - Persiste o `ToolResult` (`kind='tool_result'`, migration 0004).
//!    - Adiciona [`ChatMessage::tool`] ao contexto.
//!    - **Volta a chamar o modelo** (próximo round do loop).
//! 5. Se NÃO tinha `ToolCall`: `Done` final, fecha como
//!    `MessageStatus::Completed` / `RunStatus::Completed`.
//!
//! ## O que **não** está aqui
//!
//! - **Cancelamento hierárquico** (Etapa 5) — o `CancellationToken`
//!   é aceito no `new` mas ainda não é monitorado dentro do loop
//!   (a Etapa 5 adiciona o `tokio::select!` com `cancel.cancelled()`).
//! - **Watchdog de 60s** (Etapa 5) — o `event_timeout` do
//!   `ChatRequest` é o que o provider consulta; o executor não tem
//!   um watchdog próprio.
//! - **Recovery de crash** (Etapa 5).
//! - **UI de aprovação** (Etapa 6).
//! - **Atualização granular de `runs.state`** (Etapa 5.x) — a
//!   Etapa 4 só atualiza `messages.status` e `runs.status` (Fase 2).
//!   A coluna `state` detalhada (22 valores) fica pro watchdog, que
//!   reconstrói a partir do `journal`. `RunState` em memória é
//!   carregado do `Run` carregado pelo caller.
//! - **Cost tracking** — a Etapa 4 não usa `CostModel`; o executor
//!   só repassa `Usage`. O custo é calculado quando o `Run` fecha,
//!   no caller (Etapa 4.x).
//!
//! ## Persistência
//!
//! Cada `StreamEvent` é gravado **antes** de ser processado. Falha na
//! persistência é propagada como `Err` e o loop é abortado (mesma
//! regra do `ChatOrchestrator` da Fase 2). A transação é single
//! `INSERT` (atômico por si no SQLite); a Etapa 5 (watchdog) é que
//! introduz o `BEGIN IMMEDIATE; ...; COMMIT;` explícito quando
//! precisar agrupar appends.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use frederico_agent_engine::{Budget, RunState};
use frederico_core::ConversationId;
use frederico_core::{MessageId, ModelId, RunId, ToolId};
use frederico_provider_engine::{
    ChatMessage, ChatRequest, ProviderAdapter, ProviderError, StopReason, StreamEvent,
    ToolDescriptor,
};
use frederico_storage::{
    ApprovalQueueRepo, Database, MessageEventRepo, MessageRepo, MessageStatus, RunRepo, RunStatus,
    StorageError,
};
use frederico_tool_registry::{
    validate_tool_call, AuditEntry, AuditSink, Jail, JailResolver, JailResolverError,
    NoopAuditSink, PermissionSet, Tool, ToolCall as RegistryToolCall, ToolContext, ToolRegistry,
    ValidationContext, ValidationOutcome,
};
use futures::StreamExt;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::budget::{BudgetEnforcer, BudgetError};
use crate::persist::persist_journal;
use crate::state_mapping::run_state_for_event;
use crate::tool_definitions::tools_to_descriptors;

/// Log resumido de uma `tool_call` que o executor executou. Vai no
/// [`RunOutcome::tool_calls`] pra audit e observabilidade.
#[derive(Debug, Clone)]
pub struct ToolCallLog {
    /// `id` do `StreamEvent::ToolCall` que está sendo respondido.
    pub id: String,
    /// `ToolId` validado e executado.
    pub tool_id: ToolId,
    /// Argumentos como `serde_json::Value` (parsed de `arguments_json`).
    pub arguments: serde_json::Value,
    /// `true` se a ferramenta executou com sucesso (`ToolResult.ok`).
    pub ok: bool,
    /// Output conforme `output_schema` (sucesso) ou erro estruturado
    /// (`{"error_code": "TOOL_...", "error_message": "..."}`).
    pub output: serde_json::Value,
}

/// Resultado consolidado do [`RunExecutor::run`].
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// `StopReason` final do último round. `Stop` → `Completed`;
    /// `Length` → `Completed` (fim normal, só truncou); `Error` →
    /// `Failed`; `ToolCalls` não pode acontecer no fim (o executor
    /// sempre executa a `tool_call` antes de sair do loop externo).
    pub stop_reason: StopReason,
    /// [`RunState`] final mapeado pelo executor.
    pub final_state: RunState,
    /// Conteúdo acumulado do Assistant (todos os `Delta` concatenados).
    pub accumulated_content: String,
    /// Tokens de prompt do último `Usage` recebido (ou 0).
    pub prompt_tokens: u32,
    /// Tokens de completion do último `Usage` recebido (ou 0).
    pub completion_tokens: u32,
    /// Lista de `tool_calls` executadas, em ordem de execução.
    pub tool_calls: Vec<ToolCallLog>,
}

/// Erros do `RunExecutor`. `From<ProviderError>` e `From<StorageError>`
/// permitem usar `?` nos adapters/repos.
#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("provedor: {0}")]
    Provider(#[from] ProviderError),
    #[error("budget: {0}")]
    Budget(#[from] BudgetError),
    /// `validate_tool_call` devolveu `ApprovalRequired` e a UI da
    /// Etapa 6 ainda não consome o `ApprovalRequest`. Por enquanto,
    /// o executor aborta — a Etapa 6 substitui por enfileiramento
    /// na fila de aprovação.
    #[error("aprovação requerida — UI da Etapa 6 ainda não consome `ApprovalRequest`")]
    ApprovalRequired,
    /// O `ToolManifest` existe no `ToolRegistry` mas nenhuma
    /// implementação concreta de `Tool` foi passada pro `RunExecutor`
    /// no `new`. A Etapa 4 só tem `FilesReadTool`; uma ferramenta
    /// nova que apareça no registry sem executor cai aqui.
    #[error("ferramenta '{0}' registrada como manifesto mas sem implementação executável")]
    ToolNotExecutable(ToolId),
    /// O `validate_tool_call` foi chamado com um `ToolId` que o
    /// `ToolRegistry` não conhece — em teoria o Passo 1 do validador
    /// já rejeita; esse erro é o fallback de defesa em profundidade.
    #[error("ferramenta '{0}' não está no registro")]
    UnknownTool(String),
    /// Falha ao resolver o `Jail` para a `ConversationId` do run
    /// (Etapa 1 da Fase de Ligação, ADR-0022 §D3). O `JailResolver`
    /// propagou `JailResolverError` (mkdir falhou, jail não pôde
    /// ser construído, etc.). Erro duro — não há fallback; o run
    /// falha com mensagem legível pro usuário.
    #[error("falha ao resolver jail: {0}")]
    JailResolution(#[from] JailResolverError),
}

pub type ExecutorResult<T> = Result<T, ExecutorError>;

/// O `RunExecutor` que fecha o loop `tool_call`. Carrega tudo o que é
/// estável entre rounds: adapter, registry, jail, permissões, tools
/// concretas. O `BudgetEnforcer` é **per-execução** — `new` cria um
/// zerado e o caller não precisa se preocupar.
pub struct RunExecutor {
    adapter: Arc<dyn ProviderAdapter>,
    registry: ToolRegistry,
    /// Resolvedor de jail por `ConversationId`. O `Jail` efetivo é
    /// resolvido **uma vez por run** no início de `run()` (não por
    /// tool_call), e cacheado em `cached_jail` para uso pela
    /// validação (Passo 7 do `validate_tool_call`) e pelo
    /// `ToolContext` entregue a `Tool::execute`. Ver
    /// ADR-0022 §D3 — `JailResolver` trait mora no
    /// `frederico-tool-registry`; `FileSystemJailResolver` (a
    /// impl default) mora no `frederico-app`.
    jail_resolver: Arc<dyn JailResolver>,
    /// `Jail` resolvido para a conversa do run. Carregado em
    /// `run()` (uma query de `Run` + uma resolução). `None`
    /// antes do `run()` ser chamado.
    cached_jail: Option<Jail>,
    /// `ConversationId` carregado em `run()`. Imutável durante o
    /// run (a conversa não muda de ID no meio da execução).
    cached_conversation_id: Option<ConversationId>,
    db: Database,
    permissions: PermissionSet,
    allowed_for_run: Vec<ToolId>,
    /// Mapa `ToolId → Arc<dyn Tool>`. Construído no `new` a partir do
    /// `Vec<Arc<dyn Tool>>` que o caller passa.
    tools: HashMap<ToolId, Arc<dyn Tool>>,
    /// Budget enforcer interno. `current_step` começa em 0; o
    /// `run()` chama `tick()` a cada round.
    budget: BudgetEnforcer,
    /// `CancellationToken`. A Etapa 5 monitora no `tokio::select!`.
    cancel: CancellationToken,
    /// Watchdog entre eventos. Default 60s. Customizável via
    /// [`RunExecutor::with_event_timeout`] (usado nos testes pra
    /// não esperar 60s).
    event_timeout: Duration,
    /// Sink de auditoria (Fase 3, Etapa 5.x, Passo 10). Default
    /// `NoopAuditSink` (não grava nada). O caller (chat
    /// orchestrator) injeta um `DbAuditSink` em produção.
    audit_sink: Arc<dyn AuditSink>,
}

impl RunExecutor {
    /// Constrói o executor.
    ///
    /// - `adapter`: o `ProviderAdapter` que produz o stream.
    /// - `registry`: o `ToolRegistry` com os manifestos.
    /// - `jail_resolver`: o `JailResolver` (trait em
    ///   `frederico-tool-registry`). O `Jail` efetivo é resolvido
    ///   em `run()` por `ConversationId` (query única no
    ///   `RunRepo::get`). Substitui o `jail: Jail` direto das
    ///   Etapas anteriores — ADR-0022 §D3.
    /// - `db`: o `Database` (clonável, internamente `Arc`).
    /// - `permissions`: o `PermissionSet` da execução. Etapa 4 usa
    ///   `default()` (deny); Etapa 6 carrega o real do assistente.
    /// - `allowed_for_run`: os `ToolId` permitidos pra este Run
    ///   (passa pelo `ToolRegistry::effective_tools`).
    /// - `tools`: as implementações concretas (`FilesReadTool` etc.).
    ///   A partir da Etapa 1 da Fase de Ligação, `FilesReadTool`
    ///   **não** carrega `Jail` no construtor (o jail vem do
    ///   `ToolContext` por chamada).
    /// - `budget`: o `Budget` da execução (default 50 steps).
    /// - `cancel`: o `CancellationToken` (Etapa 5 monitora).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: Arc<dyn ProviderAdapter>,
        registry: ToolRegistry,
        jail_resolver: Arc<dyn JailResolver>,
        db: Database,
        permissions: PermissionSet,
        allowed_for_run: Vec<ToolId>,
        tools: Vec<Arc<dyn Tool>>,
        budget: Budget,
        cancel: CancellationToken,
    ) -> Self {
        let tools_map = tools
            .into_iter()
            .map(|t| (t.manifest().id.clone(), t))
            .collect();
        Self {
            adapter,
            registry,
            jail_resolver,
            cached_jail: None,
            cached_conversation_id: None,
            db,
            permissions,
            allowed_for_run,
            tools: tools_map,
            budget: BudgetEnforcer::new(budget),
            cancel,
            event_timeout: Duration::from_secs(60),
            audit_sink: Arc::new(NoopAuditSink),
        }
    }

    /// Sobrescreve o sink de auditoria. Default é
    /// `NoopAuditSink` (não grava). Em produção, o
    /// `ChatOrchestrator` injeta um `DbAuditSink` (ver
    /// `crate::audit_sink::DbAuditSink`).
    #[must_use]
    pub fn with_audit_sink(mut self, sink: Arc<dyn AuditSink>) -> Self {
        self.audit_sink = sink;
        self
    }

    /// Sobrescreve o `event_timeout` (watchdog entre eventos).
    /// Default é 60s; os testes usam `100ms` pra rodar rápido.
    /// Encadeável.
    #[must_use]
    pub fn with_event_timeout(mut self, event_timeout: Duration) -> Self {
        self.event_timeout = event_timeout;
        self
    }

    /// Roda o loop até o modelo terminar de verdade (`Done` com
    /// `StopReason::Stop` ou `Length`) ou um erro/budget estourar.
    ///
    /// `initial_messages` é tipicamente `[user_message]` (o caller
    /// — ChatOrchestrator — já persistiu a user message antes). A
    /// Etapa 4 não carrega histórico do banco; o caller monta o
    /// contexto.
    ///
    /// **Cancelamento:** o `CancellationToken` passado no `new` é
    /// checado entre cada `stream.next()` e entre rounds. Quando
    /// cancelado, o executor finaliza como
    /// [`MessageStatus::Cancelled`] / [`RunStatus::Cancelled`] e
    /// retorna [`RunOutcome`] com `final_state = Cancelled`. **Não**
    /// interrompe a request HTTP em andamento (a Etapa 5 introduz o
    /// `tokio::select!` com `reqwest` observando o token via
    /// `ProviderAdapter`).
    pub async fn run(
        &mut self,
        message_id: MessageId,
        run_id: RunId,
        model_id: ModelId,
        initial_messages: Vec<ChatMessage>,
    ) -> ExecutorResult<RunOutcome> {
        let mut messages = initial_messages;
        let mut accumulated = String::new();
        let mut tool_calls_log: Vec<ToolCallLog> = Vec::new();
        let mut prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;
        let mut final_stop: StopReason = StopReason::Stop;
        let mut final_state: RunState = RunState::Completed;
        // Marca como "ainda não finalizado" via `None` interno. A
        // primeira atribuição abaixo sempre seta; este `let _` só
        // satisfaz o borrow checker para o caso de o compilador
        // achar que a variável pode não ser escrita.
        let _ = (&mut final_stop, &mut final_state);

        let event_repo = MessageEventRepo::new(&self.db);
        let message_repo = MessageRepo::new(&self.db);
        let run_repo = RunRepo::new(&self.db);

        // Carrega o `Run` para extrair o `conversation_id` e resolve
        // o `Jail` **uma vez por run** (não por tool_call). O
        // `conversation_id` é imutável durante o run; carregar uma
        // vez elimina query por chamada e ponto de falha em caminho
        // quente. ADR-0022 §D3. Falha do resolver aborta o run
        // antes de qualquer tool_call — não há fallback de
        // degradação (decisão registrada na conversa da Etapa 1).
        let run = run_repo.get(&run_id).await?;
        let conversation_id = run.conversation_id;
        let resolved_jail = self.jail_resolver.resolve(&conversation_id)?;
        self.cached_conversation_id = Some(conversation_id);
        self.cached_jail = Some(resolved_jail);

        loop {
            // 0. Checagem de cancelamento entre rounds. Se chegou
            //    entre o final do round anterior e o começo deste,
            //    sai como Cancelled.
            if self.cancel.is_cancelled() {
                self.finalize_status(
                    &message_repo,
                    &run_repo,
                    message_id,
                    run_id,
                    &accumulated,
                    prompt_tokens,
                    completion_tokens,
                    MessageStatus::Cancelled,
                    RunStatus::Cancelled,
                )
                .await?;
                return Ok(RunOutcome {
                    stop_reason: StopReason::Stop,
                    final_state: RunState::Cancelled,
                    accumulated_content: accumulated,
                    prompt_tokens,
                    completion_tokens,
                    tool_calls: tool_calls_log,
                });
            }

            // 1. Tick do budget. Se estourar, fecha como failed.
            if let Err(e) = self.budget.tick() {
                tracing::warn!(
                    ?run_id,
                    step = self.budget.current_step(),
                    "budget estourado: {e}"
                );
                self.finalize_status(
                    &message_repo,
                    &run_repo,
                    message_id,
                    run_id,
                    &accumulated,
                    prompt_tokens,
                    completion_tokens,
                    MessageStatus::Failed,
                    RunStatus::Failed,
                )
                .await?;
                return Ok(RunOutcome {
                    stop_reason: StopReason::Error,
                    final_state: RunState::Failed,
                    accumulated_content: accumulated,
                    prompt_tokens,
                    completion_tokens,
                    tool_calls: tool_calls_log,
                });
            }

            // 2. Monta o ChatRequest com tools.
            let descriptors = tools_to_descriptors(&self.registry, &self.allowed_for_run);
            let tool_descriptors: Vec<ToolDescriptor> = descriptors
                .into_iter()
                .map(|d| ToolDescriptor {
                    name: d.name,
                    description: d.description,
                    parameters_schema: d.parameters_schema,
                })
                .collect();

            let req = ChatRequest {
                provider: self.adapter.id(),
                model: model_id.clone(),
                messages: messages.clone(),
                tools: tool_descriptors,
                temperature: None,
                max_tokens: None,
                cancel: self.cancel.clone(),
                event_timeout: self.event_timeout,
            };

            // O `event_timeout` é o teto **entre** eventos. O
            // `tokio::select!` reseta o sleep a cada iteração do
            // loop (cada `stream.next()`). Default 60s.
            let event_timeout = req.event_timeout;

            // 3. Stream.
            let mut stream = self.adapter.stream(req)?;

            // 3a. `ToolCall` pendente deste round (se houver). O
            //     provider manda o(s) `ToolCall` e depois um
            //     `Done { stop_reason: ToolCalls }` — o executor
            //     consome os dois e processa o tool_call após o Done.
            let mut tool_call_collected: Option<(String, ToolId, serde_json::Value)> = None;
            let mut iter_stop = StopReason::Stop;

            loop {
                // `tokio::select!` com 3 braços:
                // 1. `cancel.cancelled()` (biased — prioridade alta)
                // 2. `tokio::time::sleep(event_timeout)` — watchdog
                //    entre eventos (reseta a cada iteração; é o
                //    tempo **entre** eventos, não o tempo total).
                // 3. `stream.next()` — o próximo evento do adapter.
                let next_ev: Option<StreamEvent> = tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => {
                        // Cancel chegou — sai do while e cai no
                        // "se cancelou" abaixo.
                        break;
                    }
                    _ = tokio::time::sleep(event_timeout) => {
                        // Watchdog estourou — finaliza como
                        // Timeout/Interrupted. A view
                        // `runs_with_status` mapeia
                        // `interrupted → timeout` (a coluna
                        // `state` carrega `interrupted`; a
                        // derivada `status` carrega `timeout`).
                        tracing::warn!(
                            ?run_id,
                            timeout = ?event_timeout,
                            "watchdog estourou — nenhum evento do provider"
                        );
                        self.finalize_status(
                            &message_repo,
                            &run_repo,
                            message_id,
                            run_id,
                            &accumulated,
                            prompt_tokens,
                            completion_tokens,
                            MessageStatus::Timeout,
                            RunStatus::Timeout,
                        )
                        .await?;
                        return Ok(RunOutcome {
                            stop_reason: StopReason::Error,
                            final_state: RunState::Interrupted,
                            accumulated_content: accumulated,
                            prompt_tokens,
                            completion_tokens,
                            tool_calls: tool_calls_log,
                        });
                    }
                    ev = stream.next() => ev,
                };

                let Some(ev) = next_ev else {
                    // Stream terminou (adapter fechou sem Done
                    // explícito).
                    break;
                };

                // Journal-then-emit: persiste ANTES de processar. O
                // `seq` retornado vai pro `runs.last_event_seq` numa
                // única transação com o `runs.state` granular e o
                // `last_heartbeat_at` (Etapa 5.x.3 + 5.x.4).
                let seq = persist_journal(&event_repo, &message_id, &ev).await?;
                if let Some(new_state) = run_state_for_event(&ev) {
                    run_repo
                        .set_state_and_heartbeat_tx(&run_id, new_state, seq as u64)
                        .await?;
                }

                match ev {
                    StreamEvent::Delta { content } => {
                        accumulated.push_str(&content);
                        message_repo.set_content(&message_id, &accumulated).await?;
                    }
                    StreamEvent::Usage {
                        prompt_tokens: p,
                        completion_tokens: c,
                    } => {
                        prompt_tokens = p;
                        completion_tokens = c;
                    }
                    StreamEvent::ToolCall {
                        id,
                        name,
                        arguments_json,
                    } => {
                        let args: serde_json::Value = serde_json::from_str(&arguments_json)
                            .unwrap_or_else(|e| {
                                tracing::warn!(
                                    "argumentos_json do tool_call não parseou: {e}; usando Null"
                                );
                                serde_json::Value::Null
                            });
                        let tool_id = ToolId::new(name.clone());
                        tool_call_collected = Some((id, tool_id, args));
                    }
                    StreamEvent::ToolResult { .. } => {
                        // O ToolResult é emitido pelo próprio executor;
                        // nunca vem do provider. Se chegar aqui, é bug
                        // do adapter.
                        tracing::error!("provider emitiu ToolResult — bug do adapter");
                    }
                    StreamEvent::Done { stop_reason } => {
                        iter_stop = stop_reason;
                        break;
                    }
                    StreamEvent::Error(e) => {
                        self.finalize_error(
                            &message_repo,
                            &run_repo,
                            message_id,
                            run_id,
                            &accumulated,
                            prompt_tokens,
                            completion_tokens,
                            &e,
                        )
                        .await?;
                        return Ok(RunOutcome {
                            stop_reason: StopReason::Error,
                            final_state: RunState::Failed,
                            accumulated_content: accumulated,
                            prompt_tokens,
                            completion_tokens,
                            tool_calls: tool_calls_log,
                        });
                    }
                    StreamEvent::Cancelled => {
                        self.finalize_status(
                            &message_repo,
                            &run_repo,
                            message_id,
                            run_id,
                            &accumulated,
                            prompt_tokens,
                            completion_tokens,
                            MessageStatus::Cancelled,
                            RunStatus::Cancelled,
                        )
                        .await?;
                        return Ok(RunOutcome {
                            stop_reason: StopReason::Stop,
                            final_state: RunState::Cancelled,
                            accumulated_content: accumulated,
                            prompt_tokens,
                            completion_tokens,
                            tool_calls: tool_calls_log,
                        });
                    }
                }
            }

            // 3b. Se o `break` acima foi por cancelamento (e não
            //     por `Done`), finaliza como Cancelled. Sem isso,
            //     o executor cairia no "5. sem tool_call" e
            //     fecharia como Completed — mentindo pro user.
            if self.cancel.is_cancelled() {
                self.finalize_status(
                    &message_repo,
                    &run_repo,
                    message_id,
                    run_id,
                    &accumulated,
                    prompt_tokens,
                    completion_tokens,
                    MessageStatus::Cancelled,
                    RunStatus::Cancelled,
                )
                .await?;
                return Ok(RunOutcome {
                    stop_reason: StopReason::Stop,
                    final_state: RunState::Cancelled,
                    accumulated_content: accumulated,
                    prompt_tokens,
                    completion_tokens,
                    tool_calls: tool_calls_log,
                });
            }

            // 4. Se o provider mandou ToolCall, executa.
            if let Some((call_id, tool_id, args)) = tool_call_collected {
                let result = self
                    .handle_tool_call(
                        &event_repo,
                        &message_repo,
                        run_id,
                        message_id,
                        call_id.clone(),
                        tool_id.clone(),
                        args.clone(),
                    )
                    .await?;

                tool_calls_log.push(ToolCallLog {
                    id: call_id.clone(),
                    tool_id: tool_id.clone(),
                    arguments: args.clone(),
                    ok: result.ok,
                    output: result.output.clone(),
                });

                // 4a. Adiciona ao contexto: assistant (placeholder
                //     do tool_call) + tool (resposta). O
                //     `ScriptedProviderAdapter` ignora — a integração
                //     OpenAI-compat real (Etapa 4.1) traduz o
                //     `Role::Tool` + `tool_call_id` no payload JSON
                //     que o provider espera.
                messages.push(ChatMessage::assistant(format!(
                    "[tool_call id={call_id} name={tool_id} args={args}]",
                    call_id = call_id,
                    tool_id = tool_id,
                    args = serde_json::to_string(&args).unwrap_or_default(),
                )));
                messages.push(ChatMessage::tool(
                    tool_id.to_string(),
                    serde_json::to_string(&result.output).unwrap_or_default(),
                    call_id,
                ));

                // 4b. Continua o loop (próximo round).
                continue;
            }

            // 5. Sem tool_call — Done, finaliza como Completed
            //    (a Etapa 4 não diferencia Stop de Length; ambos
            //    são término natural do modelo). O custo em
            //    microcents fica 0 — a Etapa 4.x reintroduz o cálculo
            //    via `CostModel` quando o `ChatOrchestrator` for
            //    refatorado pra consumir o `RunExecutor`.
            final_stop = iter_stop;
            final_state = RunState::Completed;
            self.finalize_status(
                &message_repo,
                &run_repo,
                message_id,
                run_id,
                &accumulated,
                prompt_tokens,
                completion_tokens,
                MessageStatus::Completed,
                RunStatus::Completed,
            )
            .await?;
            break;
        }

        Ok(RunOutcome {
            stop_reason: final_stop,
            final_state,
            accumulated_content: accumulated,
            prompt_tokens,
            completion_tokens,
            tool_calls: tool_calls_log,
        })
    }

    /// Valida e executa uma `tool_call`. Persiste o `ToolResult` no
    /// journal. Grava entrada de auditoria via `AuditSink` (Etapa
    /// 5.x, Passo 10). Devolve o `ToolResult` cru (sem
    /// persistência — quem chamou já tem).
    #[allow(clippy::too_many_arguments)]
    async fn handle_tool_call(
        &self,
        event_repo: &MessageEventRepo<'_>,
        _message_repo: &MessageRepo<'_>,
        run_id: RunId,
        message_id: MessageId,
        call_id: String,
        tool_id: ToolId,
        args: serde_json::Value,
    ) -> ExecutorResult<ToolResultRaw> {
        // 1. validate_tool_call (10 passos).
        let manifest = self
            .registry
            .get(&tool_id)
            .ok_or_else(|| ExecutorError::UnknownTool(tool_id.to_string()))?
            .clone();

        let call = RegistryToolCall {
            tool_id: tool_id.clone(),
            version: manifest.version.clone(),
            arguments: args.clone(),
            approval: None,
        };

        let cached_jail = self
            .cached_jail
            .as_ref()
            .expect("cached_jail é populado no início de run()")
            .clone();
        let conversation_id = self
            .cached_conversation_id
            .expect("cached_conversation_id é populado no início de run()");

        let ctx = ValidationContext::with_permissions(
            self.registry.clone(),
            cached_jail.clone(),
            self.allowed_for_run.clone(),
            self.permissions.clone(),
            None,
        );

        let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "null".to_string());

        match validate_tool_call(&ctx, &call) {
            ValidationOutcome::Approved { .. } => {
                // 2. Executa via Tool::execute. O `ToolContext` é
                //    construído por tool_call (custo O(1), sem I/O)
                //    carregando o `Jail` cacheado e os IDs imutáveis
                //    do run. ADR-0022 §D3.
                let tool = self
                    .tools
                    .get(&tool_id)
                    .ok_or_else(|| ExecutorError::ToolNotExecutable(tool_id.clone()))?
                    .clone();

                let tool_ctx = ToolContext::new(conversation_id, run_id, message_id, cached_jail);

                let start = std::time::Instant::now();
                let result = tool.execute(&tool_ctx, &args).await;
                let duration = start.elapsed();

                // 3. Emite StreamEvent::ToolResult.
                let output_value = if result.ok {
                    result.output.clone()
                } else {
                    serde_json::json!({
                        "error_code": "TOOL_EXECUTION_FAILED",
                        "error_message": result.error_message,
                    })
                };

                let result_event = StreamEvent::ToolResult {
                    id: call_id.clone(),
                    ok: result.ok,
                    output: output_value.clone(),
                };
                persist_journal(event_repo, &message_id, &result_event).await?;

                // 5.x — Passo 10: grava auditoria (best effort).
                let audit_entry = AuditEntry {
                    tool_id: tool_id.clone(),
                    tool_version: manifest.version.clone(),
                    arguments_json: args_json.clone(),
                    result_ok: result.ok,
                    result_json: serde_json::to_string(&output_value)
                        .unwrap_or_else(|_| "null".to_string()),
                    duration,
                };
                let _ = self.audit_sink.record(audit_entry);

                Ok(ToolResultRaw {
                    ok: result.ok,
                    output: output_value,
                })
            }
            ValidationOutcome::Rejected(rejected) => {
                // O validador rejeitou antes de executar — emite
                // ToolResult de erro pro modelo receber o código
                // canônico.
                let output_value = serde_json::json!({
                    "error_code": rejected.code.as_str(),
                    "error_message": rejected.message,
                    "tool_id": tool_id.to_string(),
                });
                let result_event = StreamEvent::ToolResult {
                    id: call_id.clone(),
                    ok: false,
                    output: output_value.clone(),
                };
                persist_journal(event_repo, &message_id, &result_event).await?;

                // 5.x — Passo 10: grava auditoria da rejeição.
                let audit_entry = AuditEntry {
                    tool_id: tool_id.clone(),
                    tool_version: manifest.version.clone(),
                    arguments_json: args_json.clone(),
                    result_ok: false,
                    result_json: serde_json::to_string(&output_value)
                        .unwrap_or_else(|_| "null".to_string()),
                    duration: Duration::ZERO,
                };
                let _ = self.audit_sink.record(audit_entry);

                Ok(ToolResultRaw {
                    ok: false,
                    output: output_value,
                })
            }
            ValidationOutcome::ApprovalRequired(req) => {
                // Etapa 6.1: enfileira o `ApprovalRequest` na
                // `approval_queue` (SQLite) pra UI consumir via
                // `AppOp::ApprovalList` / `AppOp::ApprovalRespond`.
                // O run finaliza como `Cancelled` (com nota de
                // "aguardando aprovação" no `Message.error`) —
                // re-executar manualmente após o usuário responder.
                // A Etapa 6.2 evolui pra "pausar o run e
                // continuar após resposta" (caminho B).
                let request_json = serde_json::to_string(&req).unwrap_or_else(|_| "{}".to_string());
                let approval_repo = ApprovalQueueRepo::new(&self.db);
                match approval_repo
                    .enqueue(&run_id, &tool_id.to_string(), &request_json)
                    .await
                {
                    Ok(approval_id) => {
                        tracing::info!(
                            run_id = ?run_id,
                            tool_id = %tool_id,
                            approval_id = %approval_id,
                            "ApprovalRequired enfileirado na approval_queue"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            run_id = ?run_id,
                            tool_id = %tool_id,
                            "falha ao enfileirar ApprovalRequired: {e}"
                        );
                    }
                }
                // 5.x — Passo 10: não grava auditoria aqui (a
                // Etapa 6.2 grava quando o usuário responde).
                Err(ExecutorError::ApprovalRequired)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_status(
        &self,
        message_repo: &MessageRepo<'_>,
        run_repo: &RunRepo<'_>,
        message_id: MessageId,
        run_id: RunId,
        accumulated: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        message_status: MessageStatus,
        run_status: RunStatus,
    ) -> ExecutorResult<()> {
        message_repo.set_content(&message_id, accumulated).await?;
        message_repo
            .set_usage_and_cost(&message_id, prompt_tokens, completion_tokens, 0)
            .await?;
        message_repo.set_status(&message_id, message_status).await?;
        run_repo.set_status(&run_id, run_status).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_error(
        &self,
        message_repo: &MessageRepo<'_>,
        run_repo: &RunRepo<'_>,
        message_id: MessageId,
        run_id: RunId,
        accumulated: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        err: &ProviderError,
    ) -> ExecutorResult<()> {
        message_repo.set_content(&message_id, accumulated).await?;
        message_repo
            .set_usage_and_cost(&message_id, prompt_tokens, completion_tokens, 0)
            .await?;
        let err_json = serde_json::to_string(err).unwrap_or_else(|_| err.to_string());
        message_repo.set_error(&message_id, &err_json).await?;
        message_repo
            .set_status(&message_id, MessageStatus::Failed)
            .await?;
        run_repo.set_status(&run_id, RunStatus::Failed).await?;
        Ok(())
    }
}

/// Resultado de `handle_tool_call` (cru, sem persistência).
struct ToolResultRaw {
    ok: bool,
    output: serde_json::Value,
}
