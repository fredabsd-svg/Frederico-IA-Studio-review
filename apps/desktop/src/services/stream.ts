/**
 * Subscrição aos eventos de streaming do orquestrador.
 *
 * A casca Tauri emite:
 * - `run://<run_id>/event` — cada `StreamEvent` (delta, usage, done, ...)
 * - `run://<run_id>/status` — transição de status do run
 *
 * Camada `services/` — única que importa `@tauri-apps/api/event`
 * (mesma regra do `api.ts`).
 */

import { listen } from "@tauri-apps/api/event";
import type { RunStatusEvent, StreamEvent } from "./contracts";

export interface RunStreamSubscription {
  /** Para a escuta e libera recursos. */
  unlisten: () => void;
}

/** Subscrição tipada aos eventos de um run. */
export async function subscribeRun(
  runId: string,
  onEvent: (ev: StreamEvent) => void,
  onStatus: (status: RunStatusEvent) => void,
): Promise<RunStreamSubscription> {
  const eventChannel = `run://${runId}/event`;
  const statusChannel = `run://${runId}/status`;

  const unlistenEvent = await listen<StreamEvent>(eventChannel, (e) => {
    onEvent(e.payload);
  });
  const unlistenStatus = await listen<RunStatusEvent>(statusChannel, (e) => {
    onStatus(e.payload);
  });

  return {
    unlisten: () => {
      unlistenEvent();
      unlistenStatus();
    },
  };
}
