/**
 * `components/PipelineCreateForm.tsx` — formulário de criação
 * de pipeline (Fase 6, Etapa 7 UI/Polish).
 *
 * Modal que:
 * 1. Pede o `parent_run_id` (= `RunId` da conversa onde o
 *    pipeline vai rodar). O user escolhe via dropdown de
 *    conversas existentes (`listConversations()`).
 * 2. Pede a lista de stages (cada um com `model_id` +
 *    `provider_id` + `input`). Pode adicionar/remover
 *    dinamicamente. Default: 2 stages com `simulated` /
 *    `simulated-echo` (provider simulado, sempre disponível).
 * 3. Chama `startPipeline(parent_run_id, stages)` e devolve
 *    o `pipeline_id` pro caller via `onCreated`.
 *
 * **Por que `simulated` como default:** o `simulated` é o
 * único provider que **sempre** está configurado
 * (`build_provider_map` no `main.rs` insere ele sem precisar
 * de credencial). Isso permite testar o Modo Equipe sem
 * configurar OpenAI/OpenRouter/etc. O user troca depois via
 * dropdown de provider.
 *
 * **Por que `simulated` aceita qualquer model_id:** o
 * `FakeProviderAdapter` (Etapa 3 PR 2) é o adapter do
 * `simulated` e responde a qualquer model_id com um echo
 * determinístico. Isso evita ter que validar o modelo no
 * dropdown — o backend (`MultimodelOrchestrator`) é quem
 * detecta modelo inválido (`PipelineError::ModelNotFound`).
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  listCatalog,
  listConversations,
  startPipeline,
  type ConversationView,
  type ModelDescriptorView,
  type StageSpecView,
} from "../services";

interface StageDraft {
  /** Key local pro React (estável entre renders). */
  key: string;
  provider_id: string;
  model_id: string;
  input: string;
}

function newStageDraft(): StageDraft {
  return {
    key: crypto.randomUUID(),
    provider_id: "simulated",
    model_id: "simulated-echo",
    input: "",
  };
}

interface Props {
  onClose: () => void;
  onCreated: (pipelineId: string) => void;
}

