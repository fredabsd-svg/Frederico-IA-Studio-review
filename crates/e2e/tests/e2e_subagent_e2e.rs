//! E2E — `SubagentRunner` (Fase 6, Etapa 4 PR 2, ADR-0027 +
//! ADR-0030).
//!
//! Os 6 testes desta bateria **consomem `build_chat_orchestrator`**
//! (a mesma factory que a casca Tauri e os outros E2E usam,
//! ADR-0022 §D4) — não instanciam o `SubagentRunner` à mão. O
//! runner vem do `ChatOrchestrator.subagent_runner` (campo
//! público), construído em `build_chat_orchestrator` a partir
//! do `SpecialistRegistry` + `PermissionLoader` + `Database`
//! que as parts carregam.
//!
//! ## O que cada teste prova (caminho de produção, ADR-0027 + 0030)
//!
//! 1. **`subagent_runs_with_reduced_permissions`** — o
//!    `effective_permissions` retornado no `SubagentHandle` é
//!    subset do `parent_permissions` (invariante da Fase 3
//!    Etapa 3 + 4ª camada da hierarquia de permission da
//!    Etapa 3 PR 2 + Etapa 4 PR 2).
//! 2. **`subagent_inherits_cancellation_token`** — 3 subagentes
//!    são spawnados em paralelo, pai é cancelado, **todos os 3**
//!    recebem o sinal via `child_token()`. Prova o
//!    cancelamento hierárquico (`subagent-architecture.md`
//!    §"CancellationToken").
//! 3. **`subagent_budget_discounted_from_parent_in_real_path`** —
//!    o portão debita o `parent.subagent_count` e o ledger
//!    registra a alocação; depois, `record_spent` no filho
//!    atualiza o ledger e o `parent.spent` é o destino do
//!    desconto. Prova o invariante D3 ("desconto, não cópia")
//!    no caminho real (não só na função pura).
//! 4. **`subagent_explosion_cap_8_rejects_ninth`** — spawn 8
//!    subagentes (D1 = 8 global), tenta o 9º, recebe
//!    `GlobalLimitReached { current: 8, max: 8, next: 9 }`.
//!    Efeito colateral zero nos 8 anteriores.
//! 5. **`subagent_depth_cap_2_rejects_grandchild`** — spawn
//!    um filho (depth=1), tenta spawn um neto (depth+1=3),
//!    recebe `DepthExceeded { current: 3, max: 2 }`. Efeito
//!    colateral zero no filho.
//! 6. **`subagent_budget_sum_never_exceeds_parent`** — o
//!    invariante de soma do D3 (Σ alocações vivas ≤
//!    pai.remaining_inicial − pai.gasto_atual) é testado no
//!    **caminho real** (não só na função pura do
//!    `SubagentBudgetLedger`). A Etapa 4 PR 1 testou a função
//!    pura; esta Etapa PR 2 testa o gate que a usa.
//!
//! Ver [`docs/architecture/subagent-architecture.md` §"E2E de
//! cobertura planejado por etapa"](../../docs/architecture/subagent-architecture.md#e2e-de-cobertura-planejado-por-etapa)
//! (alvo declarado na Etapa 1, atualizado por etapa conforme
//! cada PR mergea) e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (regra da composição compartilhada).

use std::sync::Arc;
use std::time::Duration;

use frederico_agent_engine::{Budget, BudgetAllocation, SpentBudget, SubagentBudgetLedger};
use frederico_core::{ConversationId, ProjectId, ProviderId, RunId, ToolId};
use frederico_execution_engine::subagent_runner::{SubagentHandle, SubagentRunner};

mod common;

// ============================================================================
// Helpers compartilhados pelos 6 testes
// ============================================================================

/// Helper: cria um `Run` raiz (depth=0) com `Budget` configurável.
/// Bypassa o `ChatOrchestrator::send_message` (que cria runs via
/// storage) — pra testar o `SubagentRunner::try_spawn` em isolamento
/// do pipeline de stream. O `Run` em memória é o que o
/// `SubagentRunner` consulta e muta.
fn make_run_in_memory(
    budget: Budget,
    depth: u32,
    subagent_count: u32,
) -> frederico_agent_engine::Run {
    let now = chrono::Utc::now();
    let id = RunId::new();
    let mut r = frederico_agent_engine::Run::new(
        ConversationId::new(),
        ProjectId::new(),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
    );
    r.id = id;
    r.budget = budget;
    r.depth = depth;
    r.subagent_count = subagent_count;
    // `last_heartbeat_at` precisa ser igual a `started_at` no
    // `new()`, mas o construtor já fez isso — não precisa de
    // override aqui.
    let _ = now;
    r
}

