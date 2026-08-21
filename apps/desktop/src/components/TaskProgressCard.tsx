import { useEffect, useState } from "react";
import type { FaseLive } from "./LiveExecutionPanel";

/**
 * Uma etapa da tarefa. **Derivada de `tool_call` do journal** — uma
 * etapa é uma ferramenta que o modelo pediu para executar.
 */
export interface Etapa {
  id: string;
  /** Nome da ferramenta: `files.read`, `exec.python`… */
  ferramenta: string;
  /** Argumentos como vieram, para o detalhe expandido. */
  argumentos: string;
  estado: "executando" | "concluida" | "falhou" | "cancelada";
  erro?: string;
  saida?: string;
}

const ROTULO_FASE: Record<FaseLive, string> = {
  ocioso: "Pronto para executar",
  executando: "Executando",
  concluido: "Concluído",
  falhou: "Falhou",
  cancelado: "Cancelado",
};

const GLIFO: Record<Etapa["estado"], string> = {
  executando: "●",
  concluida: "✓",
  falhou: "✕",
  cancelada: "—",
};

/**
 * Card de progresso da tarefa.
 *
 * **A diferença entre este card e o do protótipo é deliberada.** O
 * protótipo mostra cinco etapas nomeadas ("Ler e analisar as
 * planilhas", "Gerar código de análise"…) porque foi desenhado
 * contra um núcleo que planeja a tarefa em etapas antes de
 * executar. O núcleo de hoje não planeja: ele emite `tool_call`
 * quando o modelo pede uma ferramenta, e nada antes disso.
 *
 * Então o card mostra as ferramentas que **foram de fato pedidas**,
 * na ordem em que foram. Inventar os cinco títulos do protótipo
 * daria uma tela mais bonita e uma afirmação falsa — o app estaria
 * dizendo que planejou etapas que ninguém planejou.
 *
 * Quando o núcleo ganhar planejamento por etapas, os títulos
 * entram aqui sem mudar o resto.
 */
export function TaskProgressCard(props: {
  etapas: Etapa[];
  fase: FaseLive;
  liveAberto: boolean;
  onAlternarLive: () => void;
}) {
  const [abertas, setAbertas] = useState<Set<string>>(new Set());
  const concluidas = props.etapas.filter(
    (e) => e.estado === "concluida",
  ).length;
  const total = props.etapas.length;
  const ativa = props.etapas.find(
    (e) => e.estado === "executando" || e.estado === "falhou",
  );

  useEffect(() => {
    const automaticas = props.etapas
      .filter((e) => e.estado === "executando" || e.estado === "falhou")
      .map((e) => e.id);
    if (automaticas.length === 0) return;
    setAbertas((atuais) => {
      const novas = new Set(atuais);
      let mudou = false;
      for (const id of automaticas) {
        if (!novas.has(id)) {
          novas.add(id);
          mudou = true;
        }
      }
      return mudou ? novas : atuais;
    });
  }, [props.etapas]);

  function alternar(id: string) {
    setAbertas((s) => {
      const novo = new Set(s);
      if (novo.has(id)) novo.delete(id);
      else novo.add(id);
      return novo;
    });
  }

  return (
    <div className="tarefa">
      <header className="tarefa-cabecalho">
        <span className={`tarefa-dot ${props.fase}`} aria-hidden="true" />
        <div className="tarefa-titulos">
          <strong className={`tarefa-estado ${props.fase}`} aria-live="polite">
            {ROTULO_FASE[props.fase]}
          </strong>
          {ativa && (
            <span className="tarefa-operacao" title={ativa.ferramenta}>
              {ativa.estado === "falhou" ? "Atenção em " : "Executando "}
              {ativa.ferramenta}
            </span>
          )}
          {total > 0 && (
            <span className="tarefa-contagem" data-numerico>
              {concluidas} de {total}{" "}
              {total === 1 ? "ferramenta" : "ferramentas"}
            </span>
          )}
        </div>
        <button className="btn-live" onClick={props.onAlternarLive}>
          {props.liveAberto ? "Fechar Live" : "Abrir Live"}
        </button>
      </header>

      {/* A barra só existe quando há etapas. Barra a 0% com uma
          etapa só seria ruído: o estado já está escrito ao lado. */}
      {total > 0 && (
        <div
          className="tarefa-barra"
          role="progressbar"
          aria-valuenow={concluidas}
          aria-valuemin={0}
          aria-valuemax={total}
        >
          <span style={{ width: `${(concluidas / total) * 100}%` }} />
        </div>
      )}

      {props.etapas.map((e) => {
        const aberta = abertas.has(e.id);
        return (
          <div className="etapa" key={e.id}>
            <button
              className="etapa-linha"
              onClick={() => alternar(e.id)}
              aria-expanded={aberta}
            >
              <span className={`etapa-glifo ${e.estado}`} aria-hidden="true">
                {GLIFO[e.estado]}
              </span>
              <span className="etapa-ferramenta">{e.ferramenta}</span>
              {/* Estado nunca só por cor: o glifo e este texto
                  dizem a mesma coisa que a cor diz. */}
              <span className="etapa-estado-texto">
                {e.estado === "executando"
                  ? "em curso"
                  : e.estado === "concluida"
                    ? "concluída"
                    : e.estado === "falhou"
                      ? "falhou"
                      : "cancelada"}
              </span>
              <span className="etapa-seta" aria-hidden="true">
                {aberta ? "▴" : "▾"}
              </span>
            </button>
            {aberta && (
              <div className="etapa-detalhe">
                {e.erro && <p className="etapa-erro">{e.erro}</p>}
                <pre>{e.argumentos || "(sem argumentos)"}</pre>
                {e.saida && <pre className="etapa-saida">{e.saida}</pre>}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
