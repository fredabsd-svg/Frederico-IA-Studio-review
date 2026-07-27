/**
 * Operações de provedores (cadastro de credencial, listagem).
 * Camada `services/` — única que fala com o núcleo (ADR-0003).
 */

import { dispatch } from "./api";
import type { ProviderConfigView } from "./contracts";

export function listProviders(): Promise<ProviderConfigView[]> {
  return dispatch<ProviderConfigView[]>({ kind: "provider_list" });
}

export function setCredential(provider: string, value: string): Promise<null> {
  return dispatch<null>({
    kind: "provider_set_credential",
    provider,
    value,
  });
}

export function deleteCredential(provider: string): Promise<null> {
  return dispatch<null>({
    kind: "provider_delete_credential",
    provider,
  });
}
