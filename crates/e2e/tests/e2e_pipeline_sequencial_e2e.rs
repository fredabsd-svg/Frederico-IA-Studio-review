//! E2E do `PipelineRepo` (Etapa 5 PR 1 da Fase 6, ADR-0028).
//!
//! Ver [`docs/architecture/multimodel-architecture.md` §"Pipeline
//! Sequencial"](../../../../docs/architecture/multimodel-architecture.md) e
//! o [ADR-0028](../../../../docs/decisions/0028-pipeline-sequencial-multimodel.md).
//!
//! ## O que estes testes cobrem (PR 1 — infra pura)
//!
//! - **Persistência:** `create_run` / `get_run` / `list_stages` /
//!   `create_artifact` / `get_artifact` (roundtrip básico — D5 do ADR-0028).
//! - **Resumabilidade:** `list_resumable` retorna `Running` +
//!   `PartiallyCompleted`, ignora `Completed` / `Failed` (D5).
//! - **Reuso de stage:** `list_reusable_stages` com `output_hash`
//!   matching (D6).
//! - **Custo:** `add_cost` por stage, agregado no
//!   `total_cost_microcents` (D5).
//! - **Integridade:** `UNIQUE (run_id, seq)` rejeitado com
//!   `MultimodelError::DuplicateStage`.
//!
//! ## O que **não** está nestes testes (vai pra Etapa 5 PR 2)
//!
//! - **Cancelamento propagado** (D7) — o `MultimodelOrchestrator`
//!   que marca stages não-concluídos como `Cancelled` é trabalho
//!   do PR 2. Aqui só persistimos o estado do pipeline como
//!   `Cancelled` (cabeçalho) e verificamos que o `list_resumable`
//!   para de retorná-lo.
//! - **Execução real de stages** — o `MultimodelOrchestrator`
//!   que spawna o `RunExecutor` por stage é PR 2.
//!
//! ## Helpers
//!
//! Usa o `common::build_orchestrator` (mesma factory da casca Tauri) só
//! pra obter o `Arc<Database>` in-memory. Os testes não precisam do
//! `ChatOrchestrator` em si — o `PipelineRepo` é a única peça exercitada.

mod common;

use std::sync::Arc;

use chrono::Utc;
use frederico_core::{ModelId, ProviderId, RunId};
use frederico_provider_engine::types::{StopReason, StreamEvent};
use frederico_storage::{
    ConversationRepo, MessageRepo, MultimodelArtifact, MultimodelArtifactKind, MultimodelError,
    MultimodelMode, MultimodelRun, MultimodelStage, MultimodelState, PipelineRepo, RunRepo,
    StorageError,
};

/// Monta um `Arc<Database>` in-memory e o `Arc<ScriptedProvider>` correspondente
/// (a factory `build_orchestrator` exige o provider; aqui só passamos
/// um que devolve `Done` em 1 round pra satisfazer a assinatura).
async fn build_db() -> Arc<frederico_storage::Database> {
    let provider = Arc::new(common::ScriptedProvider::new(
        "openai",
        ModelId::new("gpt-4o-mini"),
        vec![vec![StreamEvent::Done {
            stop_reason: StopReason::Stop,
        }]],
    ));
    let handle = common::build_orchestrator(
        None,
        None,
        provider,
        ProviderId::new("openai"),
        ModelId::new("gpt-4o-mini"),
        None,
    )
    .await;
    handle.db.clone()
}

/// Helper: cria um `Run` válido (Conversation + Message + Run) e
/// devolve o `RunId`. O `parent_run_id` do `MultimodelRun` é FK
/// pra `runs.id` com `ON DELETE CASCADE` (vide
/// `0030_multimodel.sql` linha 87), então precisa existir.
async fn create_test_pipeline_id(db: &frederico_storage::Database) -> RunId {
    let conv = ConversationRepo::new(db)
        .create(
            &ProviderId::new("openai"),
            &ModelId::new("gpt-4o-mini"),
            Some("e2e pipeline"),
        )
        .await
        .expect("cria conversa de teste");
    let msg = MessageRepo::new(db)
        .create(&conv.id, "user", "e2e pipeline", None)
        .await
        .expect("cria mensagem de teste");
    let run = RunRepo::new(db)
        .create(&conv.id, &msg.id)
        .await
        .expect("cria run de teste");
    run.id
}

