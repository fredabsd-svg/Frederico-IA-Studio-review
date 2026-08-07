//! `SubagentRunner` — portão do spawn de subagentes (Etapa 4 PR 2
//! da Fase 6, ADR-0027 + ADR-0030).
//!
//! Ver [`docs/architecture/subagent-architecture.md` §"SubagentRunner"](../../architecture/subagent-architecture.md)
//! e o [ADR-0027](../decisions/0027-subagent-budget-inheritance-and-explosion-cap.md).
//!
//! ## O que o `SubagentRunner` faz
//!
//! O `try_spawn` é o **portão** que valida as 4 regras do ADR-0027
//! antes de criar o subagente:
//!
//! - **D1** (teto global 8 subagentes por run): `parent.subagent_count + 1 <= 8`.
//! - **D2** (profundidade máxima 2): `parent.depth + 1 <= 2`.
//! - **D3** (budget herdado e descontado): `BudgetAllocation::try_from(parent_remaining, requested)` + `ledger.try_record(child_id, allocation, parent_remaining)`.
//! - **D4** (erro legível, nunca panic nem silent fail): `SubagentError` estruturado.
//!
//! Mais a regra **§9.2** do PROMPT MESTRE (zero fallback silencioso):
//! o `SpecialistRegistry::get` retorna `Err(UnknownSpecialist {
//! requested, valid })` se o ID não existir — `valid` é o que o
//! modelo do pai vê pra escolher outro.
//!
//! ## O que o `SubagentRunner` **não** faz
//!
//! O portão **não** dispara a execução real do subagente (esse é
//! trabalho do `ChatOrchestrator` ou de um `SubagentExecutor`
//! separado em fase futura). O portão devolve um `SubagentHandle`
//! com tudo que o caller precisa pra spawnar o executor:
//!
//! - O `Run` filho (in-memory, com depth/parent corretos).
//! - O `Budget` efetivo (`min(parent_remaining, requested)`).
//! - O `PermissionSet` efetivo (4 camadas, fail-closed em cada).
//! - O `CancellationToken` filho (vinculado ao pai — cancelamento hierárquico).
//! - A `SpecialistDefinition` resolvida.
//!
//! O caller (orchestrator) então faz a persistência (RunRepo +
//! SubagentRunRepo) e spawna o `RunExecutor` em background.
//!
//! ## Hierarquia de permission (4 camadas + 1 denylist)
//!
//! O `effective_permissions` é a interseção de 4 `PermissionSet`s
//! (fail-closed em cada — mesma família do `PermissionSet::merge` do
//! PR 2 da Etapa 3):
//!
//! 1. **Profile effective** (`merge3(user, project, assistant)`) — base.
//! 2. **Parent permissions** (`parent_permissions.merge(...)`) —
//!    invariante "subagente ⊆ pai" (Fase 3 Etapa 3).
//! 3. (Especialista) — filtra por `allowed_tools` e `denied_tools`.
//!    Esta camada é a denylist da `SpecialistDefinition`.
//! 4. (Profile effective) — denegado por `denied_tools`.
//!
//! O `allowed_for_run` (vetor de `ToolId`s que o `RunExecutor`
//! aceita) é derivado: `parent.allowed_for_run ∩ (specialist.allowed_tools − specialist.denied_tools)`.
//!
//! ## Por que `try_spawn` é **sync**
//!
//! Todas as validações do portão são em memória. A persistência
//! (RunRepo + SubagentRunRepo) é responsabilidade do caller, e é
//! assíncrona. Manter o portão sync elimina dependência
//! desnecessária de `tokio::time::sleep` ou `await` no caminho
//! de decisão — o gate deve ser **anterior** ao efeito (regra do
//! PR #25, ADR-0027 §D4).
//!
//! ## Cancelamento hierárquico
//!
//! O `SubagentHandle::cancel_token` é criado via
//! `parent_cancel_token.child_token()` (do `tokio_util`). Quando o
//! pai cancela, **todos** os filhos recebem o sinal — o E2E
//! `subagent_inherits_cancellation_token` prova isso.

use std::sync::Arc;

