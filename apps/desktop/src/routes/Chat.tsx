import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { SessionSidebar } from "../components/SessionSidebar";
import { ModelSelector } from "../components/ModelSelector";
import {
  LiveExecutionPanel,
  tempoRelativo,
  type FaseLive,
  type LinhaLive,
} from "../components/LiveExecutionPanel";
import {
  TaskProgressCard,
  type Etapa,
} from "../components/TaskProgressCard";
import { saudacao } from "../saudacao";
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

const SUGESTOES_INICIAIS = [
  "Analisar planilhas e gerar um relatório",
  "Executar um script Python no sandbox",
  "Gerar um documento Word com dados",
] as const;

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
  onLiveEvent: (event: StreamEvent, ocorridoEm: number) => void,
): Promise<() => void> {
  if (!message.run_id) {
    // Mensagem sem run_id — não há nada a fazer.
    return () => {};
  }
  const events = await getRunEvents(message.id, 0);
  // Replay do journal.
  for (const ev of events) {
    const liveEvent = streamEventFromJournal(ev);
    if (liveEvent) {
      const ocorridoEm = Date.parse(ev.created_at);
      onLiveEvent(liveEvent, Number.isFinite(ocorridoEm) ? ocorridoEm : Date.now());
    }
    applyEvent(ev, onDelta, onError, onStatus);
  }
  // Se o run já terminou, não precisa subscrever.
  const terminal = ["completed", "failed", "cancelled", "timeout"];
  if (terminal.includes(message.status)) {
    return () => {};
  }
  // Senão, subscreve.
  const sub = await subscribeRun(message.run_id, (envelope) => {
    // PR do bug do stream (Etapa 5.X): o payload é um
    // `StreamEventEnvelope { seq, event }` — o backend agora
    // carrega o `seq` do journal pra que a reconexão seja exata
    // (sem perder nem duplicar). A camada de apresentação
    // continua consumindo o `event` puro; o `seq` é relevante
    // só pra reconexão via `RunGetEvents { since_seq }`, que
    // hoje é feita com `since_seq: 0` no mount (suficiente
    // porque o reload de janela é raro). A Etapa futura
    // "reconexão por `fromSeq`" usa o `lastSeq()` retornado
    // pelo `RunStreamSubscription`.
    onLiveEvent(envelope.event, Date.now());
    onEventToCallbacks(envelope.event, onDelta, onError);
  }, onStatus);
  return sub.unlisten;
}

