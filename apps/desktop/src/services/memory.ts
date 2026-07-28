/**
 * `services/memory.ts` — consumer do painel de memória
 * (Fase 4, Etapa 5).
 *
 * A UI consome via `AppOp::Memory*` (lista, retrieve,
 * apply_correction, confirm_pending, reject_pending,
 * purge_expired). O componente `MemoryPanel` (em
 * `components/MemoryPanel.tsx`) usa estas funções.
 *
 * **Por que uma camada isolada?** Regra do `ADR-0003`: a
 * camada `services/` é a **única** que fala com Tauri.
 * Componentes React nunca importam `@tauri-apps/api/core`
 * diretamente. Se o app virar servidor amanhã, este
 * arquivo vira um cliente HTTP e o resto do frontend
 * não muda.
 */

import { dispatch } from "./api";
import type {
  CorrectionResultView,
  MemoryHitView,
  MemoryView,
  NewMemoryInputView,
} from "./contracts";

/**
 * Lista memórias em um escopo. `includePending = true` traz
 * também a fila de revisão de `ExternalContent` (a UI usa
 * isso na aba "pendentes de revisão").
 */
export async function listMemories(
  scopeType: string,
  scopeId: string,
  includePending: boolean = false,
): Promise<MemoryView[]> {
  return dispatch<MemoryView[]>({
    kind: "memory_list",
    scope_type: scopeType,
    scope_id: scopeId,
    include_pending: includePending,
  });
}

/**
 * Retrieval híbrido (lexical + semântico quando embedding
 * estiver disponível) no escopo. Devolve hits com
 * `ScoreBreakdown` visível (explicabilidade `PROMPT MESTRE`
 * §10.11).
 */
export async function retrieveMemories(
  scopeType: string,
  scopeId: string,
  query: string,
  k: number = 8,
): Promise<MemoryHitView[]> {
  return dispatch<MemoryHitView[]>({
    kind: "memory_retrieve",
    scope_type: scopeType,
    scope_id: scopeId,
    query,
    k,
  });
}

/**
 * Aplica correção ("corrija para X"): insere a substituta +
 * marca a antiga como superseded, atomicamente. Devolve
 * `CorrectionResultView` com `old_id`, `new_record` e
 * `superseded_at` — a UI usa o `superseded_at` pra mostrar
 * "essa memória foi substituída por X há 3 dias".
 */
export async function applyCorrection(
  oldId: string,
  replacement: NewMemoryInputView,
): Promise<CorrectionResultView> {
  return dispatch<CorrectionResultView>({
    kind: "memory_apply_correction",
    old_id: oldId,
    replacement,
  });
}

/**
 * Confirma uma memória com `pending_review = true` (fila de
 * ExternalContent). Vira `user_confirmed = true` e
 * `pending_review = false`.
 */
export async function confirmPending(id: string): Promise<void> {
  await dispatch<void>({ kind: "memory_confirm_pending", id });
}

/**
 * Rejeita (deleta) uma memória com `pending_review = true`.
 * Audit trail preservado em log do app, não no banco.
 */
export async function rejectPending(id: string): Promise<void> {
  await dispatch<void>({ kind: "memory_reject_pending", id });
}

/**
 * Limpa memórias expiradas + superseded + soft-deleted.
 * Best-effort, análogo ao `purge_expired` no startup.
 * Devolve o número de linhas deletadas.
 */
export async function purgeExpiredMemories(): Promise<number> {
  return dispatch<number>({ kind: "memory_purge_expired" });
}
