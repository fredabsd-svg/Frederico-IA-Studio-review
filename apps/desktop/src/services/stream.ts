/**
 * Subscrição aos eventos de streaming do orquestrador.
 *
 * A casca Tauri emite:
 * - `run://<run_id>/event` — cada `StreamEvent` envolto em
 *   `StreamEventEnvelope { seq, event }` (o `seq` é o do journal —
 *   a UI usa pra reconectar sem perder nem duplicar)
 * - `run://<run_id>/status` — transição de status do run
 *
 * Camada `services/` — única que importa `@tauri-apps/api/event`
 * (mesma regra do `api.ts`).
 */

import { listen } from "@tauri-apps/api/event";
import type { RunStatusEvent, StreamEventEnvelope } from "./contracts";

export interface RunStreamSubscription {
  /** Para a escuta e libera recursos. */
  unlisten: () => void;
  /** Último `seq` recebido neste canal (pra reconexão). */
  lastSeq: () => number | null;
}

/** Subscrição tipada aos eventos de um run. */
export async function subscribeRun(
  runId: string,
  onEvent: (envelope: StreamEventEnvelope) => void,
  onStatus: (status: RunStatusEvent) => void,
): Promise<RunStreamSubscription> {
  const eventChannel = `run://${runId}/event`;
  const statusChannel = `run://${runId}/status`;

  // Rastreia o último `seq` visto neste canal pra que o caller
  // possa reconectar via `RunGetEvents { since_seq: <último seq> }`
  // quando a janela cair no meio do stream (§12.6). Sem isso, a
  // reconexão ou perde eventos (since=0 do início = duplica) ou
  // pula eventos (since=last = perde os do gap).
  let lastSeqSeen: number | null = null;

  const unlistenEvent = await listen<StreamEventEnvelope>(
    eventChannel,
    (e) => {
      if (typeof e.payload?.seq === "number") {
        lastSeqSeen = e.payload.seq;
      }
      onEvent(e.payload);
    },
  );
  const unlistenStatus = await listen<RunStatusEvent>(statusChannel, (e) => {
    onStatus(e.payload);
  });

  return {
    unlisten: () => {
      unlistenEvent();
      unlistenStatus();
    },
    lastSeq: () => lastSeqSeen,
  };
}
