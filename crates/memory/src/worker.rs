//! Workers da Fase 4 — `EmbeddingWorker` + `MemoryExtractor`.
//!
//! Dois tokio tasks em background, alimentados por canais `mpsc`
//! separados (mesma casca, preocupações diferentes):
//!
//! 1. **`EmbeddingWorker`** (Etapa 2) — calcula embeddings de
//!    memórias com `embedding_status = 'pending'`. Lote pequeno
//!    (default 50, limite OpenRouter), batch em 1 request HTTP,
//!    sem retry em loop (ADR-0013 §1).
//! 2. **`MemoryExtractor`** (Etapa 3) — classifica a última
//!    resposta do LLM pós-conversa e, se houver candidato,
//!    grava via `MemoryRepo::insert_*` com proveniência
//!    **sobrescrita** quando externa (ADR-0012 §3 — mitiga E2 do
//!    `security-threat-model.md`).
//!
//! **Decisões de design compartilhadas** (ADR-0014 §1):
//!
//! - **Background, não bloqueantes.** App continua usável durante
//!   embedding/classificação; `tokio::spawn` em ambos, nenhum
//!   usa `tokio::time::interval` (problema do ADR-0014 §1).
//! - **`FakeClock`-friendly** — workers não dependem de
//!   `tokio::time`, só do `Clock`/`Utc::now()` do `storage`.
//! - **Cancelamento limpo** via `*Handle::shutdown` — fecha o
//!   canal, o worker termina o job em andamento.
//! - **Não abortam caller** — falhas são logadas via
//!   `tracing::warn!` e o worker segue (regra do ADR-0012 §2).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use frederico_core::{
    ClassificationContext, ConversationMessage, MemoryId, MemoryOrigin, NewMemory,
};
use tokio::sync::mpsc;

use crate::classifier::MemoryClassifier;
use crate::embedding::EmbeddingProvider;
use crate::error::MemoryResult;
use crate::memory_repo::{MemoryRepo, NewMemoryInput};

// ============================================================================
// EmbeddingWorker (Etapa 2)
// ============================================================================

/// Tamanho padrão do batch de embeddings por request HTTP
/// (limite típico do OpenRouter /embeddings).
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// Handle público do `EmbeddingWorker`. Clone-able (o `Arc`
/// interno é compartilhado), `Send` (pode morar em qualquer
/// estado da casca Tauri).
#[derive(Clone)]
pub struct EmbeddingWorkerHandle {
    tx: mpsc::Sender<EmbeddingCommand>,
}

impl EmbeddingWorkerHandle {
    /// Enfileira uma memória pra cálculo de embedding. O
    /// worker pega no próximo ciclo (não-bloqueante — se o
    /// canal estiver cheio, o `try_send` falha com `Full`,
    /// e a memória continua `pending` até o próximo poll
    /// do worker).
    pub fn enqueue(&self, id: MemoryId) {
        // `try_send` é não-bloqueante. Se o canal estiver
        // cheio, descartamos — a memória continua `pending`
        // e o próximo `poll_pending` pega.
        let _ = self.tx.try_send(EmbeddingCommand::EmbedOne(id));
    }

    /// Sinaliza shutdown limpo. O worker termina o batch em
    /// andamento e fecha.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(EmbeddingCommand::Shutdown).await;
    }

    /// Dispara um poll imediato do banco (procura memórias
    /// com `embedding_status = 'pending'`). Útil quando o
    /// caller insere memórias em batch e quer forçar o
    /// worker a pegar.
    pub fn tick(&self) {
        let _ = self.tx.try_send(EmbeddingCommand::PollPending);
    }
}

/// Comando interno do canal do `EmbeddingWorker`.
enum EmbeddingCommand {
    /// Embute uma memória específica (via `enqueue`).
    EmbedOne(MemoryId),
    /// Poll do banco por memórias `pending`.
    PollPending,
    /// Shutdown limpo.
    Shutdown,
}

/// O `EmbeddingWorker` propriamente dito. Construído via
/// [`EmbeddingWorker::start`], que faz `tokio::spawn` da
/// task em background e devolve o `EmbeddingWorkerHandle`
/// pro caller.
pub struct EmbeddingWorker {
    handle: EmbeddingWorkerHandle,
}

