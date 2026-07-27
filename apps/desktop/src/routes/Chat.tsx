import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  cancelRun,
  createConversation,
  deleteConversation,
  getConversation,
  getRunEvents,
  listCatalog,
  listConversations,
  sendMessage,
  setConversationModel,
  subscribeRun,
  type ConversationView,
  type MessageEventView,
  type MessageView,
  type ModelDescriptorView,
  type ProviderErrorView,
  type RunStatusEvent,
  type StreamEvent,
} from "../services";

interface State {
  loading: boolean;
  error: string | null;
  conversations: ConversationView[];
  current: {
    conversation: ConversationView;
    messages: MessageView[];
  } | null;
  catalog: ModelDescriptorView[];
}

const initial: State = {
  loading: true,
  error: null,
  conversations: [],
  current: null,
  catalog: [],
};

/**
 * Recarrega janela no meio do stream: para cada mensagem `streaming`,
 * busca `RunGetEvents` com `since_seq: 0` e reconstrói o conteúdo a
 * partir do journal. Depois subscreve via `subscribeRun` pra
 * continuar recebendo deltas.
 */
async function reloadStreamingMessage(
  message: MessageView,
  onDelta: (text: string) => void,
  onError: (err: ProviderErrorView) => void,
  onStatus: (status: RunStatusEvent) => void,
): Promise<() => void> {
  if (!message.run_id) {
    // Mensagem sem run_id — não há nada a fazer.
    return () => {};
  }
  const events = await getRunEvents(message.id, 0);
  // Replay do journal.
  for (const ev of events) {
    applyEvent(ev, onDelta, onError, onStatus);
  }
  // Se o run já terminou, não precisa subscrever.
  const terminal = ["completed", "failed", "cancelled", "timeout"];
  if (terminal.includes(message.status)) {
    return () => {};
  }
  // Senão, subscreve.
  const sub = await subscribeRun(message.run_id, (ev) => {
    // Tauri events carregam apenas o payload, sem o envelope. A
    // casca emite o `StreamEvent` serializado direto. Mapeamos:
    onEventToCallbacks(ev, onDelta, onError);
  }, onStatus);
  return sub.unlisten;
}

function applyEvent(
  ev: MessageEventView,
  onDelta: (text: string) => void,
  onError: (err: ProviderErrorView) => void,
  onStatus: (status: RunStatusEvent) => void,
): void {
  const data = ev.data as Record<string, unknown> | null;
  if (!data) return;
  if (ev.kind === "delta" && typeof data.content === "string") {
    onDelta(data.content);
  } else if (ev.kind === "error" && data) {
    onError({
      code: (data.kind as string) ?? "unknown",
      title: "Erro do provedor",
      detail: (data.upstream_message as string) ?? "",
      action: null,
      retry_after_secs: null,
    });
  } else if (ev.kind === "done") {
    onStatus({ status: "completed" });
  } else if (ev.kind === "cancelled") {
    onStatus({ status: "cancelled" });
  }
}

function onEventToCallbacks(
  ev: StreamEvent,
  onDelta: (text: string) => void,
  onError: (err: ProviderErrorView) => void,
): void {
  if (ev.kind === "delta") {
    onDelta(ev.content);
  } else if (ev.kind === "error") {
    onError(ev.message);
  }
}

