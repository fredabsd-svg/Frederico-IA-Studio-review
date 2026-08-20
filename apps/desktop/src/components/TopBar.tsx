import { Link, useLocation } from "react-router-dom";

/**
 * Barra superior do Studio (48px).
 *
 * Substitui a fileira de links de texto que servia de navegação. Os
 * destinos continuam os mesmos — as rotas não mudaram —, mas os que
 * são ferramenta (métricas, configurações) viram botões-ícone à
 * direita, e os que são lugar (Chat, Modo Equipe, Memórias)
 * continuam links nomeados.
 *
 * **Ícone sem rótulo é ícone com `aria-label`.** Todo botão daqui
 * tem `aria-label` e `title`: o primeiro para quem usa leitor de
 * tela, o segundo para quem passa o mouse e não reconheceu o
 * desenho.
 */
export function TopBar(props: { conectado: boolean }) {
  const { pathname } = useLocation();
  return (
    <header className="topbar">
      <div className="topbar-esq">
        <span className="marca" aria-hidden="true">
          F
        </span>
        <h1 className="topbar-titulo">Frederico IA Studio</h1>
        <nav className="topbar-nav">
          <Link
            to="/chat"
            className={pathname.startsWith("/chat") ? "ativo" : ""}
          >
            Chat
          </Link>
          <Link to="/team" className={pathname === "/team" ? "ativo" : ""}>
            Modo Equipe
          </Link>
          <Link
            to="/memories"
            className={pathname === "/memories" ? "ativo" : ""}
          >
            Memórias
          </Link>
        </nav>
      </div>
      <div className="topbar-dir">
        {/* O estado de conexão nunca é só a cor do ponto: o texto
            ao lado diz a mesma coisa, para quem não distingue verde
            de cinza. */}
        <span className="conexao">
          <span
            className={`conexao-dot ${props.conectado ? "ok" : "off"}`}
            aria-hidden="true"
          />
          {props.conectado ? "Conectado" : "Sem provedor"}
        </span>
        <Link
          to="/settings"
          className="btn-icone"
          aria-label="Configurações"
          title="Configurações"
        >
          <IconeEngrenagem />
        </Link>
        <Link
          to="/sobre"
          className="btn-icone"
          aria-label="Sobre o aplicativo"
          title="Sobre"
        >
          <IconeInfo />
        </Link>
      </div>
    </header>
  );
}

/* Ícones desenhados aqui, não importados: o handoff pede Lucide,
   mas acrescentar uma dependência de ~1.500 ícones para usar dois
   não se paga num app empacotado. O traço segue o mesmo padrão
   (stroke 2, 16px, currentColor). */

function IconeEngrenagem() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function IconeInfo() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="10" />
      <path d="M12 16v-4M12 8h.01" />
    </svg>
  );
}