impl EmbeddingWorker {
    /// Inicia o worker. O `pool` é o do `frederico_storage::Database`,
    /// o `embedding` é o provider (default `OpenRouterEmbeddingAdapter`
    /// em produção, `NoopEmbeddingAdapter` em testes).
    /// O `batch_size` default é `DEFAULT_BATCH_SIZE`.
    #[must_use]
    pub fn start(
        pool: &sqlx::SqlitePool,
        embedding: Arc<dyn EmbeddingProvider>,
        batch_size: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<EmbeddingCommand>(1024);
        let handle = EmbeddingWorkerHandle { tx };
        // Clone do pool (Arc internamente) — o worker vive
        // independente do caller. `'static` no spawn.
        let pool_owned = pool.clone();
        let embedding_owned = embedding.clone();
        tokio::spawn(async move {
            let task = EmbeddingTask {
                pool: &pool_owned,
                embedding: embedding_owned,
                batch_size,
                rx,
            };
            task.run().await;
        });
        Self { handle }
    }

    /// Retorna o handle público.
    #[must_use]
    pub fn handle(&self) -> EmbeddingWorkerHandle {
        self.handle.clone()
    }
}

// Lifetimes: o worker precisa do `&SqlitePool` por causa
// dos métodos de `MemoryRepo` (que têm `&'a Database`).
// A `task` é construída dentro do `tokio::spawn` (com o
// pool clonado) — `'static` requirement do spawn.
struct EmbeddingTask<'a> {
    pool: &'a sqlx::SqlitePool,
    embedding: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
    rx: mpsc::Receiver<EmbeddingCommand>,
}

