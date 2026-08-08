/**
 * `services/pipelines.ts` — consumer do Modo Equipe (Fase 6,
 * Etapa 7 UI/Polish).
 *
 * A UI consome via 4 Tauri commands dedicados (não via
 * `ipc_dispatch`/`AppOp`):
 *
 * - `start_pipeline(parent_run_id, Vec<StageSpec>) -> String`
 *   (Etapa 6, fecha o D5 do ADR-0028)
 * - `cancel_pipeline(pipeline_id) -> ()` (D7 do ADR-0028)
 * - `list_resumable_pipelines() -> Vec<MultimodelRun>` (D5)
 * - `list_pipeline_stages(run_id) -> Vec<MultimodelStage>`
 *   (Etapa 7 — pra UI mostrar o progresso de cada stage)
 *
 * **Por que Tauri commands dedicados e não `ipc_dispatch`:**
 * o `ipc_dispatch` enfileira no canal do `ChatOrchestrator`;
 * um command dedicado é mais barato e não disputa com
 * `MessageSend` no startup (que já chama `list_conversations` +
 * `list_specialists` + `list_resumable_pipelines` em paralelo).
 *
 * **Por que uma camada isolada?** Regra do `ADR-0003`: a
 * camada `services/` é a **única** que fala com Tauri.
 * Componentes React nunca importam `@tauri-apps/api/core`
 * diretamente. Se o app virar servidor amanhã, este arquivo
 * vira um cliente HTTP e o resto do frontend não muda.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  MultimodelRunView,
  MultimodelStageView,
  StageSpecView,
} from "./contracts";

/**
 * Inicia um pipeline sequencial. Retorna o `pipeline_id`
 * (= `MultimodelRun.id` recém-criado) — a UI armazena pra
 * poder cancelar.
 *
 * Execução é **assíncrona via `tokio::spawn` no backend**: a
 * casca Tauri devolve imediatamente. O progresso vem via
 * `list_resumable_pipelines` + `list_pipeline_stages` (a UI
 * faz polling a cada 2s, mesmo padrão do `App.tsx` para a fila
 * de aprovação).
 *
 * **Erros:** `Err(String)` com mensagem PT-BR. A UI
 * discrimina por substring: `"não encontrado"` (run_id não
 * existe), `"provider"` (provider do stage não tem adapter),
 * `"modelo"` (modelo não está no catálogo), `"pipeline precisa"`
 * (stages vazio).
 */
export async function startPipeline(
  parentRunId: string,
  stages: StageSpecView[],
): Promise<string> {
  return await invoke<string>("start_pipeline", {
    parentRunId,
    stages,
  });
}

/**
 * Cancela um pipeline em curso (D7 do ADR-0028). Cascateia o
 * `CancellationToken` pro `RunExecutor` do stage em curso;
 * stages futuros são marcados `Cancelled` direto.
 *
 * **Idempotente** com tolerância: cancelar 2x o mesmo
 * pipeline retorna `Ok(())` na primeira e `Err("pipeline X
 * não encontrado")` na segunda se a task já droppou o
 * token — a UI exibe esse erro como "pipeline já terminou"
 * sem chamar `alert`.
 */
export async function cancelPipeline(pipelineId: string): Promise<void> {
  await invoke<void>("cancel_pipeline", { pipelineId });
}

/**
 * Lista os `MultimodelRun`s em estado `Running` ou
 * `PartiallyCompleted` (D5 do ADR-0028). A UI carrega no
 * startup e renderiza na sidebar do Modo Equipe com
 * "retomar pipeline interrompido" (state=partially_completed)
 * e "cancelar" (state=running).
 *
 * **Por que só esses 2 estados:** `pending`/`completed`/
 * `failed`/`cancelled` não são "resumable" no sentido de
 * oferecer continuação. `pending` some quando o primeiro
 * stage começa; `completed`/`failed`/`cancelled` são
 * terminais.
 */
export async function listResumablePipelines(): Promise<MultimodelRunView[]> {
  return await invoke<MultimodelRunView[]>("list_resumable_pipelines");
}

/**
 * Lista os `MultimodelStage`s de um pipeline, ordenados por
 * `seq` ASC. A UI usa pra renderizar o grafo de stages
 * (estado + cost + input/output hash) e pra implementar
 * "retomar" (carrega os stages do `MultimodelRun`
 * parcialmente completed e chama `startPipeline` de novo
 * com os mesmos stages — o D6 do ADR-0028 garante que stages
 * já completados com mesmo `output_hash` são pulados
 * automaticamente).
 */
export async function listPipelineStages(
  runId: string,
): Promise<MultimodelStageView[]> {
  return await invoke<MultimodelStageView[]>("list_pipeline_stages", {
    runId,
  });
}
