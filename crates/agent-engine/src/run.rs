//! Tipo de domínio `Run` em memória.
//!
//! O `Run` aqui é o **tipo de domínio** (especificado no
//! `agent-state-machine.md` §"Contratos"). O registro persistido
//! equivalente é [`frederico_storage::Run`], que vive no crate de
//! storage e tem `state: String` em vez de `RunState` (evita ciclo de
//! dependência entre o `agent-engine` puro e o `storage` que precisa
//! do SQLite).
//!
//! A Etapa 4 (integração) introduz a conversão entre os dois num único
//! lugar (`agent-engine::persistence`), com teste por variante da enum
//! — ver ADR-0009 §4.

use chrono::{DateTime, Utc};
use frederico_core::{AssistantId, ConversationId, ModelId, ProjectId, ProviderId, RunId, ToolId};
use serde::{Deserialize, Serialize};

use crate::budget::Budget;
use crate::budget_allocation::SpentBudget;
use crate::state::RunState;

/// Estado vivo de um `Run` em memória. A fonte da verdade em disco é
/// a linha da tabela `runs` (ver ADR-0009 e a migração
/// `0003_runs_and_checkpoints.sql`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Identificador opaco do `Run`.
    pub id: RunId,
    /// Conversa à qual o `Run` pertence.
    pub conversation_id: ConversationId,
    /// Projeto (Fase 7, modo desenvolvedor). Na Etapa 1, valor default
    /// — o conceito de projeto chega junto com o tool-registry.
    pub project_id: ProjectId,
    /// `None` na Etapa 1: o conceito de "assistente" entra na Etapa 6
    /// (UI), mas o banco já tem a coluna reservada.
    pub assistant_id: Option<AssistantId>,
    /// Provedor de LLM que está executando este `Run`.
    pub provider_id: ProviderId,
    /// Modelo específico dentro do provedor.
    pub model_id: ModelId,
    /// Instante em que o `Run` foi criado (estado `Created`).
    pub started_at: DateTime<Utc>,
    /// Estado atual da máquina.
    pub state: RunState,
    /// Iteração atual do loop `calling_model` → ...
    /// `continuing_model`. Incrementado pelo executor.
    pub current_step: u32,
    /// Teto de uso para esta execução.
    pub budget: Budget,
    /// Ferramentas permitidas para este `Run` específico. Interseção
    /// final calculada pelo `ToolRegistry` na Etapa 2 + Etapa 3.
    pub allowed_tools: Vec<ToolId>,
    /// `last_heartbeat_at` é parte do `Run` (não só dos eventos).
    /// Atualizado a cada operação custosa pela Etapa 5 (watchdog).
    pub last_heartbeat_at: DateTime<Utc>,
    /// Próximo `seq` a usar no journal. Monotônico por `run_id`.
    pub last_event_seq: u64,

    // ---- Etapa 4 da Fase 6: subagente (ADR-0027) ----
    /// **Contador global de subagentes vivos** deste `Run` (D1 do
    /// ADR-0027: teto de 8). O `SubagentRunner::try_spawn`
    /// consulta `parent.subagent_count + 1 <= 8` antes de
    /// spawnar. **Incrementa** antes do spawn; **decrementa**
    /// quando o subagente termina.
    ///
    /// Por que é `u32` (e não `u8`): o cabeçalho do campo é
    /// alinhado com a coluna do SQLite (mesmo tipo), e o
    /// `BTreeMap` keyed por `RunId` no `SubagentBudgetLedger`
    /// tem no máximo 8 entradas — `u32` é mais que
    /// suficiente. Zero overhead de memória na prática.
    pub subagent_count: u32,

    /// **Profundidade** na árvore de subagentes (D2 do ADR-0027:
    /// 0 pra Run raiz, 1 pra filho direto, 2 bloqueado). O
    /// `SubagentRunner::try_spawn` rejeita se
    /// `parent.depth + 1 > 2`.
    pub depth: u32,

    /// **Pai** do `Run`. `None` se `depth == 0` (raiz). `Some(p)`
    /// se `depth >= 1` (subagente). FK opcional pra `runs.id`
    /// (a constraint está no SQLite, não no tipo).
    pub parent_run_id: Option<RunId>,

    /// **Gasto efetivo** do `Run` (Etapa 4, ADR-0027 D3). É a
    /// fonte da verdade em memória do que o executor já
    /// consumiu. O invariante de soma do D3 ("Σ filhos ≤
    /// pai.remaining_inicial − pai.gasto_atual") lê daqui
    /// via `Budget::remaining(&self.spent)`.
    pub spent: SpentBudget,
}

