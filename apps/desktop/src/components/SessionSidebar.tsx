import type { ConversationView } from "../services";

/**
 * Sidebar de sessões (236px).
 *
 * **O que não está aqui:** a seção "ARQUIVOS DO WORKSPACE" do
 * protótipo. O núcleo não tem hoje um conceito de arquivo de
 * workspace listável pela UI, e desenhar a seção com nomes
 * inventados seria exatamente o que o projeto proíbe — mostrar o
 * que o sistema não fez. A seção entra quando houver o que listar.
 *
 * O rodapé de uso do mês também ficou de fora pelo mesmo motivo: o
 * custo por conversa existe (`total_cost_microcents`), mas
 * "uso do mês" exigiria uma agregação por período que ninguém
 * calcula ainda. O total por sessão aparece na linha de cada item,
 * que é o dado real.
 */
export function SessionSidebar(props: {
  conversas: ConversationView[];
  atual: string | undefined;
  formatCost: (v: number) => string;
  onSelecionar: (id: string) => void;
  onApagar: (id: string) => void;
  onNova: () => void;
}) {
  return (
    <aside className="sidebar">
      <button className="btn-nova-sessao" onClick={props.onNova}>
        <span className="mais" aria-hidden="true">
          +
        </span>{" "}
        Nova sessão
      </button>

      <h2 className="sidebar-secao">Sessões</h2>
      {props.conversas.length === 0 ? (
        <p className="sidebar-vazio">Nenhuma sessão ainda.</p>
      ) : (
        <ul className="sessoes">
          {props.conversas.map((c) => (
            <li
              key={c.id}
              className={c.id === props.atual ? "sessao ativa" : "sessao"}
            >
              <button
                className="sessao-btn"
                onClick={() => props.onSelecionar(c.id)}
                title={`${c.provider_id}/${c.model_id}`}
              >
                <span className="sessao-titulo">
                  {c.title || `${c.provider_id}/${c.model_id}`}
                </span>
                <span className="sessao-meta" data-numerico>
                  {props.formatCost(c.total_cost_microcents)}
                </span>
              </button>
              <button
                className="sessao-apagar"
                onClick={() => props.onApagar(c.id)}
                aria-label={`Apagar a sessão ${c.title || c.model_id}`}
                title="Apagar sessão"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
