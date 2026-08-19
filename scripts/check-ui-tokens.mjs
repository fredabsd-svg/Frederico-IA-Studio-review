#!/usr/bin/env node
/**
 * check-ui-tokens — porta 3 do ADR-0045 §D2.
 *
 * Recusa cor, tamanho de fonte, raio e espaçamento escritos direto
 * na regra. Fora do bloco de tokens, esses quatro eixos só existem
 * como `var(--token)`.
 *
 * ## Por que uma guarda, e não uma convenção
 *
 * O `styles.css` chegou a 40 literais de cor, 10 tamanhos de fonte
 * e 4 raios não porque alguém decidiu, mas porque cada linha nova
 * escolhia o seu. Convenção que ninguém verifica é convenção que
 * volta na primeira pressa — o mesmo raciocínio da guarda de versão
 * literal do PR #58 e do `git_has_no_process_spawn` do
 * `git-engine`.
 *
 * O ADR-0045 §Consequências avisa que isto **vai incomodar na
 * primeira vez**, e que o incômodo é o efeito pretendido: token
 * novo se acrescenta ao `:root`, onde a porta de contraste o mede.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const raiz = join(dirname(fileURLToPath(import.meta.url)), "..");
const CSS = join(raiz, "apps", "desktop", "src", "styles.css");

const css = readFileSync(CSS, "utf8");
const linhas = css.split(/\r?\n/);

/**
 * O bloco de tokens vai do começo do arquivo até o fim do
 * `@media (prefers-color-scheme: light)`. É lá que os literais
 * **devem** morar.
 */
const fimDosTokens = (() => {
  const i = linhas.findIndex((l) => l.includes("* { box-sizing: border-box; }"));
  if (i < 0) {
    throw new Error(
      "não achei o fim do bloco de tokens (`* { box-sizing: border-box; }`)",
    );
  }
  return i;
})();

/** Valores que não são escolha visual e não precisam de token. */
const NEUTROS = new Set([
  "0",
  "auto",
  "inherit",
  "initial",
  "unset",
  "none",
  "transparent",
  "currentColor",
]);

const violacoes = [];

for (let i = fimDosTokens; i < linhas.length; i += 1) {
  const linha = linhas[i];
  const numero = i + 1;
  // Comentário não é regra.
  const semComentario = linha.replace(/\/\*[\s\S]*?\*\//g, "");
  if (semComentario.trim().startsWith("*") || semComentario.trim().startsWith("/*")) {
    continue;
  }

  // --- cor
  for (const m of semComentario.matchAll(/#[0-9a-fA-F]{3,8}\b|\brgba?\([^)]*\)/g)) {
    violacoes.push({
      numero,
      eixo: "cor",
      valor: m[0],
      dica: "acrescente um token no `:root` e use `var(--nome)`",
    });
  }

  // --- tamanho de fonte
  for (const m of semComentario.matchAll(/font-size:\s*([^;]+)/g)) {
    const v = m[1].trim();
    if (!v.startsWith("var(") && !NEUTROS.has(v)) {
      violacoes.push({
        numero,
        eixo: "tamanho de fonte",
        valor: v,
        dica: "use um dos seis degraus (`--fonte-xs` … `--fonte-xl`)",
      });
    }
  }

  // --- raio
  for (const m of semComentario.matchAll(/border-radius:\s*([^;]+)/g)) {
    const v = m[1].trim();
    if (!v.startsWith("var(") && !NEUTROS.has(v)) {
      violacoes.push({
        numero,
        eixo: "raio",
        valor: v,
        dica: "use `--raio-sm`, `--raio-md` ou `--raio-lg`",
      });
    }
  }

  // --- espaçamento
  for (const m of semComentario.matchAll(
    /\b((?:padding|margin|gap)(?:-(?:top|right|bottom|left))?):\s*([^;{}]+)/g,
  )) {
    for (const parte of m[2].trim().split(/\s+/)) {
      if (parte.startsWith("var(") || parte.startsWith("calc(") || NEUTROS.has(parte)) {
        continue;
      }
      if (/^-?[0-9.]+(px|rem|em|%)$/.test(parte)) {
        violacoes.push({
          numero,
          eixo: "espaçamento",
          valor: `${m[1]}: ${parte}`,
          dica: "use um dos seis degraus (`--esp-1` … `--esp-6`)",
        });
      }
    }
  }
}

if (violacoes.length > 0) {
  console.error("check-ui-tokens: FALHOU\n");
  for (const v of violacoes) {
    console.error(
      `  styles.css:${v.numero} — ${v.eixo} literal \`${v.valor}\`\n      ${v.dica}`,
    );
  }
  console.error(
    `\n${violacoes.length} literal(is) visual(is) fora do bloco de tokens (ADR-0045 §D2 porta 3).`,
  );
  console.error(
    "Token novo entra no `:root`, onde o `check-ui-contrast.mjs` o mede.",
  );
  process.exit(1);
}

// Controle positivo: se a leitura do arquivo quebrar, o script
// passaria por vacuidade. Conferir que o bloco de tokens tem o que
// deveria ter impede isso.
const blocoTokens = linhas.slice(0, fimDosTokens).join("\n");
const obrigatorios = ["--bg", "--fg", "--accent", "--foco", "--esp-1", "--raio-md", "--fonte-md"];
const faltando = obrigatorios.filter((t) => !blocoTokens.includes(`${t}:`));
if (faltando.length > 0) {
  console.error(
    `check-ui-tokens: o bloco de tokens não define ${faltando.join(", ")} — a leitura do arquivo está errada`,
  );
  process.exit(1);
}

console.log(
  `check-ui-tokens: OK (nenhum literal visual fora do bloco de tokens; ${linhas.length - fimDosTokens} linhas varridas)`,
);
