/**
 * Tipos espelhados de `packages/shared-contracts/src/lib.rs`.
 *
 * Mantidos manualmente por enquanto. A Fase 2+ gera este arquivo a partir
 * do JSON Schema (REGRAS §1.9) — quando o gerador entra, este arquivo vira
 * `ARQUIVO GERADO — não edite`.
 */

export type AppOp =
  | { kind: "get_app_info" }
  | { kind: "ping" }
  | { kind: "provider_list" }
  | { kind: "provider_set_credential"; provider: string; value: string }
  | { kind: "provider_delete_credential"; provider: string }
  | { kind: "model_catalog_list" }
  | { kind: "model_catalog_for_provider"; provider: string }
  | { kind: "conversation_create"; provider: string; model: string; title: string | null }
  | { kind: "conversation_list" }
  | { kind: "conversation_get"; id: string }
  | { kind: "conversation_rename"; id: string; title: string | null }
  | {
      kind: "conversation_set_model";
      id: string;
      provider: string;
      model: string;
    }
  | { kind: "conversation_delete"; id: string }
  | { kind: "message_send"; conversation_id: string; content: string }
  | { kind: "run_get_events"; message_id: string; since_seq: number }
  | { kind: "run_cancel"; run_id: string }
  // --- Etapa 6: fila de aprovação de tool calls ---
  | { kind: "approval_list" }
  | {
      kind: "approval_respond";
      approval_id: string;
      decision: { approved: boolean; scope?: "Once" | "Run" | "Project"; reason?: string };
    }
  // --- Etapa 5 (Fase 4): painel de memória ---
  | {
      kind: "memory_list";
      scope_type: string;
      scope_id: string;
      include_pending: boolean;
    }
  | {
      kind: "memory_retrieve";
      scope_type: string;
      scope_id: string;
      query: string;
      k: number;
    }
  | {
      kind: "memory_apply_correction";
      old_id: string;
      replacement: NewMemoryInputView;
    }
  | { kind: "memory_confirm_pending"; id: string }
  | { kind: "memory_reject_pending"; id: string }
  | { kind: "memory_purge_expired" };

export interface AppInfo {
  version: string;
  started_at: string;
  last_seen_at: string;
}

export interface IpcResponse {
  ok: boolean;
  payload: unknown | null;
  error: string | null;
}

// --- Views (espelhadas do shared-contracts) --------------------------

export interface ProviderConfigView {
  provider: string;
  display_name: string;
  configured: boolean;
  last_ok_at: string | null;
  last_error_at: string | null;
  last_error: string | null;
}

export interface ModelDescriptorView {
  provider: string;
  model: string;
  display_name: string;
  context_window: number;
  modalities: unknown;
  capabilities: unknown;
  pricing_input_microcents_per_million: number;
  pricing_output_microcents_per_million: number;
}

export interface ConversationView {
  id: string;
  title: string | null;
  provider_id: string;
  model_id: string;
  created_at: string;
  updated_at: string;
  total_cost_microcents: number;
}

export interface MessageView {
  id: string;
  conversation_id: string;
  role: "user" | "assistant" | "system" | string;
  content: string;
  status:
    | "pending"
    | "streaming"
    | "completed"
    | "failed"
    | "cancelled"
    | "timeout";
  run_id: string | null;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  cost_microcents: number;
  /** ProviderErrorView serializado (PT-BR + ação). */
  error: string | null;
  created_at: string;
  finished_at: string | null;
}

export interface MessageEventView {
  id: number;
  message_id: string;
  seq: number;
  kind: "delta" | "usage" | "tool_call" | "done" | "error" | "cancelled";
  data: unknown;
  created_at: string;
}

export interface MessageSendResult {
  user_message: MessageView;
  run_id: string;
}

export interface ConversationWithMessages {
  conversation: ConversationView;
  messages: MessageView[];
}

// --- Eventos Tauri do stream ----------------------------------------

/** `StreamEvent` serializado (espelha o Rust). */
export type StreamEvent =
  | { kind: "delta"; content: string }
  | { kind: "usage"; prompt_tokens: number; completion_tokens: number }
  | {
      kind: "tool_call";
      id: string;
      name: string;
      arguments_json: string;
    }
  | {
      kind: "done";
      stop_reason: "stop" | "length" | "tool_calls" | "error";
    }
  | { kind: "error"; message: ProviderErrorView }
  | { kind: "cancelled" };

export interface ProviderErrorView {
  code: string;
  title: string;
  detail: string;
  action: string | null;
  retry_after_secs: number | null;
}

/** Evento `run://<run_id>/status` payload. */
export interface RunStatusEvent {
  status:
    | "created"
    | "running"
    | "completed"
    | "failed"
    | "cancelled"
    | "timeout";
}

// --- Etapa 6: tipos da fila de aprovação ---

/** View de uma entry da fila de aprovação (espelha `ApprovalEntryView` do Rust). */
export interface ApprovalEntryView {
  id: string;
  run_id: string;
  tool_id: string;
  /** `ApprovalRequest` serializado (JSON string com a request original). */
  request_json: string;
  created_at: string;
}

