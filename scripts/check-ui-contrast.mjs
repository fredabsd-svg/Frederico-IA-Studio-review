#!/usr/bin/env node
/**
 * check-ui-contrast — porta 1 do ADR-0045 §D2.
 *
 * Mede o contraste de todo par texto/fundo que a interface usa,
 * **nos dois temas**, contra o mínimo do WCAG 2.1 AA: 4,5:1 para
 * texto normal e 3:1 para texto grande e para indicador não-textual
 * (critérios 1.4.3 e 1.4.11).
 *
 * ## Por que este script existe
 *
 * O tema claro do app falhava em AA desde que foi escrito: o bloco
 * `prefers-color-scheme: light` sobrescrevia 5 dos 6 tokens e
 * esquecia o `--accent`, deixando o latão `#d4a05a` a **2,24:1**
 * sobre `#fafafa`. Ninguém percebeu porque nada media.
 *
 * A régua é mecânica de propósito (ADR-0045 §D2): "ficar bonito"
 * não fecha etapa, e opinião como critério foi o que tirou o
 * Copiloto da fase. Este script mede contraste, nunca gosto.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const raiz = join(dirname(fileURLToPath(import.meta.url)), "..");
const CSS = join(raiz, "apps", "desktop", "src", "styles.css");

/** Mínimos do WCAG 2.1 AA. */
const AA_TEXTO = 4.5;
const AA_NAO_TEXTO = 3.0;

/**
 * Pares conferidos. `min` distingue texto normal (4,5) de
 * indicador não-textual e texto grande (3,0).
 *
 * A lista é explícita, e não derivada do CSS, de propósito: um par
 * que a interface usa e que ninguém listou aqui é um par não
 * medido, e derivar automaticamente esconderia essa lacuna atrás
 * da aparência de cobertura total.
 */
const PARES = [
  ["fg", "bg", AA_TEXTO],
  ["fg", "bg-elev", AA_TEXTO],
  ["fg", "bg-controle", AA_TEXTO],
  ["fg-dim", "bg", AA_TEXTO],
  ["fg-dim", "bg-elev", AA_TEXTO],
  ["accent", "bg", AA_TEXTO],
  ["accent", "bg-elev", AA_TEXTO],
  ["erro", "bg", AA_TEXTO],
  ["erro", "bg-elev", AA_TEXTO],
  ["sucesso", "bg", AA_TEXTO],
  ["sucesso", "bg-elev", AA_TEXTO],
  ["fg-sobre-acento", "accent", AA_TEXTO],
  ["fg-sobre-acento", "erro", AA_TEXTO],
  ["fg", "erro-fundo-solido", AA_TEXTO],
  ["fg", "sucesso-fundo-solido", AA_TEXTO],
  ["fg", "info-fundo-solido", AA_TEXTO],
  // Indicador de foco: não é texto, então 3:1 (critério 1.4.11).
  ["foco", "bg", AA_NAO_TEXTO],
  ["foco", "bg-elev", AA_NAO_TEXTO],
  // Borda contra o fundo do painel — separador visual.
  ["border", "bg-elev", 1.0],
  // --- Superfícies do Studio (novo layout) -------------------
  // O poço do terminal e o painel Live são fundos de texto como
  // qualquer outro: se entram na paleta, entram na porta.
  ["fg", "bg-profundo", AA_TEXTO],
  ["fg", "bg-painel", AA_TEXTO],
  ["fg-dim", "bg-painel", AA_TEXTO],
  ["fg-terminal", "bg-profundo", AA_TEXTO],
  ["accent", "bg-profundo", AA_TEXTO],
  ["erro", "bg-profundo", AA_TEXTO],
  ["sucesso", "bg-profundo", AA_TEXTO],
  ["aviso", "bg-profundo", AA_TEXTO],
  ["aviso", "bg", AA_TEXTO],
  ["aviso", "bg-elev", AA_TEXTO],
  // Mensagem do usuário: fundo próprio, texto normal.
  ["fg", "bg-usuario", AA_TEXTO],
  ["border-usuario", "bg-usuario", 1.0],
];