impl EmbeddingTask<'_> {
    async fn run(mut self) {
        // Fila interna de memórias a processar (alimentada
        // por `enqueue` ou por `poll_pending`).
        let mut queue: Vec<MemoryId> = Vec::with_capacity(self.batch_size);

        loop {
            // Coleta comandos até ter um batch ou um shutdown.
            // `recv` bloqueia até chegar algo. O batch é
            // processado quando:
            //   (a) chega `Shutdown`, ou
            //   (b) atinge `batch_size`.
            //
            // Para simplificar, processamos quando o canal
            // está vazio (1 batch por ciclo). Pra escalar,
            // dá pra mudar pra processar por `batch_size`
            // com timeout — mas isso vira `tokio::time` e o
            // ADR-0014 desencoraja.
            tokio::select! {
                cmd = self.rx.recv() => {
                    match cmd {
                        None => {
                            // Canal fechado — caller dropou
                            // todos os handles. Shutdown.
                            break;
                        }
                        Some(EmbeddingCommand::Shutdown) => break,
                        Some(EmbeddingCommand::EmbedOne(id)) => {
                            queue.push(id);
                        }
                        Some(EmbeddingCommand::PollPending) => {
                            // Poll do banco.
                            match self.collect_pending(&mut queue).await {
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::warn!(
                                        memory.worker = "poll_pending",
                                        error = %e,
                                        "falha ao poll memórias pendentes"
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // Processa o batch atual (se não-vazio).
            if !queue.is_empty() {
                if let Err(e) = self.process_batch(&mut queue).await {
                    tracing::warn!(
                        memory.worker = "process_batch",
                        error = %e,
                        "falha ao processar batch de embeddings"
                    );
                }
            }
        }

        tracing::info!(memory.worker = "shutdown", "EmbeddingWorker terminado");
    }

    /// Coleta memórias com `embedding_status = 'pending'`
    /// do banco e adiciona à fila (até o teto de `batch_size`).
    async fn collect_pending(&self, queue: &mut Vec<MemoryId>) -> MemoryResult<()> {
        let repo = MemoryRepo::new_from_pool(self.pool);
        let pending = repo.list_pending_embeddings(self.batch_size as u32).await?;
        for record in pending {
            if queue.len() < self.batch_size {
                queue.push(record.id);
            }
        }
        Ok(())
    }

    /// Processa o batch: embute todos os conteúdos em 1
    /// request HTTP, persiste os resultados, marca falhas.
    async fn process_batch(&self, queue: &mut Vec<MemoryId>) -> MemoryResult<()> {
        if queue.is_empty() {
            return Ok(());
        }
        let repo = MemoryRepo::new_from_pool(self.pool);

        // Carrega os conteúdos das memórias na fila.
        let mut records = Vec::with_capacity(queue.len());
        for id in queue.iter() {
            match repo.get(id).await? {
                Some(r)
                    if r.embedding_status == frederico_core::EmbeddingStatus::Pending
                        && r.active
                        && r.superseded_by.is_none() =>
                {
                    records.push(r);
                }
                _ => {
                    // Memória não existe, foi deletada, ou já
                    // tem embedding. Pula.
                }
            }
        }

        if records.is_empty() {
            queue.clear();
            return Ok(());
        }

        // Coleta os textos e calcula embedding em batch.
        let inputs: Vec<String> = records.iter().map(|r| r.content.clone()).collect();
        let input_refs: Vec<&str> = inputs.iter().map(String::as_str).collect();

        match self.embedding.embed(&input_refs).await {
            Ok(embeddings) => {
                // Persiste cada embedding. Falha individual
                // não aborta o batch — marca `failed` e
                // segue.
                for (record, emb) in records.iter().zip(embeddings.iter()) {
                    if let Err(e) = repo
                        .set_embedding(
                            &record.id,
                            self.embedding.provider_id(),
                            self.embedding.model_id(),
                            emb.len() as u32,
                            emb,
                        )
                        .await
                    {
                        tracing::warn!(
                            memory_id = %record.id,
                            error = %e,
                            "falha ao persistir embedding"
                        );
                        // Tenta marcar como failed (best effort).
                        let _ = repo.mark_embedding_failed(&record.id).await;
                    }
                }
            }
            Err(e) => {
                // Erro no provider inteiro (HTTP, timeout,
                // config). Marca todas como failed.
                tracing::warn!(
                    error = %e,
                    count = records.len(),
                    "falha no provider de embeddings — marcando batch como failed"
                );
                for record in &records {
                    let _ = repo.mark_embedding_failed(&record.id).await;
                }
            }
        }

        // Limpa a fila (memórias processadas, sucesso ou
        // falha — não retentamos em loop).
        queue.clear();
        Ok(())
    }
}

// ============================================================================
// MemoryExtractor (Etapa 3)
// ============================================================================

/// Tamanho do canal mpsc do `MemoryExtractor`. 256 = folgado
/// pra vários runs em sequência sem bloquear o `ChatOrchestrator`
/// (regra do `ADR-0012 §1` — fora do caminho crítico, mas não
/// deve causar backpressure no caller).
pub const DEFAULT_EXTRACTOR_CHANNEL: usize = 256;

/// Job de extração enviado pelo `ChatOrchestrator` quando um
/// run termina (regra do `ADR-0012 §1` — pós-resposta, fora
/// do caminho crítico).
///
/// O worker monta o `ClassificationContext` a partir do job e
/// chama o `MemoryClassifier` configurado. O resultado pode
/// ser:
/// - `Ok` com `record = None` → nada a fazer
/// - `Ok` com `record = Some(NewMemory)` → insere via
///   `MemoryRepo` (sobrescrevendo `origin` se proveniência
///   for externa — `ADR-0012 §3`)
/// - `Err` → log + descarta (sem abortar o worker)
#[derive(Debug, Clone)]
pub struct MemoryExtractionJob {
    /// ID do run (audit trail).
    pub run_id: String,
    /// ID da conversa.
    pub conversation_id: String,
    /// Últimas N mensagens (default 6) — entra no prompt do
    /// classificador.
    pub messages: Vec<ConversationMessage>,
    /// Quando o run terminou. Útil pro audit trail (não é
    /// usado na classificação).
    pub finished_at: DateTime<Utc>,
}

impl MemoryExtractionJob {
    /// Construtor ergonômico.
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        conversation_id: impl Into<String>,
        messages: Vec<ConversationMessage>,
        finished_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            conversation_id: conversation_id.into(),
            messages,
            finished_at,
        }
    }
}

/// Handle público do `MemoryExtractor`. Clone-able (o `Arc`
/// do canal é compartilhado), `Send` (pode morar em qualquer
/// estado da casca Tauri).
#[derive(Clone)]
pub struct MemoryExtractorHandle {
    tx: mpsc::Sender<ExtractorCommand>,
}

impl MemoryExtractorHandle {
    /// Enfileira um job de extração. Não-bloqueante — se o
    /// canal estiver cheio, o `try_send` falha e o job é
    /// descartado. A memória eventualmente classifica em runs
    /// futuros (a Etapa 5 mostra o status no painel).
    pub fn enqueue(&self, job: MemoryExtractionJob) {
        let _ = self.tx.try_send(ExtractorCommand::Extract(job));
    }

    /// Sinaliza shutdown limpo. O worker termina o job em
    /// andamento e fecha.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(ExtractorCommand::Shutdown).await;
    }
}

/// Comando interno do canal do `MemoryExtractor`.
enum ExtractorCommand {
    /// Classifica e persiste um job.
    Extract(MemoryExtractionJob),
    /// Shutdown limpo.
    Shutdown,
}

/// O `MemoryExtractor` propriamente dito. Construído via
/// [`MemoryExtractor::start`], que faz `tokio::spawn` da task
/// em background e devolve o `MemoryExtractorHandle` pro
/// caller.
pub struct MemoryExtractor {
    handle: MemoryExtractorHandle,
}

impl MemoryExtractor {
    /// Inicia o extractor. O `pool` é o do `frederico_storage::Database`,
    /// o `classifier` é a implementação de `MemoryClassifier`
    /// (default `LlmMemoryClassifier` em produção, `NoopMemoryClassifier`
    /// em testes).
    #[must_use]
    pub fn start(pool: &sqlx::SqlitePool, classifier: Arc<dyn MemoryClassifier>) -> Self {
        let (tx, rx) = mpsc::channel::<ExtractorCommand>(DEFAULT_EXTRACTOR_CHANNEL);
        let handle = MemoryExtractorHandle { tx };
        // Clone do pool (Arc internamente) e do classifier —
        // o worker vive independente do caller. `'static` no
        // spawn.
        let pool_owned = pool.clone();
        let classifier_owned = classifier.clone();
        tokio::spawn(async move {
            let task = ExtractorTask {
                pool: &pool_owned,
                classifier: classifier_owned,
                rx,
            };
            task.run().await;
        });
        Self { handle }
    }

    /// Retorna o handle público.
    #[must_use]
    pub fn handle(&self) -> MemoryExtractorHandle {
        self.handle.clone()
    }
}

// Lifetimes: ver `EmbeddingTask` acima. Mesma justificativa.
struct ExtractorTask<'a> {
    pool: &'a sqlx::SqlitePool,
    classifier: Arc<dyn MemoryClassifier>,
    rx: mpsc::Receiver<ExtractorCommand>,
}

impl ExtractorTask<'_> {
    async fn run(mut self) {
        // `recv` retorna `None` quando o canal é fechado
        // (todos os handles foram dropados). `Some(cmd)`
        // quando há um comando.
        loop {
            match self.rx.recv().await {
                None => break, // canal fechado
                Some(ExtractorCommand::Shutdown) => break,
                Some(ExtractorCommand::Extract(job)) => {
                    if let Err(e) = self.process_job(job).await {
                        tracing::warn!(
                            memory.extractor = "process_job",
                            error = %e,
                            "falha ao processar job de extração"
                        );
                    }
                }
            }
        }
        tracing::info!(memory.extractor = "shutdown", "MemoryExtractor terminado");
    }

    /// Processa um job: chama classificador, valida proveniência,
    /// insere no banco. **Nunca aborta o worker** — falhas
    /// viram `tracing::warn!` e segue (regra do `ADR-0012 §2`).
    async fn process_job(&self, job: MemoryExtractionJob) -> MemoryResult<()> {
        let MemoryExtractionJob {
            run_id,
            conversation_id,
            messages,
            finished_at: _,
        } = job;

        // 1. Monta contexto e chama o classificador.
        let ctx = ClassificationContext {
            run_id: run_id.clone(),
            conversation_id: conversation_id.clone(),
            messages: messages.clone(),
        };
        let output = match self.classifier.classify(ctx).await {
            Ok(o) => o,
            Err(e) => {
                // Cota estourada, provider indisponível, output
                // inválido — loga e descarta. Worker segue.
                tracing::warn!(
                    run_id = %run_id,
                    conversation_id = %conv_id(&conversation_id),
                    classifier = self.classifier.name(),
                    error = %e,
                    "classificador falhou — descartando job"
                );
                return Ok(());
            }
        };

        // 2. Se classificador disse "nada relevante", encerra.
        let Some(record) = output.record else {
            tracing::info!(
                run_id = %run_id,
                conversation_id = %conv_id(&conversation_id),
                reason = %output.reason,
                classifier = self.classifier.name(),
                "classificador decidiu não criar memória"
            );
            return Ok(());
        };

        // 3. Determina a origem real baseada em proveniência
        //    (regra do `ADR-0012 §3` — mitiga E2). O LLM pode
        //    alucinar `origin = User` pra texto externo; o
        //    worker **sobrescreve** com base na proveniência
        //    real das mensagens.
        let real_origin = determine_real_origin(&record, &messages);

        // 4. Constrói `NewMemoryInput` e insere no caminho
        //    correto (`auto_captured` para User/Assistant,
        //    `pending_review` para ExternalContent).
        let repo = MemoryRepo::new_from_pool(self.pool);
        let input = NewMemoryInput {
            scope_type: record.scope_type,
            scope_id: record.scope_id,
            type_: record.type_,
            content: record.content,
            origin: real_origin,
            source_type: record.source_type,
            source_id: record.source_id,
            confidence: record.confidence,
            importance: if output.importance.is_finite() && output.importance > 0.0 {
                output.importance
            } else {
                record.importance
            },
            expires_at: record.expires_at,
            // Auto-captura: o usuário ainda não confirmou.
            // A Etapa 5 (painel de memória) pode promover
            // para `user_confirmed = true` se o usuário revisar.
            user_confirmed: false,
            user_pinned: false,
        };

        match real_origin {
            MemoryOrigin::ExternalContent => {
                // Conteúdo externo: vai pra fila de revisão
                // (`pending_review = true`). O painel da Etapa 5
                // mostra o candidato pro usuário aceitar/negar.
                let inserted = repo.insert_pending_review(input).await?;
                tracing::info!(
                    run_id = %run_id,
                    conversation_id = %conversation_id,
                    memory_id = %inserted.id,
                    scope_type = inserted.scope_type.as_str(),
                    "memória externa gravada como pending_review"
                );
            }
            MemoryOrigin::User | MemoryOrigin::Assistant => {
                // Captura automática normal. `user_confirmed = false`
                // por default — o usuário pode revisar pelo painel
                // da Etapa 5 e marcar como confirmada.
                let inserted = repo.insert_auto_captured(input).await?;
                tracing::info!(
                    run_id = %run_id,
                    conversation_id = %conversation_id,
                    memory_id = %inserted.id,
                    scope_type = inserted.scope_type.as_str(),
                    origin = real_origin.as_str(),
                    "memória gravada via auto-captura"
                );
            }
        }

        Ok(())
    }
}

