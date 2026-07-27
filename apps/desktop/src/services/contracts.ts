/**
 * Tipos espelhados de `packages/shared-contracts/src/lib.rs`.
 *
 * Mantidos manualmente por enquanto. A Fase 2+ gera este arquivo a partir
 * do JSON Schema (REGRAS §1.9) — quando o gerador entra, este arquivo vira
 * `ARQUIVO GERADO — não edite`.
 */

export type AppOp =
  | { kind: "get_app_info" }
  | { kind: "ping" };

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
