export function About() {
  return (
    <section>
      <h2>Sobre</h2>
      <p>
        Frederico IA Studio é um estúdio de IA desktop para Windows 10/11.
        Esta é a versão <strong>0.1.0</strong> — Fase 1 (Fundação).
      </p>
      <p>
        O que <strong>funciona</strong> hoje:
      </p>
      <ul>
        <li>Casca Tauri + React + TypeScript abrindo.</li>
        <li>Navegação entre rotas.</li>
        <li>SQLite local com migração inicial aplicada.</li>
        <li>IPC núcleo ↔ casca (uma operação: <code>get_app_info</code>).</li>
        <li>Logs estruturados via <code>tracing</code>.</li>
      </ul>
      <p>
        O que <strong>não</strong> funciona ainda (chega nas próximas fases):
      </p>
      <ul>
        <li>Chat com provedores (Fase 2).</li>
        <li>Tools, execuções, checkpoints (Fase 3).</li>
        <li>Memória e continuidade (Fase 4).</li>
        <li>Documentos Word/Excel/PDF (Fase 5).</li>
      </ul>
    </section>
  );
}