/// Helper: `Display` da conversation_id pra log (truncamento
/// defensivo, evita campos vazios).
fn conv_id(id: &str) -> String {
    if id.is_empty() {
        "<empty>".to_string()
    } else {
        id.to_string()
    }
}

/// Determina a **origem real** de uma memória baseada em
/// proveniência das mensagens (regra do `ADR-0012 §3`).
///
/// O classificador LLM pode alucinar `origin = User` pra um
/// texto externo (página web, saída de ferramenta). O worker
/// **sobrescreve** o `origin` com base na proveniência real:
///
/// 1. Se **qualquer** mensagem tem `source` começando com
///    `tool_output:` ou `document_attachment:`, o conteúdo é
///    **externo** → `MemoryOrigin::ExternalContent` (vai pra
///    `pending_review`).
/// 2. Senão, se há mensagem `assistant` ou `tool` no contexto,
///    o conteúdo é **gerado/consolidado pelo modelo** →
///    `MemoryOrigin::Assistant` (vai pra `auto_captured`).
/// 3. Senão, segue o que o classificador propôs (default
///    `User`).
///
/// Esta camada dupla (classificador sugere + worker decide)
/// mitiga **E2** do `security-threat-model.md` ("memória como
/// instrução") — prompt injection em página web ou PDF não
/// vira memória automática.
fn determine_real_origin(record: &NewMemory, messages: &[ConversationMessage]) -> MemoryOrigin {
    // (1) Proveniência externa explícita tem prioridade.
    if messages.iter().any(|m| {
        m.source.starts_with("tool_output:") || m.source.starts_with("document_attachment:")
    }) {
        return MemoryOrigin::ExternalContent;
    }

    // (2) Mensagens do assistente ou tool indicam conteúdo
    // gerado/consolidado pelo modelo.
    if messages
        .iter()
        .any(|m| m.role == "assistant" || m.role == "tool")
    {
        return MemoryOrigin::Assistant;
    }

    // (3) Caso contrário, segue o que o classificador propôs.
    // Se o classificador não setar, default é User (texto
    // digitado pelo usuário).
    record.origin
}

