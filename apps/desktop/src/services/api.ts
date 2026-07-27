import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, IpcResponse, AppOp } from "./contracts";

/**
 * Camada `services/` — única que faz `invoke` no Tauri (ADR-0003).
 *
 * Componentes React **nunca** importam `@tauri-apps/api/core` diretamente.
 * Se o app virar servidor amanhã, este arquivo vira um cliente HTTP e o
 * resto do frontend não muda.
 */

export async function dispatch(op: AppOp): Promise<IpcResponse> {
  return invoke<IpcResponse>("ipc_dispatch", { request: { op } });
}

export async function getAppInfo(): Promise<AppInfo> {
  const r = await dispatch({ kind: "get_app_info" });
  if (!r.ok || !r.payload) {
    throw new Error(r.error ?? "resposta inválida do núcleo");
  }
  return r.payload as unknown as AppInfo;
}

export async function ping(): Promise<boolean> {
  const r = await dispatch({ kind: "ping" });
  return r.ok && (r.payload as { pong?: boolean } | null)?.pong === true;
}