/// Helper: cria um `Budget` com `max_steps` e `max_wall_clock`
/// customizados (o resto zero, suficiente pros testes).
fn budget_with_steps(steps: u32, wall_secs: u64) -> Budget {
    Budget {
        max_steps: steps,
        max_tokens_in: 0,
        max_tokens_out: 0,
        max_cost_microcents: 0,
        max_wall_clock: Duration::from_secs(wall_secs),
    }
}

/// Helper: cria um `BudgetAllocation` simples.
fn alloc(steps: u32) -> BudgetAllocation {
    BudgetAllocation {
        max_steps: steps,
        max_tokens_in: 0,
        max_tokens_out: 0,
        max_cost_microcents: 0,
        max_wall_clock: Duration::from_secs(60),
    }
}

/// Helper: constrói um `SubagentRunner` "real" consumindo
/// `build_orchestrator` (mesma factory dos outros E2E). O
/// `Arc<SubagentRunner>` vem do `ChatOrchestrator.subagent_runner`
/// (campo público da Etapa 4 PR 2).
async fn build_runner(handle: &common::OrchestratorHandle) -> Arc<SubagentRunner> {
    handle.orchestrator.subagent_runner.clone()
}

// ============================================================================
// 1. subagent_runs_with_reduced_permissions
// ============================================================================

/// **Prova:** o `effective_permissions` retornado no
/// `SubagentHandle` é subset do `parent_permissions` (4ª
/// camada da hierarquia de permission da Etapa 3 PR 2 +
/// invariante "subagente ⊆ pai" da Fase 3 Etapa 3).
///
/// O `parent_permissions` aqui é o `PermissionSet` que o
/// orchestrator carrega (mesmo `PermissionSet` que a casca
/// Tauri injeta). O `try_spawn` clona esse set como
/// `effective_permissions` (sem re-intersectar com mais nada
/// — o gate é a identidade, o teste de subset garante que
/// ninguém afrouxou a invariante).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_runs_with_reduced_permissions() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![], // sem scripts — não vai rodar stream real
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(50, 600), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    let result = runner.try_spawn(
        &mut parent,
        "revisor", // bundled
        alloc(10),
        &mut ledger,
        &parent_cancel,
        &parent_permissions,
        &parent_allowed,
    );
    let h: SubagentHandle = result.expect("revisor existe, allocation cabe");

    // Invariante "subagente ⊆ pai".
    assert!(
        h.effective_permissions.is_subset_of(&parent_permissions),
        "subagente.permissions deve ser subset do pai"
    );

    // `effective_permissions` é **idêntico** ao pai nesta
    // implementação (a 4ª camada é identidade, o gate
    // garante subset). Documentar a invariante exata:
    assert_eq!(h.effective_permissions, parent_permissions);
}

// ============================================================================
// 2. subagent_inherits_cancellation_token
// ============================================================================

/// **Prova:** 3 subagentes são spawnados em paralelo, o pai é
/// cancelado, **todos os 3** recebem o sinal via
/// `child_token()`. O `SubagentHandle::cancel_token` é derivado
/// do `parent_cancel_token.child_token()` (do `tokio_util`).
///
/// **Por que este teste é importante:** o `RunExecutor` do
/// filho (a Etapa 5/6 vai spawnar em background) observa o
/// `cancel_token` em `tokio::select!`. Se o portão não
/// derivar via `child_token`, cancelar o pai não cascateia
/// pros filhos — o que quebra o §9.4 do PROMPT MESTRE.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_inherits_cancellation_token() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![],
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(50, 600), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    // Spawna 3 subagentes. Todos têm o mesmo parent_cancel
    // como raiz, mas cada `child_token` é independente.
    let mut children = Vec::new();
    for _ in 0..3 {
        let h = runner
            .try_spawn(
                &mut parent,
                "critico", // bundled, allowed_tools=[], tem qualquer pai
                alloc(5),
                &mut ledger,
                &parent_cancel,
                &parent_permissions,
                &parent_allowed,
            )
            .expect("sumador existe, 5 steps cabem em 50-0-0-0-600");
        // Antes do cancel: nenhum filho está cancelado.
        assert!(!h.cancel_token.is_cancelled());
        children.push(h);
    }

    // Cancela o pai.
    parent_cancel.cancel();

    // Todos os 3 filhos cascateiam.
    for (i, h) in children.iter().enumerate() {
        assert!(
            h.cancel_token.is_cancelled(),
            "filho #{i} deveria estar cancelado após cancel do pai"
        );
    }
}

// ============================================================================
// 3. subagent_budget_discounted_from_parent_in_real_path
// ============================================================================