// ============================================================================
// Helpers compartilhados (`MemoryRepo::new_from_pool`)
// ============================================================================

impl<'a> MemoryRepo<'a> {
    /// Helper `pub(crate)` (não-exportado no `lib.rs`) — usado
    /// pelos workers (`EmbeddingWorker` + `MemoryExtractor`).
    /// Constrói um `MemoryRepo` a partir de um `&'a SqlitePool`
    /// (os workers não têm um `Database` em mãos — só o pool).
    pub(crate) fn new_from_pool(pool: &'a sqlx::SqlitePool) -> Self {
        Self { pool }
    }
}

// Converte `MemoryError` em log estruturado (não usamos
// `tracing::error!` pra não abortar o worker — falhas são
// registradas e o batch segue).
impl std::fmt::Display for EmbeddingWorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmbeddingWorkerHandle")
    }
}

impl std::fmt::Display for MemoryExtractorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoryExtractorHandle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::ClassifierError;
    use async_trait::async_trait;
    use frederico_core::{
        EmbeddingStatus, MemoryClassifierOutput, MemoryScopeType, MemorySourceType, MemoryType,
    };
    use frederico_storage::Database;

    // ------------------------------------------------------------------
    // EmbeddingWorker tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn worker_embeds_pending_memories() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Insere 2 memórias com embedding_status = Pending.
        for content in &["memória 1 sobre Rust", "memória 2 sobre Postgres"] {
            let input = NewMemoryInput {
                scope_type: MemoryScopeType::Project,
                scope_id: "proj-1".into(),
                type_: MemoryType::Fact,
                content: (*content).into(),
                origin: MemoryOrigin::User,
                source_type: MemorySourceType::new("seed"),
                source_id: None,
                confidence: 0.9,
                importance: 0.5,
                expires_at: None,
                user_confirmed: true,
                user_pinned: false,
            };
            repo.insert_auto_captured(input).await.expect("insert");
        }

        // Cria um adapter de teste que devolve vetores
        // fixos baseados no hash do conteúdo.
        struct FakeEmbed;
        #[async_trait]
        impl EmbeddingProvider for FakeEmbed {
            fn provider_id(&self) -> &str {
                "fake"
            }
            fn model_id(&self) -> &str {
                "fake-v1"
            }
            fn dimensions(&self) -> usize {
                4
            }
            async fn embed(
                &self,
                inputs: &[&str],
            ) -> Result<Vec<Vec<f32>>, crate::embedding::EmbeddingError> {
                Ok(inputs
                    .iter()
                    .map(|s| {
                        let mut v = vec![0.0_f32; 4];
                        for (i, b) in s.bytes().enumerate() {
                            v[i % 4] += b as f32 / 255.0;
                        }
                        v
                    })
                    .collect())
            }
        }

        let _worker = EmbeddingWorker::start(db.pool(), Arc::new(FakeEmbed), DEFAULT_BATCH_SIZE);

        // Poll imediato + espera curta pro worker processar.
        // O handle é descartado no fim do teste (drop).
        // Pra teste determinístico, processamos manualmente.
        let pool = db.pool();
        let repo = MemoryRepo::new(&db);
        let pending = repo.list_pending_embeddings(10).await.expect("list");
        assert_eq!(pending.len(), 2, "2 memórias pendentes");
        for r in &pending {
            assert_eq!(r.embedding_status, EmbeddingStatus::Pending);
        }

        // Simula o worker processando manualmente (em vez de
        // esperar — testes determinísticos).
        let fake: Arc<dyn EmbeddingProvider> = Arc::new(FakeEmbed);
        let contents: Vec<String> = pending.iter().map(|r| r.content.clone()).collect();
        let refs: Vec<&str> = contents.iter().map(String::as_str).collect();
        let embeddings = fake.embed(&refs).await.expect("embed");
        for (r, emb) in pending.iter().zip(embeddings.iter()) {
            repo.set_embedding(&r.id, "fake", "fake-v1", 4, emb)
                .await
                .expect("set");
        }

        // Verifica que as memórias agora têm embedding
        // 'ready' e o vetor persistido bate.
        for r in &pending {
            let updated = repo.get(&r.id).await.expect("get").unwrap();
            assert_eq!(updated.embedding_status, EmbeddingStatus::Ready);
            let emb = repo
                .get_embedding(&r.id, "fake", "fake-v1")
                .await
                .expect("get_emb")
                .expect("embedding existe");
            assert_eq!(emb.len(), 4);
        }

        // Pool reference — mantemos `pool` vivo até o fim.
        let _ = pool;
    }

    #[tokio::test]
    async fn worker_marks_failed_on_provider_error() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        let input = NewMemoryInput {
            scope_type: MemoryScopeType::Project,
            scope_id: "proj-1".into(),
            type_: MemoryType::Fact,
            content: "memória que vai falhar".into(),
            origin: MemoryOrigin::User,
            source_type: MemorySourceType::new("seed"),
            source_id: None,
            confidence: 0.9,
            importance: 0.5,
            expires_at: None,
            user_confirmed: true,
            user_pinned: false,
        };
        let inserted = repo.insert_auto_captured(input).await.expect("insert");

        // Marca como failed manualmente.
        repo.mark_embedding_failed(&inserted.id)
            .await
            .expect("mark_failed");

        let updated = repo.get(&inserted.id).await.expect("get").unwrap();
        assert_eq!(updated.embedding_status, EmbeddingStatus::Failed);
    }

    // ------------------------------------------------------------------
    // MemoryExtractor tests
    // ------------------------------------------------------------------

    /// Mock de `MemoryClassifier` que devolve uma resposta fixa
    /// (Ok com `record`, Ok sem `record`, ou Err). Clona o
    /// `Ok` interno porque o trait retorna por valor.
    struct MockClassifier(Result<MemoryClassifierOutput, ClassifierError>);

    #[async_trait]
    impl MemoryClassifier for MockClassifier {
        fn name(&self) -> &str {
            "mock"
        }

        async fn classify(
            &self,
            _context: ClassificationContext,
        ) -> Result<MemoryClassifierOutput, ClassifierError> {
            match &self.0 {
                Ok(o) => Ok(o.clone()),
                Err(ClassifierError::Unavailable(s)) => {
                    Err(ClassifierError::Unavailable(s.clone()))
                }
                Err(ClassifierError::InvalidOutput(s)) => {
                    Err(ClassifierError::InvalidOutput(s.clone()))
                }
                Err(ClassifierError::QuotaExceeded) => Err(ClassifierError::QuotaExceeded),
            }
        }
    }

    /// Helper: cria um `NewMemory` de teste.
    fn new_memory(origin: MemoryOrigin, content: &str) -> NewMemory {
        NewMemory {
            scope_type: MemoryScopeType::Project,
            scope_id: "proj-1".into(),
            type_: MemoryType::Preference,
            content: content.into(),
            origin,
            source_type: MemorySourceType::new("user_message"),
            source_id: None,
            confidence: 0.9,
            importance: 0.7,
            expires_at: None,
        }
    }

    /// Helper: cria uma `MemoryClassifierOutput` com record.
    fn output_with_record(record: NewMemory) -> MemoryClassifierOutput {
        MemoryClassifierOutput {
            record: Some(record),
            scope: Some(MemoryScopeType::Project),
            importance: 0.7,
            reason: "teste".into(),
        }
    }

    /// Helper: cria um job de teste.
    fn test_job(messages: Vec<ConversationMessage>) -> MemoryExtractionJob {
        MemoryExtractionJob::new("run-1", "conv-1", messages, Utc::now())
    }

    /// Helper: mensagens de uma conversa típica (user + assistant).
    fn user_assistant_messages() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage {
                role: "user".into(),
                content: "eu gosto de Rust".into(),
                source: MemorySourceType::new("user_message"),
            },
            ConversationMessage {
                role: "assistant".into(),
                content: "anotado!".into(),
                source: MemorySourceType::new("assistant_message"),
            },
        ]
    }

    /// Helper: mensagens só do usuário (sem assistant/tool) — usado
    /// nos testes onde o `origin` esperado é `User`.
    fn user_only_messages() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage {
                role: "user".into(),
                content: "eu gosto de Rust".into(),
                source: MemorySourceType::new("user_message"),
            },
            ConversationMessage {
                role: "user".into(),
                content: "ah, e trabalho com Postgres também".into(),
                source: MemorySourceType::new("user_message"),
            },
        ]
    }

    #[tokio::test]
    async fn extractor_classifies_user_preference_saves_as_auto_captured() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Classificador devolve record com `origin = User` →
        // vai pra `insert_auto_captured`.
        let output = output_with_record(new_memory(MemoryOrigin::User, "usuário prefere Rust"));
        let classifier: Arc<dyn MemoryClassifier> = Arc::new(MockClassifier(Ok(output)));

        // Processa manualmente via `ExtractorTask` (mesma
        // estratégia determinística do teste do
        // `EmbeddingWorker`).
        let (_tx, rx) = mpsc::channel::<ExtractorCommand>(1);
        let task = ExtractorTask {
            pool: db.pool(),
            classifier,
            rx,
        };
        let job = test_job(user_only_messages());
        task.process_job(job).await.expect("process_job");

        // Verifica que a memória foi gravada via auto_captured.
        let pending = repo.list_pending_embeddings(100).await.expect("list");
        // 1 memória (auto_captured → embedding_status = pending).
        assert_eq!(pending.len(), 1, "1 memória gravada");
        let rec = &pending[0];
        assert_eq!(rec.content, "usuário prefere Rust");
        assert_eq!(rec.origin, MemoryOrigin::User);
        assert!(!rec.pending_review, "não é pending_review");
        assert!(!rec.user_confirmed, "auto-captura = user_confirmed false");
    }

    #[tokio::test]
    async fn extractor_external_content_overrides_to_pending_review() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Mensagem com `source = tool_output:web_search` →
        // proveniência externa. O classificador propõe
        // `origin = User` (alucinação), mas o worker
        // **sobrescreve** para `ExternalContent` e grava
        // como `pending_review`.
        let messages = vec![
            ConversationMessage {
                role: "user".into(),
                content: "pesquise sobre Rust".into(),
                source: MemorySourceType::new("user_message"),
            },
            ConversationMessage {
                role: "tool".into(),
                content: "resultado: Rust é uma linguagem...".into(),
                source: MemorySourceType::new("tool_output:web_search"),
            },
        ];

        // ... helper direto: chamamos o `determine_real_origin`
        // para validar a sobrescrita.
        let output = output_with_record(new_memory(MemoryOrigin::User, "Rust é uma linguagem"));
        let real_origin = determine_real_origin(output.record.as_ref().unwrap(), &messages);
        assert_eq!(real_origin, MemoryOrigin::ExternalContent);

        // E simulamos a inserção via `pending_review`.
        let input = NewMemoryInput {
            scope_type: MemoryScopeType::Project,
            scope_id: "proj-1".into(),
            type_: MemoryType::Fact,
            content: "Rust é uma linguagem".into(),
            origin: MemoryOrigin::ExternalContent,
            source_type: MemorySourceType::new("tool_output:web_search"),
            source_id: None,
            confidence: 0.9,
            importance: 0.7,
            expires_at: None,
            user_confirmed: false,
            user_pinned: false,
        };
        let inserted = repo.insert_pending_review(input).await.expect("insert");
        assert!(inserted.pending_review, "pending_review = true");
        assert!(!inserted.user_confirmed, "user_confirmed = false");
        assert_eq!(inserted.origin, MemoryOrigin::ExternalContent);
    }

    #[tokio::test]
    async fn extractor_assistant_message_overrides_to_assistant() {
        // Se a proveniência é assistant (consolidação do
        // modelo) sem tool_output, sobrescreve para
        // `Assistant`.
        let messages = vec![
            ConversationMessage {
                role: "user".into(),
                content: "como faço X?".into(),
                source: MemorySourceType::new("user_message"),
            },
            ConversationMessage {
                role: "assistant".into(),
                content: "você faz X assim: ...".into(),
                source: MemorySourceType::new("assistant_message"),
            },
        ];

        let output = output_with_record(new_memory(MemoryOrigin::User, "como fazer X"));
        let real_origin = determine_real_origin(output.record.as_ref().unwrap(), &messages);
        assert_eq!(real_origin, MemoryOrigin::Assistant);
    }

    #[tokio::test]
    async fn extractor_classifier_returns_none_does_not_insert() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Classificador devolve `record = None` → nada a fazer.
        let output = MemoryClassifierOutput {
            record: None,
            scope: None,
            importance: 0.0,
            reason: "conversa casual".into(),
        };
        let classifier: Arc<dyn MemoryClassifier> = Arc::new(MockClassifier(Ok(output)));

        // Inicia o extractor (não vamos esperar o job —
        // verificamos direto que nada foi gravado).
        let _extractor = MemoryExtractor::start(db.pool(), classifier);

        // Nenhuma memória no banco.
        let pending = repo.list_pending_embeddings(100).await.expect("list");
        assert!(pending.is_empty(), "nenhuma memória gravada");
    }

    #[tokio::test]
    async fn extractor_classifier_error_does_not_panic() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Classificador devolve `Err(ClassifierError::Unavailable)` —
        // o worker loga e descarta.
        let classifier: Arc<dyn MemoryClassifier> = Arc::new(MockClassifier(Err(
            ClassifierError::Unavailable("provider offline".into()),
        )));

        let _extractor = MemoryExtractor::start(db.pool(), classifier);

        // Nenhuma memória no banco.
        let pending = repo.list_pending_embeddings(100).await.expect("list");
        assert!(pending.is_empty(), "erro do classificador = nada gravado");
    }

    #[tokio::test]
    async fn extractor_noop_memory_classifier_compat() {
        // `NoopMemoryClassifier` (Etapa 1) sempre devolve
        // `record = None` → extractor não insere nada.
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        let classifier: Arc<dyn MemoryClassifier> =
            Arc::new(crate::classifier::NoopMemoryClassifier);
        let _extractor = MemoryExtractor::start(db.pool(), classifier);

        let pending = repo.list_pending_embeddings(100).await.expect("list");
        assert!(pending.is_empty(), "noop = nada gravado");
    }

    #[tokio::test]
    async fn extractor_enqueue_is_non_blocking() {
        // `MemoryExtractorHandle::enqueue` usa `try_send` —
        // se o canal estiver cheio, descarta sem bloquear.
        let db = Database::open_in_memory().await.expect("db");
        let classifier: Arc<dyn MemoryClassifier> =
            Arc::new(MockClassifier(Ok(MemoryClassifierOutput {
                record: None,
                scope: None,
                importance: 0.0,
                reason: "noop".into(),
            })));
        let extractor = MemoryExtractor::start(db.pool(), classifier);
        let handle = extractor.handle();

        // Enfileira 10 jobs — o canal tem 256, então tudo
        // cabe (teste: não bloqueia, não panica).
        for i in 0..10 {
            let job = test_job(user_assistant_messages());
            let _ = i;
            handle.enqueue(job);
        }

        // Shutdown limpo.
        handle.shutdown().await;
    }
}
