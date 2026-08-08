/**
 * `routes/Team.tsx` — rota `/team` (Fase 6, Etapa 7 UI/Polish
 * do Modo Equipe).
 *
 * Wrapper simples do `PipelineView` (mesma forma do
 * `routes/Memories.tsx` que wrappa `MemoryPanel`). A UI
 * inteira do Modo Equipe vive em `components/PipelineView.tsx`
 * (sidebar + detalhe + modal de criação).
 */

import { PipelineView } from "../components/PipelineView";

export function Team() {
  return (
    <div className="route-team">
      <PipelineView />
    </div>
  );
}
