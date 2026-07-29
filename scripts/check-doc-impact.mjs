#!/usr/bin/env node
// REGRAS §1.10, quarto item: um PR que mexe em migrações, no tool-registry ou
// nos contratos compartilhados e não toca em `docs/` só passa se declarar
// explicitamente a válvula de escape do §1.3 na descrição.
//
// Uso: BASE_SHA=<sha> PR_BODY="<corpo>" node scripts/check-doc-impact.mjs
// Sem BASE_SHA o script não faz nada (push direto, execução local).

import { execFileSync } from "node:child_process";

const BASE = process.env.BASE_SHA;
const CORPO = process.env.PR_BODY ?? "";

if (!BASE) {
  console.log("check-doc-impact: sem BASE_SHA (não é PR) — nada a verificar");
  process.exit(0);
}

// Caminhos cuja mudança quase sempre muda comportamento, contrato ou schema.
const SENSIVEIS = [
  { prefixo: "crates/storage/migrations/", rotulo: "migrações" },
  { prefixo: "crates/tool-registry/", rotulo: "tool-registry" },
  { prefixo: "packages/shared-contracts/", rotulo: "contratos compartilhados" },
];

// A válvula de escape do §1.3, verbatim na descrição do PR.
const ESCAPE = /sem impacto documental/i;

let alterados;
try {
  alterados = execFileSync(
    "git",
    ["diff", "--name-only", `${BASE}...HEAD`],
    { encoding: "utf8" },
  )
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
} catch (e) {
  console.error(
    `check-doc-impact: não consegui diffar contra ${BASE}. ` +
      "O checkout precisa de fetch-depth: 0.",
  );
  console.error(String(e.message ?? e));
  process.exit(1);
}

const tocados = SENSIVEIS.filter(({ prefixo }) =>
  alterados.some((f) => f.startsWith(prefixo)),
);

if (tocados.length === 0) {
  console.log("check-doc-impact: OK (nenhum caminho sensível alterado)");
  process.exit(0);
}

const tocouDocs = alterados.some(
  (f) => f.startsWith("docs/") || f === "CHANGELOG.md" || f === "README.md",
);

if (tocouDocs) {
  console.log(
    `check-doc-impact: OK (${tocados.map((t) => t.rotulo).join(", ")} alterado(s), documentação acompanhada)`,
  );
  process.exit(0);
}

if (ESCAPE.test(CORPO)) {
  console.log(
    `check-doc-impact: OK por declaração explícita — ${tocados.map((t) => t.rotulo).join(", ")} alterado(s) ` +
      'sem mudança em docs/, e a descrição do PR declara "Sem impacto documental" (§1.3). ' +
      "Se o motivo não convencer, o PR não entra — isso é revisão humana.",
  );
  process.exit(0);
}

console.error(
  `FALHA [impacto documental §1.10] este PR altera ${tocados.map((t) => t.rotulo).join(", ")} ` +
    "e não toca em docs/.",
);
console.error(
  'Atualize a documentação afetada no mesmo PR (§1.3), ou declare na descrição: ' +
    '"Sem impacto documental — <motivo>".',
);
console.error(
  "Arquivos sensíveis alterados:\n" +
    alterados
      .filter((f) => SENSIVEIS.some(({ prefixo }) => f.startsWith(prefixo)))
      .map((f) => `  - ${f}`)
      .join("\n"),
);
process.exit(1);