export function Chat() {
  const params = useParams<{ id?: string }>();
  const navigate = useNavigate();
  const [state, setState] = useState<State>(initial);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [streamingMessageId, setStreamingMessageId] = useState<string | null>(
    null,
  );
  // Acumulador de deltas por mensagem (render otimista).
  const accumRef = useRef<Map<string, string>>(new Map());
  // Erros por mensagem.
  const errorsRef = useRef<Map<string, ProviderErrorView>>(new Map());
  // Unlisten handlers ativos.
  const unlistensRef = useRef<Map<string, () => void>>(new Map());

  const refreshCatalog = useCallback(async () => {
    try {
      const c = await listCatalog();
      setState((s) => ({ ...s, catalog: c }));
    } catch {
      // sem catálogo — mantém o anterior.
    }
  }, []);

  const refreshConversations = useCallback(async () => {
    const list = await listConversations();
    setState((s) => ({ ...s, conversations: list }));
  }, []);

  const loadCurrent = useCallback(async (id: string) => {
    const data = await getConversation(id);
    setState((s) => ({ ...s, current: data }));
    // Recarrega streams ativos.
    for (const m of data.messages) {
      if (m.status === "streaming" && m.run_id) {
        const runId = m.run_id;
        const unsub = await reloadStreamingMessage(
          m,
          (text) => {
            accumRef.current.set(m.id, (accumRef.current.get(m.id) ?? "") + text);
            setState((s) => ({ ...s })); // força re-render
          },
          (err) => {
            errorsRef.current.set(m.id, err);
            setState((s) => ({ ...s }));
          },
          (status) => {
            if (
              status.status === "completed" ||
              status.status === "failed" ||
              status.status === "cancelled" ||
              status.status === "timeout"
            ) {
              unlistensRef.current.get(runId)?.();
              unlistensRef.current.delete(runId);
              setStreamingMessageId(null);
            }
          },
        );
        unlistensRef.current.set(runId, unsub);
        setStreamingMessageId(m.id);
      }
    }
  }, []);

  // Carga inicial: lista de conversas + catálogo.
  useEffect(() => {
    (async () => {
      try {
        await refreshCatalog();
        await refreshConversations();
        if (params.id) {
          await loadCurrent(params.id);
        }
        setState((s) => ({ ...s, loading: false }));
      } catch (e) {
        setState((s) => ({
          ...s,
          loading: false,
          error: e instanceof Error ? e.message : String(e),
        }));
      }
    })();
    return () => {
      // Cleanup: cancela todas as subscrições.
      for (const u of unlistensRef.current.values()) u();
      unlistensRef.current.clear();
    };
  }, [params.id, refreshCatalog, refreshConversations, loadCurrent]);

  async function handleNewConversation(provider: string, model: string) {
    const c = await createConversation(provider, model, null);
    await refreshConversations();
    navigate(`/chat/${c.id}`);
  }

  async function handleSelect(id: string) {
    navigate(`/chat/${id}`);
  }

  async function handleDelete(id: string) {
    await deleteConversation(id);
    if (params.id === id) navigate("/chat");
    await refreshConversations();
  }

  async function handleSetModel(provider: string, model: string) {
    if (!state.current) return;
    await setConversationModel(state.current.conversation.id, provider, model);
    await loadCurrent(state.current.conversation.id);
    await refreshConversations();
  }

  async function handleSend() {
    if (!state.current || !draft.trim() || sending) return;
    setSending(true);
    const content = draft.trim();
    setDraft("");
    try {
      const result = await sendMessage(state.current.conversation.id, content);
      // Recarrega a conversa (traz o user message e o assistant
      // message com `streaming`).
      await loadCurrent(state.current.conversation.id);
      // Subscrição ao run.
      if (result.user_message) {
        const msgs = await getConversation(state.current.conversation.id);
        const asst = msgs.messages.find((m) => m.role === "assistant");
        if (asst && asst.run_id) {
          const runId = asst.run_id;
          setStreamingMessageId(asst.id);
          const sub = await subscribeRun(
            runId,
            (ev) => {
              if (ev.kind === "delta") {
                accumRef.current.set(
                  asst.id,
                  (accumRef.current.get(asst.id) ?? "") + ev.content,
                );
                setState((s) => ({ ...s }));
              } else if (ev.kind === "error") {
                errorsRef.current.set(asst.id, ev.message);
                setState((s) => ({ ...s }));
              }
            },
            (status) => {
              if (
                status.status === "completed" ||
                status.status === "failed" ||
                status.status === "cancelled" ||
                status.status === "timeout"
              ) {
                unlistensRef.current.get(runId)?.();
                unlistensRef.current.delete(runId);
                setStreamingMessageId(null);
                // Re-fetch para pegar status final + usage + cost.
                if (state.current) {
                  loadCurrent(state.current.conversation.id);
                }
              }
            },
          );
          unlistensRef.current.set(runId, sub.unlisten);
        }
      }
    } catch (e) {
      setState((s) => ({
        ...s,
        error: e instanceof Error ? e.message : String(e),
      }));
    } finally {
      setSending(false);
    }
  }

  async function handleStop() {
    const id = streamingMessageId;
    if (!id || !state.current) return;
    const msgs = state.current.messages;
    const target = msgs.find((m) => m.id === id);
    if (!target?.run_id) return;
    try {
      await cancelRun(target.run_id);
    } catch (e) {
      console.error("cancel falhou:", e);
    }
  }

  // === Render ========================================================

  if (state.loading) return <p>Carregando…</p>;

  if (state.conversations.length === 0) {
    return (
      <section>
        <h2>Sem conversas ainda</h2>
        <p>
          Crie uma conversa nova escolhendo provedor e modelo. Configure a
          chave em <Link to="/settings">Configurações</Link> antes (ou use o{" "}
          <code>simulated</code> pra testes).
        </p>
        <NewConversationForm
          catalog={state.catalog}
          onCreate={handleNewConversation}
        />
      </section>
    );
  }

  return (
    <section className="chat">
      <aside className="conversations">
        <header>
          <strong>Conversas</strong>
          <Link to="/settings" className="link-small">
            Configurações
          </Link>
        </header>
        <ul>
          {state.conversations.map((c) => (
            <li
              key={c.id}
              className={c.id === params.id ? "active" : ""}
            >
              <button
                className="link"
                onClick={() => handleSelect(c.id)}
                title={`${c.provider_id}/${c.model_id}`}
              >
                {c.title || `${c.provider_id}/${c.model_id}`}
                <small>{formatCost(c.total_cost_microcents)}</small>
              </button>
              <button
                className="link-danger"
                onClick={() => handleDelete(c.id)}
                title="Apagar conversa"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      </aside>
      <main className="messages-pane">
        {state.current ? (
          <>
            <ConversationHeader
              conv={state.current.conversation}
              catalog={state.catalog}
              onChangeModel={handleSetModel}
            />
            <ul className="messages">
              {state.current.messages.map((m) => {
                const streamed = accumRef.current.get(m.id) ?? "";
                const display = m.role === "user" ? m.content : streamed || m.content;
                const err = errorsRef.current.get(m.id);
                const isStreaming =
                  streamingMessageId === m.id ||
                  (m.role === "assistant" && m.status === "streaming");
                return (
                  <li
                    key={m.id}
                    className={`msg msg-${m.role} msg-${m.status}`}
                  >
                    <div className="msg-meta">
                      <span className="msg-role">
                        {m.role === "user"
                          ? "você"
                          : m.role === "assistant"
                            ? "assistente"
                            : m.role}
                      </span>
                      {m.role === "assistant" && (
                        <span className="msg-status">
                          {m.status === "streaming" ? "digitando…" : m.status}
                        </span>
                      )}
                      {m.role === "assistant" &&
                        (m.prompt_tokens !== null ||
                          m.completion_tokens !== null) && (
                          <small>
                            {m.prompt_tokens ?? 0}↑ {m.completion_tokens ?? 0}↓ ·{" "}
                            {formatCost(m.cost_microcents)}
                          </small>
                        )}
                    </div>
                    <div className="msg-body">{display}</div>
                    {err && <ErrorView err={err} />}
                    {isStreaming && streamingMessageId === m.id && (
                      <button className="stop" onClick={handleStop}>
                        Parar
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
            <form
              className="composer"
              onSubmit={(e) => {
                e.preventDefault();
                handleSend();
              }}
            >
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                placeholder="Escreva uma mensagem… (Shift+Enter para quebrar linha, Enter envia)"
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    handleSend();
                  }
                }}
                rows={3}
                disabled={sending}
              />
              <button type="submit" disabled={sending || !draft.trim()}>
                {sending ? "Enviando…" : "Enviar"}
              </button>
            </form>
          </>
        ) : (
          <p>Selecione uma conversa à esquerda ou crie uma nova.</p>
        )}
        {state.error && (
          <div className="error">
            <strong>Erro:</strong> {state.error}
          </div>
        )}
      </main>
    </section>
  );
}

function ConversationHeader(props: {
  conv: ConversationView;
  catalog: ModelDescriptorView[];
  onChangeModel: (provider: string, model: string) => void;
}) {
  const { conv, catalog, onChangeModel } = props;
  const [editing, setEditing] = useState(false);
  const models = catalog.filter((m) => m.provider === conv.provider_id);
  return (
    <header className="conv-header">
      {editing ? (
        <select
          value={conv.model_id}
          onChange={(e) => {
            onChangeModel(conv.provider_id, e.target.value);
            setEditing(false);
          }}
          onBlur={() => setEditing(false)}
          autoFocus
        >
          {models.length === 0 && <option value={conv.model_id}>{conv.model_id}</option>}
          {models.map((m) => (
            <option key={m.model} value={m.model}>
              {m.display_name} ({m.model})
            </option>
          ))}
        </select>
      ) : (
        <button
          className="link"
          onClick={() => setEditing(true)}
          title="Trocar modelo"
        >
          {conv.provider_id} / {conv.model_id}
        </button>
      )}
      <small>Total: {formatCost(conv.total_cost_microcents)}</small>
    </header>
  );
}

function NewConversationForm(props: {
  catalog: ModelDescriptorView[];
  onCreate: (provider: string, model: string) => void;
}) {
  const [provider, setProvider] = useState("simulated");
  const [model, setModel] = useState("fake-model-v1");
  const models = props.catalog.filter((m) => m.provider === provider);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        props.onCreate(provider, model);
      }}
    >
      <label>
        Provedor:{" "}
        <select
          value={provider}
          onChange={(e) => {
            setProvider(e.target.value);
            const first = props.catalog.find((m) => m.provider === e.target.value);
            if (first) setModel(first.model);
          }}
        >
          {Array.from(new Set(props.catalog.map((m) => m.provider))).map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
      </label>{" "}
      <label>
        Modelo:{" "}
        <select value={model} onChange={(e) => setModel(e.target.value)}>
          {models.map((m) => (
            <option key={m.model} value={m.model}>
              {m.display_name}
            </option>
          ))}
        </select>
      </label>{" "}
      <button type="submit">Criar</button>
    </form>
  );
}

function ErrorView({ err }: { err: ProviderErrorView }) {
  return (
    <div className="msg-error">
      <strong>{err.title || "Erro"}</strong>
      {err.code && <small> ({err.code})</small>}
      {err.detail && <p>{err.detail}</p>}
      {err.action && <p className="action">→ {err.action}</p>}
    </div>
  );
}

function formatCost(microcents: number): string {
  if (microcents === 0) return "—";
  // microcents → cents (÷ 1e6), format 4 casas.
  const cents = microcents / 1_000_000;
  return `R$ ${cents.toFixed(4)}`;
}
