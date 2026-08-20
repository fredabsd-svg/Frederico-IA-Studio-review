import { useEffect, useMemo, useRef, useState } from "react";
import type { ModelDescriptorView } from "../services";

/**
 * Seletor de modelos em popover, no lugar do `<select>` nativo.
 *
 * **O que veio do `<select>` antigo e não pode se perder:**
 *
 * 1. Agrupamento por provedor — o mesmo `model` aparece em
 *    provedores diferentes (o OpenRouter reexpõe modelos da OpenAI
 *    e da Anthropic com o mesmo nome).
 * 2. O caso "fora do catálogo": o modelo gravado na conversa pode
 *    não existir mais na lista (o catálogo muda entre versões, e
 *    agora também entre aberturas — ADR-0052). Mostrar outro
 *    modelo como se fosse o dela seria mentira silenciosa.
 *
 * **O que o protótipo pede e não foi construído:** a linha
 * "Seleção automática" com estratégia multi-modelo por etapa, os
 * favoritos e os chips de filtro por capacidade. Nenhum dos três
 * tem dado por trás — não há roteador multi-modelo por etapa, nem
 * armazenamento de favorito, e capacidade por modelo existe no
 * descritor mas não como filtro. Entram quando houver o que ligar.
 */
export function ModelSelector(props: {
  catalogo: ModelDescriptorView[];
  providerAtual: string;
  modelAtual: string;
  onEscolher: (provider: string, model: string) => void;
}) {
  const [aberto, setAberto] = useState(false);
  const [busca, setBusca] = useState("");
  const caixaRef = useRef<HTMLDivElement>(null);
  const buscaRef = useRef<HTMLInputElement>(null);

  // Escape e clique-fora fecham. O `<select>` nativo dava os dois
  // de graça; o popover precisa devolvê-los à mão, senão ele fica
  // preso aberto e engole o teclado.
  useEffect(() => {
    if (!aberto) return;
    function onTecla(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.stopPropagation();
        setAberto(false);
      }
    }
    function onClique(e: MouseEvent) {
      if (!caixaRef.current?.contains(e.target as Node)) setAberto(false);
    }
    document.addEventListener("keydown", onTecla);
    document.addEventListener("mousedown", onClique);
    buscaRef.current?.focus();
    return () => {
      document.removeEventListener("keydown", onTecla);
      document.removeEventListener("mousedown", onClique);
    };
  }, [aberto]);

  const atual = props.catalogo.find(
    (m) => m.provider === props.providerAtual && m.model === props.modelAtual,
  );

  const porProvedor = useMemo(() => {
    const termo = busca.trim().toLowerCase();
    const mapa = new Map<string, ModelDescriptorView[]>();
    for (const m of props.catalogo) {
      if (
        termo &&
        !m.display_name.toLowerCase().includes(termo) &&
        !m.model.toLowerCase().includes(termo) &&
        !m.provider.toLowerCase().includes(termo)
      ) {
        continue;
      }
      const lista = mapa.get(m.provider);
      if (lista) lista.push(m);
      else mapa.set(m.provider, [m]);
    }
    return mapa;
  }, [props.catalogo, busca]);

  const nenhumResultado = porProvedor.size === 0;

  return (
    <div className="seletor-modelo" ref={caixaRef}>
      <button
        type="button"
        className="btn-modelo"
        onClick={() => setAberto((v) => !v)}
        aria-haspopup="dialog"
        aria-expanded={aberto}
      >
        <span className="btn-modelo-dot" aria-hidden="true" />
        {atual ? atual.display_name : `${props.modelAtual} (fora do catálogo)`}
        <span className="btn-modelo-seta" aria-hidden="true">
          ▾
        </span>
      </button>

      {aberto && (
        <div className="popover-modelo" role="dialog" aria-label="Escolher modelo">
          <div className="popover-busca">
            <input
              ref={buscaRef}
              type="text"
              value={busca}
              onChange={(e) => setBusca(e.target.value)}
              placeholder="Buscar modelo…"
              aria-label="Buscar modelo"
            />
          </div>

          {/* O modelo da conversa que sumiu do catálogo continua
              visível e escolhido — a lista não pode fingir que ele
              não existe. */}
          {!atual && (
            <div className="popover-fora">
              <strong>
                {props.providerAtual} / {props.modelAtual}
              </strong>
              <span>
                em uso nesta sessão, mas fora do catálogo atual do provedor
              </span>
            </div>
          )}

          <div className="popover-lista">
            {nenhumResultado && (
              <p className="popover-vazio">Nenhum modelo com esse nome.</p>
            )}
            {[...porProvedor.entries()].map(([provider, modelos]) => (
              <section key={provider}>
                <h3 className="popover-provedor">{provider}</h3>
                {modelos.map((m) => {
                  const escolhido =
                    m.provider === props.providerAtual &&
                    m.model === props.modelAtual;
                  return (
                    <button
                      key={`${m.provider}/${m.model}`}
                      type="button"
                      className={escolhido ? "modelo-linha ativa" : "modelo-linha"}
                      onClick={() => {
                        props.onEscolher(m.provider, m.model);
                        setAberto(false);
                      }}
                    >
                      <span className="modelo-nome">{m.display_name}</span>
                      <span className="modelo-id">{m.model}</span>
                      <span className="modelo-custo" data-numerico>
                        {custoRelativo(m)}
                      </span>
                    </button>
                  );
                })}
              </section>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Custo de entrada por milhão de tokens — ou **"não informado"**.
 *
 * Em dólares, que é a moeda em que os provedores publicam. O custo
 * já gasto aparece em reais na linha da resposta, porque ali o
 * valor é real e convertido; aqui é tabela de preço, e converter
 * exigiria uma cotação que o app não busca.
 *
 * Modelo local (Ollama, LM Studio) não tem preço de tabela, e o
 * embutido grava zero. Zero seria lido como "de graça", que é
 * outra afirmação: o custo existe, em energia e hardware, e o app
 * não o conhece. O handoff é explícito sobre isso — nunca inventar
 * custo.
 */
function custoRelativo(m: ModelDescriptorView): string {
  const entrada = m.pricing_input_microcents_per_million;
  if (entrada <= 0) return "não informado";
  // `microcents` é 10⁻⁵ de dólar por milhão de tokens.
  const dolares = entrada / 100_000;
  return `US$ ${dolares.toFixed(2)}/1M`;
}