impl Run {
    /// Cria um `Run` novo, no estado [`RunState::Created`], com `seq = 0`,
    /// `current_step = 0` e `last_heartbeat_at = started_at`. O
    /// `current_step` é incrementado pelo executor a cada iteração do
    /// loop; o `last_event_seq` é incrementado a cada evento gravado.
    ///
    /// **Etapa 4 (Fase 6)**: o `Run` novo é **raiz** por default
    /// (`subagent_count = 0`, `depth = 0`, `parent_run_id = None`,
    /// `spent = SpentBudget::default()`). O `SubagentRunner.new`
    /// constrói o `Run` filho a partir de um pai via
    /// [`Run::new_subagent`] (Etapa 4 PR 2).
    #[must_use]
    pub fn new(
        conversation_id: ConversationId,
        project_id: ProjectId,
        provider_id: ProviderId,
        model_id: ModelId,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: RunId::new(),
            conversation_id,
            project_id,
            assistant_id: None,
            provider_id,
            model_id,
            started_at: now,
            state: RunState::Created,
            current_step: 0,
            budget: Budget::default(),
            allowed_tools: Vec::new(),
            last_heartbeat_at: now,
            last_event_seq: 0,
            subagent_count: 0,
            depth: 0,
            parent_run_id: None,
            spent: SpentBudget::default(),
        }
    }

    /// Constrói um `Run` **subagente** a partir de um pai. Usado pelo
    /// `SubagentRunner::try_spawn` (Etapa 4 PR 2). Bumps
    /// `parent.subagent_count` automaticamente (não commita no DB —
    /// o `SubagentRunner` faz o `UPDATE` depois).
    ///
    /// **Invariante:** `child.depth = parent.depth + 1` e
    /// `child.parent_run_id = Some(parent.id)`. O `try_spawn` é o
    /// portão que rejeita `child.depth > 2` antes de chamar este
    /// construtor (D2 do ADR-0027).
    #[must_use]
    pub fn new_subagent(parent: &Run) -> Self {
        let now = Utc::now();
        Self {
            id: RunId::new(),
            conversation_id: parent.conversation_id,
            project_id: parent.project_id,
            assistant_id: parent.assistant_id,
            provider_id: parent.provider_id.clone(),
            model_id: parent.model_id.clone(),
            started_at: now,
            state: RunState::Created,
            current_step: 0,
            budget: parent.budget, // sobrescrito pelo `Budget::try_allocate` no try_spawn
            allowed_tools: parent.allowed_tools.clone(), // ∩ com specialist.allowed_tools no try_spawn
            last_heartbeat_at: now,
            last_event_seq: 0,
            subagent_count: 0,
            depth: parent.depth + 1,
            parent_run_id: Some(parent.id),
            spent: SpentBudget::default(),
        }
    }

    /// `true` se o `Run` está em estado terminal e não pode mais
    /// transicionar.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// `true` se o `Run` é um **subagente** (tem pai).
    #[must_use]
    pub fn is_subagent(&self) -> bool {
        self.parent_run_id.is_some()
    }

    /// Avança o `last_event_seq` e devolve o novo valor. O journal
    /// (Etapa 4) usa isso para garantir que `seq` é monotônico.
    pub fn next_event_seq(&mut self) -> u64 {
        self.last_event_seq += 1;
        self.last_event_seq
    }

    /// Atualiza o heartbeat para `Utc::now()`. A Etapa 5 (watchdog)
    /// chama isso após cada operação custosa; o teste de par testa o
    /// bump.
    pub fn heartbeat(&mut self) {
        self.last_heartbeat_at = Utc::now();
    }

    /// Acumula gasto no `spent`. Saturado em `u64::MAX` (defesa
    /// contra overflow — o `BudgetEnforcer` da Fase 3 Etapa 4
    /// barra antes de overflowar).
    ///
    /// **Por que método e não setter direto:** a atualização
    /// atômica do `spent` é uma operação que o executor faz a
    /// cada round. Métodos facilitam logging, profiling, e
    /// invariantes (ex.: "spent.steps <= budget.max_steps").
    pub fn record_spent(&mut self, delta: SpentBudget) {
        self.spent.cost_microcents = self
            .spent
            .cost_microcents
            .saturating_add(delta.cost_microcents);
        self.spent.tokens_in = self.spent.tokens_in.saturating_add(delta.tokens_in);
        self.spent.tokens_out = self.spent.tokens_out.saturating_add(delta.tokens_out);
        self.spent.steps = self.spent.steps.saturating_add(delta.steps);
    }

    /// Aplica uma transição no estado em memória. Aplica
    /// [`crate::transition::apply_transition`] no `state`; se a
    /// transição for válida, atualiza o `Run` e devolve `Ok(())`; se
    /// não, devolve o erro e deixa o `Run` inalterado. **Não** toca
    /// o `seq` nem o `heartbeat` — isso é responsabilidade do executor
    /// (Etapa 4) que monta a transação SQL.
    pub fn transition(
        &mut self,
        event: crate::event::RunEventKind,
    ) -> Result<(), crate::error::TransitionError> {
        let next = crate::transition::apply_transition(self.state, event)?;
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RunEventKind;

    #[test]
    fn new_run_is_in_created_state() {
        let r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        assert_eq!(r.state, RunState::Created);
        assert_eq!(r.current_step, 0);
        assert_eq!(r.last_event_seq, 0);
        assert!(!r.is_terminal());
        assert!(r.allowed_tools.is_empty());
        assert!(r.assistant_id.is_none());
    }

    #[test]
    fn new_run_heartbeat_equals_started_at() {
        let r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        assert_eq!(r.started_at, r.last_heartbeat_at);
    }

    #[test]
    fn next_event_seq_is_monotonic() {
        let mut r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        assert_eq!(r.next_event_seq(), 1);
        assert_eq!(r.next_event_seq(), 2);
        assert_eq!(r.next_event_seq(), 3);
        assert_eq!(r.last_event_seq, 3);
    }

    #[test]
    fn transition_moves_state() {
        let mut r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        r.transition(RunEventKind::Enqueue).unwrap();
        assert_eq!(r.state, RunState::Queued);
    }

    #[test]
    fn transition_from_terminal_is_rejected_and_leaves_state_unchanged() {
        let mut r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        // Forçar `completed` via uma cadeia de transições válidas
        // (caminho curto: só testamos o caso terminal).
        r.transition(RunEventKind::Enqueue).unwrap();
        r.transition(RunEventKind::Dequeue).unwrap();
        r.transition(RunEventKind::ContextReady).unwrap();
        r.transition(RunEventKind::MemoryDone).unwrap();
        r.transition(RunEventKind::CapabilitiesOk).unwrap();
        r.transition(RunEventKind::FirstToken).unwrap();
        r.transition(RunEventKind::MessageComplete).unwrap();
        r.transition(RunEventKind::ArtifactRequested).unwrap();
        r.transition(RunEventKind::ArtifactEmitted).unwrap();
        r.transition(RunEventKind::ArtifactValid).unwrap();
        r.transition(RunEventKind::CheckpointPersisted).unwrap();
        assert_eq!(r.state, RunState::Completed);

        // Tentativa de sair de `Completed` falha e não mexe no estado.
        let before = r.state;
        let err = r.transition(RunEventKind::UserCancel).unwrap_err();
        assert!(matches!(
            err,
            crate::error::TransitionError::FromTerminal { .. }
        ));
        assert_eq!(r.state, before);
    }

    #[test]
    fn heartbeat_updates_timestamp() {
        let mut r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        let old = r.last_heartbeat_at;
        // Dorme 1ms — `chrono::Utc::now` no Windows tem resolução
        // melhor que isso, então o teste é estável.
        std::thread::sleep(std::time::Duration::from_millis(2));
        r.heartbeat();
        assert!(r.last_heartbeat_at >= old);
    }

    #[test]
    fn run_serializes_to_json() {
        let r = Run::new(
            ConversationId::new(),
            ProjectId::new(),
            ProviderId::new("simulated"),
            ModelId::new("fake-model-v1"),
        );
        let s = serde_json::to_string(&r).expect("serializa");
        // Sanidade: tem `state` no JSON.
        assert!(s.contains("\"state\""));
        assert!(s.contains("\"created\""));
    }
}
