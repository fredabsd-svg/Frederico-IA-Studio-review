//! Recovery de crash no startup (Fase 3, Etapa 5.x).
//!
//! ## O problema
//!
//! O `RunExecutor` (Etapa 4) é responsável por manter o `Run` vivo:
//! persiste eventos, atualiza `runs.state` granular, e marca
//! `runs.last_heartbeat_at` a cada iteração. Se a casca Tauri crashar
//! no meio de um run (Tauri crash, Windows reinicia, OOM, etc.), o
//! `Run` fica preso num estado não-terminal com `last_heartbeat_at`
//! antigo — e a próxima vez que o app abrir, o usuário vai ver
//! "rodando" pra sempre, sem nunca terminar.
//!
//! ## A solução
//!
//! Quando a casca Tauri abre o banco no startup, ela dispara uma
//! task de recovery:
//!
//! 1. Lista runs não-terminais (`state NOT IN ('completed', 'failed',
//!    'cancelled', 'interrupted')`) cujo `last_heartbeat_at` é mais
//!    velho que `threshold_seconds` segundos atrás.
//! 2. Pra cada um, marca como `interrupted` (terminal — o app
//!    crashou). A view `runs_with_status` mapeia `interrupted →
//!    timeout`.
//! 3. Loga quantos foram marcados. A UI pode listar pra o usuário
//!    revisar.
//!
//! O threshold default é **120s** (2 min) — mais do que o
//! `event_timeout` do executor (60s) porque o executor pode estar
//! esperando o `delta` final do provider. Se passou 2 min sem
//! heartbeat, o run é considerado morto.
//!
//! ## Por que aqui, não no `provider-engine`?
//!
//! A `RunRepo` está no `storage` (já usado pelo `execution-engine`).
//! O recovery é lógica de execução, então vive no `execution-engine`
//! (re-exportado no `lib.rs`).

use std::time::Duration;

use frederico_core::RunId;
use frederico_storage::{Database, RunRepo, StorageResult};
use tracing::{info, warn};

/// Threshold default pra considerar um run "morto": 120s. Maior que
/// o `event_timeout` (60s) porque o executor pode estar esperando
/// o delta final do provider.
pub const DEFAULT_STALE_THRESHOLD_SECS: u64 = 120;

/// Roda o recovery de crash: lista runs não-terminais com
/// `last_heartbeat_at` mais velho que `threshold`, marca cada um
/// como `interrupted` (terminal). Devolve o número de runs
/// marcados.
pub async fn recover_stale_runs(
    run_repo: &RunRepo<'_>,
    threshold: Duration,
) -> StorageResult<usize> {
    let threshold_secs = threshold.as_secs();
    let stale = run_repo.list_stale_heartbeats(threshold_secs).await?;
    if stale.is_empty() {
        info!(
            "recovery: nenhum run stale encontrado (threshold = {}s)",
            threshold_secs
        );
        return Ok(0);
    }
    info!(
        "recovery: {} runs stale encontrados (threshold = {}s) — marcando como interrupted",
        stale.len(),
        threshold_secs
    );
    let mut marked = 0usize;
    for run in &stale {
        warn!(
            run_id = %run.id,
            state = %run.state,
            last_heartbeat_at = %run.last_heartbeat_at,
            started_at = %run.started_at,
            "recovery: marcando run stale como interrupted"
        );
        if let Err(e) = run_repo.mark_interrupted(&run.id).await {
            warn!(run_id = %run.id, "recovery: falha ao marcar como interrupted: {e}");
        } else {
            marked += 1;
        }
    }
    info!("recovery: {marked} runs marcados como interrupted");
    Ok(marked)
}

/// Helper que roda o recovery no startup. Recebe um `Database`
/// (que é `Arc<SqlitePool>` internamente — clonar é barato) e
/// constrói o `RunRepo` dentro da task. Devolve o `JoinHandle` se
/// o caller quiser esperar.
///
/// **Por que recebe `Database` em vez de `RunRepo`?** Porque o
/// `RunRepo` tem borrow do `&Database`, e o `tokio::spawn` exige
/// `'static`. Receber o `Database` (que vive em `Arc`) e construir
/// o `RunRepo` dentro do closure resolve isso sem `unsafe`.
pub fn spawn_recover_stale_runs(
    db: Database,
    threshold: Duration,
) -> tokio::task::JoinHandle<StorageResult<usize>> {
    tokio::spawn(async move {
        let run_repo = RunRepo::new(&db);
        recover_stale_runs(&run_repo, threshold).await
    })
}

