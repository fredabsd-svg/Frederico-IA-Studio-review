import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  deleteCredential,
  listCatalog,
  listProviders,
  setCredential,
  type ModelDescriptorView,
  type ProviderConfigView,
} from "../services";

interface State {
  loading: boolean;
  error: string | null;
  providers: ProviderConfigView[];
  catalog: ModelDescriptorView[];
}

const initial: State = {
  loading: true,
  error: null,
  providers: [],
  catalog: [],
};

export function Settings() {
  const [state, setState] = useState<State>(initial);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const refresh = useCallback(async () => {
    const [providers, catalog] = await Promise.all([
      listProviders(),
      listCatalog(),
    ]);
    setState((s) => ({ ...s, providers, catalog }));
  }, []);

  useEffect(() => {
    (async () => {
      try {
        await refresh();
        setState((s) => ({ ...s, loading: false }));
      } catch (e) {
        setState((s) => ({
          ...s,
          loading: false,
          error: e instanceof Error ? e.message : String(e),
        }));
      }
    })();
  }, [refresh]);

  if (state.loading) return <p>Carregando…</p>;

  // Constrói a lista de provedores conhecidos a partir do catálogo
  // (fonte da verdade) + status do storage.
  const knownProviders = Array.from(
    new Set(state.catalog.map((m) => m.provider)),
  );
  const byProvider = new Map<string, ProviderConfigView>();
  for (const p of state.providers) byProvider.set(p.provider, p);

  return (
    <section>
      <h2>Configurações de provedores</h2>
      <p>
        As credenciais são guardadas no Windows Credential Manager (DPAPI),
        vinculadas ao seu usuário Windows. O caminho da chave nunca é
        improvisado (ver <code>docs/decisions/0007-credential-store-trait.md</code>).
      </p>
      {state.error && (
        <div className="error">
          <strong>Erro:</strong> {state.error}
        </div>
      )}
      <table>
        <thead>
          <tr>
            <th>Provedor</th>
            <th>Status</th>
            <th>Último ok</th>
            <th>Último erro</th>
            <th>Ação</th>
          </tr>
        </thead>
        <tbody>
          {knownProviders.map((p) => {
            const cfg = byProvider.get(p);
            const configured = cfg?.configured ?? false;
            return (
              <tr key={p}>
                <td>
                  <strong>{p}</strong>
                </td>
                <td>
                  {configured ? (
                    <span className="status-ok">configurado</span>
                  ) : (
                    <span className="status-pending">não configurado</span>
                  )}
                </td>
                <td>
                  <small>{cfg?.last_ok_at || "—"}</small>
                </td>
                <td>
                  <small>{cfg?.last_error || "—"}</small>
                </td>
                <td>
                  {editing === p ? (
                    <>
                      <input
                        type="password"
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        placeholder="sk-…"
                      />
                      <button
                        onClick={async () => {
                          try {
                            await setCredential(p, draft);
                            setEditing(null);
                            setDraft("");
                            await refresh();
                          } catch (e) {
                            setState((s) => ({
                              ...s,
                              error:
                                e instanceof Error ? e.message : String(e),
                            }));
                          }
                        }}
                      >
                        Salvar
                      </button>
                      <button onClick={() => setEditing(null)}>Cancelar</button>
                    </>
                  ) : configured ? (
                    <>
                      <button
                        onClick={() => {
                          setEditing(p);
                          setDraft("");
                        }}
                      >
                        Trocar
                      </button>
                      <button
                        className="link-danger"
                        onClick={async () => {
                          await deleteCredential(p);
                          await refresh();
                        }}
                      >
                        Remover
                      </button>
                    </>
                  ) : (
                    <button
                      onClick={() => {
                        setEditing(p);
                        setDraft("");
                      }}
                    >
                      Configurar
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <p>
        <Link to="/chat">← Voltar ao chat</Link>
      </p>
    </section>
  );
}
