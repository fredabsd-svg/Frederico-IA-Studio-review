/**
 * `components/PipelineView.tsx` — UI do Modo Equipe (Fase 6,
 * Etapa 7 UI/Polish).
 *
 * Renderiza:
 * - Sidebar com `listResumablePipelines()` (refresh polling
 *   2s, mesmo padrão do `App.tsx` para a fila de aprovação).
 * - Detalhe do pipeline selecionado: cabeçalho (`state` +
 *   `total_cost_microcents` + `parent_run_id` truncado +
 *   `created_at`/`updated_at`) + `listPipelineStages()`
 *   ordenado por `seq` (cada stage com `model_id`/`provider_id`/
 *   `state`/`cost_microcents`/`input_hash`/`output_hash`).
 * - Botão "Cancelar" (visível se `state === "running"`) que
 *   chama `cancelPipeline` e dá refresh imediato.
 * - Botão "Criar pipeline" (sidebar) que abre
 *   `PipelineCreateForm` em modal.
 *
 * **Por que polling e não `EventSink`:** o backend
 * (`MultimodelOrchestrator`) ainda não emite eventos
 * `MultimodelRunProgress` dedicados (a Etapa 6
 * `pipeline_d6_reuso_does_not_panic_when_no_reusable_stage`
 * deixa o progresso só via `PipelineRepo`). Polling 2s é a
 * abordagem da Etapa 7 — uma Etapa futura pode plugar
 * `EventSink` e virar push-based.
 *
 * **Por que "Retomar" fica como TODO:** retomar um pipeline
 * `partially_completed` requer o `input` original de cada
 * stage (pra calcular `input_hash` e o D6 do ADR-0028 detectar
 * reuso). Hoje o `MultimodelStage` persiste só `input_hash`
 * (FNV-1a) e `input_artifact_id` (referência ao artefato
 * consumido) — não o texto. Pra retomar fielmente seria
 * preciso adicionar `input_text: String` ao
 * `MultimodelStage` (mudança de schema, migração 0031). Esse
 * trabalho é de fase futura; a Etapa 7 entrega o
 * `pipeline_view` que **consome o estado** e o "cancelar" /
 * "criar" que **não precisam** do input original. A regra
 * "degradação declarada > substituição silenciosa" (memory
 * 2026-08-03) manda sinalizar a limitação na UI, não
 * fingir que funciona.
 */

import { useCallback, useEffect, useState } from "react";
import {
  cancelPipeline,
  listPipelineStages,
  listResumablePipelines,
  type MultimodelRunView,
  type MultimodelStageView,
} from "../services";
import { PipelineCreateForm } from "./PipelineCreateForm";

/** Trunca um UUID pra exibir. */
function shortId(id: string | null): string {
  if (!id) return "—";
  return id.slice(0, 8);
}

/** Formata um ISO timestamp pra formato curto. */
function shortTime(iso: string | null): string {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString("pt-BR", {
      hour: "2-digit",
      minute: "2-digit",
      day: "2-digit",
      month: "2-digit",
    });
  } catch {
    return iso;
  }
}

/** Cor do badge de `state` (mesmo padrão do `MemoryPanel`). */
function stateBadgeClass(state: string): string {
  switch (state) {
    case "completed":
      return "badge badge-info";
    case "running":
    case "streaming":
      return "badge";
    case "failed":
    case "cancelled":
    case "partially_completed":
      return "badge badge-warn";
    case "pending":
    default:
      return "badge badge-error";
  }
}

