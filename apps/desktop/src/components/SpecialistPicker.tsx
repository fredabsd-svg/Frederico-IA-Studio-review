/**
 * `components/SpecialistPicker.tsx` — dropdown de especialistas
 * (Fase 6, Etapa 3, ADR-0030 §D5).
 *
 * Lista os especialistas disponíveis (`SpecialistSummary` do
 * `list_specialists` Tauri command) com filtro por capability.
 * Renderiza mas **não dispara spawn** — a Etapa 3 entrega o
 * **componente base** que a Etapa 6 (UI do Modo Equipe) e o
 * `SubagentRunner` da Etapa 4 consomem.
 *
 * ## Estado
 *
 * - `loading` (inicial: true): mostra "Carregando…". Termina
 *   quando o `listSpecialists()` resolve (sucesso ou erro).
 * - `error` (string | null): mensagem PT-BR do erro. Botão
 *   "Tentar de novo" recarrega.
 * - `specialists` (SpecialistSummary[]): lista completa do
 *   registry. Recarregada só via botão "Tentar de novo" — o
 *   registry é estático (bundled + override de arquivo, Etapa
 *   6 introduz hot-reload via filesystem watch).
 * - `capabilityFilter` (string): filtro livre. Match
 *   case-insensitive em `name`, `description` e
 *   `capability_tags`. Vazio = sem filtro.
 * - `selected` (string | null): ID do especialista escolhido.
 *   O `onSelect` prop é chamado quando o usuário clica num
 *   card. **Não dispara spawn** — a Etapa 4 vai plugar o
 *   `SubagentRunner` aqui.
 *
 * ## Pendência de Vitest
 *
 * O projeto não tem Vitest no `apps/desktop` (verificado em
 * 2026-08-06: `package.json` não tem `vitest` em
 * `devDependencies`). A ADR-0030 §D5 menciona "Suíte de
 * testes do componente (Vitest + Testing Library)" mas isso é
 * trabalho de hardeings separado (adicionar Vitest ao
 * `apps/desktop` requer config nova, `@testing-library/react`,
 * setup de jsdom, etc.). O teste manual da UI é o que cobre a
 * Etapa 3 por ora — o E2E em `crates/e2e/tests/` cobre o
 * contrato de dados (registry → SpecialistSummary → JSON
 * serializado).
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface SpecialistSummaryView {
  id: string;
  name: string;
  description: string;
  default_model_capabilities: string[];
  default_model: string;
  capability_tags: string[];
}

/** Wrapper sobre o Tauri command `list_specialists`. */
async function listSpecialists(): Promise<SpecialistSummaryView[]> {
  // O `list_specialists` está registrado no `invoke_handler!`
  // (main.rs) como `#[tauri::command]`. A tipagem do
  // `invoke<T>()` casa com o `Result<Vec<SpecialistSummary>, String>`
  // do Rust — sucesso = array de summaries, erro = string PT-BR.
  return await invoke<SpecialistSummaryView[]>("list_specialists");
}

export interface SpecialistPickerProps {
  /** Callback quando o usuário escolhe um especialista.
   *  Não dispara spawn — só notifica o ID escolhido. */
  onSelect?: (specialist: SpecialistSummaryView) => void;
  /** Habilita/desabilita o picker inteiro. */
  disabled?: boolean;
  /** Texto do label (default: "Especialista"). */
  label?: string;
  /** Placeholder do campo de filtro (default: "Buscar…"). */
  filterPlaceholder?: string;
}

export function SpecialistPicker({
  onSelect,
  disabled = false,
  label = "Especialista",
  filterPlaceholder = "Buscar…",
}: SpecialistPickerProps) {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [specialists, setSpecialists] = useState<SpecialistSummaryView[]>(
    [],
  );
  const [capabilityFilter, setCapabilityFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listSpecialists();
      setSpecialists(list);
    } catch (e) {
      setError(
        `Não foi possível carregar a lista de especialistas: ${String(
          e,
        )}. O ` +
          `SpecialistRegistry pode estar com erro de configuração — ` +
          `verifique os logs da casca.`,
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = capabilityFilter.trim().toLowerCase();
    if (q === "") return specialists;
    return specialists.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q) ||
        s.capability_tags.some((t) => t.toLowerCase().includes(q)),
    );
  }, [specialists, capabilityFilter]);

  const handleSelect = useCallback(
    (s: SpecialistSummaryView) => {
      setSelected(s.id);
      onSelect?.(s);
    },
    [onSelect],
  );

  if (loading) {
    return (
      <div className="specialist-picker specialist-picker--loading">
        <span className="specialist-picker__label">{label}</span>
        <span className="specialist-picker__status">Carregando…</span>
      </div>
    );
  }

  if (error) {
    return (
      <div className="specialist-picker specialist-picker--error">
        <span className="specialist-picker__label">{label}</span>
        <span className="specialist-picker__error" role="alert">
          {error}
        </span>
        <button
          type="button"
          className="specialist-picker__retry"
          onClick={load}
          disabled={disabled}
        >
          Tentar de novo
        </button>
      </div>
    );
  }

  return (
    <div
      className="specialist-picker"
      data-disabled={disabled || undefined}
    >
      <label className="specialist-picker__label" htmlFor="specialist-filter">
        {label}
      </label>
      <input
        id="specialist-filter"
        className="specialist-picker__filter"
        type="text"
        placeholder={filterPlaceholder}
        value={capabilityFilter}
        onChange={(e) => setCapabilityFilter(e.target.value)}
        disabled={disabled}
      />
      <ul className="specialist-picker__list" role="listbox">
        {filtered.length === 0 ? (
          <li className="specialist-picker__empty" aria-live="polite">
            Nenhum especialista combina com o filtro.
          </li>
        ) : (
          filtered.map((s) => (
            <li
              key={s.id}
              className={
                "specialist-picker__item" +
                (selected === s.id
                  ? " specialist-picker__item--selected"
                  : "")
              }
              role="option"
              aria-selected={selected === s.id}
            >
              <button
                type="button"
                className="specialist-picker__item-button"
                onClick={() => handleSelect(s)}
                disabled={disabled}
              >
                <div className="specialist-picker__item-header">
                  <span className="specialist-picker__item-name">{s.name}</span>
                  <code className="specialist-picker__item-id">{s.id}</code>
                </div>
                <p className="specialist-picker__item-description">
                  {s.description}
                </p>
                <div className="specialist-picker__item-meta">
                  <span className="specialist-picker__item-model">
                    modelo: <code>{s.default_model}</code>
                  </span>
                  {s.capability_tags.length > 0 ? (
                    <span className="specialist-picker__item-tags">
                      {s.capability_tags.map((t) => (
                        <span
                          key={t}
                          className="specialist-picker__item-tag"
                        >
                          {t}
                        </span>
                      ))}
                    </span>
                  ) : (
                    <span
                      className="specialist-picker__item-tag specialist-picker__item-tag--warning"
                      title="default_model não resolvido no catálogo — capability_tags vazia"
                    >
                      modelo não resolvido
                    </span>
                  )}
                </div>
              </button>
            </li>
          ))
        )}
      </ul>
    </div>
  );
}