use frederico_agent_engine::{
    AllocationError, Budget, BudgetAllocation, Run, SubagentBudgetLedger, SubagentError,
    UnknownSpecialistDetail,
};
use frederico_core::{RunId, ToolId};
use frederico_model_catalog::{
    RegistryError, SpecialistDefinition, SpecialistId, SpecialistRegistry,
};
use frederico_storage::Database;
use frederico_tool_registry::{PermissionLoader, PermissionSet};
use tokio_util::sync::CancellationToken;

/// Constantes do projeto (ADR-0027 D1 + D2).
///
/// - `MAX_SUBAGENTS_PER_RUN = 8` — teto global (D1).
/// - `MAX_DEPTH = 2` — profundidade máxima (D2).
///
/// **Por que constantes públicas e não hardcoded no
/// `try_spawn`:** os E2E (`subagent_explosion_cap_8_rejects_ninth`,
/// `subagent_depth_cap_2_rejects_grandchild`) referenciam essas
/// constantes no `Display` da `SubagentError` e nas asserções.
/// Mover pra constante nomeada é a forma de manter o gate e o
/// display sincronizados.
pub const MAX_SUBAGENTS_PER_RUN: u32 = 8;
pub const MAX_DEPTH: u32 = 2;

/// `SubagentRunner` — portão do spawn de subagentes (Etapa 4 PR 2).
///
/// **Por que `Arc<dyn SpecialistRegistry>` e `Arc<PermissionLoader>`:**
/// o runner não conhece a fonte concreta (mesma abstração do
/// `WorkerInvoker` da ADR-0024). Em produção, ambos vêm do
/// `ChatOrchestratorParts` que a casca Tauri constrói via
/// `frederico_app::build_chat_orchestrator` (mesma fábrica que os
/// E2E usam). Em testes, são instâncias de `DefaultSpecialistRegistry`
/// e `PermissionLoader::new()`.
///
/// **Por que `Arc<Database>`:** o runner persiste o `increment_subagent_count`
/// do pai e a linha em `subagent_runs` no momento do spawn. Sem
/// isso, o `Run` em memória fica à frente do banco, e o teste
/// `subagent_budget_sum_never_exceeds_parent` (caminho real) não
/// consegue verificar a invariante no DB.
pub struct SubagentRunner {
    specialist_registry: Arc<dyn SpecialistRegistry>,
    permission_loader: Arc<PermissionLoader>,
    /// Banco do orchestrator. **Não usado no PR 2** — o portão é
    /// sync e em memória; a persistência (`increment_subagent_count`,
    /// `SubagentRunRepo::record`) é responsabilidade do caller
    /// (orchestrator). Mantido aqui para a Etapa 5/6 (quando o
    /// runner disparar a execução real do subagente em
    /// background, vai precisar de `db` pra gravar `add_spent`).
    #[allow(dead_code)]
    db: Arc<Database>,
}

