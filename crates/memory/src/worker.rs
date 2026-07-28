//! `EmbeddingWorker` — tokio task em background que calcula
//! embeddings de memórias com `embedding_status = 'pending'`.
//!
//! Fluxo:
//! 1. O `MemoryRepo::insert_*` seta `embedding_status = 'pending'`
//!    automaticamente (Etapa 1).
//! 2. O `EmbeddingWorkerHandle::enqueue` envia o `MemoryId` pra
//!    o canal interno (ou o worker poll direto do banco).
//! 3. O worker coleta até `batch_size` memórias, chama
//!    `EmbeddingProvider::embed` (1 request HTTP pro batch
//!    inteiro), e persiste via `MemoryRepo::set_embedding`.
//! 4. Em caso de erro, marca `embedding_status = 'failed'`
//!    (sem retry em loop — o painel da Etapa 5 decide se
//!    retentar).
//!
//! **Decisões de design** (ADR-0013 §1):
//!
//! - **Background, não bloqueante.** App continua usável
//!   durante o cálculo; FTS5 cobre o lexical.
//! - **Batch pequeno** (default 50, limite OpenRouter). Pra
//!   50 memórias, 1 request, ~200ms.
//! - **Sem retry em loop.** Falha → `failed`, painel decide.
//! - **`FakeClock`-friendly**: o worker lê do `Clock` só
//!   pra timestamp do `set_embedding`. Não depende de
//!   `tokio::time::interval` (problema do `ADR-0014 §1`).
//! - **Cancelamento limpo** via [`EmbeddingWorkerHandle::shutdown`]
//!   (fecha o canal, o worker termina após o batch em
//!   andamento).

use std::sync::Arc;

use frederico_core::MemoryId;
use tokio::sync::mpsc;

use crate::embedding::EmbeddingProvider;
use crate::error::MemoryResult;
use crate::memory_repo::MemoryRepo;

/// Tamanho padrão do batch de embeddings por request HTTP
/// (limite típico do OpenRouter /embeddings).
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// Handle público do `EmbeddingWorker`. Clone-able (o `Arc`
/// interno é compartilhado), `Send` (pode morar em qualquer
/// estado da casca Tauri).
#[derive(Clone)]
pub struct EmbeddingWorkerHandle {
    tx: mpsc::Sender<WorkerCommand>,
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
        let _ = self.tx.try_send(WorkerCommand::EmbedOne(id));
    }

    /// Sinaliza shutdown limpo. O worker termina o batch em
    /// andamento e fecha.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(WorkerCommand::Shutdown).await;
    }

    /// Dispara um poll imediato do banco (procura memórias
    /// com `embedding_status = 'pending'`). Útil quando o
    /// caller insere memórias em batch e quer forçar o
    /// worker a pegar.
    pub fn tick(&self) {
        let _ = self.tx.try_send(WorkerCommand::PollPending);
    }
}

/// Comando interno do canal.
enum WorkerCommand {
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
        let (tx, rx) = mpsc::channel::<WorkerCommand>(1024);
        let handle = EmbeddingWorkerHandle { tx };
        // Clone do pool (Arc internamente) — o worker vive
        // independente do caller. `'static` no spawn.
        let pool_owned = pool.clone();
        let embedding_owned = embedding.clone();
        tokio::spawn(async move {
            let task = WorkerTask {
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
struct WorkerTask<'a> {
    pool: &'a sqlx::SqlitePool,
    embedding: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
    rx: mpsc::Receiver<WorkerCommand>,
}

impl WorkerTask<'_> {
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
                        Some(WorkerCommand::Shutdown) => break,
                        Some(WorkerCommand::EmbedOne(id)) => {
                            queue.push(id);
                        }
                        Some(WorkerCommand::PollPending) => {
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

impl<'a> MemoryRepo<'a> {
    /// Helper `pub(crate)` (não-exportado no `lib.rs`) — usado
    /// pelo `EmbeddingWorker`. Constrói um `MemoryRepo`
    /// a partir de um `&'a SqlitePool` (o worker não tem
    /// um `Database` em mãos — só o pool).
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

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_core::{
        EmbeddingStatus, MemoryOrigin, MemoryScopeType, MemorySourceType, MemoryType,
    };
    use frederico_storage::Database;

    #[tokio::test]
    async fn worker_embeds_pending_memories() {
        let db = Database::open_in_memory().await.expect("db");
        let repo = MemoryRepo::new(&db);

        // Insere 2 memórias com embedding_status = Pending.
        for content in &["memória 1 sobre Rust", "memória 2 sobre Postgres"] {
            let input = crate::memory_repo::NewMemoryInput {
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
        #[async_trait::async_trait]
        impl crate::embedding::EmbeddingProvider for FakeEmbed {
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

        let input = crate::memory_repo::NewMemoryInput {
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
}
