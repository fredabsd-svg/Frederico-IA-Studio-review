import { HashRouter, Link, Navigate, Route, Routes } from "react-router-dom";
import { Chat } from "./routes/Chat";
import { Settings } from "./routes/Settings";
import { About } from "./routes/About";

/**
 * Frederico IA Studio — casca React.
 *
 * Rotas:
 * - `/` — redireciona para `/chat`.
 * - `/chat` — chat sem conversa selecionada (cria uma nova).
 * - `/chat/:id` — chat com conversa selecionada.
 * - `/settings` — configuração de credenciais de provedores.
 * - `/sobre` — sobre (Fase 1 keep-alive).
 *
 * Camada `services/` é a **única** que faz `invoke` no Tauri e
 * `listen` em eventos. Componentes React nunca importam
 * `@tauri-apps/api` diretamente.
 */
export function App() {
  return (
    <HashRouter>
      <div className="layout">
        <header className="topbar">
          <h1>Frederico IA Studio</h1>
          <nav>
            <Link to="/chat">Chat</Link>
            <Link to="/settings">Configurações</Link>
            <Link to="/sobre">Sobre</Link>
          </nav>
        </header>
        <main>
          <Routes>
            <Route path="/" element={<Navigate to="/chat" replace />} />
            <Route path="/chat" element={<Chat />} />
            <Route path="/chat/:id" element={<Chat />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/sobre" element={<About />} />
          </Routes>
        </main>
        <footer>
          <small>Frederico IA Studio — v0.2.0 (Fase 2: Chat e provedores)</small>
        </footer>
      </div>
    </HashRouter>
  );
}