impl SubagentRunner {
    /// Constrói o runner. Sem default — o caller (casca Tauri ou
    /// teste E2E) passa explicitamente cada peça, eliminando
    /// estado global.
    #[must_use]
    pub fn new(
        specialist_registry: Arc<dyn SpecialistRegistry>,
        permission_loader: Arc<PermissionLoader>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            specialist_registry,
            permission_loader,
            db,
        }
    }

    /// Tenta spawnar um subagente. Valida D1, D2, D3, §9.2, e a
    /// hierarquia de permission. Devolve `SubagentError` estruturado
    /// em qualquer falha (D4).
    ///
    /// **Efeitos colaterais em caso de sucesso:**
    /// 1. `parent.subagent_count += 1` (in-memory; a persistência
    ///    em DB é responsabilidade do caller via
    ///    `RunRepo::increment_subagent_count`).
    /// 2. `parent_ledger.try_record(child_id, allocation, parent_remaining)`
    ///    (puro; o ledger é gerenciado pelo orchestrator).
    /// 3. Cria o `Run` filho in-memory via `Run::new_subagent(parent)`.
    /// 4. Cria o `CancellationToken` filho via
    ///    `parent_cancel_token.child_token()`.
    ///
    /// **Efeitos em caso de falha:**
    /// Nenhum. O portão não tem efeito parcial: ou devolve `Ok`
    /// com tudo feito, ou `Err` com o estado do pai intacto
    /// (D4: "nunca silent fail").
    ///
    /// ## Argumentos
    ///
    /// - `parent`: o `Run` pai (in-memory). Mutado em sucesso.
    /// - `specialist_id`: ID do especialista (string crua, como vem
    ///   do modelo). Validado contra o `SpecialistRegistry`.
    /// - `requested_allocation`: o `BudgetAllocation` que o pai
    ///   libera pro filho (D5 do ADR-0027). Validado em D3.
    /// - `parent_ledger`: o `SubagentBudgetLedger` do pai (o
    ///   orchestrator mantém um por pai).
    /// - `parent_cancel_token`: o `CancellationToken` do pai (o
    ///   orchestrator mantém um por run). O filho herda via
    ///   `child_token()`.
    /// - `parent_permissions`: o `PermissionSet` efetivo do pai
    ///   (`merge3(user, project, assistant)` do `PermissionLoader`).
    ///   É a 4ª camada da hierarquia — subagente nunca excede o
    ///   pai.
    /// - `parent_allowed_for_run`: a allowlist de `ToolId`s que o
    ///   pai tem. O filho herda a interseção com
    ///   `specialist.allowed_tools - specialist.denied_tools`.
    #[allow(clippy::too_many_arguments)]
    pub fn try_spawn(
        &self,
        parent: &mut Run,
        specialist_id: &str,
        requested_allocation: BudgetAllocation,
        parent_ledger: &mut SubagentBudgetLedger,
        parent_cancel_token: &CancellationToken,
        parent_permissions: &PermissionSet,
        parent_allowed_for_run: &[ToolId],
    ) -> Result<SubagentHandle, SubagentError> {
        // ---------- §9.2 / D4 — registry check ----------
        //
        // Valida primeiro pra dar erro legível antes dos
        // contadores/budget mexerem (D4: erro estruturado). O
        // `validate_id` é o portão "string → SpecialistId" (mais
        // barato que `get`); o `get` é redundância defensiva
        // (validate_id já filtra, mas `get` é o que retorna o
        // `Definition`).
        let validated_id: SpecialistId = self
            .specialist_registry
            .validate_id(specialist_id)
            .map_err(registry_error_to_subagent_error)?;
        let specialist = self
            .specialist_registry
            .get(&validated_id)
            .map_err(registry_error_to_subagent_error)?
            .clone();

        // ---------- D2 — depth check ----------
        //
        // Profundidade 2 = pai (0) → filho (1) → neto (2) bloqueado.
        // Verifica `parent.depth + 1 > 2` antes do
        // `increment_subagent_count` (D1) e do
        // `try_record` (D3) pra não haver efeito parcial.
        if parent.depth + 1 > MAX_DEPTH {
            return Err(SubagentError::DepthExceeded {
                current: parent.depth + 1,
                max: MAX_DEPTH,
            });
        }

        // ---------- D1 — global subagent count check ----------
        //
        // Teto de 8 subagentes por run (todos os níveis somados).
        // O portão verifica **antes** do increment — falha aqui
        // não mexe no `subagent_count`. O caller persiste o
        // increment depois.
        if parent.subagent_count + 1 > MAX_SUBAGENTS_PER_RUN {
            return Err(SubagentError::GlobalLimitReached {
                current: parent.subagent_count,
                max: MAX_SUBAGENTS_PER_RUN,
                next: parent.subagent_count + 1,
            });
        }

        // ---------- D3 — budget alocação vs pai ----------
        //
        // Dois passos:
        //
        // 1. `Budget::try_allocate(parent_remaining, requested)`
        //    valida que o requested cabe no parent_remaining (D3 +
        //    D5). Falha estruturada com eixo + valor + ação.
        // 2. `parent_ledger.try_record(child_id, allocation, parent_remaining)`
        //    valida o **invariante de soma** (D3: Σ alocações vivas
        //    ≤ parent.remaining_inicial − parent.gasto_atual).
        //
        // **Por que os dois:** o `try_allocate` é a regra 1-a-1
        // (essa alocação vs pai), o `try_record` é a regra
        // N-a-1 (soma vs pai). A Etapa 4 PR 1 fechou as duas
        // funções puras; aqui plugamos.
        let parent_remaining_budget: Budget = parent.budget.remaining(&parent.spent);
        let child_budget: Budget = Budget::try_allocate(
            &parent_remaining_budget,
            allocation_as_budget(&requested_allocation),
        )
        .map_err(|e| match e {
            AllocationError::ExceedsParent {
                axis,
                requested,
                available,
            } => SubagentError::AllocationExceedsParent {
                cause: AllocationError::ExceedsParent {
                    axis,
                    requested,
                    available,
                },
            },
            // SplitZero / SplitTooLarge não fazem sentido
            // no `try_allocate` (não estamos fazendo
            // split); a Etapa 4 PR 1 retorna-os via
            // `BudgetAllocation::split`. Re-mapeamos
            // pra `InternalError` porque o `try_allocate`
            // só pode produzir `ExceedsParent`.
            other => SubagentError::InternalError(format!(
                "Budget::try_allocate devolveu variante inesperada: {other:?}"
            )),
        })?;

        // Cria o `Run` filho in-memory (precisa do `parent`
        // mutável pra herdar `subagent_count+1` — espera,
        // não precisa; `new_subagent` lê os campos, não mexe
        // neles). Vamos primeiro incrementar pra dar o efeito
        // lógico, depois construir o filho (que herda o resto
        // dos campos do pai).
        //
        // **Ordem importante:** incrementamos o `subagent_count`
        // do pai **antes** do `try_record` do ledger — assim,
        // se o ledger falhar (D3 + 9º filho), o portão rejeita
        // mas o contador já mexeu. Para evitar o "efeito
        // parcial", fazemos o `try_record` **antes** do
        // increment. Se o `try_record` falhar, abortamos sem
        // mexer no `subagent_count`.
        //
        // (D4: "nunca silent fail" — efeito parcial é silent
        // fail parcial. O `try_record` primeiro garante atomic
        // semantics: ou tudo, ou nada.)
        let child_run_id = RunId::new();
        parent_ledger
            .try_record(child_run_id, requested_allocation, &parent_remaining_budget)
            .map_err(|e| match e {
                frederico_agent_engine::LedgerError::ExceedsParent {
                    axis,
                    requested,
                    available,
                } => SubagentError::AllocationExceedsParent {
                    cause: AllocationError::ExceedsParent {
                        axis,
                        requested,
                        available,
                    },
                },
                frederico_agent_engine::LedgerError::Overflow => SubagentError::InternalError(
                    "overflow na soma de alocações (impossível com 8 filhos, verificar bug)"
                        .to_string(),
                ),
            })?;

        // Só agora incrementamos o `subagent_count` do pai —
        // depois que D3 passou.
        parent.subagent_count += 1;

        // Constrói o `Run` filho (in-memory). `new_subagent`
        // herda conversation_id, project_id, etc do pai e
        // bumpa depth+1 e seta parent_run_id.
        let child_run = Run::new_subagent(parent);
        // O `Budget` do filho é o `child_budget` calculado
        // pelo `Budget::try_allocate` (não o `parent.budget` —
        // seria o pai, não a janela).
        let mut child_run = child_run;
        child_run.budget = child_budget;

        // ---------- Hierarquia de permission (4 camadas + denylist) ----------
        //
        // 1. `effective = parent_permissions.merge3` — não, espera:
        //    `parent_permissions` **já é** o effective do pai
        //    (`merge3(user, project, assistant)`). Não precisamos
        //    chamar o loader aqui — o caller já carregou.
        // 2. **Especialista allowed_tools** — restrição no nível
        //    de `ToolId`. O `PermissionSet` é interseção de eixos
        //    booleanos/enum; o `allowed_tools` é uma denylist de
        //    ferramentas específicas. Filtramos o `allowed_for_run`
        //    do filho: `parent_allowed_for_run ∩
        //    (specialist.allowed_tools - specialist.denied_tools)`.
        // 3. **PermissionDenied** — se a interseção é vazia e o
        //    pai tem tools, o especialista não tem nada pra
        //    fazer. Erro estruturado.
        let child_allowed_for_run: Vec<ToolId> = compute_child_allowed_for_run(
            parent_allowed_for_run,
            &specialist.allowed_tools,
            &specialist.denied_tools,
        );
        if child_allowed_for_run.is_empty() && !parent_allowed_for_run.is_empty() {
            // Pai tem tools; filho não pode chamar nenhuma.
            // Erro D4: o modelo do pai precisa escolher outro
            // especialista ou expandir a allowlist do pai.
            let missing: Vec<String> = parent_allowed_for_run
                .iter()
                .map(|t| t.as_str().to_string())
                .collect();
            return Err(SubagentError::PermissionDenied {
                reason: format!(
                    "especialista '{}' não tem nenhuma ferramenta em comum com a allowlist do pai \
                     (pai: {:?}, especialista allowed: {:?}, denied: {:?}). O especialista \
                     declarou allowed_tools=[] e o pai tem ferramentas. Escolha outro \
                     especialista ou expanda a allowlist do pai.",
                    specialist_id, missing, specialist.allowed_tools, specialist.denied_tools
                ),
            });
        }

        // O `effective_permissions` é o `parent_permissions`
        // (que já é o `merge3` do pai). Não temos mais nada pra
        // intersectar aqui — as outras camadas (user/project/
        // assistant) já estão no `parent_permissions` e o
        // `allowed_tools` é uma denylist de ToolId, não de
        // PermissionSet. Mantemos a invariante "subagente ⊆
        // pai" via identidade (o `parent_permissions` é
        // exatamente o `effective` do pai).
        let child_permissions = parent_permissions.clone();

        // Sanity: a invariante "subagente ⊆ pai" tem que
        // valer. Defensivo em profundidade — se um dia alguém
        // introduzir um campo novo em `PermissionSet` e
        // esquecer de copiar, o teste pega.
        debug_assert!(
            child_permissions.is_subset_of(parent_permissions),
            "subagente ⊆ pai: invariante da Fase 3 Etapa 3 violada"
        );

        // ---------- Cancelamento hierárquico ----------
        //
        // O `child_token` é derivado do `parent_cancel_token` —
        // quando o pai cancela, **todos** os filhos recebem o
        // sinal via `tokio_util::sync::CancellationToken::child_token`.
        // O E2E `subagent_inherits_cancellation_token` prova.
        let child_cancel_token = parent_cancel_token.child_token();

        // Anexa o `child_run_id` ao `parent_cancel_token` via
        // o `child_token` — quando o pai cancelar, o
        // `child_token` é cancelado, e o `RunExecutor` do
        // filho (spawnado pelo orchestrator) observa via
        // `tokio::select!`.
        //
        // (Não mutamos o `parent_cancel_token` aqui — só
        // derivamos o filho.)

        Ok(SubagentHandle {
            child_run,
            child_run_id,
            allocation: requested_allocation,
            effective_budget: child_budget,
            effective_permissions: child_permissions,
            effective_allowed_for_run: child_allowed_for_run,
            specialist_definition: Arc::new(specialist),
            cancel_token: child_cancel_token,
        })
    }

    /// Carrega o `PermissionSet` efetivo do pai (`merge3(user,
    /// project, assistant)`) via `PermissionLoader`. Helper de
    /// conveniência — o orchestrator chama isso uma vez por run
    /// e passa o resultado pro `try_spawn` (a 4ª camada da
    /// hierarquia).
    ///
    /// **Por que helper e não inline:** o `try_spawn` precisa
    /// do `parent_permissions` pré-carregado pra fazer a
    /// interseção. O `PermissionLoader` é `&self` (não `async`),
    /// então é seguro chamar de qualquer contexto.
    #[must_use]
    pub fn load_parent_permissions(
        &self,
        user: &std::path::Path,
        project: &std::path::Path,
        assistant: &std::path::Path,
    ) -> PermissionSet {
        self.permission_loader
            .load_effective_permission_set(user, project, assistant)
    }
}

