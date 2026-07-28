/**
 * `routes/Memories.tsx` — rota `/memories` (Fase 4, Etapa 5).
 *
 * Wrapper simples do `MemoryPanel` com escopo default
 * `project:default`. A UI pode evoluir pra deixar o usuário
 * escolher o escopo via dropdown (já está dentro do painel).
 */

import { MemoryPanel } from "../components/MemoryPanel";

export function Memories() {
  return (
    <div className="route-memories">
      <MemoryPanel initialScopeType="project" initialScopeId="default" />
    </div>
  );
}
