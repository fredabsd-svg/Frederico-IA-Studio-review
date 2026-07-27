export function About() {
  return (
    <section>
      <h2>Sobre</h2>
      <p>
        Frederico IA Studio é um estúdio de IA desktop para Windows 10/11.
        Versão <strong>0.2.0</strong> — Fase 2 (Chat e provedores) em
        andamento.
      </p>
      <p>O que <strong>funciona</strong> hoje:</p>
      <ul>
        <li>Casca Tauri + React + TypeScript + Vite (Fase 1).</li>
        <li>Navegação entre rotas: <code>/chat</code>, <code>/settings</code>, <code>/sobre</code>.</li>
        <li>SQLite local com 2 migrações (<code>0001_initial</code> + <code>0002_chat_core</code>).</li>
        <li>IPC núcleo ↔ casca: 18 operações de chat, provedor, catálogo, conversa, mensagem e run.</li>
        <li>Catálogo embutido de 13 modelos (OpenAI, Anthropic, OpenRouter, DeepSeek, Mistral, NVIDIA NIM, Ollama, LM Studio, simulated).</li>
        <li>Adapters reais para OpenAI-compat (cobre 7 provedores) e Anthropic (formato genuinamente diferente).</li>
        <li>Chat com streaming de tokens via eventos Tauri, recarregar-janela-meio-stream via <code>RunGetEvents</code>.</li>
        <li>Cancelamento de run, watchdog de 60s, custo por <code>Usage</code>.</li>
        <li>Erros de provedor com tradução PT-BR + ação sugerida.</li>
        <li>Logs estruturados via <code>tracing</code>.</li>
      </ul>
      <p>O que <strong>não</strong> funciona ainda:</p>
      <ul>
        <li>Execução de <code>tool_call</code> (Fase 3 — Motor de execução e ferramentas).</li>
        <li>Memória e continuidade (Fase 4).</li>
        <li>Documentos Word/Excel/PDF (Fase 5).</li>
        <li>Impl real de DPAPI (a <code>WindowsCredentialStore</code> é stub na v0.2).</li>
      </ul>
      <p>
        O projeto anterior morreu em parte porque a documentação prometia
        e o código divergia. Aqui a regra é a oposta:{" "}
        <code>docs/status.md</code> é a fonte da verdade do que está
        pronto. Funcionalidade só entra nele quando os testes da
        respectiva fase passam.
      </p>
    </section>
  );
}
