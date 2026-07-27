import { HashRouter, Link, Route, Routes } from "react-router-dom";
import { Home } from "./routes/Home";
import { About } from "./routes/About";

/**
 * Frederico IA Studio — casca React.
 *
 * A Fase 1 entrega navegação básica com 2 rotas e 1 botão que dispara
 * IPC contra o núcleo (via `services/api.ts`). Sem chat, sem tools,
 * sem documentos — só a casca pra provar o caminho vertical.
 */
export function App() {
  return (
    <HashRouter>
      <div className="layout">
        <header className="topbar">
          <h1>Frederico IA Studio</h1>
          <nav>
            <Link to="/">Início</Link>
            <Link to="/sobre">Sobre</Link>
          </nav>
        </header>
        <main>
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/sobre" element={<About />} />
          </Routes>
        </main>
        <footer>
          <small>Frederico IA Studio — v0.1.0 (Fase 1: Fundação)</small>
        </footer>
      </div>
    </HashRouter>
  );
}
