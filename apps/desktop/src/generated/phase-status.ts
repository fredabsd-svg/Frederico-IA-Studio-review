/**
 * ARQUIVO GERADO — não edite; fonte: docs/status.md
 *
 * Regenerar: `node scripts/generate-phase-status.mjs`
 * O CI falha se este arquivo divergir da fonte (REGRAS §1.9/§1.10).
 */

export type EstadoDaFase =
  | "não iniciada"
  | "em andamento"
  | "concluída"
  | "bloqueada";

export interface Fase {
  /** Identificador da fase. Nem sempre numérico — existe a `5b`. */
  id: string;
  nome: string;
  estado: EstadoDaFase;
}

export const FASES: readonly Fase[] = [
  { id: "0", nome: "Fundação documental", estado: "concluída" },
  { id: "1", nome: "Fundação (Tauri + Rust + SQLite)", estado: "concluída" },
  { id: "2", nome: "Chat e provedores", estado: "concluída" },
  { id: "3", nome: "Motor de execução e ferramentas", estado: "concluída" },
  { id: "4", nome: "Memória e continuidade", estado: "concluída" },
  { id: "5", nome: "Documentos", estado: "concluída" },
  { id: "5b", nome: "Fase de Ligação (integração casca + document-kits no ToolRegistry)", estado: "concluída" },
  { id: "6", nome: "Multimodelo e subagentes", estado: "concluída" },
  { id: "7", nome: "Execução isolada (Modo Desenvolvedor: núcleo)", estado: "em andamento" },
  { id: "8", nome: "Modo Desenvolvedor integrado", estado: "não iniciada" },
  { id: "9", nome: "Produção", estado: "não iniciada" },
];