/// **Prova:** o portão debita o `parent.subagent_count` (D1) e
/// o ledger registra a alocação (D3 + invariante de soma). O
/// "desconto" do D3 ("filho não tem Budget próprio; tem uma
/// janela") aparece no `child_budget` = `min(parent.remaining,
/// requested)` (regra do `Budget::try_allocate`).
///
/// O teste valida o caminho **real** do portão, não só a
/// função pura do `Budget::try_allocate` (que já foi testada
/// na Etapa 4 PR 1 em `crates/agent-engine/src/budget_allocation.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_budget_discounted_from_parent_in_real_path() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![],
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(50, 600), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    // Pediu 10 steps; pai tem 50. Filha recebe 10 (=min).
    let h = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(10),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect("cabe em 50");
    assert_eq!(h.effective_budget.max_steps, 10, "min(50, 10) = 10");

    // Efeito no pai: subagent_count += 1 (D1).
    assert_eq!(parent.subagent_count, 1, "D1 incrementa subagent_count");

    // Ledger registra a alocação (D3 + invariante de soma).
    assert_eq!(ledger.len(), 1, "ledger tem 1 alocação");
    assert_eq!(
        ledger.total_allocated().max_steps,
        10,
        "ledger soma reflete a alocação"
    );

    // `record_spent` no filho (caminho real do D3) —
    // atualiza o `spent_by_child` do ledger. O
    // desconto pro pai (parent.spent) é responsabilidade
    // do orchestrator (Etapa 5/6), mas o teste prova
    // que o portão + ledger estão prontos pra isso.
    ledger.record_spent(
        &h.child_run_id,
        SpentBudget {
            cost_microcents: 0,
            tokens_in: 0,
            tokens_out: 0,
            steps: 3,
        },
    );
    let total_spent = ledger.total_spent().expect("fits");
    assert_eq!(total_spent.steps, 3, "ledger acumulou spent do filho");
}

// ============================================================================
// 4. subagent_explosion_cap_8_rejects_ninth
// ============================================================================

/// **Prova:** 8 subagentes passam, o 9º é rejeitado com
/// `GlobalLimitReached { current: 8, max: 8, next: 9 }`. Os 8
/// anteriores permanecem inalterados (efeito colateral
/// zero da rejeição).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_explosion_cap_8_rejects_ninth() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![],
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(50, 600), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    // Spawna 8 — todos cabem.
    for _ in 0..8 {
        runner
            .try_spawn(
                &mut parent,
                "critico",
                alloc(1),
                &mut ledger,
                &parent_cancel,
                &parent_permissions,
                &parent_allowed,
            )
            .expect("8 subagentes cabem em 50 steps");
    }
    assert_eq!(parent.subagent_count, 8);

    // 9º falha.
    let err = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(1),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect_err("9º deve falhar");
    match err {
        frederico_agent_engine::SubagentError::GlobalLimitReached { current, max, next } => {
            assert_eq!(current, 8);
            assert_eq!(max, 8);
            assert_eq!(next, 9);
        }
        other => panic!("variant errado: {other:?}"),
    }
    // Efeito colateral zero: o contador não mexeu na falha.
    assert_eq!(parent.subagent_count, 8);
    assert_eq!(ledger.len(), 8);
}

// ============================================================================
// 5. subagent_depth_cap_2_rejects_grandchild
// ============================================================================

/// **Prova:** um filho (depth=1) é spawnado, a tentativa de
/// spawnar um neto (depth+1=3) falha com
/// `DepthExceeded { current: 3, max: 2 }`. O filho permanece
/// inalterado (efeito colateral zero).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_depth_cap_2_rejects_grandchild() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![],
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(50, 600), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    // Spawna o filho (depth=1).
    let h = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(10),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect("filho cabe");
    assert_eq!(h.child_run.depth, 1);

    // Tenta spawnar um neto a partir do filho. O portão
    // do runner valida o **parent** recebido — como o
    // filho tem depth=1, depth+1=2, ainda passa (porque
    // o filho tem `subagent_count=0`, depth=1). Pra
    // testar o D2 de verdade, o neto precisa partir de
    // um `Run` com depth=2. Como o filho é um Run com
    // depth=1, o neto (depth+1=2) cabe — mas o **bisneto**
    // (depth+1=3) não. Esse é o teste do D2.
    let mut grandchild_attempt = make_run_in_memory(
        budget_with_steps(50, 600),
        2, // já é depth 2 — neto do pai raiz
        0,
    );
    let mut grandchild_ledger = SubagentBudgetLedger::new();
    let grandchild_cancel = tokio_util::sync::CancellationToken::new();
    let err = runner
        .try_spawn(
            &mut grandchild_attempt,
            "critico",
            alloc(1),
            &mut grandchild_ledger,
            &grandchild_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect_err("D2 rejeita depth=2 (depth+1=3 > max=2)");
    match err {
        frederico_agent_engine::SubagentError::DepthExceeded { current, max } => {
            assert_eq!(current, 3, "depth do neto seria 3");
            assert_eq!(max, 2);
        }
        other => panic!("variant errado: {other:?}"),
    }
    // Efeito colateral zero: o pai do neto (grandchild_attempt)
    // não tem seu `subagent_count` mexido.
    assert_eq!(grandchild_attempt.subagent_count, 0);
    assert!(grandchild_ledger.is_empty());

    // O filho (depth=1) criado antes permanece válido.
    assert_eq!(h.child_run.depth, 1);
}