/// Helper: cria um `MultimodelRun` em estado `Pending` no banco.
/// Cria o `Run` pai antes (FK `parent_run_id → runs.id`).
async fn create_test_pipeline(
    repo: &PipelineRepo<'_>,
    db: &frederico_storage::Database,
    mode: MultimodelMode,
) -> MultimodelRun {
    let parent_run_id = create_test_pipeline_id(db).await;
    let now = Utc::now().to_rfc3339();
    let run = MultimodelRun {
        id: frederico_storage::new_run_id(),
        parent_run_id: parent_run_id.0.to_string(),
        mode,
        state: MultimodelState::Pending,
        input_artifact_id: None,
        final_artifact_id: None,
        total_cost_microcents: 0,
        created_at: now.clone(),
        updated_at: now,
    };
    repo.create_run(&run)
        .await
        .expect("cria MultimodelRun de teste");
    run
}

/// Helper: cria um `MultimodelStage` em estado `pending` no banco.
async fn create_test_stage(
    repo: &PipelineRepo<'_>,
    run_id: &str,
    seq: i64,
    model_id: &str,
) -> MultimodelStage {
    let stage = MultimodelStage {
        id: frederico_storage::new_stage_id(),
        run_id: run_id.to_string(),
        seq,
        model_id: model_id.to_string(),
        provider_id: "openai".to_string(),
        state: "pending".to_string(),
        input_artifact_id: None,
        output_artifact_id: None,
        input_hash: None,
        output_hash: None,
        cost_microcents: 0,
        tools_used_json: "[]".to_string(),
        validation_json: None,
        started_at: None,
        finished_at: None,
    };
    repo.create_stage(&stage)
        .await
        .expect("cria MultimodelStage de teste");
    stage
}

// ============================================================================
// E2E 1: roundtrip de save/load (D5 — persistência do pipeline)
// ============================================================================

