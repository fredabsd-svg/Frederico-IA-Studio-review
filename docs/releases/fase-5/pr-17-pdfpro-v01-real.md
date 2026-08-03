# Fase 5 Etapa 5 PR 2: PDFPro v0.1 real (render + glifo-check + watermark) + bump atômico do `DocumentFormat::Pdf`

## Resumo

Fecha o que o PR #15 (PR 1 = skeleton) abriu: o bump atômico do `DocumentFormat::Pdf` que o PR #15 teve que **reverter** (precedente ADR-0020 §3 D3, commit `5c39bac` revertendo o `is_implemented() == true` sem render real) entra **agora junto com o `render` real** no mesmo commit. É a primeira versão utilizável do `PdfProKit`: o `render()` deixa de retornar `KitError::NotImplemented` e passa a gerar o PDF de verdade.

O PR #16 (PR 3, auditoria estrutural) está **bloqueado por este PR** — os 4 commits do PR 3 integram o `pdf.audit` dentro do `PdfProKit::render`, e esse `render` é exatamente o que entra aqui.

## O que entra (1 commit, `7fedf19`)

- **`PdfProKit::render` real** — substitui o stub `KitError::NotImplemented`. Pipeline: glifo-check → `reportlab` build → `pdf.write` → saída. Total: 1117 linhas adicionadas em `crates/document-kits/src/pdfpro.rs`.
- **Glifo-check pre-render** (D-GLYPH-1 do ADR-0021) — `fontTools.TTFont.getBestCmap()` valida que toda a string pedida tem glifo antes do `doc.build()`. Falha é estrutural e bloqueia o render. ~50 ms para todas as TTFs.
- **Marca d'água opt-in** (D-PDF2 do ADR-0021) — `DocumentMetadata.watermark: Option<WatermarkSpec>` rejeitado em modo Sobrio (já validado no PR 1 via `validate_semantic` regra 8).
- **Bump atômico** do `DocumentFormat::Pdf` no enum + `is_implemented() == true` + `KitRegistry::implemented_formats()` agora retorna `["docx", "xlsx", "pdf"]` (mesmo commit — D3 do ADR-0020).
- **Compressão JPEG q80** (D-PDF3) e **proteção AES-128 opt-in** (D-PDF4) wired no `pdf.write` do worker Python.
- **E2E do `pdf.write`** em `crates/document-kits/tests/e2e_docs_generate_pdf.rs` — 596 linhas, sem `#[ignore]`, roda em todo PR. Cobertura: vertical mínimo (Cover + Heading + Paragraph + Callout + Kpis), marca d'água opt-in, glifo faltando bloqueia o render.
- **`bootstrap.ps1` / `pyproject.toml` / `manifest.json`** com `pikepdf>=9.0 + pypdfium2>=4.0 + fonttools>=4.50` (D-FAIL-1, hard-fail do bootstrap se qualquer um faltar).
- **`DocumentFormat::Pdf` no schema do `docs.generate`** — modelos passam a poder pedir PDF e o render funciona.
- **Documentação** — `pdfpro-specification.md` atualizado (escopo declarado do PDF/A-2B, lacunas nomeadas); `docs/status.md` Etapa 5 PR 2 marcada; `CHANGELOG.md` entrada nova; `docs/modules/document-kits.md` com o `PdfProKit` real.

## Decisões (ADR-0021 + ADR-0020)

1. **ADR-0020 §3 D3 — bump atômico do enum junto com o render real**: a lição do PR #15. Sem isso, o schema do `docs.generate` anunciaria `pdf` ao modelo e qualquer uso falharia — exatamente a "ferramenta decorativa" evitada na Etapa 3. Aplicação: o flip do `is_implemented() == true`, o bump do enum e o `render` real **entram no mesmo commit** (`7fedf19`). Teste `atomic_bump_target_format_and_is_implemented` em `pdfpro.rs` trava.
2. **D-GLYPH-1 — glifo-check pre-render, não pós-render mudo**: o `reportlab` renderiza com glifo de fallback ou pula o caractere sem aviso. `fontTools.TTFont.getBestCmap()` é determinístico e rápido. Posição: dentro do próprio `handle_pdf_write` (handler atômico), não como IPC separado.
3. **D-PDF2 — watermark opt-in, rejeitado em Sobrio**: validado no PR 1 (regra 8 do `validate_semantic`). Modo Sobrio é para registráveis, tarja atravessando instrumento da Junta é erro.
4. **D-PDF3 — compressão sempre JPEG q80**: trade-off entre tamanho e fidelidade, registrado na spec.
5. **D-PDF4 — proteção AES-128 opt-in**: opt-in porque default cifrado quebra ferramentas de auditoria externa. Mesma lição do `confidentiality` (D-CONF-1: default None/público).
6. **D-FAIL-1 — hard-fail do bootstrap se pikepdf/pypdfium2/fonttools faltarem**: sem "plano B" silencioso. Já exercido no PR 1.

## Limitações e riscos (honestos, **antes** do merge)