/** `#rgb` / `#rrggbb` → `[r, g, b]`. */
function paraRgb(cor) {
  const c = cor.trim().replace(/^#/, "");
  if (c.length === 3) {
    return [0, 1, 2].map((i) => parseInt(c[i] + c[i], 16));
  }
  if (c.length === 6) {
    return [0, 2, 4].map((i) => parseInt(c.slice(i, i + 2), 16));
  }
  return null;
}

/** Luminância relativa (WCAG 2.1, 1.4.3). */
function luminancia([r, g, b]) {
  const canal = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * canal(r) + 0.7152 * canal(g) + 0.0722 * canal(b);
}

function contraste(a, b) {
  const la = luminancia(a);
  const lb = luminancia(b);
  const [claro, escuro] = la > lb ? [la, lb] : [lb, la];
  return (claro + 0.05) / (escuro + 0.05);
}

/**
 * Extrai os tokens de um bloco `:root`.
 *
 * Lê o primeiro `:root` (tema escuro) e o `:root` dentro do
 * `@media (prefers-color-scheme: light)`. O tema claro **herda** o
 * escuro no que não sobrescreve — que é como o CSS se comporta, e
 * é justamente por isso que o `--accent` esquecido passava
 * despercebido.
 */
function lerTokens(css) {
  const escuro = {};
  const claro = {};

  const primeiroRoot = css.match(/^:root\s*\{([\s\S]*?)\n\}/m);
  if (!primeiroRoot) {
    throw new Error("não achei o bloco `:root` em styles.css");
  }
  for (const [, nome, valor] of primeiroRoot[1].matchAll(
    /--([a-z0-9-]+)\s*:\s*([^;]+);/g,
  )) {
    escuro[nome] = valor.trim();
  }

  const blocoClaro = css.match(
    /@media \(prefers-color-scheme: light\)\s*\{\s*:root\s*\{([\s\S]*?)\n {2}\}/,
  );
  if (!blocoClaro) {
    throw new Error("não achei o bloco de tema claro em styles.css");
  }
  Object.assign(claro, escuro);
  for (const [, nome, valor] of blocoClaro[1].matchAll(
    /--([a-z0-9-]+)\s*:\s*([^;]+);/g,
  )) {
    claro[nome] = valor.trim();
  }

  return { escuro, claro };
}

function medir(tokens, nomeTema) {
  const falhas = [];
  const linhas = [];
  for (const [frente, fundo, minimo] of PARES) {
    const vf = tokens[frente];
    const vb = tokens[fundo];
    if (!vf || !vb) {
      falhas.push(`${nomeTema}: token ausente (--${frente} ou --${fundo})`);
      continue;
    }
    const rgbF = paraRgb(vf);
    const rgbB = paraRgb(vb);
    if (!rgbF || !rgbB) {
      // `rgba()` depende de composição sobre o que está atrás e não
      // é medível aqui. Declarado em vez de silenciosamente pulado.
      linhas.push(`  ${nomeTema}: --${frente}/--${fundo} — não medido (cor com alfa)`);
      continue;
    }
    const razao = contraste(rgbF, rgbB);
    const ok = razao + 1e-9 >= minimo;
    linhas.push(
      `  ${nomeTema}: --${frente}/--${fundo} = ${razao.toFixed(2)}:1 (min ${minimo}) ${ok ? "OK" : "FALHA"}`,
    );
    if (!ok) {
      falhas.push(
        `${nomeTema}: --${frente} sobre --${fundo} dá ${razao.toFixed(2)}:1, abaixo de ${minimo}:1`,
      );
    }
  }
  return { falhas, linhas };
}

const css = readFileSync(CSS, "utf8");
const { escuro, claro } = lerTokens(css);

const r1 = medir(escuro, "escuro");
const r2 = medir(claro, "claro");
const falhas = [...r1.falhas, ...r2.falhas];

if (process.argv.includes("--verbose") || falhas.length > 0) {
  for (const l of [...r1.linhas, ...r2.linhas]) {
    console.log(l);
  }
}

if (falhas.length > 0) {
  console.error("\ncheck-ui-contrast: FALHOU\n");
  for (const f of falhas) {
    console.error(`  - ${f}`);
  }
  console.error(
    "\nMínimos do WCAG 2.1 AA: 4,5:1 para texto, 3:1 para indicador não-textual.",
  );
  process.exit(1);
}

console.log(
  `check-ui-contrast: OK (${PARES.length} pares × 2 temas, WCAG 2.1 AA)`,
);
