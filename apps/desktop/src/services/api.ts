import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, AppOp, IpcResponse } from "./contracts";

/**
 * Camada `services/` — única que faz `invoke` no Tauri (ADR-0003).
 *
 * Componentes React **nunca** importam `@tauri-apps/api/core` diretamente.
 * Se o app virar servidor amanhã, este arquivo vira um cliente HTTP e o
 * resto do frontend não muda.
 */

export async function dispatch<T = unknown>(op: AppOp): Promise<T> {
  const r = await invoke<IpcResponse>("ipc_dispatch", { request: { op } });
  if (!r.ok) {
    throw new Error(r.error ?? "resposta inválida do núcleo");
  }
  return r.payload as T;
}

export async function getAppInfo(): Promise<AppInfo> {
  return dispatch<AppInfo>({ kind: "get_app_info" });
}

export async function ping(): Promise<boolean> {
  const r = await dispatch<{ pong: boolean }>({ kind: "ping" });
  return r.pong === true;
}