export function PipelineView() {
  const [pipelines, setPipelines] = useState<MultimodelRunView[]>([]);
  const [selected, setSelected] = useState<MultimodelRunView | null>(null);
  const [stages, setStages] = useState<MultimodelStageView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await listResumablePipelines();
      setPipelines(list);
      setError(null);
    } catch (e) {
      setError(
        `Não foi possível listar pipelines resumable: ${
          e instanceof Error ? e.message : String(e)
        }. O backend pode estar indisponível.`,
      );
    } finally {
      setLoading(false);
    }
  }, []);

  // Polling 2s (mesmo padrão do `App.tsx` para a fila de aprovação).
  useEffect(() => {
    refresh();
    const id = window.setInterval(refresh, 2000);
    return () => window.clearInterval(id);
  }, [refresh]);

  // Carrega os stages quando seleciona um pipeline.
  useEffect(() => {
    if (!selected) {
      setStages([]);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const list = await listPipelineStages(selected.id);
        if (!cancelled) {
          setStages(list);
        }
      } catch (e) {
        if (!cancelled) {
          setStages([]);
          setError(
            `Não foi possível listar stages do pipeline ${shortId(
              selected.id,
            )}: ${e instanceof Error ? e.message : String(e)}`,
          );
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected]);

  // Se o pipeline selecionado sumiu da lista (terminou ou foi
  // cancelado), limpa a seleção. Roda junto com o polling de
  // `pipelines` — se o `selected.id` não tá mais na lista,
  // desseleciona.
  useEffect(() => {
    if (selected && !pipelines.find((p) => p.id === selected.id)) {
      setSelected(null);
      setStages([]);
    }
  }, [pipelines, selected]);

  const handleCancel = useCallback(
    async (pipelineId: string) => {
      try {
        await cancelPipeline(pipelineId);
        // Re-fetch imediato (não espera 2s) — o user clicou
        // "Cancelar" e quer ver o efeito já.
        await refresh();
      } catch (e) {
        setError(
          `Falha ao cancelar pipeline: ${
            e instanceof Error ? e.message : String(e)
          }`,
        );
      }
    },
    [refresh],
  );

  const handleCreated = useCallback(
    async (newId: string) => {
      setShowCreate(false);
      // Re-fetch + seleciona o pipeline recém-criado.
      await refresh();
      const list = await listResumablePipelines();
      const created = list.find((p) => p.id === newId);
      if (created) setSelected(created);
    },
    [refresh],
  );

  if (loading) {
    return <p>Carregando pipelines…</p>;
  }

  return (
    <div className="pipeline-view">
      <aside className="pipeline-sidebar">
        <header className="pipeline-sidebar__header">
          <h2>Modo Equipe</h2>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setShowCreate(true)}
          >
            + Novo pipeline
          </button>
        </header>
        {error && (
          <div className="error" role="alert">
            {error}
          </div>
        )}
        {pipelines.length === 0 ? (
          <p className="pipeline-empty">
            Nenhum pipeline em curso ou interrompido. Crie um acima pra
            começar.
          </p>
        ) : (
          <ul className="pipeline-list" role="listbox">
            {pipelines.map((p) => (
              <li
                key={p.id}
                className={
                  "pipeline-list__item" +
                  (selected?.id === p.id ? " pipeline-list__item--selected" : "")
                }
                role="option"
                aria-selected={selected?.id === p.id}
              >
                <button
                  type="button"
                  className="pipeline-list__item-button"
                  onClick={() => setSelected(p)}
                >
                  <div className="pipeline-list__item-header">
                    <code className="pipeline-list__item-id">
                      {shortId(p.id)}
                    </code>
                    <span className={stateBadgeClass(p.state)}>
                      {p.state}
                    </span>
                  </div>
                  <div className="pipeline-list__item-meta">
                    <span>custo: ${(p.total_cost_microcents / 1_000_000).toFixed(4)}</span>
                    <span title={p.updated_at}>
                      atualizado: {shortTime(p.updated_at)}
                    </span>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </aside>

      <section className="pipeline-detail">
        {!selected ? (
          <p className="pipeline-empty">
            Selecione um pipeline na sidebar pra ver os estágios.
          </p>
        ) : (
          <>
            <header className="pipeline-detail__header">
              <div>
                <h2>Pipeline {shortId(selected.id)}</h2>
                <p className="pipeline-detail__meta">
                  parent: <code>{shortId(selected.parent_run_id)}</code> ·
                  estado:{" "}
                  <span className={stateBadgeClass(selected.state)}>
                    {selected.state}
                  </span>{" "}
                  · custo total: $
                  {(selected.total_cost_microcents / 1_000_000).toFixed(4)} ·
                  criado: {shortTime(selected.created_at)} · atualizado:{" "}
                  {shortTime(selected.updated_at)}
                </p>
              </div>
              {selected.state === "running" && (
                <button
                  type="button"
                  className="btn-danger"
                  onClick={() => handleCancel(selected.id)}
                >
                  Cancelar pipeline
                </button>
              )}
              {selected.state === "partially_completed" && (
                <button
                  type="button"
                  className="btn-primary"
                  disabled
                  title="Retomar requer armazenar o input original de cada stage no DB (mudança de schema, migração 0031) — trabalho de fase futura. Por ora, crie um novo pipeline."
                >
                  Retomar (em breve)
                </button>
              )}
            </header>

            <h3>Estágios ({stages.length})</h3>
            {stages.length === 0 ? (
              <p className="pipeline-empty">
                Nenhum estágio registrado pra este pipeline (pode estar
                pendente de inicialização).
              </p>
            ) : (
              <ol className="pipeline-stages">
                {stages.map((s) => (
                  <li key={s.id} className="pipeline-stage">
                    <div className="pipeline-stage__header">
                      <span className="pipeline-stage__seq">#{s.seq}</span>
                      <code className="pipeline-stage__id">
                        {shortId(s.id)}
                      </code>
                      <span className={stateBadgeClass(s.state)}>
                        {s.state}
                      </span>
                    </div>
                    <div className="pipeline-stage__meta">
                      <span>
                        provider: <code>{s.provider_id}</code>
                      </span>
                      <span>
                        modelo: <code>{s.model_id}</code>
                      </span>
                      <span>
                        custo: $
                        {(s.cost_microcents / 1_000_000).toFixed(4)}
                      </span>
                    </div>
                    <div className="pipeline-stage__hashes">
                      {s.input_hash && (
                        <span>
                          in_hash: <code>{s.input_hash.slice(0, 16)}</code>
                        </span>
                      )}
                      {s.output_hash && (
                        <span>
                          out_hash: <code>{s.output_hash.slice(0, 16)}</code>
                        </span>
                      )}
                    </div>
                    <div className="pipeline-stage__times">
                      <span>started: {shortTime(s.started_at)}</span>
                      <span>finished: {shortTime(s.finished_at)}</span>
                    </div>
                  </li>
                ))}
              </ol>
            )}
          </>
        )}
      </section>

      {showCreate && (
        <PipelineCreateForm
          onClose={() => setShowCreate(false)}
          onCreated={handleCreated}
        />
      )}
    </div>
  );
}
