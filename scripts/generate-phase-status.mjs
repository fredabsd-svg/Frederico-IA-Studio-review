#!/usr/bin/env node
/**
 * Gera `apps/desktop/src/generated/phase-status.ts` a partir da
 * tabela de fases do `docs/status.md` (REGRAS §1.9 — "gerado
 * vence manual").
 *
 * Por que este gerador existe: a tela `/sobre` mantinha à mão uma
 * lista de "o que funciona" e "o que não funciona ainda". Ela
 * parou de ser verdade na Fase 3 e continuou na tela até a Fase 7
 * — afirmando ao usuário que tool calls, memória e documentos não
 * funcionavam, muito depois de as três fases terem fechado. É
 * exatamente o defeito que a §1.9 existe para prevenir: se a
 * mesma verdade vive em dois lugares, um deles vai mentir.
 *
 * O que é extraído: só as 3 colunas que a máquina lê sem
 * ambiguidade — número da fase, nome e estado. A coluna
 * "Evidência" é prosa longa e fica de fora de propósito; quem
 * quer o detalhe vai ao `docs/status.md`.
 *
 * Uso:
 *   node scripts/generate-phase-status.mjs           # escreve
 *   node scripts/generate-phase-status.mjs --check   # só confere
 *
 * O modo `--check` é o que roda no `check-docs.mjs` e no CI: sai
 * com código 1 se o arquivo gerado divergir da fonte (§1.10).
 */

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const FONTE = "docs/status.md";
const DESTINO = "apps/desktop/src/generated/phase-status.ts";

/** Estados válidos da §1.8. Qualquer outro é erro de fonte. */
const ESTADOS = new Set([
  "não iniciada",
  "em andamento",
  "concluída",
  "bloqueada",
]);

/**
 * Lê a tabela de fases do `status.md`.
 *
 * Formato esperado por linha:
 *   `| <fase> | <nome> | <estado> | <evidência> | ...`
 *
 * A fase não é necessariamente numérica — existe uma `5b` (Fase
 * de Ligação). Por isso o identificador viaja como string.
 */
function lerFases() {
  const texto = readFileSync(join(ROOT, FONTE), "utf8");
  const fases = [];
  for (const linha of texto.split("\n")) {
    const m = linha.match(/^\|\s*([0-9]+[a-z]?)\s*\|([^|]*)\|([^|]*)\|/);
    if (!m) continue;
    const [, id, nome, estado] = m;
    const estadoLimpo = estado.trim();
    if (!ESTADOS.has(estadoLimpo)) {
      throw new Error(
        `${FONTE}: fase ${id} tem estado "${estadoLimpo}", fora da lista da §1.8 ` +
          `(${[...ESTADOS].join(" | ")})`,
      );
    }
    fases.push({ id: id.trim(), nome: nome.trim(), estado: estadoLimpo });
  }
  if (fases.length === 0) {
    throw new Error(`${FONTE}: nenhuma linha de fase reconhecida na tabela`);
  }
  return fases;
}

function render(fases) {
  const linhas = fases
    .map(
      (f) =>
        `  { id: ${JSON.stringify(f.id)}, nome: ${JSON.stringify(f.nome)}, ` +
        `estado: ${JSON.stringify(f.estado)} },`,
    )
    .join("\n");

  return `/**
 * ARQUIVO GERADO — não edite; fonte: ${FONTE}
 *
 * Regenerar: \`node scripts/generate-phase-status.mjs\`
 * O CI falha se este arquivo divergir da fonte (REGRAS §1.9/§1.10).
 */

export type EstadoDaFase =
  | "não iniciada"
  | "em andamento"
  | "concluída"
  | "bloqueada";

export interface Fase {
  /** Identificador da fase. Nem sempre numérico — existe a \`5b\`. */
  id: string;
  nome: string;
  estado: EstadoDaFase;
}

export const FASES: readonly Fase[] = [
${linhas}
];
`;
}

function main() {
  const check = process.argv.includes("--check");
  const esperado = render(lerFases());
  const destino = join(ROOT, DESTINO);

  if (check) {
    let atual;
    try {
      atual = readFileSync(destino, "utf8");
    } catch {
      console.error(
        `generate-phase-status: ${DESTINO} não existe. Rode ` +
          `\`node scripts/generate-phase-status.mjs\`.`,
      );
      process.exit(1);
    }
    // Normaliza CRLF: o repositório roda em Windows e o git pode
    // reescrever a quebra de linha na checagem de saída.
    if (atual.replace(/\r\n/g, "\n") !== esperado) {
      console.error(
        `generate-phase-status: ${DESTINO} divergiu de ${FONTE} (REGRAS §1.9). ` +
          `Rode \`node scripts/generate-phase-status.mjs\` e commite o resultado.`,
      );
      process.exit(1);
    }
    console.log(`generate-phase-status: OK (${DESTINO} em dia com ${FONTE})`);
    return;
  }

  mkdirSync(dirname(destino), { recursive: true });
  writeFileSync(destino, esperado, "utf8");
  console.log(`generate-phase-status: ${DESTINO} gerado a partir de ${FONTE}`);
}

main();
