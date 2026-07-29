#!/usr/bin/env node
// Verificações documentais do CI — implementa o que as REGRAS §1.4, §1.10 e
// §1.13 já prometiam e o pipeline não cobrava.
//
// Uso: node scripts/check-docs.mjs [--json]
// Sai com 1 se qualquer verificação falhar.
//
// Escrito em Node (não em PowerShell como os demais scripts) por um motivo
// prático: o CI já configura Node, e assim a lógica é executável em qualquer
// máquina de desenvolvimento, não só em Windows.
//
// O que este script NÃO faz, deliberadamente: dizer que um documento "confere
// com o código". Isso é revisão humana (REGRAS §1.13, tabela final). Aqui só
// entra o que é mecânico.

import { readFileSync, existsSync, readdirSync, statSync } from "node:fs";
import { join, dirname, normalize, relative, basename } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = normalize(join(dirname(fileURLToPath(import.meta.url)), ".."));

const ESTADOS_VALIDOS = [
  "especificado",
  "parcialmente implementado",
  "implementado",
];
const ESTADOS_IMPLEMENTADOS = ["parcialmente implementado", "implementado"];
const CARIMBO_MAX_DIAS = 60;

const falhas = [];
const avisos = [];
function falha(check, arquivo, msg) {
  falhas.push({ check, arquivo, msg });
}

// ---------------------------------------------------------------------------
// Utilitários
// ---------------------------------------------------------------------------

function listarMd(dir, acc = []) {
  if (!existsSync(dir)) return acc;
  for (const nome of readdirSync(dir)) {
    if (["node_modules", "target", ".git", "dist"].includes(nome)) continue;
    const p = join(dir, nome);
    if (statSync(p).isDirectory()) listarMd(p, acc);
    else if (nome.endsWith(".md")) acc.push(p);
  }
  return acc;
}

const rel = (p) => relative(ROOT, p).split("\\").join("/");

/** Lê o cabeçalho do §1.13 de um spec. Devolve `null` se não houver bloco. */
function lerCabecalho(texto) {
  const bloco = texto.slice(0, 800);
  const pega = (rotulo) => {
    const m = bloco.match(new RegExp(`${rotulo}:\\s*(.+)`));
    return m ? m[1].trim() : null;
  };
  const estado = pega("Estado");
  if (estado === null) return null;
  return {
    estado,
    verificado: pega("Verificado contra o código em"),
    fase: pega("Fase correspondente"),
  };
}

/**
 * Fases citadas por um cabeçalho. `"3 (Etapa 1)"` → `[3]`;
 * `"1-3"` → `[1,2,3]`.
 */
function fasesDoCabecalho(fase) {
  const antesDoParenteses = fase.split("(")[0];
  const intervalo = antesDoParenteses.match(/(\d)\s*-\s*(\d)/);
  if (intervalo) {
    const [, a, b] = intervalo;
    const out = [];
    for (let i = Number(a); i <= Number(b); i++) out.push(i);
    return out;
  }
  return [...antesDoParenteses.matchAll(/\d/g)].map((m) => Number(m[0]));
}

/**
 * Documento de escopo global (visão de produto, roadmap, estratégia de
 * testes): descreve o programa inteiro, não uma fase. A trava do §1.13 não
 * se aplica — a isenção está escrita na própria regra, não escondida aqui.
 */
function ehEscopoGlobal(fase) {
  const f = fase.toLowerCase();
  if (f.includes("global") || f.includes("roadmap")) return true;
  const fases = fasesDoCabecalho(fase);
  return fases.length >= 9;
}

/** Slug de heading no estilo GitHub, para validar âncoras. */
function slug(titulo) {
  return titulo
    .trim()
    .toLowerCase()
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");
}