/** Decisão do usuário (espelha `ApprovalDecision` do tool-registry). */
export interface ApprovalDecision {
  approved: boolean;
  /** Escopo da aprovação: `Once` (esta chamada), `Run` (este run), `Project` (todos os runs do projeto). */
  scope?: "Once" | "Run" | "Project";
  /** Razão da decisão (opcional; mostrada na auditoria). */
  reason?: string;
}

// --- Etapa 5 (Fase 4): tipos do painel de memória -------------------

/** View de uma memória (espelha `frederico_core::MemoryRecord`). */
export interface MemoryView {
  id: string;
  scope_type: string;
  scope_id: string;
  /** `"preference" | "fact" | "decision" | "correction" | ...` */
  type_: string;
  content: string;
  /** `"user" | "assistant" | "external_content"`. */
  origin: string;
  source_type: string;
  source_id: string | null;
  confidence: number;
  importance: number;
  embedding_status: string;
  created_at: string;
  updated_at: string;
  expires_at: string | null;
  superseded_by: string | null;
  superseded_at: string | null;
  user_confirmed: boolean;
  user_pinned: boolean;
  pending_review: boolean;
}

/** Decomposição do score final (`PROMPT MESTRE` §10.11 — explicabilidade). */
export interface ScoreBreakdownView {
  lexical: number;
  recency: number;
  semantic: number;
  importance: number;
  confirmation: number;
  scope_match: boolean;
}

/** Hit do retrieval com score e explicabilidade. */
export interface MemoryHitView {
  record: MemoryView;
  score: number;
  score_breakdown: ScoreBreakdownView;
  /** Frase curta explicando por que essa memória foi recuperada. */
  explanation: string;
}

/** Resultado do `MemoryApplyCorrection`. */
export interface CorrectionResultView {
  old_id: string;
  new_record: MemoryView;
  superseded_at: string;
}

/** Input do `MemoryApplyCorrection` (espelha `NewMemoryInputView` do Rust). */
export interface NewMemoryInputView {
  scope_type: string;
  scope_id: string;
  /** O JSON usa `type_` (não `type`) por causa da palavra reservada em Rust. */
  type_: string;
  content: string;
  origin: string;
  source_type: string;
  source_id: string | null;
  confidence: number;
  importance: number;
  expires_at: string | null;
}

// --- Fase 6 Etapa 7 (UI/Polish do Modo Equipe): tipos do Pipeline ---

/**
 * Modo do `MultimodelRun` (Etapa 5: só `Pipeline`; Etapas
 * futuras plugam `Comparison`/`Conselho`/`Debate` per ADR-0028
 * §D3).
 */
export type MultimodelMode = "pipeline";

/**
 * Estado de um `MultimodelRun`. Espelha o enum `MultimodelState`
 * do `frederico_storage::multimodel` (Etapa 5 PR 1).
 * `partially_completed` é exclusivo do pipeline — significa
 * "alguns stages concluíram, app foi fechado antes do resto".
 */
export type MultimodelState =
  | "pending"
  | "running"
  | "completed"
  | "partially_completed"
  | "failed"
  | "cancelled";

/**
 * `StageSpec` (espelha `frederico_execution_engine::pipeline_orchestrator::StageSpec`).
 * 3 campos: `model_id` (ex.: `gpt-4o-mini`), `provider_id`
 * (ex.: `openrouter`), `input` (texto que vai pro modelo como
 * `ChatMessage::user(input)`).
 *
 * **Por que 3 campos só e não `ChatRequest` completo:** o
 * `MultimodelOrchestrator` da Etapa 5 PR 2 não expõe
 * temperature/max_tokens/tools — a Etapa 6 da Fase 6 (UI) pluga
 * esses controles no `StageSpec` quando o Modo Equipe ganhar o
 * formulário de criação.
 */
export interface StageSpecView {
  model_id: string;
  provider_id: string;
  input: string;
}

/**
 * Cabeçalho de um `MultimodelRun` (espelha
 * `frederico_storage::MultimodelRun`). Carrega o `state` do
 * pipeline (não dos stages individuais — esses são carregados
 * por `listPipelineStages`). `total_cost_microcents` é a soma
 * dos `cost_microcents` dos stages (D5 do ADR-0028: rastreamento
 * de custo é parte do estado persistido).
 */
export interface MultimodelRunView {
  id: string;
  parent_run_id: string;
  mode: MultimodelMode;
  state: MultimodelState;
  input_artifact_id: string | null;
  final_artifact_id: string | null;
  total_cost_microcents: number;
  created_at: string;
  updated_at: string;
}

/**
 * Um stage de um pipeline (espelha
 * `frederico_storage::MultimodelStage`). `state` é o `RunState`
 * do stage individual (mesmo formato do `runs.state`: `pending`
 * / `running` / `streaming` / `completed` / `failed` /
 * `cancelled`). `output_hash` é FNV-1a (Etapa 5 PR 1) — a UI
 * mostra os primeiros 8 chars pra detectar reuso (D6 do
 * ADR-0028).
 */
export interface MultimodelStageView {
  id: string;
  run_id: string;
  seq: number;
  model_id: string;
  provider_id: string;
  /** `RunState` como string. */
  state: string;
  input_artifact_id: string | null;
  output_artifact_id: string | null;
  input_hash: string | null;
  output_hash: string | null;
  cost_microcents: number;
  tools_used_json: string;
  validation_json: string | null;
  started_at: string | null;
  finished_at: string | null;
}