/// Helper síncrono: dado um `Vec<RunId>` retornado por
/// `list_stale_heartbeats`, marca cada um como `interrupted`.
/// Útil pra testes que não querem o overhead de `spawn`.
pub async fn mark_all_interrupted(
    run_repo: &RunRepo<'_>,
    run_ids: &[RunId],
) -> StorageResult<usize> {
    let mut n = 0;
    for id in run_ids {
        run_repo.mark_interrupted(id).await?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use frederico_core::ProviderId;
    use frederico_storage::{ConversationRepo, Database, MessageRepo, RunStatus};

    fn tempdir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let unique = format!(
            "frederico-exec-recovery-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            n,
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn recover_marks_only_stale_runs() {
        let db = Database::open(&tempdir().join("r1.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);

        // Cria 2 conversas, cada uma com 1 message e 1 run.
        let conv1 = conv_repo
            .create(
                &ProviderId::new("p"),
                &frederico_core::ModelId::new("m"),
                None,
            )
            .await
            .unwrap();
        let conv2 = conv_repo
            .create(
                &ProviderId::new("p"),
                &frederico_core::ModelId::new("m"),
                None,
            )
            .await
            .unwrap();
        let msg1 = msg_repo
            .create(&conv1.id, "user", "oi", None)
            .await
            .unwrap();
        let msg2 = msg_repo
            .create(&conv2.id, "user", "oi", None)
            .await
            .unwrap();
        let run1 = run_repo.create(&conv1.id, &msg1.id).await.unwrap();
        let run2 = run_repo.create(&conv2.id, &msg2.id).await.unwrap();

        // Simula que run1 está stale (heartbeat velho) e run2
        // acabou de criar (heartbeat atual). Usa o helper
        // `force_heartbeat_at_for_test` (escondido por `#[doc(hidden)]`)
        // pra setar o `last_heartbeat_at` 1 hora no passado.
        let one_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        run_repo
            .force_heartbeat_at_for_test(&run1.id, &one_hour_ago)
            .await
            .unwrap();

        // Recovery com threshold de 60s.
        let marked = recover_stale_runs(&run_repo, Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(marked, 1, "esperava 1 run marcado, veio {marked}");

        // run1 está interrupted.
        let r1 = run_repo.get(&run1.id).await.unwrap();
        assert_eq!(r1.state, "interrupted");
        assert!(r1.finished_at.is_some());

        // run2 continua created.
        let r2 = run_repo.get(&run2.id).await.unwrap();
        assert_eq!(r2.state, "created");
        assert!(r2.finished_at.is_none());

        // A view `runs_with_status` mapeia `interrupted → timeout`.
        let _ = run2;
    }

    #[tokio::test]
    async fn recover_skips_terminal_runs() {
        let db = Database::open(&tempdir().join("r2.db")).await.unwrap();
        let conv_repo = ConversationRepo::new(&db);
        let msg_repo = MessageRepo::new(&db);
        let run_repo = RunRepo::new(&db);

        let conv = conv_repo
            .create(
                &ProviderId::new("p"),
                &frederico_core::ModelId::new("m"),
                None,
            )
            .await
            .unwrap();
        let msg = msg_repo.create(&conv.id, "user", "oi", None).await.unwrap();
        let run = run_repo.create(&conv.id, &msg.id).await.unwrap();

        // Marca o run como completed (terminal) com heartbeat velho.
        run_repo
            .set_status(&run.id, RunStatus::Completed)
            .await
            .unwrap();
        run_repo
            .set_state(&run.id, frederico_agent_engine::RunState::Completed)
            .await
            .unwrap();
        let one_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        run_repo
            .force_heartbeat_at_for_test(&run.id, &one_hour_ago)
            .await
            .unwrap();

        // Recovery: deve pular runs terminais.
        let marked = recover_stale_runs(&run_repo, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(marked, 0, "runs terminais não devem ser marcados");

        // Continua completed.
        let r = run_repo.get(&run.id).await.unwrap();
        assert_eq!(r.state, "completed");
    }
}
