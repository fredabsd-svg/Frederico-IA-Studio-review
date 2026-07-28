/**
 * `components/MemoryPanel.tsx` — painel de memória (Fase 4, Etapa 5).
 *
 * Lista memórias do escopo selecionado, mostra o
 * `ScoreBreakdown` (explicabilidade `PROMPT MESTRE` §10.11),
 * e oferece o fluxo de "corrija para X" (botão em cada card
 * que abre um form inline).
 *
 * **Pendentes de revisão** (`pending_review = true`) ganham
 * badge e botões "Confirmar" / "Rejeitar" — a Etapa 5 do
 * backend expõe `MemoryList { include_pending: true }` pra
 * trazer a fila junto.
 *
 * **Expiradas** aparecem com badge "expirada" mas só se não
 * forem pinned (a regra do `ADR-0014 §1`).
 *
 * **Correção** marca a antiga como superseded e insere a
 * nova atomicamente (`MemoryRepo::apply_correction` no Rust,
 * `BEGIN IMMEDIATE`). Após aplicar, o card da antiga some
 * (filtro `superseded_by IS NULL`) e a nova aparece no topo
 * com tipo `correction`.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  applyCorrection,
  confirmPending,
  listMemories,
  purgeExpiredMemories,
  rejectPending,
  retrieveMemories,
  type CorrectionResultView,
  type MemoryHitView,
  type MemoryView,
  type NewMemoryInputView,
} from "../services";

interface Props {
  /** Escopo default. Default: `"project"` + escopo vazio. */
  initialScopeType?: string;
  initialScopeId?: string;
}

const SCOPE_TYPES = [
  "project",
  "preference",
  "profile",
  "assistant",
  "client",
  "conversation",
  "document",
  "task",
  "session",
] as const;

const MEMORY_TYPES = [
  "preference",
  "fact",
  "decision",
  "correction",
  "project_instruction",
  "client_context",
  "procedure",
  "delivery_pattern",
  "temporary",
  "conversation_summary",
  "document_reference",
  "user_pinned",
] as const;

const ORIGINS = ["user", "assistant", "external_content"] as const;