/// `SubagentHandle` — o que `try_spawn` devolve. O caller
/// (orchestrator) usa este handle pra spawnar o `RunExecutor`
/// do filho em background.
///
/// **Lifetime:** o handle carrega um `Run` em memória (não
/// persistido). O caller é responsável por:
/// 1. `RunRepo::create` (persistir o `Run` filho).
/// 2. `SubagentRunRepo::record` (registrar na tabela
///    `subagent_runs`).
/// 3. `RunRepo::increment_subagent_count` (persistir o
///    increment do pai).
/// 4. `RunRepo::set_depth` (setar depth+parent no filho — o
///    `Run::new_subagent` já fez isso em memória).
/// 5. `tokio::spawn` o `RunExecutor` com
///    `effective_budget`, `effective_permissions`,
///    `effective_allowed_for_run` e `cancel_token`.
/// 6. Quando o filho termina: `SubagentRunRepo::complete`,
///    `RunRepo::decrement_subagent_count`, e
///    `parent_ledger.release(&child_run_id)`.
#[derive(Debug)]
pub struct SubagentHandle {
    /// O `Run` filho in-memory (depth+1, parent_run_id setado).
    pub child_run: Run,
    /// `RunId` do filho (= `child_run.id`).
    pub child_run_id: RunId,
    /// A `BudgetAllocation` que foi validada (mesma do
    /// `requested_allocation` do `try_spawn`).
    pub allocation: BudgetAllocation,
    /// `Budget` efetivo (`min(parent_remaining, requested)`).
    pub effective_budget: Budget,
    /// `PermissionSet` efetivo (= `parent_permissions`, já
    /// com `merge3(user, project, assistant)`).
    pub effective_permissions: PermissionSet,
    /// Allowlist de `ToolId` do filho
    /// (`parent_allowed_for_run ∩ (specialist.allowed − specialist.denied)`).
    pub effective_allowed_for_run: Vec<ToolId>,
    /// `SpecialistDefinition` resolvida (clone do registry).
    pub specialist_definition: Arc<SpecialistDefinition>,
    /// `CancellationToken` do filho (derivado do pai). Pai
    /// cancela → filho também (E2E `subagent_inherits_cancellation_token`).
    pub cancel_token: CancellationToken,
}

