import { getVersion } from "@tauri-apps/api/app";
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

/**
 * Versão do produto, lida do binário em runtime.
 *
 * **Fonte única:** `apps/desktop/src-tauri/tauri.conf.json`
 * (campo `version`), que o Tauri embute no executável na build.
 * Nunca escreva um número de versão à mão no frontend — o
 * `check-docs.mjs` falha se aparecer um literal
 * (REGRAS §1.9). A tela `/sobre` chegou a anunciar "0.2.0"
 * enquanto `tauri.conf.json` e `package.json` diziam 0.1.0.
 *
 * Difere de `getAppInfo().version`, que é a versão **gravada no
 * banco** na primeira inicialização (`frederico_core::APP_VERSION`)
 * — útil pra saber com que versão o banco foi criado, não pra
 * dizer ao usuário qual versão ele está rodando agora.
 */
export async function getAppVersion(): Promise<string> {
  return getVersion();
}

export async function ping(): Promise<boolean> {
  const r = await dispatch<{ pong: boolean }>({ kind: "ping" });
  return r.pong === true;
}
