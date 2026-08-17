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

import { spawnSync } from "node:child_process";
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
    if (["node_modules", "target", ".git", "dist", "runtime"].includes(nome)) continue;
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

/**
 * Estado de cada fase + se a linha dela carrega a marca
 * `somente-planejamento` (ADR-0037).
 *
 * A marca vive na coluna "Evidência" e afrouxa a trava do §1.13
 * enquanto a fase tiver apenas a Etapa 1 (planejamento) fechada —
 * momento em que o código ainda não existe e `especificado` é o
 * estado verdadeiro do spec. Conferida por substring,
 * case-insensitive, mesma forma da `regra não-aplicável` do §3.5.
 */
function estadoDasFases() {
  const texto = readFileSync(join(ROOT, "docs/status.md"), "utf8");
  const fases = new Map();
  for (const m of texto.matchAll(
    /^\|\s*(\d)\s*\|[^|]*\|\s*([^|]+?)\s*\|([^|]*)\|/gm,
  )) {
    fases.set(Number(m[1]), {
      estado: m[2].trim(),
      somentePlanejamento: /somente-planejamento/i.test(m[3]),
    });
  }
  return fases;
}

function checarSpecs() {
  const fases = estadoDasFases();
  // Uma fase só "começou", para efeito da trava, quando tem código —
  // isto é, quando não está mais marcada como somente-planejamento.
  const iniciadas = new Set(
    [...fases.entries()]
      .filter(
        ([, f]) =>
          (f.estado === "em andamento" || f.estado === "concluída") &&
          !f.somentePlanejamento,
      )
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
// §1.9 — arquivos gerados
// ---------------------------------------------------------------------------

/**
 * Confere que todo arquivo gerado bate com o que seu gerador
 * produz hoje. A §1.10 já exigia isso ("um arquivo marcado como
 * gerado divergir do que o script de geração produz"); até aqui
 * a regra existia sem cobrança mecânica.
 */
function checarArquivosGerados() {
  const geradores = [
    {
      script: "scripts/generate-phase-status.mjs",
      alvo: "apps/desktop/src/generated/phase-status.ts",
    },
  ];
  for (const g of geradores) {
    const r = spawnSync(process.execPath, [join(ROOT, g.script), "--check"], {
      cwd: ROOT,
      encoding: "utf8",
    });
    if (r.status !== 0) {
      falha(
        "gerado",
        g.alvo,
        `divergiu da fonte — rode \`node ${g.script}\` e commite. ` +
          `${(r.stderr || r.stdout || "").trim()}`,
      );
    }
  }
}

/**
 * Proíbe **versão** e **fase** literais no código do frontend (§1.9).
 *
 * Motivo concreto: a tela `/sobre` anunciou "Versão 0.2.0" por
 * várias fases enquanto `tauri.conf.json` e `package.json` diziam
 * 0.1.0 — três fontes, duas mentindo. A versão exibida ao usuário
 * vem de `getAppVersion()`, que lê o binário; o estado das fases
 * vem de `generated/phase-status.ts`, derivado do `docs/status.md`.
 * Nenhum dos dois precisa de literal no `src/`.
 *
 * O casamento é deliberadamente estreito — semver de 3 partes com
 * borda de palavra — para não pegar versões de dependência, IDs,
 * datas ou strings de teste que legitimamente contenham números.
 *
 * ## Dois furos que esta guarda já teve, e que os testes fixam
 *
 * 1. **O prefixo `v` escapava.** A expressão era
 *    `(?<![\w.])\d+\.\d+\.\d+(?![\w.])`, e `v` é caractere de
 *    palavra: o lookbehind recusava o casamento logo depois dele.
 *    `"Versão 0.2.0"` era pega; `"v0.3.0"` passava — e `v` é a
 *    forma mais comum de escrever versão. O rodapé do `App.tsx`
 *    exibiu `v0.3.0` por cinco fases com a guarda instalada e
 *    verde.
 * 2. **Fase literal não era coberta.** A mesma linha do rodapé
 *    dizia "(Fase 3: Motor de execução e ferramentas)" com a Fase 7
 *    concluída. Nenhuma expressão de versão pegaria isso.
 */
function checarVersaoLiteralNoFrontend() {
  const DIR = join(ROOT, "apps/desktop/src");
  if (!existsSync(DIR)) return;
  // `[vV]?` fecha o furo 1 sem alargar o casamento: o lookbehind
  // continua exigindo borda antes do prefixo, então `rev1.2.3` e
  // `div1.2.3` seguem fora.
  const SEMVER = /(?<![\w.])[vV]?\d+\.\d+\.\d+(?![\w.])/;
  // Fecha o furo 2. Exige dígito depois de "Fase" — `interface Fase`
  // e `"Fase de Ligação"` (que existe no gerado) não casam.
  const FASE_LITERAL = /\bFase\s+\d+/;

  const arquivos = [];
  (function varrer(dir) {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name);
      if (e.isDirectory()) varrer(p);
      else if (/\.(ts|tsx)$/.test(e.name)) arquivos.push(p);
    }
  })(DIR);

  for (const arquivo of arquivos) {
    const linhas = readFileSync(arquivo, "utf8").split("\n");
    linhas.forEach((linha, i) => {
      // Comentário citando o histórico ("chegou a anunciar 0.2.0")
      // é documentação, não fonte da verdade — e é justamente onde
      // este gate quer que a explicação viva.
      const t = linha.trim();
      if (t.startsWith("*") || t.startsWith("//") || t.startsWith("/*")) return;
      const m = linha.match(SEMVER);
      if (m) {
        falha(
          "versão-literal",
          rel(arquivo),
          `linha ${i + 1}: número de versão literal "${m[0]}" no frontend. ` +
            `A versão vem de \`getAppVersion()\` (fonte: tauri.conf.json), ` +
            `nunca escrita à mão (REGRAS §1.9).`,
        );
      }
      const f = linha.match(FASE_LITERAL);
      if (f) {
        falha(
          "fase-literal",
          rel(arquivo),
          `linha ${i + 1}: afirmação de fase literal "${f[0]}" no frontend. ` +
            `O estado das fases vem de \`generated/phase-status.ts\`, ` +
            `derivado do \`docs/status.md\` (REGRAS §1.9). ` +
            `Se a menção for histórica, ela vive em comentário.`,
        );
      }
    });
  }
}

// ---------------------------------------------------------------------------

checarSpecs();
checarDocsDeModulo();
checarLinks();
checarArquivosGerados();
checarVersaoLiteralNoFrontend();

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
      ? "check-docs: OK (cabeçalhos, carimbos, trava §1.13, docs de módulo, links, arquivos gerados, versão literal)"
      : `check-docs: ${falhas.length} falha(s)`,
  );
}

process.exit(falhas.length === 0 ? 0 : 1);
