import { useEffect, useRef } from "react";

/** Uma linha do console. Sempre derivada de um evento do journal. */
export interface LinhaLive {
  /** `mm:ss` desde o começo do run. */
  tempo: string;
  tipo: "comando" | "saida" | "aviso" | "erro" | "ok";
  texto: string;
}

export type FaseLive =
  | "ocioso"
  | "executando"
  | "concluido"
  | "falhou"
  | "cancelado";

const ROTULO: Record<FaseLive, string> = {
  ocioso: "Pronto",
  executando: "Executando",
  concluido: "Concluído",
  falhou: "Falhou",
  cancelado: "Cancelado",
};

/**
 * Painel de execução ao vivo: o console à direita.
 *
 * **Cada linha daqui é um evento real do journal** (`message_events`
 * — delta, tool_call, usage, error, done, cancelled). Nada é
 * inventado nem interpolado para "parecer" execução; quando o
 * núcleo não emite nada, o console fica vazio e diz isso.
 *
 * **Autoscroll que não rouba o scroll.** Só desce sozinho se o
 * usuário já estava no fundo. Quem rolou para cima para ler uma
 * linha antiga continua onde estava — o comportamento oposto é o
 * que torna log em streaming impossível de ler.
 */
export function LiveExecutionPanel(props: {
  linhas: LinhaLive[];
  fase: FaseLive;
  operacao: string | null;
  onFechar: () => void;
  onCancelar: (() => void) | null;
}) {
  const consoleRef = useRef<HTMLDivElement>(null);
  const noFundoRef = useRef(true);

  useEffect(() => {
    const el = consoleRef.current;
    if (!el || !noFundoRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [props.linhas]);

  function aoRolar() {
    const el = consoleRef.current;
    if (!el) return;
    // 24px de tolerância: "no fundo" não pode exigir precisão de
    // pixel, senão um scroll suave deixa o usuário preso fora.
    noFundoRef.current =
      el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  }

  return (
    <aside className="live" aria-label="Execução ao vivo">
      <header className="live-cabecalho">
        <span className={`live-dot ${props.fase}`} aria-hidden="true" />
        <div className="live-titulos">
          <strong aria-live="polite">Execução ao vivo — {ROTULO[props.fase]}</strong>
          {props.operacao && (
            <span className="live-operacao" title={props.operacao}>
              {props.operacao}
            </span>
          )}
        </div>
        <button
          className="btn-icone"
          onClick={props.onFechar}
          aria-label="Fechar o painel de execução"
          title="Fechar"
        >
          ×
        </button>
      </header>

      <div className="live-barra">
        <span className="live-rotulo">Console</span>
        {props.onCancelar && (
          <button className="btn-ghost perigo" onClick={props.onCancelar}>
            Cancelar
          </button>
        )}
        <button
          className="btn-ghost"
          onClick={() => {
            const texto = props.linhas
              .map((l) => `${l.tempo} ${l.texto}`)
              .join("\n");
            void navigator.clipboard?.writeText(texto);
          }}
        >
          Copiar
        </button>
      </div>

      <div className="live-console" ref={consoleRef} onScroll={aoRolar}>
        {props.linhas.length === 0 ? (
          <p className="live-vazio">
            Sem eventos ainda. O que o núcleo emitir aparece aqui.
          </p>
        ) : (
          props.linhas.map((l, i) => (
            <div className={`live-linha ${l.tipo}`} key={i}>
              <span className="live-tempo" data-numerico>
                {l.tempo}
              </span>
              <span className="live-texto">{l.texto}</span>
            </div>
          ))
        )}
        {props.fase === "executando" && (
          <span className="live-cursor" aria-hidden="true">
            ▍
          </span>
        )}
      </div>
    </aside>
  );
}

/** `mm:ss` desde o início do run. */
export function tempoRelativo(inicio: number, agora: number): string {
  const s = Math.max(0, Math.floor((agora - inicio) / 1000));
  const mm = String(Math.floor(s / 60)).padStart(2, "0");
  const ss = String(s % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}
