import { useCallback, useEffect, useState } from "react";
import { HashRouter, Link, Navigate, Route, Routes } from "react-router-dom";
import { ApprovalModal } from "./components/ApprovalModal";
import {
  listApprovals,
  respondToApproval,
  type ApprovalDecision,
  type ApprovalEntryView,
} from "./services";
import { Chat } from "./routes/Chat";
import { Settings } from "./routes/Settings";
import { About } from "./routes/About";
import { Memories } from "./routes/Memories";

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
 *
 * **Modal de aprovação (Etapa 6.2):** o `App` polla a fila de
 * aprovação a cada 2s e renderiza o `ApprovalModal` quando
 * chega uma request. O modal é global (fora do `Routes`) —
 * aparece em qualquer rota (Chat, Settings, About). O
 * polling é a abordagem da Etapa 6.2; a Etapa 6.2.x pode
 * adicionar `emit_run_event` no `EventSink` pra "approval_
 * required" e virar push-based.
 */
export function App() {
  const [pendingApproval, setPendingApproval] =
    useState<ApprovalEntryView | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await listApprovals();
      // Pega a primeira pending. Fila 1-por-run (Etapa 6.1) —
      // múltiplas em paralelo é 6.x.
      setPendingApproval(list.length > 0 ? list[0] : null);
    } catch {
      // Backend não está pronto ou sem permissões. Ignora.
    }
  }, []);

  useEffect(() => {
    refresh();
    const id = window.setInterval(refresh, 2000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const handleApprove = useCallback(
    async (approvalId: string, decision: ApprovalDecision) => {
      try {
        await respondToApproval(approvalId, decision);
      } finally {
        setPendingApproval(null);
        // Re-fetch imediato (não espera 2s) — se houver outra
        // pending, mostra já.
        refresh();
      }
    },
    [refresh],
  );

  const handleReject = useCallback(
    async (approvalId: string, decision: ApprovalDecision) => {
      try {
        await respondToApproval(approvalId, decision);
      } finally {
        setPendingApproval(null);
        refresh();
      }
    },
    [refresh],
  );

  const handleClose = useCallback(() => {
    // Fechar sem responder — o polling vai trazer de volta
    // quando o user navegar pela app. Não responde ao IPC.
    setPendingApproval(null);
  }, []);

  return (
    <HashRouter>
      <div className="layout">
        <header className="topbar">
          <h1>Frederico IA Studio</h1>
          <nav>
            <Link to="/chat">Chat</Link>
            <Link to="/memories">Memórias</Link>
            <Link to="/settings">Configurações</Link>
            <Link to="/sobre">Sobre</Link>
          </nav>
        </header>
        <main>
          <Routes>
            <Route path="/" element={<Navigate to="/chat" replace />} />
            <Route path="/chat" element={<Chat />} />
            <Route path="/chat/:id" element={<Chat />} />
            <Route path="/memories" element={<Memories />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/sobre" element={<About />} />
          </Routes>
        </main>
        <footer>
          <small>Frederico IA Studio — v0.3.0 (Fase 3: Motor de execução e ferramentas)</small>
        </footer>
        {pendingApproval && (
          <ApprovalModal
            entry={pendingApproval}
            onApprove={handleApprove}
            onReject={handleReject}
            onClose={handleClose}
          />
        )}
      </div>
    </HashRouter>
  );
}