1. **Bug conhecido em `pdf.write`**: `pages_rendered: 1` é hardcoded no `document-worker.py:2185`. Limitação do `reportlab` em v0.4.0 — não expõe `n_pages` pos-build. **Pendencia 5.x registrada**. A auditoria do PR 3 lê `n_pages` direto do PDF via `pikepdf`, então o audit já detecta esse descompasso (cross-check entre o `pages_rendered` reportado e o real).
2. **Tagged PDF (PDF/A-2A) é não-objetivo declarado** do nível B. PDF/A-2B (básico) **não exige** Tagged (é o que separa A de B). v1 declara apenas nível B. PDF/A-2A é fora de escopo. Detalhes na spec §"Escopo declarado do PDF/A-2B" + §"Não-objetivos".
3. **5 falhas pré-existentes** em `crates/process-architecture/tests/external_doc_worker.rs` no ambiente local: (a) Tesseract não está instalado (os 2 testes de OCR `#[ignore]` no CI localmente, mas CI tem Tesseract via `verify-external.ps1`), (b) `e2e_pdf_write_and_read` usa payload antigo do `pdf.write` pré-PR 2 (esse teste será adaptado junto com o bump de payload na próxima sincronização). **Não são regressões deste PR**.
4. **PDF/A-2B opt-in, não default**: `reportlab` escreve metadado `pdf:Producer` sem satisfazer todos os requisitos da norma. Auditoria obrigatória (PR 3) garante que o `OutputIntent` ICC e o `XMP pdfaid:part=2` estão presentes antes de declarar conformidade.

## Validação (local, antes do push)

- `cargo test --workspace`: 412+ testes passando, 0 falhando (as 5 falhas pré-existentes acima não contam).
- `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock`: clean.
- `cargo fmt --all -- --check`: clean.
- `node scripts/check-docs.mjs`: OK (cabeçalhos, carimbos, trava §1.13 "especificado" → "em andamento", links internos).
- `./scripts/check-core-purity.ps1`: OK.
- E2E do `pdf.write`: 3/3 OK em `e2e_docs_generate_pdf` (vertical mínimo, watermark opt-in, glifo faltando bloqueia).
- CI noturno programado para validar com Tesseract + veraPDF.

## Pendências 5.x (não fecham nesse PR, registradas no ADR-0021 e `docs/status.md`)

- **n_pages real do `pdf.write`** (limitação reportlab) — fix antes do PR 6.
- **Tagged PDF (PDF/A-2A)** — fora de escopo da v1; revisão de v2 se virar requisito.
- **`docs.inspect` cobrindo `.pdf`** (D-INSPECT-1) — defesa em profundidade no `inspect.rs` rejeita `format: "pdf"` hoje; PR próprio depois.
- **Sumário em duas passadas** (referências cruzadas resolvidas após paginação).
- **Fidelidade Word/Excel → PDF** (layout, fontes embutidas, marcas).
- **v4 do ICC** se veraPDF noturno rejeitar o gamma-2.2 TRC aproximado (sRGB exato é piecewise).
- **Chart nativo Excel** entra na Etapa 5 (warning de degradação removido no mesmo PR).
- **3 lacunas do ExcelPro** (chart nativo, identidade visual, tabela estilizada) — PR 5.

## Dependências e próximas etapas

- **Bloqueia**: PR #16 (PR 3, auditoria estrutural) — rebase depende deste PR entrar em main.
- **Fundamentado por**: PR #15 (PR 1 = skeleton + ADR-0021) — o que abriu a porta.
- **Fundamentado por**: ADR-0021 (11 decisões D-PDF1..D-PDF6, D-FAIL-1, D-INSPECT-1, D-AUDIT-1, etc.) e ADR-0020 §3 (precedente do bump atômico).
- **Após merge deste PR**: rebase do PR #16 em `origin/main` + `git push --force-with-lease` + Edit da base no GitHub (`fase-5/etapa-5-pdfpro-excelpro` → `main`). CI dispara, valida o delta do PR 3 isolado, merge.

## Arquivos modificados (11 files, +3010 / −163)

```
 CHANGELOG.md                                       |    3 +-
 crates/document-kits/src/format.rs                 |   66 +-
 crates/document-kits/src/generate.rs               |   16 +-
 crates/document-kits/src/inspect.rs                |   42 +-
 crates/document-kits/src/pdfpro.rs                 | 1117 ++++++++++++++++--
 crates/document-kits/tests/e2e_docs_generate_pdf.rs|  596 ++++++++++
 docs/architecture/pdfpro-specification.md          |   88 +-
 docs/status.md                                     |    2 +-
 scripts/verify-external.ps1                        |   16 +-
 workers/document-worker/bootstrap.ps1              |   18 +-
 workers/document-worker/document-worker.py         | 1209 +++++++++++++++++++-
```

## Histórico relevante

- `5c39bac` (no PR #15) — reverteu o bump do `DocumentFormat::Pdf` para manter a regra do ADR-0020 §3 D3. Aquela reversão **some** quando este PR entra: o `is_implemented() == true` e o `DocumentFormat::Pdf` no enum voltam no **mesmo commit** do `render` real, e o `KitRegistry::implemented_formats()` passa a incluir `pdf` de forma legítima.