function ancorasDe(texto) {
  const out = new Set();
  for (const m of texto.matchAll(/^#{1,6}\s+(.+?)\s*$/gm)) {
    out.add(slug(m[1].replace(/`/g, "")));
  }
  return out;
}

// ---------------------------------------------------------------------------
// 1+2+3. Cabeçalho, carimbo e trava do caminho inverso (§1.13)
// ---------------------------------------------------------------------------

function estadoDasFases() {
  const texto = readFileSync(join(ROOT, "docs/status.md"), "utf8");
  const fases = new Map();
  for (const m of texto.matchAll(/^\|\s*(\d)\s*\|[^|]*\|\s*([^|]+?)\s*\|/gm)) {
    fases.set(Number(m[1]), m[2].trim());
  }
  return fases;
}

function checarSpecs() {
  const fases = estadoDasFases();
  const iniciadas = new Set(
    [...fases.entries()]
      .filter(([, e]) => e === "em andamento" || e === "concluída")
      .map(([n]) => n),
  );
  const hoje = new Date();

  for (const arquivo of listarMd(join(ROOT, "docs/architecture"))) {
    const texto = readFileSync(arquivo, "utf8");
    const cab = lerCabecalho(texto);
    if (!cab) {
      falha("cabeçalho", rel(arquivo), "sem o cabeçalho exigido pelo §1.13");
      continue;
    }
    if (!ESTADOS_VALIDOS.includes(cab.estado)) {
      falha(
        "cabeçalho",
        rel(arquivo),
        `Estado "${cab.estado}" fora da lista (${ESTADOS_VALIDOS.join(" | ")})`,
      );
    }
    if (!cab.verificado) {
      falha("cabeçalho", rel(arquivo), 'sem "Verificado contra o código em"');
    }
    if (!cab.fase) {
      falha("cabeçalho", rel(arquivo), 'sem "Fase correspondente"');
      continue;
    }

    // Carimbo de verificação vencido (§1.11 + §1.13).
    if (ESTADOS_IMPLEMENTADOS.includes(cab.estado)) {
      const data = new Date(cab.verificado);
      if (Number.isNaN(data.getTime())) {
        falha(
          "carimbo",
          rel(arquivo),
          `Estado "${cab.estado}" exige data legível (AAAA-MM-DD), veio "${cab.verificado}"`,
        );
      } else {
        const dias = Math.floor((hoje - data) / 86_400_000);
        if (dias > CARIMBO_MAX_DIAS) {
          falha(
            "carimbo",
            rel(arquivo),
            `carimbo vencido: ${dias} dias (limite ${CARIMBO_MAX_DIAS})`,
          );
        }
      }
    }

    // Trava do caminho inverso (§1.13): nenhum spec continua "especificado"
    // depois que a fase dele começa.
    if (cab.estado === "especificado" && !ehEscopoGlobal(cab.fase)) {
      const comecadas = fasesDoCabecalho(cab.fase).filter((n) =>
        iniciadas.has(n),
      );
      if (comecadas.length > 0) {
        falha(
          "trava §1.13",
          rel(arquivo),
          `Estado "especificado" mas a(s) fase(s) ${comecadas.join(", ")} já começaram no status.md`,
        );
      }
    }
  }
}

// ---------------------------------------------------------------------------
// 4. Documento por módulo (§1.4)
// ---------------------------------------------------------------------------

/** Nome do doc esperado para um membro do workspace. */
function docDoMembro(caminhoMembro) {
  const nome = basename(caminhoMembro);
  // `apps/desktop/src-tauri` é documentado como `desktop` (é a casca do app).
  if (nome === "src-tauri") return basename(dirname(caminhoMembro));
  return nome;
}

function membrosDoWorkspace() {
  const texto = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const bloco = texto.match(/members\s*=\s*\[([^\]]*)\]/s);
  if (!bloco) return [];
  return [...bloco[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function checarDocsDeModulo() {
  for (const membro of membrosDoWorkspace()) {
    const esperado = join(ROOT, "docs/modules", `${docDoMembro(membro)}.md`);
    if (!existsSync(esperado)) {
      falha(
        "doc de módulo §1.4",
        membro,
        `falta docs/modules/${docDoMembro(membro)}.md`,
      );
    }
  }
}

// ---------------------------------------------------------------------------
// 5. Links internos (§1.10)
// ---------------------------------------------------------------------------

function checarLinks() {
  const cacheAncoras = new Map();
  for (const arquivo of listarMd(ROOT)) {
    const texto = readFileSync(arquivo, "utf8");
    // Ignora blocos de código, onde caminhos de exemplo são comuns.
    const semCodigo = texto
      .replace(/```[\s\S]*?```/g, "")
      .replace(/`[^`\n]*`/g, "");
    for (const m of semCodigo.matchAll(/\[([^\]]*)\]\(([^)\s]+)\)/g)) {
      const alvo = m[2].trim();
      if (/^(https?:|mailto:|#)/.test(alvo)) continue;
      const [caminho, ancora] = alvo.split("#");
      if (!caminho) continue;
      const destino = normalize(join(dirname(arquivo), caminho));
      if (!existsSync(destino)) {
        falha("link interno", rel(arquivo), `aponta para "${alvo}" (não existe)`);
        continue;
      }
      if (ancora && destino.endsWith(".md")) {
        if (!cacheAncoras.has(destino)) {
          cacheAncoras.set(destino, ancorasDe(readFileSync(destino, "utf8")));
        }
        if (!cacheAncoras.get(destino).has(slug(decodeURIComponent(ancora)))) {
          falha(
            "âncora",
            rel(arquivo),
            `âncora "#${ancora}" não existe em ${rel(destino)}`,
          );
        }
      }
    }
  }
}

// ---------------------------------------------------------------------------

checarSpecs();
checarDocsDeModulo();
checarLinks();

const json = process.argv.includes("--json");
if (json) {
  console.log(JSON.stringify({ falhas, avisos }, null, 2));
} else {
  for (const f of falhas) {
    console.error(`FALHA [${f.check}] ${f.arquivo}: ${f.msg}`);
  }
  for (const a of avisos) {
    console.warn(`aviso [${a.check}] ${a.arquivo}: ${a.msg}`);
  }
  console.log(
    falhas.length === 0
      ? "check-docs: OK (cabeçalhos, carimbos, trava §1.13, docs de módulo, links)"
      : `check-docs: ${falhas.length} falha(s)`,
  );
}

process.exit(falhas.length === 0 ? 0 : 1);