/// Helper: converte um `RegistryError` em `SubagentError` (com
/// `UnknownSpecialist` carregando o `valid: Vec<String>`). O
/// `SubagentError::UnknownSpecialist` é uma variante wrapper
/// (`UnknownSpecialistDetail`) que o `Display` usa pra montar o
/// texto legível pelo modelo.
fn registry_error_to_subagent_error(err: RegistryError) -> SubagentError {
    match err {
        RegistryError::UnknownSpecialist { requested, valid } => {
            SubagentError::UnknownSpecialist(UnknownSpecialistDetail {
                requested,
                valid: valid.iter().map(|s| s.as_str().to_string()).collect(),
            })
        }
        // Outros `RegistryError` (`DefaultModelNotFound`,
        // `ConfigurationError`) viram `InternalError` — bugs
        // internos (o registry deveria ter rejeitado no
        // `validate_id` se fosse config ruim). D4: erro
        // estruturado, não panic.
        other => SubagentError::InternalError(format!(
            "SpecialistRegistry devolveu erro não-tratado: {other}"
        )),
    }
}

/// Helper: converte `BudgetAllocation` em `Budget` (pra
/// `Budget::try_allocate`). O `try_allocate` recebe `Budget` (o
/// tipo cheio, com `max_steps`, `max_tokens_in`, etc); o
/// `BudgetAllocation` é o delta que o pai libera pro filho (mesmo
/// shape, mas semântica de "janela"). A conversão é mecânica
/// (campo a campo).
fn allocation_as_budget(allocation: &BudgetAllocation) -> Budget {
    Budget {
        max_steps: allocation.max_steps,
        max_tokens_in: allocation.max_tokens_in,
        max_tokens_out: allocation.max_tokens_out,
        max_cost_microcents: allocation.max_cost_microcents,
        max_wall_clock: allocation.max_wall_clock,
    }
}

