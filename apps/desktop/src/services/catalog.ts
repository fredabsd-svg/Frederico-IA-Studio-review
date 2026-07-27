/**
 * Operações de catálogo de modelos. Camada `services/`.
 */

import { dispatch } from "./api";
import type { ModelDescriptorView } from "./contracts";

export function listCatalog(): Promise<ModelDescriptorView[]> {
  return dispatch<ModelDescriptorView[]>({ kind: "model_catalog_list" });
}

export function listCatalogForProvider(
  provider: string,
): Promise<ModelDescriptorView[]> {
  return dispatch<ModelDescriptorView[]>({
    kind: "model_catalog_for_provider",
    provider,
  });
}
