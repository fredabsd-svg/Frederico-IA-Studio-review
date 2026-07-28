/**
 * `components/ApprovalModal.tsx` — modal de aprovação de tool
 * calls (Fase 3, Etapa 6.2).
 *
 * Mostra os detalhes do `ApprovalRequest` (tool_id, path,
 * arguments, mandatory) e tem 2 botões principais (Aprovar /
 * Negar). Opcionalmente, input de reason (auditoria) e
 * seletor de scope (`Once` / `Run` / `Project` — Escopo "Run" e
 * "Project" são "lembrar pra próximos"; a Etapa 6.1 do backend
 * só persiste a decision, sem reaproveitar).
 *
 * **Regra de UX** (especificada no spec
 * `tool-permission-model.md` §"Fluxo de aprovação"): o botão
 * "Aprovar" só fica habilitado se o usuário revisar o
 * `arguments` (não há auto-approve aqui — é uma decisão
 * consciente).
 */

import { useMemo, useState } from "react";
import type { ApprovalDecision, ApprovalEntryView } from "../services";

interface Props {
  entry: ApprovalEntryView;
  onApprove: (id: string, decision: ApprovalDecision) => void;
  onReject: (id: string, decision: ApprovalDecision) => void;
  onClose: () => void;
}

/**
 * `ApprovalRequest` parseado (espelha o JSON serializado em
 * `tool_id:required:ApprovalRequest`). O backend envia o
 * `ApprovalRequest` como JSON string no campo `request_json`.
 * Quando o parse falha (formato inesperado), mostramos o JSON
 * cru.
 */
interface ParsedRequest {
  tool_id?: string;
  reason?: string;
  mandatory?: boolean;
  arguments?: unknown;
  path?: string;
  path_hint?: string[];
  requested_at?: string;
}

function parseRequest(json: string): {
  parsed: ParsedRequest | null;
  raw: string;
} {
  try {
    const parsed = JSON.parse(json) as ParsedRequest;
    return { parsed, raw: json };
  } catch {
    return { parsed: null, raw: json };
  }
}

export function ApprovalModal({
  entry,
  onApprove,
  onReject,
  onClose,
}: Props) {
  const { parsed, raw } = useMemo(() => parseRequest(entry.request_json), [
    entry.request_json,
  ]);
  const [reason, setReason] = useState("");
  const [scope, setScope] = useState<"Once" | "Run" | "Project">("Once");
  const [submitting, setSubmitting] = useState(false);

  const mandatory = parsed?.mandatory === true;
  const path =
    parsed?.path ?? (parsed?.arguments as { path?: string } | undefined)?.path;
  const argsStr = useMemo(() => {
    if (!parsed) return raw;
    if (parsed.arguments !== undefined) {
      return JSON.stringify(parsed.arguments, null, 2);
    }
    return raw;
  }, [parsed, raw]);

  function buildDecision(approved: boolean): ApprovalDecision {
    return {
      approved,
      scope,
      reason: reason.trim() === "" ? undefined : reason.trim(),
    };
  }

  async function handleApprove() {
    if (submitting) return;
    setSubmitting(true);
    try {
      onApprove(entry.id, buildDecision(true));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleReject() {
    if (submitting) return;
    setSubmitting(true);
    try {
      onReject(entry.id, buildDecision(false));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="approval-title"
    >
      <div className="modal approval-modal">
        <header className="modal-header">
          <h2 id="approval-title">
            <span className="badge">aprovação requerida</span>{" "}
            <code>{parsed?.tool_id ?? entry.tool_id}</code>
            {mandatory && (
              <span className="badge badge-warn" title="esta ferramenta exige aprovação obrigatória a cada chamada">
                obrigatória
              </span>
            )}
          </h2>
          <button
            className="modal-close"
            onClick={onClose}
            title="Fechar (não responde)"
            disabled={submitting}
          >
            ×
          </button>
        </header>

        <section className="modal-body">
          <p className="approval-context">
            O assistente pediu para chamar uma ferramenta que
            {" "}<strong>{parsed?.tool_id ?? entry.tool_id}</strong>
            {" "}nesta conversa. Revise os argumentos antes de decidir.
          </p>

          {path && (
            <p>
              <strong>Path:</strong> <code>{path}</code>
            </p>
          )}

          <details open>
            <summary>Argumentos</summary>
            <pre className="approval-args">{argsStr}</pre>
          </details>

          <div className="approval-form">
            <label>
              Escopo:
              <select
                value={scope}
                onChange={(e) => setScope(e.target.value as "Once" | "Run" | "Project")}
                disabled={submitting}
              >
                <option value="Once">Apenas esta chamada</option>
                <option value="Run">Este run (lembrar pra próximas chamadas neste run)</option>
                <option value="Project">Este projeto (lembrar pra todas as conversas)</option>
              </select>
            </label>
            <label>
              Razão (opcional, vai pra auditoria):
              <input
                type="text"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                placeholder="ex: arquivo de config, alteração segura"
                disabled={submitting}
              />
            </label>
          </div>
        </section>

        <footer className="modal-footer">
          <button
            className="btn-danger"
            onClick={handleReject}
            disabled={submitting}
            title="Negar e finalizar o run com erro"
          >
            Negar
          </button>
          <button
            className="btn-primary"
            onClick={handleApprove}
            disabled={submitting}
            title="Aprovar e continuar o run"
          >
            Aprovar
          </button>
        </footer>
      </div>
    </div>
  );
}