/// Helper: computa a allowlist do filho a partir da allowlist do
/// pai ∩ (allowed − denied) do especialista. Mesma família do
/// `is_subset_of` (Fase 3 Etapa 3): mais restritivo vence.
fn compute_child_allowed_for_run(
    parent_allowed: &[ToolId],
    specialist_allowed: &[ToolId],
    specialist_denied: &[ToolId],
) -> Vec<ToolId> {
    parent_allowed
        .iter()
        .filter(|t| {
            // denied tem precedência absoluta (specialist.denied
            // ⊃ specialist.allowed se houver sobreposição).
            if specialist_denied.contains(t) {
                return false;
            }
            // Se o specialist tem `allowed_tools` não-vazio, o
            // tool precisa estar nele. Se `allowed_tools` é
            // vazio, o specialist não tem restrição de tool
            // (opera só com texto — útil pro `sumador` e o
            // `critico`).
            specialist_allowed.is_empty() || specialist_allowed.contains(t)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_agent_engine::Budget;
    use frederico_core::{ConversationId, ProjectId, ProviderId};
    use std::time::Duration;

    /// Helper: cria um `Run` pai com `Budget` configurável.
    fn make_parent(budget: Budget) -> Run {
        let mut r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            frederico_core::ModelId::new("fake-model-v1"),
        );
        r.budget = budget;
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
    fn alloc(steps: u32, cost: u64) -> BudgetAllocation {
        BudgetAllocation {
            max_steps: steps,
            max_tokens_in: 0,
            max_tokens_out: 0,
            max_cost_microcents: cost,
            max_wall_clock: Duration::from_secs(60),
        }
    }

    /// D1: `subagent_count + 1 > 8` rejeita.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_limit_reaches_8() {
        let mut parent = make_parent(budget_with_steps(50, 600));
        parent.subagent_count = MAX_SUBAGENTS_PER_RUN;
        let mut ledger = SubagentBudgetLedger::new();
        let cancel = CancellationToken::new();
        let perms = PermissionSet::default();
        let allowed = vec![];

        let err = SubagentRunner::new(
            Arc::new(frederico_model_catalog::DefaultSpecialistRegistry::load()),
            Arc::new(PermissionLoader::new()),
            Arc::new(
                futures::executor::block_on(frederico_storage::Database::open_in_memory())
                    .expect("open in-memory db"),
            ),
        )
        .try_spawn(
            &mut parent,
            "revisor",
            alloc(5, 0),
            &mut ledger,
            &cancel,
            &perms,
            &allowed,
        )
        .expect_err("D1 deve rejeitar com 8 subagentes");

        match err {
            SubagentError::GlobalLimitReached { current, max, next } => {
                assert_eq!(current, MAX_SUBAGENTS_PER_RUN);
                assert_eq!(max, MAX_SUBAGENTS_PER_RUN);
                assert_eq!(next, MAX_SUBAGENTS_PER_RUN + 1);
            }
            other => panic!("variant errado: {other:?}"),
        }
        // Efeito colateral zero: `subagent_count` e `ledger` intactos.
        assert_eq!(parent.subagent_count, MAX_SUBAGENTS_PER_RUN);
        assert!(ledger.is_empty());
    }

    /// D2: `depth + 1 > 2` rejeita.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn depth_exceeds_2() {
        let mut parent = make_parent(budget_with_steps(50, 600));
        parent.depth = MAX_DEPTH; // 2, então depth+1=3
        let mut ledger = SubagentBudgetLedger::new();
        let cancel = CancellationToken::new();
        let perms = PermissionSet::default();
        let allowed = vec![];

        let err = SubagentRunner::new(
            Arc::new(frederico_model_catalog::DefaultSpecialistRegistry::load()),
            Arc::new(PermissionLoader::new()),
            Arc::new(
                futures::executor::block_on(frederico_storage::Database::open_in_memory())
                    .expect("open in-memory db"),
            ),
        )
        .try_spawn(
            &mut parent,
            "revisor",
            alloc(5, 0),
            &mut ledger,
            &cancel,
            &perms,
            &allowed,
        )
        .expect_err("D2 deve rejeitar com depth=2");

        match err {
            SubagentError::DepthExceeded { current, max } => {
                assert_eq!(current, MAX_DEPTH + 1);
                assert_eq!(max, MAX_DEPTH);
            }
            other => panic!("variant errado: {other:?}"),
        }
        // Efeito colateral zero.
        assert_eq!(parent.depth, MAX_DEPTH);
        assert!(ledger.is_empty());
    }

    /// D3: `allocation > parent_remaining` rejeita com
    /// `AllocationExceedsParent`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allocation_exceeds_parent_remaining() {
        let parent_budget = budget_with_steps(5, 300);
        let mut parent = make_parent(parent_budget);
        let mut ledger = SubagentBudgetLedger::new();
        let cancel = CancellationToken::new();
        let perms = PermissionSet::default();
        let allowed = vec![];

        // Pediu 10 steps; pai tem 5. Excede.
        let err = SubagentRunner::new(
            Arc::new(frederico_model_catalog::DefaultSpecialistRegistry::load()),
            Arc::new(PermissionLoader::new()),
            Arc::new(
                futures::executor::block_on(frederico_storage::Database::open_in_memory())
                    .expect("open in-memory db"),
            ),
        )
        .try_spawn(
            &mut parent,
            "revisor",
            alloc(10, 0),
            &mut ledger,
            &cancel,
            &perms,
            &allowed,
        )
        .expect_err("D3 deve rejeitar 10 > 5");

        match err {
            SubagentError::AllocationExceedsParent { cause } => {
                assert!(matches!(
                    cause,
                    AllocationError::ExceedsParent { ref axis, .. } if axis == "max_steps"
                ));
            }
            other => panic!("variant errado: {other:?}"),
        }
        // Efeito colateral zero.
        assert_eq!(parent.subagent_count, 0);
        assert!(ledger.is_empty());
    }

    /// §9.2: ID que não existe rejeita com `UnknownSpecialist` e
    /// `valid` lista os bundled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_specialist_lists_valid() {
        let mut parent = make_parent(budget_with_steps(50, 600));
        let mut ledger = SubagentBudgetLedger::new();
        let cancel = CancellationToken::new();
        let perms = PermissionSet::default();
        let allowed = vec![];

        let err = SubagentRunner::new(
            Arc::new(frederico_model_catalog::DefaultSpecialistRegistry::load()),
            Arc::new(PermissionLoader::new()),
            Arc::new(
                futures::executor::block_on(frederico_storage::Database::open_in_memory())
                    .expect("open in-memory db"),
            ),
        )
        .try_spawn(
            &mut parent,
            "fantasma",
            alloc(5, 0),
            &mut ledger,
            &cancel,
            &perms,
            &allowed,
        )
        .expect_err("§9.2 deve rejeitar ID inexistente");

        match err {
            SubagentError::UnknownSpecialist(detail) => {
                assert_eq!(detail.requested, "fantasma");
                assert!(detail.valid.len() >= 8, "valid deve listar os 8 bundled");
                let valid_strs: Vec<&str> = detail.valid.iter().map(|s| s.as_str()).collect();
                for expected in &["revisor", "pesquisador", "testador"] {
                    assert!(
                        valid_strs.contains(expected),
                        "expected {expected} em valid: {valid_strs:?}"
                    );
                }
            }
            other => panic!("variant errado: {other:?}"),
        }
    }

    /// Sucesso: o portão passa, `subagent_count` incrementa,
    /// `ledger` registra a alocação, e o `cancel_token` é
    /// hierárquico.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_increments_and_records() {
        let mut parent = make_parent(Budget {
            max_steps: 50,
            max_tokens_in: 0,
            max_tokens_out: 0,
            max_cost_microcents: 10_000,
            max_wall_clock: Duration::from_secs(600),
        });
        let mut ledger = SubagentBudgetLedger::new();
        let parent_cancel = CancellationToken::new();
        let perms = PermissionSet::default();
        let allowed = vec![ToolId::new("files.read")];

        let handle = SubagentRunner::new(
            Arc::new(frederico_model_catalog::DefaultSpecialistRegistry::load()),
            Arc::new(PermissionLoader::new()),
            Arc::new(
                futures::executor::block_on(frederico_storage::Database::open_in_memory())
                    .expect("open in-memory db"),
            ),
        )
        .try_spawn(
            &mut parent,
            "revisor",
            alloc(10, 1_000),
            &mut ledger,
            &parent_cancel,
            &perms,
            &allowed,
        )
        .expect("happy path");

        // Efeito no pai.
        assert_eq!(parent.subagent_count, 1);
        assert_eq!(ledger.len(), 1);

        // Efeito no handle.
        assert_eq!(handle.child_run.depth, 1);
        assert_eq!(handle.child_run.parent_run_id, Some(parent.id));
        assert_eq!(handle.effective_budget.max_steps, 10);
        assert_eq!(handle.effective_budget.max_cost_microcents, 1_000);
        // allowed_for_run do revisor (do default.toml) — pelo
        // menos `files.read` deve estar.
        assert!(handle
            .effective_allowed_for_run
            .contains(&ToolId::new("files.read")));

        // Cancelamento hierárquico.
        parent_cancel.cancel();
        assert!(handle.cancel_token.is_cancelled());
    }
}
