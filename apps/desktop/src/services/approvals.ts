/**
 * `services/approvals.ts` — consumer da fila de aprovação
 * (Fase 3, Etapa 6.2).
 *
 * A UI consome via `AppOp::ApprovalList` (polling) e responde
 * via `AppOp::ApprovalRespond`. O componente `ApprovalModal`
 * (em `components/ApprovalModal.tsx`) usa estas funções.
 *
 * **Por que polling e não Tauri event?** A casca Rust enfileira
 * `ApprovalRequired` no momento que o `RunExecutor` recebe o
 * Passo 9 do `validate_tool_call`. Não há `emit_run_event` no
 * Tauri `EventSink` pra "approval_required" (a Etapa 6.2.x
 * pode adicionar pra ser push-based em vez de pull). O polling
 * a cada 2s cobre o caso comum sem complicar a infra.
 */

import { dispatch } from "./api";
import type { ApprovalDecision, ApprovalEntryView } from "./contracts";

/** Lista entries pending da fila de aprovação. */
export async function listApprovals(): Promise<ApprovalEntryView[]> {
  return dispatch<ApprovalEntryView[]>({ kind: "approval_list" });
}

/** Responde a uma approval pendente (approve ou reject). */
export async function respondToApproval(
  approvalId: string,
  decision: ApprovalDecision,
): Promise<void> {
  await dispatch<void>({
    kind: "approval_respond",
    approval_id: approvalId,
    decision,
  });
}
