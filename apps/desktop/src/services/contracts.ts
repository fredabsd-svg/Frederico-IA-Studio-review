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
  | { kind: "run_cancel"; run_id: string };

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
