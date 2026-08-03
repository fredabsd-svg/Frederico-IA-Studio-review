# docs: promove Fase 5 (Documentos) a `concluída` no `status.md`

## Resumo

Fecha oficialmente a Fase 5 (Documentos) no `docs/status.md`. A Etapa 5 (PDFPro completo) já fechou com o PR #19 (`0e3471f`) e o CI daquele PR já foi verde — a única coisa que faltava era a promoção formal no `status.md` pra satisfazer a regra de promoção:

> §"Regra de promoção" do `status.md`: promover uma fase de `em andamento` pra `concluída` exige, simultaneamente: (1) suíte de testes 100% verde; (2) specs com Estado `parcialmente implementado`/`implementado` + carimbo recente; (3) entrada em `CHANGELOG.md`; (4) referência ao PR/commit.

Todos atendidos:

| # | Requisito | Status |
|---|-----------|--------|
| 1 | Suíte workspace 100% verde | ✅ (PR #19 CI run `30758984691` SUCCESS) |
| 2 | Specs com Estado atualizado + carimbo recente | ✅ `pdfpro-specification.md` (parcialmente implementado, 2026-08-01), `excelpro-specification.md` (parcialmente implementado, 2026-07-31), `process-architecture.md` (parcialmente implementado, 2026-08-01), `docs/modules/document-kits.md` (implementado, 2026-08-01) |
| 3 | CHANGELOG.md descrevendo o efeito | ✅ (entrada do PR #19 já feita) |
| 4 | Referência ao PR/commit | ✅ (PR #19 / `0e3471f`) |

## Mudanças

### 1. `docs/status.md` — linha da Fase 5

- **Status:** `em andamento` → **`concluída`**
- **Evidência:** reescrita focando no PR #19 (auditoria estrutural bloqueante do §19.4) + referenciando os PRs #11/12/13/14/15/17 que compõem as Etapas 2B/3/4/5 anteriores. A linha ficou de ~40 KB pra ~4 KB — a evidência detalhada de cada sub-Etapa vive nos commits e nos specs (não precisa ser duplicada aqui).
- **Pendências:** 8 pendências 5.x nomeadas (não bloqueiam a promoção porque estão registradas):
  1. n_pages real do `pdf.write` (limitação reportlab em v0.4.0)
  2. Tagged PDF (PDF/A-2A) — explicitamente fora de escopo declarado do A-2B
  3. `docs.inspect` cobrindo `.pdf` (D-INSPECT-1)
  4. Sumário em duas passadas
  5. Fidelidade Word/Excel → PDF
  6. v4 do ICC (parametrizado, se veraPDF noturno rejeitar TRC aproximado)
  7. Cache persistente da auditoria (D-AUDIT-1 só define a key)
  8. 3 lacunas do ExcelPro (chart nativo, identidade visual, tabela estilizada) — PR 5 da Etapa 5

### 2. `docs/architecture/process-architecture.md`

- **Carimbo:** `2026-07-30` → **`2026-08-01`**
- **Resumo da Fase 5:** atualizado pra mencionar Etapa 5 PR 2 (PDFPro v0.1 real) + Etapa 5 PR 3 (auditoria estrutural) + Schema bump `SpecVersion` 0.2.0 → 0.3.0. **8 handlers** no `document-worker` v0.4.0 (era 6).

### 3. `docs/modules/document-kits.md`

- **Carimbo:** `2026-07-31` → **`2026-08-01`**
- **Fase correspondente:** agora cobre Etapa 5 PR 1 + PR 2 + PR 3
- **Resumo:** atualizado com decisões-chave — bump atômico do enum, glifo-check pre-render, marca d'água opt-in, auditoria bloqueante, sRGB ICC v2 local, Tagged PDF fora de escopo do A-2B.

## Sagas de processo registradas (na evidência do `status.md`)

Pra ficar claro pro próximo que ler daqui a 6 meses, a linha da Fase 5 lista as 6 sagas que apareceram durante a Etapa 5:

- **PR #16** mergeada na branch errada (sem trocar base pra main) — resolvida via PR #19 com `git checkout <SHA-delta> -- .` workaround
- **PR #7** (Etapa 2A) — deadlock `Arc<Mutex<Box<dyn Pipe>>>` — resolvido com redesign do `WorkerManager` como ator (ADRs 0015 + 0016)
- **`5c39bac`** (PR #1) — bump decorativo do enum sem `render` real — revertido no PR 1, bump correto entrou no PR #17 (precedente do ADR-0020 §3 D3)
- **`sRGB2014.icc`** do color.org 404 — resolvido com gerador local via IEC 61966-2-1:1999 + ISO 15076-1 (SHA-256 pinado)
- **`e2e_pdf_write_and_read`** com formato `sections` antigo (pré-PR #2) — adaptado pra `blocks` (PR #17 commit `c11c39b`)
- **`cargo fmt`** — rustfmt do CI é mais novo que o local; resolver com `cargo fmt --all` sempre (PR #19 commit `9c31fcb`)

## Lição de processo registrada em user memory

A saga da PR #16 (mergeada na branch errada) foi a 2ª vez (depois da PR #7) que PRs empilhadas deram problema. O usuário pediu pra virar regra, e foi adicionada ao user memory:

> "PRs empilhadas: só abre a próxima PR depois que a anterior entrou em main — não confiar no rebase dance. 2 vezes (PR #7, PR #16) com custo real (PR MERGED mas delta invisível, CI não roda). Workaround: branch nova de main + `git checkout <SHA-delta> -- .` + commit."

## Validação

- `node scripts/check-docs.mjs`: **OK** (cabeçalhos, carimbos, trava §1.13, docs de módulo, links)
- `git diff --stat`: 3 files changed, 6 insertions(+), 11 deletions(-) (PR de docs-only, sem mudança de código)
- CI vai rodar de novo (docs-only PR; deve passar rápido — só o `verify (Windows)` + `check-docs.mjs` exercitam docs)

## Próximo

- **Merge com squash** (mesma prática do PR #19)
- Depois: abrir o trabalho da **Fase 6 (Multimodelo e subagentes)** OU continuar com os itens da Etapa 5.x pendentes (PR 4: auditoria visual via pypdfium2; PR 5: 3 lacunas ExcelPro; PR 6: n_pages real do pdf.write + veraPDF no ci-nightly)
- Decisão de Fase 5.x (extensão da Fase 5) vs Fase 6 (próxima fase) é do usuário — `docs/status.md` agora marca Fase 5 como concluída, então Fase 5.x vira trabalho de "próxima fase" (Fase 5.5 ou Fase 6.x, dependendo do plano do PROMPT MESTRE)

## Arquivos modificados (3 files, +6 / −11)

```
 docs/architecture/process-architecture.md  |  4 ++--
 docs/modules/document-kits.md              | 11 +++--------
 docs/status.md                             |  2 +-
```

(3 dos `pr*-description.md` untracked ficaram de fora — não são parte do PR.)