export function PipelineCreateForm({ onClose, onCreated }: Props) {
  const [conversations, setConversations] = useState<ConversationView[]>([]);
  const [catalog, setCatalog] = useState<ModelDescriptorView[]>([]);
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [parentRunId, setParentRunId] = useState<string>("");
  const [stages, setStages] = useState<StageDraft[]>([
    newStageDraft(),
    newStageDraft(),
  ]);

  // Carrega conversas e catálogo uma vez no mount.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [convs, cat] = await Promise.all([
          listConversations(),
          listCatalog(),
        ]);
        if (cancelled) return;
        setConversations(convs);
        setCatalog(cat);
        if (convs.length > 0) setParentRunId(convs[0].id);
        setLoading(false);
      } catch (e) {
        if (cancelled) return;
        setError(
          `Falha ao carregar dependências: ${
            e instanceof Error ? e.message : String(e)
          }`,
        );
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Provider IDs únicos do catálogo.
  const providerIds = useMemo(() => {
    const set = new Set<string>();
    for (const m of catalog) set.add(m.provider);
    // Garante que `simulated` aparece mesmo se o catálogo não
    // tiver (provider bundled, sempre presente).
    set.add("simulated");
    return Array.from(set).sort();
  }, [catalog]);

  // Modelos disponíveis pro `provider_id` selecionado.
  const modelsForProvider = useCallback(
    (providerId: string): ModelDescriptorView[] => {
      return catalog.filter((m) => m.provider === providerId);
    },
    [catalog],
  );

  const updateStage = (key: string, patch: Partial<StageDraft>) => {
    setStages((prev) =>
      prev.map((s) => (s.key === key ? { ...s, ...patch } : s)),
    );
  };

  const addStage = () => {
    setStages((prev) => [...prev, newStageDraft()]);
  };

  const removeStage = (key: string) => {
    setStages((prev) =>
      prev.length > 1 ? prev.filter((s) => s.key !== key) : prev,
    );
  };

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      setError(null);

      if (!parentRunId) {
        setError("Selecione uma conversa de origem (parent_run_id).");
        return;
      }
      if (stages.length === 0) {
        setError("Adicione pelo menos 1 stage.");
        return;
      }
      for (const s of stages) {
        if (!s.provider_id || !s.model_id || !s.input.trim()) {
          setError(
            "Todos os stages precisam ter provider, modelo e input preenchidos.",
          );
          return;
        }
      }

      const stageSpecs: StageSpecView[] = stages.map((s) => ({
        provider_id: s.provider_id,
        model_id: s.model_id,
        input: s.input,
      }));

      setSubmitting(true);
      try {
        const newId = await startPipeline(parentRunId, stageSpecs);
        onCreated(newId);
      } catch (err) {
        setError(
          `Falha ao criar pipeline: ${
            err instanceof Error ? err.message : String(err)
          }`,
        );
        setSubmitting(false);
      }
    },
    [parentRunId, stages, onCreated],
  );

  if (loading) {
    return (
      <div className="modal-backdrop" onClick={onClose}>
        <div
          className="modal"
          onClick={(e) => e.stopPropagation()}
          role="dialog"
          aria-label="Criar pipeline"
        >
          <div className="modal-body">
            <p>Carregando…</p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      className="modal-backdrop"
      onClick={onClose}
      role="dialog"
      aria-label="Criar pipeline"
    >
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
      >
        <header className="modal-header">
          <h2>Criar pipeline sequencial</h2>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label="Fechar"
          >
            ×
          </button>
        </header>
        <form onSubmit={handleSubmit}>
          <div className="modal-body">
            {error && (
              <div className="error" role="alert">
                {error}
              </div>
            )}

            <label>
              Conversa de origem (parent_run_id)
              <select
                value={parentRunId}
                onChange={(e) => setParentRunId(e.target.value)}
                disabled={submitting}
                required
              >
                {conversations.length === 0 ? (
                  <option value="">(nenhuma conversa — crie uma em /chat)</option>
                ) : (
                  conversations.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.title || c.id.slice(0, 8)} · {c.provider_id}:{c.model_id}
                    </option>
                  ))
                )}
              </select>
            </label>

            <h3 style={{ marginTop: "1rem" }}>Estágios ({stages.length})</h3>
            <p className="pipeline-empty" style={{ fontSize: "0.85rem" }}>
              Os estágios rodam sequencialmente. O output de cada um vira o
              input do próximo (D5 do ADR-0028).
            </p>

            {stages.map((s, idx) => {
              const models = modelsForProvider(s.provider_id);
              return (
                <div key={s.key} className="pipeline-stage-draft">
                  <div className="pipeline-stage-draft__header">
                    <strong>#{idx + 1}</strong>
                    {stages.length > 1 && (
                      <button
                        type="button"
                        className="link-danger"
                        onClick={() => removeStage(s.key)}
                        disabled={submitting}
                        aria-label={`Remover estágio ${idx + 1}`}
                      >
                        ×
                      </button>
                    )}
                  </div>
                  <label>
                    Provider
                    <select
                      value={s.provider_id}
                      onChange={(e) =>
                        updateStage(s.key, {
                          provider_id: e.target.value,
                          // Reseta o model_id se o provider mudou
                          // e o modelo antigo não está mais
                          // disponível.
                          model_id: modelsForProvider(e.target.value)[0]?.model ?? "",
                        })
                      }
                      disabled={submitting}
                    >
                      {providerIds.map((p) => (
                        <option key={p} value={p}>
                          {p}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    Modelo
                    {models.length > 0 ? (
                      <select
                        value={s.model_id}
                        onChange={(e) =>
                          updateStage(s.key, { model_id: e.target.value })
                        }
                        disabled={submitting}
                      >
                        {models.map((m) => (
                          <option key={m.model} value={m.model}>
                            {m.model}
                          </option>
                        ))}
                      </select>
                    ) : (
                      <input
                        type="text"
                        value={s.model_id}
                        onChange={(e) =>
                          updateStage(s.key, { model_id: e.target.value })
                        }
                        disabled={submitting}
                        placeholder="modelo (livre — backend valida)"
                      />
                    )}
                  </label>
                  <label>
                    Input
                    <textarea
                      value={s.input}
                      onChange={(e) =>
                        updateStage(s.key, { input: e.target.value })
                      }
                      disabled={submitting}
                      placeholder={`Texto que vai pro modelo (input do estágio #${idx + 1})`}
                      rows={3}
                    />
                  </label>
                </div>
              );
            })}

            <button
              type="button"
              className="btn-primary"
              onClick={addStage}
              disabled={submitting}
              style={{ marginTop: "0.5rem" }}
            >
              + Adicionar estágio
            </button>
          </div>
          <div className="modal-footer">
            <button
              type="button"
              className="btn-danger"
              onClick={onClose}
              disabled={submitting}
            >
              Cancelar
            </button>
            <button
              type="submit"
              className="btn-primary"
              disabled={submitting || conversations.length === 0}
            >
              {submitting ? "Criando…" : "Criar pipeline"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
