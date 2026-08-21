import { useCallback, useEffect, useState } from "react";
import {
  HashRouter,
  Navigate,
  Route,
  Routes,
  useLocation,
} from "react-router-dom";
import { ApprovalModal } from "./components/ApprovalModal";
import { TopBar } from "./components/TopBar";
import {
  getAppVersion,
  listApprovals,
  respondToApproval,
  type ApprovalDecision,
  type ApprovalEntryView,
} from "./services";
import { FASES } from "./generated/phase-status";
import { Chat } from "./routes/Chat";
import { Settings } from "./routes/Settings";
import { About } from "./routes/About";
import { Memories } from "./routes/Memories";
import { Team } from "./routes/Team";

/**
 * Frederico IA Studio — casca React.
 *
 * Rotas:
 * - `/` — redireciona para `/chat`.
 * - `/chat` — chat sem conversa selecionada (cria uma nova).
 * - `/chat/:id` — chat com conversa selecionada.
 * - `/team` — Modo Equipe (Fase 6, Etapa 7 UI/Polish):
 *   sidebar com pipelines resumable + detalhe por stage +
 *   criação de novo pipeline.
 * - `/memories` — painel de memórias.
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
  // Rodapé derivado, nunca escrito à mão (§1.9). O mesmo par de
  // fontes que a tela `/sobre` usa desde o PR #58: a versão vem do
  // binário via `getAppVersion()` (fonte: `tauri.conf.json`), e a
  // fase corrente vem do `generated/phase-status.ts`, derivado do
  // `docs/status.md`. O literal que estava aqui anunciou
  // "v0.3.0 (Fase 3)" até a Fase 7 fechar.
  const [versao, setVersao] = useState<string | null>(null);
  useEffect(() => {
    let cancelado = false;
    getAppVersion()
      .then((v) => {
        if (!cancelado) setVersao(v);
      })
      // Rodapé é informativo: se a versão não vier, ele a omite em
      // vez de exibir erro ou um valor inventado.
      .catch(() => {});
    return () => {
      cancelado = true;
    };
  }, []);

  // A fase corrente é a última que não está "não iniciada" — a
  // mesma leitura que o `status.md` faz de cima para baixo.
  const faseAtual = [...FASES]
    .reverse()
    .find((f) => f.estado !== "não iniciada");

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
        {/* `conectado` = a versão do binário respondeu, ou seja,
            a casca Tauri está de pé. Não é "há provedor
            configurado" — essa informação vive em
            `/settings` e afirmá-la aqui exigiria um IPC a mais
            para dizer algo que a tela ao lado já diz melhor. */}
        <TopBar conectado={versao !== null} />
        <Corpo>
          <Routes>
            <Route path="/" element={<Navigate to="/chat" replace />} />
            <Route path="/chat" element={<Chat />} />
            <Route path="/chat/:id" element={<Chat />} />
            <Route path="/team" element={<Team />} />
            <Route path="/memories" element={<Memories />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/sobre" element={<About />} />
          </Routes>
        </Corpo>
        <footer>
          <small>
            Frederico IA Studio
            {versao ? ` — v${versao}` : ""}
            {faseAtual
              ? ` (Fase ${faseAtual.id}: ${faseAtual.nome} — ${faseAtual.estado})`
              : ""}
          </small>
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

/**
 * `<main>` com a largura certa para cada rota.
 *
 * O Studio ocupa a janela inteira (três colunas, altura fixa); as
 * telas de leitura — Configurações, Sobre, Memórias — continuam
 * numa coluna de 900px, que é onde texto corrido se lê. Uma regra
 * só para as duas coisas deixaria o console espremido ou o texto
 * atravessando a tela.
 */
function Corpo(props: { children: React.ReactNode }) {
  const { pathname } = useLocation();
  const estudio = pathname === "/" || pathname.startsWith("/chat");
  return <main className={estudio ? "" : "coluna"}>{props.children}</main>;
}