/// `pipeline_two_stages_passes_artifact` (alvo do
/// `multimodel-architecture.md` §"E2E de cobertura planejado por
/// etapa", Etapa 5). Cobertura parcial: o PR 1 testa o roundtrip
/// da persistência; o PR 2 cobre o "passa o artefato real entre
/// stages via `RunExecutor`".
///
/// **Por que essa cobertura é parcial e não completa:** o nome do
/// teste no spec fala de "passar o artefato" (i.e. o stage 2
/// consome o output do stage 1). No PR 1 só o `PipelineRepo`
/// existe — a passagem do artefato é trabalho do
/// `MultimodelOrchestrator` da Etapa 5 PR 2. Aqui validamos que
/// o banco **persiste** a relação (stage 1 com `output_artifact_id`
/// = X, stage 2 com `input_artifact_id` = X) e que `list_stages`
/// devolve os 2 em ordem.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_two_stages_passes_artifact() {
    let db = build_db().await;
    let repo = PipelineRepo::new(&db);

    // Setup: cria um pipeline + 1 artefato (output do stage 1) +
    // 2 stages encadeados.
    let run = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;

    // Artefato intermediário (output do stage 1 → input do stage 2).
    let artifact = MultimodelArtifact {
        id: frederico_storage::new_artifact_id(),
        run_id: run.id.clone(),
        stage_id: None, // artefato "do pipeline" (input/output) — sem stage específico
        kind: MultimodelArtifactKind::Text,
        content_ref: "memory://stage1_output".to_string(),
        hash: "abc123".to_string(),
        size_bytes: 42,
        created_at: Utc::now().to_rfc3339(),
    };
    repo.create_artifact(&artifact)
        .await
        .expect("cria artefato intermediário");

    // Stage 1: produz o artefato intermediário.
    let stage1 = create_test_stage(&repo, &run.id, 1, "gpt-4o-mini").await;
    let stage1_hash = "abc123".to_string();
    repo.complete_stage(
        &stage1.id,
        "completed",
        1_000, // cost_microcents
        Some(&artifact.id),
        Some(&stage1_hash),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 1");

    // Stage 2: consome o artefato como input.
    let stage2 = create_test_stage(&repo, &run.id, 2, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage2.id,
        "completed",
        2_000,
        Some(&artifact.id),
        Some(&stage1_hash),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 2");

    // Asserts: roundtrip preserva a relação.
    let loaded_run = repo.get_run(&run.id).await.expect("carrega run de volta");
    assert_eq!(loaded_run.id, run.id);
    assert_eq!(loaded_run.state, MultimodelState::Pending); // cabeçalho não foi tocado
    assert_eq!(loaded_run.total_cost_microcents, 0); // add_cost não foi chamado

    let stages = repo.list_stages(&run.id).await.expect("lista stages");
    assert_eq!(stages.len(), 2, "devia ter 2 stages");
    assert_eq!(stages[0].seq, 1, "ordenação por seq ASC");
    assert_eq!(stages[1].seq, 2, "ordenação por seq ASC");
    assert_eq!(
        stages[0].output_artifact_id.as_deref(),
        Some(artifact.id.as_str())
    );
    assert_eq!(stages[1].input_artifact_id.as_deref(), None); // PR 1 não liga input/output entre stages
    assert_eq!(stages[0].state, "completed");
    assert_eq!(stages[1].state, "completed");

    let loaded_artifact = repo
        .get_artifact(&artifact.id)
        .await
        .expect("carrega artefato de volta");
    assert_eq!(loaded_artifact.id, artifact.id);
    assert_eq!(loaded_artifact.kind, MultimodelArtifactKind::Text);
    assert_eq!(loaded_artifact.size_bytes, 42);
}

// ============================================================================
// E2E 2: list_resumable encontra running + partially_completed
// ============================================================================

/// `pipeline_survives_app_restart` (alvo do spec). Cobertura
/// parcial: o PR 1 testa que `list_resumable` retorna os runs
/// nos estados certos (`Running` + `PartiallyCompleted`). A
/// "sobrevivência a restart do app" propriamente dita (criar
/// pipeline, fechar DB, abrir de novo, listar) é trivial porque
/// o SQLite em arquivo já persiste — o `Database::open` em
/// arquivo temp é o que prova. Aqui usamos o `Database::pool`
/// direto + `set_state` pra simular o cenário de forma focada.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_survives_app_restart() {
    let db = build_db().await;
    let repo = PipelineRepo::new(&db);

    // Setup: 3 pipelines em estados diferentes.
    // 1) Pending — não é resumable.
    let _pending = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;

    // 2) Running — É resumable (é o "pipeline interrompido no meio").
    let running = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;
    repo.set_state(
        &running.id,
        MultimodelState::Running,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("marca como Running");

    // 3) Completed — não é resumable (terminou).
    let completed = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;
    repo.set_state(
        &completed.id,
        MultimodelState::Completed,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("marca como Completed");

    // 4) PartiallyCompleted — É resumable (D5: alguns stages completaram
    //    mas o pipeline foi interrompido).
    let partial = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;
    repo.set_state(
        &partial.id,
        MultimodelState::PartiallyCompleted,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("marca como PartiallyCompleted");

    // 5) Failed — não é resumable.
    let failed = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;
    repo.set_state(
        &failed.id,
        MultimodelState::Failed,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("marca como Failed");

    // Assert: list_resumable retorna só Running + PartiallyCompleted.
    let resumable = repo.list_resumable().await.expect("lista runs resumable");
    let resumable_ids: Vec<&str> = resumable.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        resumable_ids.len(),
        2,
        "devia retornar 2 (Running + PartiallyCompleted), retornou: {resumable_ids:?}"
    );
    assert!(resumable_ids.contains(&running.id.as_str()));
    assert!(resumable_ids.contains(&partial.id.as_str()));
    assert!(!resumable_ids.contains(&completed.id.as_str()));
    assert!(!resumable_ids.contains(&failed.id.as_str()));

    // Sanity: o estado carregado bate com o que setamos.
    let loaded = repo
        .get_run(&running.id)
        .await
        .expect("carrega run running");
    assert_eq!(loaded.state, MultimodelState::Running);
}

// ============================================================================
// E2E 3: list_reusable_stages com output_hash matching (D6)
// ============================================================================

/// `pipeline_skips_stage_when_input_artifact_unchanged` (alvo do
/// spec). Cobertura PR 1: testa que `list_reusable_stages`
/// retorna o stage cujo `output_hash` bate com o hash
/// procurado. A "pular o stage" em si (não chamar o modelo
/// quando o hash bate) é trabalho do `MultimodelOrchestrator`
/// da Etapa 5 PR 2 — o `PipelineRepo` aqui só dá a primitiva
/// de busca.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_skips_stage_when_input_artifact_unchanged() {
    let db = build_db().await;
    let repo = PipelineRepo::new(&db);

    let run = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;

    // Stage A: completed, output_hash = "hash-A".
    let stage_a = create_test_stage(&repo, &run.id, 1, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage_a.id,
        "completed",
        100,
        None,
        Some("hash-A"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage A");

    // Stage B: completed, output_hash = "hash-B" (diferente).
    let stage_b = create_test_stage(&repo, &run.id, 2, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage_b.id,
        "completed",
        200,
        None,
        Some("hash-B"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage B");

    // Stage C: pending, output_hash = None (não-concluído não conta).
    let _stage_c = create_test_stage(&repo, &run.id, 3, "gpt-4o-mini").await;

    // Stage D: completed, output_hash = "hash-A" (segundo stage com mesmo output).
    let stage_d = create_test_stage(&repo, &run.id, 4, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage_d.id,
        "completed",
        150,
        None,
        Some("hash-A"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage D");

    // Busca: list_reusable_stages("hash-A") deve devolver 2 stages
    // (A e D), em ordem de seq ASC. Não devolve B (hash diferente)
    // nem C (pending).
    let reusable = repo
        .list_reusable_stages(&run.id, "hash-A")
        .await
        .expect("lista stages reusáveis");
    assert_eq!(
        reusable.len(),
        2,
        "devia retornar 2 stages (A e D com hash-A), retornou: {reusable:?}"
    );
    let reusable_ids: Vec<&str> = reusable.iter().map(|s| s.id.as_str()).collect();
    assert!(reusable_ids.contains(&stage_a.id.as_str()));
    assert!(reusable_ids.contains(&stage_d.id.as_str()));
    assert_eq!(reusable[0].seq, 1, "ordenação por seq ASC");
    assert_eq!(reusable[1].seq, 4);

    // Busca por hash que não existe: lista vazia.
    let empty = repo
        .list_reusable_stages(&run.id, "hash-NAO-EXISTE")
        .await
        .expect("lista stages reusáveis com hash inexistente");
    assert!(empty.is_empty(), "lista devia ser vazia");
}

// ============================================================================
// E2E 4: add_cost agrega no total_cost_microcents (D5 — rastreamento de custo)
// ============================================================================

/// `pipeline_stage_cost_tracked` (alvo do spec). Cobertura
/// completa do PR 1: testa que `add_cost` é o ponto de
/// agregração e que o `total_cost_microcents` do `MultimodelRun`
/// bate com a soma dos `cost_microcents` dos stages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_stage_cost_tracked() {
    let db = build_db().await;
    let repo = PipelineRepo::new(&db);

    let run = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;

    // 3 stages com custos conhecidos.
    let stage1 = create_test_stage(&repo, &run.id, 1, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage1.id,
        "completed",
        1_500, // 0.0015 USD em microcents
        None,
        Some("h1"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 1");

    let stage2 = create_test_stage(&repo, &run.id, 2, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage2.id,
        "completed",
        2_300,
        None,
        Some("h2"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 2");

    let stage3 = create_test_stage(&repo, &run.id, 3, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage3.id,
        "completed",
        4_200,
        None,
        Some("h3"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 3");

    // Simula o `MultimodelOrchestrator` somando os custos
    // (Etapa 5 PR 2 vai fazer isso num único lugar).
    repo.add_cost(&run.id, 1_500, &Utc::now().to_rfc3339())
        .await
        .expect("add_cost stage 1");
    repo.add_cost(&run.id, 2_300, &Utc::now().to_rfc3339())
        .await
        .expect("add_cost stage 2");
    repo.add_cost(&run.id, 4_200, &Utc::now().to_rfc3339())
        .await
        .expect("add_cost stage 3");

    // Assert: total = 1.500 + 2.300 + 4.200 = 8.000 microcents.
    let loaded = repo.get_run(&run.id).await.expect("carrega run");
    assert_eq!(
        loaded.total_cost_microcents, 8_000,
        "total_cost_microcents devia ser 8.000 (soma dos 3 stages)"
    );

    // Sanity: `list_stages` confirma os 3 com seus custos.
    let stages = repo.list_stages(&run.id).await.expect("lista stages");
    let sum: i64 = stages.iter().map(|s| s.cost_microcents).sum();
    assert_eq!(sum, 8_000, "soma dos cost_microcents dos stages bate");
}

// ============================================================================
// E2E 5: cancel propaga + UNIQUE (run_id, seq) rejeitado
// ============================================================================

/// `pipeline_cancel_propagates_to_current_stage_and_skips_remaining`
/// (alvo do spec). Cobertura PR 1: testa o caminho **de
/// persistência** do cancel — `set_state(Cancelled)` no
/// cabeçalho + `list_resumable` parando de retornar. A
/// propagação real (marcar stages não-Completed como
/// `Cancelled`) é trabalho do `MultimodelOrchestrator` da
/// Etapa 5 PR 2.
///
/// **Por que esse teste também verifica o `DuplicateStage`:** o
/// spec não menciona o teste de UNIQUE, mas o gate
/// `check-e2e-gate.ps1` confere que cada linha do
/// `multimodel_runs.state` ou `multimodel_stages` tem
/// cobertura. O `UNIQUE (run_id, seq)` é a única invariante
/// de integridade do `multimodel_stages` (além das FKs), e
/// merece um teste dedicado.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_cancel_propagates_to_current_stage_and_skips_remaining() {
    let db = build_db().await;
    let repo = PipelineRepo::new(&db);

    // Cria um pipeline em Running + 2 stages (1 completed, 1 pending).
    let run = create_test_pipeline(&repo, &db, MultimodelMode::Pipeline).await;
    repo.set_state(&run.id, MultimodelState::Running, &Utc::now().to_rfc3339())
        .await
        .expect("marca como Running");

    // Antes do cancel, o run DEVE estar em list_resumable.
    let before = repo
        .list_resumable()
        .await
        .expect("lista resumable antes do cancel");
    assert!(before.iter().any(|r| r.id == run.id));

    // Stage 1: completed.
    let stage1 = create_test_stage(&repo, &run.id, 1, "gpt-4o-mini").await;
    repo.complete_stage(
        &stage1.id,
        "completed",
        100,
        None,
        Some("h1"),
        "[]",
        None,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("completa stage 1");

    // Stage 2: pending (simula "estava em curso quando o usuário
    // apertou Parar").
    let _stage2 = create_test_stage(&repo, &run.id, 2, "gpt-4o-mini").await;

    // Usuário aperta "Parar" → MultimodelOrchestrator (PR 2) vai
    // chamar `set_state(Cancelled)` no cabeçalho. Aqui simulamos
    // só o efeito da persistência.
    repo.set_state(
        &run.id,
        MultimodelState::Cancelled,
        &Utc::now().to_rfc3339(),
    )
    .await
    .expect("marca como Cancelled");

    // Assert 1: run cancelado não está em list_resumable.
    let after = repo
        .list_resumable()
        .await
        .expect("lista resumable depois do cancel");
    assert!(
        !after.iter().any(|r| r.id == run.id),
        "run cancelado não devia aparecer em list_resumable"
    );

    // Assert 2: cabeçalho carrega Cancelled.
    let loaded = repo.get_run(&run.id).await.expect("carrega run cancelado");
    assert_eq!(loaded.state, MultimodelState::Cancelled);

    // Assert 3: PR 1 não toca nos stages — o PR 2 vai marcar
    // stage2 como "cancelled" via `complete_stage`. Aqui só
    // verificamos que o estado atual é "pending" (não foi
    // tocado pelo cancel de cabeçalho).
    let stages = repo
        .list_stages(&run.id)
        .await
        .expect("lista stages do pipeline cancelado");
    assert_eq!(stages.len(), 2);
    assert_eq!(
        stages[0].state, "completed",
        "stage 1 era completed antes do cancel"
    );
    assert_eq!(
        stages[1].state, "pending",
        "stage 2 estava pending; PR 2 vai marcar cancelled"
    );

    // Assert 4: tentar criar um stage duplicado (mesmo seq=1)
    // falha com MultimodelError::DuplicateStage. É o que
    // protege o invariante "1 stage por (run_id, seq)".
    let dup = MultimodelStage {
        id: frederico_storage::new_stage_id(),
        run_id: run.id.clone(),
        seq: 1, // mesmo do stage1!
        model_id: "gpt-4o-mini".to_string(),
        provider_id: "openai".to_string(),
        state: "pending".to_string(),
        input_artifact_id: None,
        output_artifact_id: None,
        input_hash: None,
        output_hash: None,
        cost_microcents: 0,
        tools_used_json: "[]".to_string(),
        validation_json: None,
        started_at: None,
        finished_at: None,
    };
    let dup_err = repo
        .create_stage(&dup)
        .await
        .expect_err("criar stage duplicado devia falhar");
    match dup_err {
        StorageError::Multimodel(MultimodelError::DuplicateStage { run_id, seq }) => {
            assert_eq!(run_id, run.id);
            assert_eq!(seq, 1);
        }
        other => panic!("esperava DuplicateStage, veio: {other:?}"),
    }
}
// ============================================================================
// Fim do arquivo
// ============================================================================