export function MemoryPanel({
  initialScopeType = "project",
  initialScopeId = "",
}: Props) {
  const [scopeType, setScopeType] = useState(initialScopeType);
  const [scopeId, setScopeId] = useState(initialScopeId);
  const [includePending, setIncludePending] = useState(false);
  const [query, setQuery] = useState("");
  const [memories, setMemories] = useState<MemoryView[]>([]);
  const [hits, setHits] = useState<MemoryHitView[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [correctionFor, setCorrectionFor] = useState<MemoryView | null>(null);
  const [lastCorrection, setLastCorrection] = useState<
    CorrectionResultView | null
  >(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listMemories(scopeType, scopeId, includePending);
      setMemories(list);
      if (query.trim() !== "") {
        const retrieved = await retrieveMemories(
          scopeType,
          scopeId,
          query,
          8,
        );
        setHits(retrieved);
      } else {
        setHits(null);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [scopeType, scopeId, includePending, query]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handlePurge = useCallback(async () => {
    if (
      !window.confirm(
        "Limpar memórias expiradas, superseded e soft-deletadas? (não tem volta)",
      )
    )
      return;
    try {
      const n = await purgeExpiredMemories();
      window.alert(`${n} memória(s) purgadas.`);
      refresh();
    } catch (e) {
      window.alert(`Erro: ${e}`);
    }
  }, [refresh]);

  const handleCorrectionApplied = useCallback(
    (result: CorrectionResultView) => {
      setCorrectionFor(null);
      setLastCorrection(result);
      refresh();
    },
    [refresh],
  );

  return (
    <div className="memory-panel">
      <header className="memory-header">
        <h2>Memórias</h2>
        <div className="memory-controls">
          <label>
            Escopo:
            <select
              value={scopeType}
              onChange={(e) => setScopeType(e.target.value)}
            >
              {SCOPE_TYPES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          </label>
          <label>
            ID:
            <input
              type="text"
              value={scopeId}
              onChange={(e) => setScopeId(e.target.value)}
              placeholder="(vazio = escopos globais)"
            />
          </label>
          <label>
            <input
              type="checkbox"
              checked={includePending}
              onChange={(e) => setIncludePending(e.target.checked)}
            />
            incluir pendentes de revisão
          </label>
          <label>
            Buscar:
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="(vazio = lista tudo)"
            />
          </label>
          <button onClick={refresh} disabled={loading}>
            Atualizar
          </button>
          <button onClick={handlePurge} disabled={loading} className="btn-danger">
            Limpar expiradas
          </button>
        </div>
      </header>

      {error && <div className="memory-error">Erro: {error}</div>}

      {lastCorrection && (
        <div className="memory-banner" role="status">
          ✅ Memória substituída. A antiga{" "}
          <code>{lastCorrection.old_id.slice(0, 8)}</code> foi marcada
          superseded e a nova{" "}
          <code>{lastCorrection.new_record.id.slice(0, 8)}</code> foi
          gravada.
        </div>
      )}

      {hits && hits.length > 0 && (
        <section className="memory-section">
          <h3>Hits da busca ({hits.length})</h3>
          {hits.map((h) => (
            <MemoryCard
              key={h.record.id}
              record={h.record}
              hit={h}
              onCorrect={() => setCorrectionFor(h.record)}
            />
          ))}
        </section>
      )}

      <section className="memory-section">
        <h3>Todas as memórias do escopo ({memories.length})</h3>
        {loading && <p>Carregando…</p>}
        {!loading && memories.length === 0 && (
          <p className="memory-empty">Nenhuma memória neste escopo.</p>
        )}
        {memories.map((m) => (
          <MemoryCard
            key={m.id}
            record={m}
            onCorrect={() => setCorrectionFor(m)}
            onConfirm={
              m.pending_review
                ? async () => {
                    try {
                      await confirmPending(m.id);
                      refresh();
                    } catch (e) {
                      window.alert(`Erro: ${e}`);
                    }
                  }
                : undefined
            }
            onReject={
              m.pending_review
                ? async () => {
                    try {
                      await rejectPending(m.id);
                      refresh();
                    } catch (e) {
                      window.alert(`Erro: ${e}`);
                    }
                  }
                : undefined
            }
          />
        ))}
      </section>

      {correctionFor && (
        <CorrectionForm
          old={correctionFor}
          scopeType={scopeType}
          scopeId={scopeId}
          onClose={() => setCorrectionFor(null)}
          onApplied={handleCorrectionApplied}
        />
      )}
    </div>
  );
}

interface MemoryCardProps {
  record: MemoryView;
  hit?: MemoryHitView;
  onCorrect: () => void;
  onConfirm?: () => Promise<void> | void;
  onReject?: () => Promise<void> | void;
}

function MemoryCard({
  record,
  hit,
  onCorrect,
  onConfirm,
  onReject,
}: MemoryCardProps) {
  const isExpired = useMemo(() => {
    if (!record.expires_at) return false;
    return new Date(record.expires_at) < new Date();
  }, [record.expires_at]);

  return (
    <article className="memory-card">
      <header className="memory-card-header">
        <span className="badge">{record.type_}</span>
        <span className="badge">{record.origin}</span>
        {record.user_pinned && (
          <span className="badge badge-warn" title="Fixada pelo usuário — bypassa expires_at">
            📌 pinned
          </span>
        )}
        {record.pending_review && (
          <span className="badge badge-warn" title="Aguardando revisão humana (ExternalContent)">
            ⏳ pendente de revisão
          </span>
        )}
        {isExpired && !record.user_pinned && (
          <span className="badge badge-error" title="Expirada — não aparece no retrieval (exceto se pinned)">
            ⏰ expirada
          </span>
        )}
        {record.superseded_by && (
          <span
            className="badge badge-info"
            title={`Substituída por ${record.superseded_by}`}
          >
            ↪ superseded
          </span>
        )}
        <span className="memory-meta">
          escopo: {record.scope_type}
          {record.scope_id ? `:${record.scope_id}` : ""} ·{" "}
          conf: {record.confidence.toFixed(2)} · imp:{" "}
          {record.importance.toFixed(2)}
        </span>
      </header>

      <p className="memory-content">{record.content}</p>

      {hit && (
        <details className="memory-score" open>
          <summary>
            score: {hit.score.toFixed(3)} — {hit.explanation}
          </summary>
          <table>
            <tbody>
              <tr>
                <td>lexical (BM25)</td>
                <td>{hit.score_breakdown.lexical.toFixed(3)}</td>
              </tr>
              <tr>
                <td>recency (decay exponencial)</td>
                <td>{hit.score_breakdown.recency.toFixed(3)}</td>
              </tr>
              <tr>
                <td>semantic (cosine)</td>
                <td>{hit.score_breakdown.semantic.toFixed(3)}</td>
              </tr>
              <tr>
                <td>importance</td>
                <td>{hit.score_breakdown.importance.toFixed(3)}</td>
              </tr>
              <tr>
                <td>confirmation (pinned/confirmed)</td>
                <td>{hit.score_breakdown.confirmation.toFixed(3)}</td>
              </tr>
              <tr>
                <td>scope_match</td>
                <td>{hit.score_breakdown.scope_match ? "✅" : "❌"}</td>
              </tr>
            </tbody>
          </table>
        </details>
      )}

      <footer className="memory-card-footer">
        {record.superseded_at && (
          <small className="memory-superseded">
            Substituída em{" "}
            {new Date(record.superseded_at).toLocaleString("pt-BR")} por{" "}
            <code>{record.superseded_by?.slice(0, 8) ?? "?"}</code>
          </small>
        )}
        {onConfirm && onReject && (
          <>
            <button onClick={onConfirm} className="btn-primary">
              Confirmar
            </button>
            <button onClick={onReject} className="btn-danger">
              Rejeitar
            </button>
          </>
        )}
        <button onClick={onCorrect} title="Corrija para X">
          Corrija para X
        </button>
      </footer>
    </article>
  );
}

interface CorrectionFormProps {
  old: MemoryView;
  scopeType: string;
  scopeId: string;
  onClose: () => void;
  onApplied: (result: CorrectionResultView) => void;
}

function CorrectionForm({
  old,
  scopeType,
  scopeId,
  onClose,
  onApplied,
}: CorrectionFormProps) {
  const [content, setContent] = useState("");
  const [type, setType] = useState<string>("correction");
  const [importance, setImportance] = useState(0.7);
  const [confidence, setConfidence] = useState(0.9);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit() {
    if (content.trim() === "") {
      window.alert("content não pode ser vazio");
      return;
    }
    setSubmitting(true);
    try {
      const replacement: NewMemoryInputView = {
        scope_type: scopeType,
        scope_id: scopeId,
        type_: type,
        content: content.trim(),
        origin: "user",
        source_type: "user_message",
        source_id: null,
        confidence,
        importance,
        expires_at: null,
      };
      const result = await applyCorrection(old.id, replacement);
      onApplied(result);
    } catch (e) {
      window.alert(`Erro: ${e}`);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true">
      <div className="modal">
        <header className="modal-header">
          <h2>Corrija para X</h2>
          <button onClick={onClose} className="modal-close">
            ×
          </button>
        </header>
        <section className="modal-body">
          <p className="correction-context">
            Substituindo a memória{" "}
            <code>{old.id.slice(0, 8)}</code>:
          </p>
          <blockquote className="correction-old">{old.content}</blockquote>

          <label>
            Tipo:
            <select value={type} onChange={(e) => setType(e.target.value)}>
              {MEMORY_TYPES.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </label>

          <label>
            Origem:
            <select value="user" disabled>
              {ORIGINS.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          </label>

          <label>
            Conteúdo corrigido:
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              rows={4}
              placeholder="digite o conteúdo da nova memória"
            />
          </label>

          <label>
            Confidence ({confidence.toFixed(2)}):
            <input
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={confidence}
              onChange={(e) => setConfidence(Number(e.target.value))}
            />
          </label>
          <label>
            Importance ({importance.toFixed(2)}):
            <input
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={importance}
              onChange={(e) => setImportance(Number(e.target.value))}
            />
          </label>
        </section>
        <footer className="modal-footer">
          <button onClick={onClose} disabled={submitting}>
            Cancelar
          </button>
          <button
            onClick={handleSubmit}
            disabled={submitting || content.trim() === ""}
            className="btn-primary"
          >
            Aplicar correção
          </button>
        </footer>
      </div>
    </div>
  );
}
