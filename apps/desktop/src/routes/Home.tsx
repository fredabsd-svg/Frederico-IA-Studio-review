import { useEffect, useState } from "react";
import { getAppInfo, ping } from "../services";

interface State {
  loading: boolean;
  error: string | null;
  info: { version: string; started_at: string; last_seen_at: string } | null;
  pong: boolean | null;
}

export function Home() {
  const [state, setState] = useState<State>({
    loading: true,
    error: null,
    info: null,
    pong: null,
  });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const info = await getAppInfo();
        const pong = await ping();
        if (!cancelled) {
          setState({ loading: false, error: null, info, pong });
        }
      } catch (e) {
        if (!cancelled) {
          setState({
            loading: false,
            error: e instanceof Error ? e.message : String(e),
            info: null,
            pong: null,
          });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (state.loading) return <p>Carregando…</p>;
  if (state.error)
    return (
      <section>
        <h2>Erro</h2>
        <pre>{state.error}</pre>
      </section>
    );

  return (
    <section>
      <h2>Estado do app</h2>
      <dl>
        <dt>Versão</dt>
        <dd>{state.info?.version}</dd>
        <dt>Iniciado em</dt>
        <dd>{state.info?.started_at}</dd>
        <dt>Última abertura</dt>
        <dd>{state.info?.last_seen_at}</dd>
        <dt>Ping no núcleo</dt>
        <dd>{state.pong ? "ok" : "falhou"}</dd>
      </dl>
      <p>
        Esta tela é o <em>vertical fino</em> do produto: a UI conversa com a
        casca, a casca despacha via IPC para o núcleo Rust, o núcleo abre o
        SQLite, roda a migração inicial e devolve o estado persistido.
      </p>
    </section>
  );
}