/** Reconstrói o enum do stream a partir da linha persistida no journal. */
function streamEventFromJournal(ev: MessageEventView): StreamEvent | null {
  const data = ev.data as Record<string, unknown> | null;
  if (!data) return null;
  switch (ev.kind) {
    case "delta":
      return typeof data.content === "string"
        ? { kind: "delta", content: data.content }
        : null;
    case "usage":
      return typeof data.prompt_tokens === "number" &&
        typeof data.completion_tokens === "number"
        ? {
            kind: "usage",
            prompt_tokens: data.prompt_tokens,
            completion_tokens: data.completion_tokens,
          }
        : null;
    case "tool_call":
      return typeof data.id === "string" &&
        typeof data.name === "string" &&
        typeof data.arguments_json === "string"
        ? {
            kind: "tool_call",
            id: data.id,
            name: data.name,
            arguments_json: data.arguments_json,
          }
        : null;
    case "tool_result":
      return typeof data.id === "string" && typeof data.ok === "boolean"
        ? {
            kind: "tool_result",
            id: data.id,
            ok: data.ok,
            output: data.output,
          }
        : null;
    case "done":
      return typeof data.stop_reason === "string"
        ? {
            kind: "done",
            stop_reason: data.stop_reason as
              | "stop"
              | "length"
              | "tool_calls"
              | "error",
          }
        : null;
    case "error":
      return {
        kind: "error",
        message: {
          code: typeof data.kind === "string" ? data.kind : "unknown",
          title: "Erro do provedor",
          detail:
            typeof data.upstream_message === "string"
              ? data.upstream_message
              : "",
          action: null,
          retry_after_secs: null,
        },
      };
    case "cancelled":
      return { kind: "cancelled" };
    default:
      return null;
  }
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

  // --- Execução ao vivo ---------------------------------------------
  //
  // Tudo aqui é derivado de evento do journal. Nenhuma linha do
  // console e nenhuma etapa existe sem um evento que a tenha
  // originado — é a diferença entre mostrar execução e encenar
  // execução.
  const [liveLinhas, setLiveLinhas] = useState<LinhaLive[]>([]);
  const [etapas, setEtapas] = useState<Etapa[]>([]);
  const [fase, setFase] = useState<FaseLive>("ocioso");
  const [liveAberto, setLiveAberto] = useState(false);
  // Início do run, para os timestamps `mm:ss` do console serem
  // relativos ao run e não ao relógio de parede.
  const inicioRef = useRef<number>(Date.now());

  // Uma sugestão escolhida na tela vazia atravessa a criação da
  // conversa e chega preenchida ao composer. A chave leva o id da
  // conversa para nunca vazar o rascunho para outra sessão.
  useEffect(() => {
    if (!params.id) return;
    const key = `frederico:rascunho:${params.id}`;
    const pending = window.sessionStorage.getItem(key);
    if (!pending) return;
    setDraft(pending);
    window.sessionStorage.removeItem(key);
  }, [params.id]);

  const anotar = useCallback(
    (tipo: LinhaLive["tipo"], texto: string, ocorridoEm = Date.now()) => {
      setLiveLinhas((ls) => [
        ...ls,
        { tempo: tempoRelativo(inicioRef.current, ocorridoEm), tipo, texto },
      ]);
    },
    [],
  );

  /**
   * Traduz um `StreamEvent` do journal em linha de console e/ou
   * mudança de etapa.
   *
   * O `delta` **não** vira linha: ele é o texto da resposta, que já
   * aparece na conversa. Repeti-lo no console encheria o log de
   * prosa e esconderia o que o console existe para mostrar —
   * ferramenta, erro, encerramento.
   */
  const registrarEvento = useCallback(
    (ev: StreamEvent, ocorridoEm = Date.now()) => {
      switch (ev.kind) {
        case "tool_call":
          setEtapas((es) => [
            ...es.map((e) =>
              e.estado === "executando" ? { ...e, estado: "concluida" as const } : e,
            ),
            {
              id: ev.id,
              ferramenta: ev.name,
              argumentos: ev.arguments_json,
              estado: "executando" as const,
            },
          ]);
          anotar("comando", `$ ${ev.name}`, ocorridoEm);
          break;
        case "tool_result": {
          const erro = ev.ok ? undefined : extrairErroDaFerramenta(ev.output);
          const saida = resumirOutput(ev.output);
          setEtapas((es) =>
            es.map((e) =>
              e.id === ev.id
                ? {
                    ...e,
                    estado: ev.ok ? ("concluida" as const) : ("falhou" as const),
                    erro,
                    saida,
                  }
                : e,
            ),
          );
          anotar(
            ev.ok ? "ok" : "erro",
            `${ev.ok ? "resultado" : "falha"} · ${saida}`,
            ocorridoEm,
          );
          if (!ev.ok) setFase("falhou");
          break;
        }
        case "usage":
          anotar(
            "saida",
            `tokens · ${ev.prompt_tokens}↑ ${ev.completion_tokens}↓`,
            ocorridoEm,
          );
          break;
        case "error":
          setEtapas((es) =>
            es.map((e) =>
              e.estado === "executando"
                ? { ...e, estado: "falhou" as const, erro: ev.message.detail ?? undefined }
                : e,
            ),
          );
          anotar("erro", ev.message.title || "erro do provedor", ocorridoEm);
          setFase("falhou");
          break;
        case "cancelled":
          setEtapas((es) =>
            es.map((e) =>
              e.estado === "executando" ? { ...e, estado: "cancelada" as const } : e,
            ),
          );
          anotar("aviso", "execução cancelada pelo usuário", ocorridoEm);
          setFase("cancelado");
          break;
        case "done":
          setEtapas((es) =>
            es.map((e) =>
              e.estado === "executando" ? { ...e, estado: "concluida" as const } : e,
            ),
          );
          anotar(
            ev.stop_reason === "error" ? "erro" : "ok",
            `encerrado · ${ev.stop_reason}`,
            ocorridoEm,
          );
          setFase(ev.stop_reason === "error" ? "falhou" : "concluido");
          break;
        default:
          break;
      }
    },
    [anotar],
  );

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

  const loadCurrent = useCallback(
    async (id: string) => {
      const data = await getConversation(id);
      setState((s) => ({ ...s, current: data }));

      // Só o run da resposta mais recente alimenta o painel. O journal
      // continua guardando todos; repetir cards mortos em cada resposta
      // antiga só duplicaria informação na tela.
      const message = [...data.messages]
        .reverse()
        .find((m) => m.role === "assistant" && m.run_id);

      for (const unlisten of unlistensRef.current.values()) unlisten();
      unlistensRef.current.clear();
      setLiveLinhas([]);
      setEtapas([]);

      if (!message?.run_id) {
        setStreamingMessageId(null);
        setFase("ocioso");
        return;
      }

      const inicio = Date.parse(message.created_at);
      inicioRef.current = Number.isFinite(inicio) ? inicio : Date.now();
      accumRef.current.delete(message.id);
      setFase(faseDaMensagem(message.status));

      const runId = message.run_id;
      const unsub = await reloadStreamingMessage(
        message,
        (text) => {
          accumRef.current.set(
            message.id,
            (accumRef.current.get(message.id) ?? "") + text,
          );
          setState((s) => ({ ...s }));
        },
        (err) => {
          errorsRef.current.set(message.id, err);
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
            setFase(
              status.status === "completed"
                ? "concluido"
                : status.status === "cancelled"
                  ? "cancelado"
                  : "falhou",
            );
            // Atualiza conteúdo, custo e status sem reiniciar o replay
            // visual que acabou de chegar.
            void getConversation(id).then((finalData) => {
              setState((s) => ({ ...s, current: finalData }));
            });
            void refreshConversations();
          }
        },
        registrarEvento,
      );

      if (message.status === "streaming") {
        unlistensRef.current.set(runId, unsub);
        setStreamingMessageId(message.id);
      } else {
        setStreamingMessageId(null);
      }
    },
    [refreshConversations, registrarEvento],
  );

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

  async function handleNewConversation(
    provider: string,
    model: string,
    initialDraft?: string,
  ) {
    const c = await createConversation(provider, model, null);
    if (initialDraft) {
      window.sessionStorage.setItem(
        `frederico:rascunho:${c.id}`,
        initialDraft,
      );
    }
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
    // Cada envio começa um console novo. Acumular entre runs
    // misturaria a saída de duas execuções sem separador, e o
    // timestamp `mm:ss` do run anterior passaria a mentir.
    inicioRef.current = Date.now();
    setLiveLinhas([]);
    setEtapas([]);
    setFase("executando");
    setLiveAberto(true);
    try {
      await sendMessage(state.current.conversation.id, content);
      // Recarrega a conversa (traz o user message e o assistant
      // message com `streaming`) e instala uma única assinatura. A
      // versão anterior assinava aqui e também dentro de `loadCurrent`,
      // duplicando cada linha do console.
      await loadCurrent(state.current.conversation.id);
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

  if (state.loading) return <p className="carregando">Carregando…</p>;

  const conv = state.current?.conversation ?? null;
  const executando = streamingMessageId !== null;

  return (
    <div className="studio">
      <SessionSidebar
        conversas={state.conversations}
        atual={params.id}
        formatCost={formatCost}
        onSelecionar={handleSelect}
        onApagar={handleDelete}
        onNova={() => navigate("/chat")}
      />

      <main className="studio-centro">
        {state.error && (
          <div className="error" role="alert">
            <strong>Erro:</strong> {state.error}
          </div>
        )}

        {conv === null ? (
          <TelaVazia
            catalog={state.catalog}
            temConversas={state.conversations.length > 0}
            onCriar={handleNewConversation}
          />
        ) : (
          <>
            <ul className="mensagens">
              {state.current!.messages.map((m) => {
                const streamed = accumRef.current.get(m.id) ?? "";
                const display =
                  m.role === "user" ? m.content : streamed || m.content;
                const err = errorsRef.current.get(m.id);
                const ultimaResposta = [...state.current!.messages]
                  .reverse()
                  .find((x) => x.role === "assistant");
                const ehUltimaResposta =
                  m.role === "assistant" && m.id === ultimaResposta?.id;
                return (
                  <li key={m.id} className={`msg msg-${m.role}`}>
                    {m.role === "user" ? (
                      <div className="bolha-usuario">{display}</div>
                    ) : (
                      <div className="resposta">
                        <div className="resposta-meta">
                          <span className="avatar" aria-hidden="true">
                            F
                          </span>
                          <strong>Assistente</strong>
                          <span className="badge-modelo">{conv.model_id}</span>
                          {(m.prompt_tokens !== null ||
                            m.completion_tokens !== null) && (
                            <span className="resposta-numeros" data-numerico>
                              {m.prompt_tokens ?? 0}↑ {m.completion_tokens ?? 0}↓
                              {" · "}
                              {formatCost(m.cost_microcents)}
                            </span>
                          )}
                        </div>

                        {/* O card de progresso acompanha a última
                            resposta: é a que está (ou acabou de
                            estar) em execução. Repeti-lo em toda
                            resposta antiga encheria a conversa de
                            cards mortos. */}
                        {ehUltimaResposta &&
                          (etapas.length > 0 || fase !== "ocioso") && (
                            <TaskProgressCard
                              etapas={etapas}
                              fase={fase}
                              liveAberto={liveAberto}
                              onAlternarLive={() => setLiveAberto((v) => !v)}
                            />
                          )}

                        <div className="resposta-corpo">
                          {display || (
                            <span className="resposta-vazia">
                              {m.status === "streaming"
                                ? "aguardando o provedor…"
                                : "sem conteúdo"}
                            </span>
                          )}
                        </div>
                        {err && <ErrorView err={err} />}
                      </div>
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
                placeholder="Descreva a tarefa…"
                aria-label="Mensagem"
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    handleSend();
                  }
                }}
                rows={2}
              />
              <div className="composer-acoes">
                <ModelSelector
                  catalogo={state.catalog}
                  providerAtual={conv.provider_id}
                  modelAtual={conv.model_id}
                  onEscolher={handleSetModel}
                />
                <span className="composer-dica">
                  Enter envia · Shift+Enter quebra linha
                </span>
                <span className="composer-espacador" />
                {/* **Um botão, não dois.** Executar e Cancelar são
                    o mesmo lugar na tela porque são a mesma decisão
                    em momentos opostos; dois botões obrigariam a
                    mirar de novo no meio de um run. */}
                {executando ? (
                  <button
                    type="button"
                    className="btn-cancelar"
                    onClick={handleStop}
                  >
                    ■ Cancelar
                  </button>
                ) : (
                  <button
                    type="submit"
                    className="btn-executar"
                    disabled={sending || !draft.trim()}
                  >
                    {sending ? "Enviando…" : "Executar →"}
                  </button>
                )}
              </div>
            </form>
          </>
        )}
      </main>

      {liveAberto && (
        <LiveExecutionPanel
          linhas={liveLinhas}
          fase={fase}
          operacao={
            etapas.find((e) => e.estado === "executando")?.ferramenta ?? null
          }
          concluidas={etapas.filter((e) => e.estado === "concluida").length}
          total={etapas.length}
          onFechar={() => setLiveAberto(false)}
          onCancelar={executando ? handleStop : null}
        />
      )}
    </div>
  );
}

/**
 * Tela vazia: saudação por horário + criação da primeira sessão.
 *
 * A saudação usa a hora local de quem abriu. `saudacao()` é pura e
 * recebe a hora — a leitura do relógio fica aqui, na borda.
 */
function TelaVazia(props: {
  catalog: ModelDescriptorView[];
  temConversas: boolean;
  onCriar: (provider: string, model: string, initialDraft?: string) => void;
}) {
  const { titulo, sublinha } = saudacao(new Date().getHours());
  return (
    <section className="tela-vazia">
      <span className="marca-grande" aria-hidden="true">
        F
      </span>
      <h2>{titulo}</h2>
      <p className="tela-vazia-sub">{sublinha}</p>
      {props.catalog.length === 0 ? (
        <p className="tela-vazia-aviso">
          Nenhum modelo disponível. Configure uma chave em{" "}
          <Link to="/settings">Configurações</Link>.
        </p>
      ) : (
        <NewConversationForm catalog={props.catalog} onCreate={props.onCriar} />
      )}
      {props.temConversas && (
        <p className="tela-vazia-sub">
          Ou escolha uma sessão na coluna da esquerda.
        </p>
      )}
    </section>
  );
}

function NewConversationForm(props: {
  catalog: ModelDescriptorView[];
  onCreate: (provider: string, model: string, initialDraft?: string) => void;
}) {
  const primeiro = props.catalog[0];
  const [provider, setProvider] = useState(primeiro?.provider ?? "");
  const [model, setModel] = useState(primeiro?.model ?? "");
  const models = props.catalog.filter((m) => m.provider === provider);
  return (
    <form
      className="nova-conversa"
      onSubmit={(e) => {
        e.preventDefault();
        props.onCreate(provider, model);
      }}
    >
      <div className="nova-conversa-campos">
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
      </label>
      <label>
        Modelo:{" "}
        <select value={model} onChange={(e) => setModel(e.target.value)}>
          {models.map((m) => (
            <option key={m.model} value={m.model}>
              {m.display_name}
            </option>
          ))}
        </select>
      </label>
      <button type="submit">Criar</button>
      </div>
      <div className="sugestoes-iniciais" aria-label="Sugestões de tarefa">
        {SUGESTOES_INICIAIS.map((sugestao) => (
          <button
            key={sugestao}
            type="button"
            onClick={() => props.onCreate(provider, model, sugestao)}
          >
            {sugestao}
          </button>
        ))}
      </div>
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

function faseDaMensagem(status: MessageView["status"]): FaseLive {
  switch (status) {
    case "completed":
      return "concluido";
    case "failed":
    case "timeout":
      return "falhou";
    case "cancelled":
      return "cancelado";
    case "pending":
    case "streaming":
      return "executando";
    default:
      return "ocioso";
  }
}

function resumirOutput(output: unknown): string {
  if (output === null || output === undefined) return "sem saída";
  const texto =
    typeof output === "string"
      ? output
      : (() => {
          try {
            return JSON.stringify(output);
          } catch {
            return String(output);
          }
        })();
  const compacto = texto.replace(/\s+/g, " ").trim();
  return compacto.length > 240 ? `${compacto.slice(0, 237)}…` : compacto;
}

function extrairErroDaFerramenta(output: unknown): string {
  if (output && typeof output === "object") {
    const data = output as Record<string, unknown>;
    if (typeof data.error_message === "string") return data.error_message;
    if (typeof data.message === "string") return data.message;
  }
  return resumirOutput(output);
}

function formatCost(microcents: number): string {
  if (microcents === 0) return "—";
  // microcents → cents (÷ 1e6), format 4 casas.
  const cents = microcents / 1_000_000;
  return `R$ ${cents.toFixed(4)}`;
}