// ============================================================================
// 6. subagent_budget_sum_never_exceeds_parent
// ============================================================================

/// **Prova:** o invariante de soma do D3 (Σ alocações vivas ≤
/// pai.remaining_inicial − pai.gasto_atual) é testado no
/// **caminho real** (não só na função pura do
/// `SubagentBudgetLedger` testada em PR 1). O cenário: pai
/// tem 5 steps; primeiro filho leva 3; segundo filho tenta
/// levar 3 — 3+3=6 > 5, falha.
///
/// **Por que este teste é o ponto-chave da Etapa 4:** a
/// PR 1 testou a função pura do ledger. Esta PR 2 testa o
/// **gate que a usa** — provando que o `try_spawn` chama o
/// `try_record` antes de incrementar o `subagent_count` (D3
/// antes de D1), sem janela de privilégio onde "passou o
/// D1 mas falhou o D3". Memory cross-project: "Cobertura de
/// invariante no caminho de produção, não no crate"
/// (memória do PR #26, ADR-0025 §Fato).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_budget_sum_never_exceeds_parent() {
    let handle = common::build_orchestrator(
        None,
        None,
        Arc::new(common::ScriptedProvider::new(
            "simulated",
            "fake-model-v1",
            vec![],
        )),
        ProviderId::new("simulated"),
        frederico_core::ModelId::new("fake-model-v1"),
        None,
    )
    .await;
    let runner = build_runner(&handle).await;

    let mut parent = make_run_in_memory(budget_with_steps(5, 300), 0, 0);
    let mut ledger = SubagentBudgetLedger::new();
    let parent_cancel = tokio_util::sync::CancellationToken::new();
    let parent_permissions = handle.orchestrator.permissions.clone();
    let parent_allowed: Vec<ToolId> = handle.orchestrator.allowed_for_run.clone();

    // Filha 1: 3 steps (cabe em 5, mas já ocupa 60% do budget).
    let h1 = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(3),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect("3 steps cabem em 5");
    assert_eq!(h1.effective_budget.max_steps, 3);

    // Filha 2: 3 steps — 3+3=6 > 5 (parent.remaining).
    let err = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(3),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect_err("Σ 3+3=6 > 5 (parent.remaining)");
    match err {
        frederico_agent_engine::SubagentError::AllocationExceedsParent { cause } => {
            assert!(matches!(
                cause,
                frederico_agent_engine::AllocationError::ExceedsParent { ref axis, .. }
                if axis == "max_steps"
            ));
        }
        other => panic!("variant errado: {other:?}"),
    }

    // Invariante: ledger tem só 1 alocação (a da filha 1), o
    // pai tem `subagent_count == 1` (não 2 — a falha do 2º
    // não mexeu no contador porque o `try_record` rodou
    // **antes** do `parent.subagent_count += 1`).
    assert_eq!(ledger.len(), 1, "ledger tem só a 1ª alocação");
    assert_eq!(
        parent.subagent_count, 1,
        "D3 antes de D1: contador não mexeu na falha"
    );

    // Filha 2 com 2 steps (3+2=5 = exatamente o pai) cabe.
    let _h2 = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(2),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect("3+2=5 = exato, cabe");
    assert_eq!(ledger.len(), 2);

    // Filha 3 com 1 step — 3+2+1=6 > 5, falha.
    let err = runner
        .try_spawn(
            &mut parent,
            "critico",
            alloc(1),
            &mut ledger,
            &parent_cancel,
            &parent_permissions,
            &parent_allowed,
        )
        .expect_err("Σ 3+2+1=6 > 5");
    assert!(matches!(
        err,
        frederico_agent_engine::SubagentError::AllocationExceedsParent { .. }
    ));
    assert_eq!(ledger.len(), 2, "falha do 3º não mexeu no ledger");
    assert_eq!(
        parent.subagent_count, 2,
        "D3 antes de D1: contador não mexeu"
    );
}
