# PR 3 (Etapa 5) do PDFPro: traz a auditoria estrutural do §19.4 (D-PDF5 + D-PDF6) para `main`

## Contexto (transparente)

A PR original **#16 (Fase 5 Etapa 5 PR 3)** abriu com base em `fase-5/etapa-5-pdfpro-excelpro` (a branch do PR 2, ainda não mergeado em main na hora) — prática de PR empilhada padrão do projeto. Aconteceram dois fatos em sequência:

1. PR #17 (PR 2 do PDFPro: render real) entrou em main (merge commit `6547a66`).
2. PR #16 foi mergeado **sem trocar a base** — foi mergeado em `fase-5/etapa-5-pdfpro-excelpro`, não em `main` (merge commit `1dc47b4` na branch errada). Por isso o delta do PR 3 nunca chegou em main e o CI end-to-end do PR 3 nunca rodou.

Esta PR (#18) é a **correção honesta**: pega o conteúdo do PR 3 (que já foi mergeado em `1dc47b4` na branch temporária) e o leva em direção a `main`, onde deveria ter ido originalmente. O diff é exatamente o delta do PR 3 — **não duplica nada do PR 2** (já está em main como `6547a66`).

**Como evitar a repetição do problema** (registrado em lição de processo): "PR empilhada tem que ter a base trocada para main antes do merge, ou a PR anterior não pode ser deletada. Regra simples: só abra a próxima PR depois que a anterior entrou no main." É a 2ª vez (PR #7 Etapa 2A e agora #16 Etapa 5 PR 3), então vira regra, não exceção.

## O que entra (mesmo conteúdo da PR #16 original)

- **Capability `pdf.audit`** no `document-worker` v0.4.0 (D-PDF5 + D-PDF6 do ADR-0021). Auditoria estrutural bloqueante do §19.4 do PROMPT MESTRE — sempre roda após o `pdf.write`, sem interruptor (§19.6): falha do audit = artefato NÃO é entregue.
- **Capability name `pdf.audit`** (sem terceiro nível, todos os outros capabilities seguem `<dominio>.<verbo>`). O PR 4 estende este mesmo handler com `kind="visual"` (pypdfium2 rasteriza + checa grade, sobreposição, página vazia, alinhamento). O `salvar()` faz uma chamada só porque §19.6 exige auditoria inteira.
- **Checks baseline (sempre)**: abertura sem exception, n_pages ≥ 1, DocInfo populado (Author/Title/Producer/Creator), todas as fontes embutidas (FontFile* em FontDescriptor), sem referências externas (URL em /A /URI, /EmbeddedFiles com /F), sem cifragem (PDF/A-2B proíbe).
- **Checks A-2B opt-in** (quando `pdfa: Some(PdfA2b)`): OutputIntent com ICC profile RGB válido (D-PDF6 — lê bytes via `read_bytes()` que descomprime FlateDecode; valida signature `acsp` no offset 36 e color space `RGB ` no offset 16), XMP com `pdfaid:part=2` e `pdfaid:conformance=B`, sem JavaScript em /OpenAction, /AA, /Names/JavaScript.
- **Cache key (D-AUDIT-1)** = `sha256(pdf_bytes) + ":" + AUDIT_RULES_VERSION` (v0.1.0 no PR 3). Cache persistente fica pra PR próprio — misturar correção com otimização faz bug de cache virar bug de auditoria.
- **19 testes Python** em `workers/document-worker/tests/test_pdf_audit.py` (5+ negativos injetando falha). Cobre baseline + A-2B + cross-cutting (path_traversal, kind_unsupported, missing_path, etc.).
- **Schema bump `SpecVersion` 0.2.0 → 0.3.0** (MINOR, backward-compat). `DocumentMetadata` ganha `pdfa: Option<PdfaSpec>` (opt-in PDF/A-2B). `PdfaFlavor` enum (`PdfA2b` na v1) + `PdfaSpec` struct. `prompt.rs` (SEMANTIC_RULES + MODE_HEADER + SPEC_INTRO) e `validate.rs` atualizados pro 0.3.0.
- **`KitError::AuditFailed` novo variant** no `kit.rs` (D-PDF5): auditoria bloqueante do §19.6 mapeia falhas do `pdf.audit` pra este erro. Artefato NÃO é entregue quando a auditoria falha.
- **`PdfProKit::render` chama `pdf.audit` após `pdf.write`**. Sucesso popula `KitOutput.extra.audit` com `rules_version`, `coverage`, `cache_key`, `checks`. Falha retorna `KitError::AuditFailed` com `code + message + lista de checks que falharam`.
- **`DocsGenerateTool` (generate.rs)** trata `AuditFailed` formatando a mensagem pro modelo com a lista de checks (`code`, `expected`, `got`) — o caller pode dizer ao usuário exatamente o que foi (fonte não embedded, PDF cifrado, falta OutputIntent, etc.).
- **sRGB ICC v2 (D-PDF6)** gerado pelo `tools/generate_srgb_icc.py` (211 linhas, determinístico) a partir dos parâmetros sRGB D50-adaptados (IEC 61966-2-1:1999) e formato ICC v2 (ISO 15076-1) — ambos padrões internacionais públicos. SHA-256 `C4188E5C06585DDAD4F8781B8F8791BB4563874AC462C934D1420EA133D74B64` (526 bytes) pinado em `bootstrap.ps1` (hard-fail se divergir, D-FAIL-1).
- **Correção factual no ADR-0021**: D-PDF5 + "Mais difícil" + "Pendências #8" diziam "PDF/A-2B exige Tagged" — **errado**. Tagged é o que separa A (accessible) de B (basic); A-2B não exige. v1 declara apenas nível B; Tagged vai pra "não-objetivo" do spec (decisão, não lacuna). PDF/A-2A (Tagged, accessible) fica como pendência 5.x.
- **E2E do kit (3/3 verde)**: `e2e_docs_generate_pdf_full_vertical`, `_watermark_opt_in`, `_missing_glyph_blocks` passam pelo render + audit. O `pdf.audit` roda dentro do render — não precisa de teste E2E separado.
- **Atualização documental** (REGRAS §1.3 e §1.13):
  - `pdfpro-specification.md`: Tagged reclassificado de "lacuna" para "não-objetivo". Nova seção "Escopo declarado do PDF/A-2B" lista o que nível B verifica e o que não verifica. Carimbo "Verificado contra o código em: 2026-08-01".
  - `docs/modules/document-kits.md`: seção "Etapa 5 (parcialmente fechada)" com PdfProKit v0.1 + pdf.audit integration + KitError::AuditFailed + schema bump 0.3.0.
  - `docs/status.md`: Fase 5 recebe evidência do PR 3. Fase 5 continua "em andamento" (PRs 4-6 fecham).
  - `CHANGELOG.md`: entrada "Não publicado" pro PR 3.
  - `workers/document-worker/README.md`: 8 handlers, v0.4.0, com `pdf.audit` na tabela de capabilities + parágrafo descritivo do handler.

## Decisões (ADR-0021 + ADR-0020)

1. **D-PDF5 — auditoria bloqueante sem interruptor (§19.6)**: falha do audit = `KitError::AuditFailed`, artefato NÃO é entregue. **Não existe "audit passou com warning"** — passa ou falha. Mesma lição do `interruptor_do_audit` que foi proibido na revisão: a auditoria é o controle que torna o render confiável, e o controle não pode ser desligado sem tornar a confiança opcional.
2. **D-PDF6 — ICC gerado localmente, não versionado**: `sRGB2014.icc` do color.org está 404 (verificado 2026-08-01). Gerar localmente é o caminho determinístico. Licença: IEC 61966-2-1 (padrão público) + ISO 15076-1 (formato público) = ICC profile é public domain na prática. Bump do ICC = bump do `AUDIT_RULES_VERSION` (D-AUDIT-1).
3. **D-AUDIT-1 — cache key com versão da regra, não só hash do .pdf**: senao, ao apertar regra, arquivos antigos passam pelo cache antigo. v0.1.0 no PR 3.
4. **D-GLYPH-1 — bug real do `pdf.write` pego pelo cross-check**: o handler retorna `pages_rendered=1` como chute (limitação do `reportlab` em v0.4.0 — não expõe n_pages pos-build). **Cross-check `expected_pages` removido do payload do `pdf.audit`**: a auditoria lê n_pages direto do PDF via pikepdf, que é a fonte da verdade. Bug fica como pendência 5.x (n_pages real do write), mas a auditoria não é desativada por causa disso.
5. **D-FAIL-1 — hard-fail do bootstrap se pikepdf/pypdfium2/fonttools faltarem**: agora pikepdf é dep obrigatória (era opt-in no PR 2). `bootstrap.ps1` valida o SHA-256 do ICC com hard-fail.
6. **Capability name `pdf.audit` com `kind` como argumento, não `pdf.audit.structural`**: mantém a gramática `<dominio>.<verbo>`. O PR 4 estende o mesmo handler com `kind="visual"`. `salvar()` faz uma chamada só — §19.6 não admite meia auditoria.
7. **pikepdf 10.x API**: `Stream.read_bytes()` descomprime; o antigo `get_data()` (pikepdf 8) foi removido. `read_raw_bytes()` retorna bytes brutos pós-compressão. Pra validar ICC embedded no OutputIntent, sempre `read_bytes()` (depois descomprime).
8. **Tagged PDF reclassificado como "não-objetivo" declarado do nível B**: PDF/A-2B (básico) **não exige** Tagged — é o que separa A de B. v1 declara apenas nível B. PDF/A-2A (Tagged, accessible) está fora de escopo. **Quem ler daqui a 6 meses e tentar "consertar" adicionando Tagged ao A-2B vai estar violando a norma** (a própria ISO 19005-2).

## Limitações e riscos (honestos)

1. **Bug conhecido em `pdf.write`**: `pages_rendered: 1` é hardcoded (`document-worker.py:2185`). Limitação do reportlab. **Pendencia 5.x**. A auditoria lê n_pages direto do PDF, então o bug é "escondido" — mas o ideal é consertar o `pdf.write`.
2. **PDF/A-2A (Tagged) está fora do escopo declarado do A-2B.** Pendência 5.x se virar requisito.
3. **Cache persistente da auditoria NÃO está implementado.** D-AUDIT-1 só define a key. PR próprio fecha.
4. **veraPDF no ci-nightly** valida PDFs com opt-in. PR 6 fecha.
5. **5 falhas pré-existentes** em `external_doc_worker.rs` no ambiente local: (a) Tesseract não instalado (os 2 testes de OCR pulados localmente; CI tem via `verify-external.ps1`); (b) `e2e_pdf_write_and_read` antes usava payload antigo do `pdf.write` pré-PR 2 — **corrigido no `c11c39b` que já entrou no PR 17** (e está no `6547a66` em main). **Não há regressão do PR 3 nos testes do `external_doc_worker.rs`.**

## Validação (local, antes do push)

- `cargo test --workspace`: 412+ testes passando, 0 falhando.
- `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock`: clean.
- `cargo fmt --all -- --check`: clean.
- `node scripts/check-docs.mjs`: OK (cabeçalhos, carimbos, trava §1.13, links internos).
- `./scripts/check-core-purity.ps1`: OK.
- E2E do `pdf.write` (3/3 OK em `e2e_docs_generate_pdf`): `full_vertical`, `watermark_opt_in`, `missing_glyph_blocks`.
- 19/19 testes Python em `tests/test_pdf_audit.py`.
- **CI ainda não rodou pra esse delta específico do PR 3** (a PR 16 original não rodou CI porque base era branch errada). Esta PR #18 é a primeira vez que o delta do PR 3 vai ser exercitado end-to-end em CI. **Por isso a importância de usar SQUASH no merge**: preserva a atomicidade do PR 3 e evita duplicação do PR 2.

## Pendências 5.x (não fecham nesse PR, registradas no ADR-0021)

- **n_pages real do `pdf.write`** (limitação reportlab).
- **Tagged PDF (PDF/A-2A)** — fora de escopo declarado.
- **`docs.inspect` cobrindo `.pdf`** (D-INSPECT-1).
- **Sumário em duas passadas** (referências cruzadas).
- **Fidelidade Word/Excel → PDF** (layout, fontes, marcas).
- **v4 do ICC** se veraPDF noturno rejeitar o gamma-2.2 TRC aproximado.
- **Cache persistente da auditoria** (D-AUDIT-1).
- **3 lacunas do ExcelPro** (chart nativo, identidade visual, tabela estilizada) — PR 5.

## Instrução de merge

**MERGE COM SQUASH** (não "Merge commit", não "Rebase and merge"). Razão: o source branch contém o PR 2 (já em main como `6547a66`); se o merge preservar os commits individuais, o main ficaria com o PR 2 duas vezes em SHAs diferentes (7fedf19 pré-squash + 6547a66 já em main) — e a evidência que `docs/status.md` cita deixa de ser confiável. Com squash, o main recebe **um commit só** com o delta real do PR 3.

## Arquivos modificados (21 files, +1847 / −94)

```
 CHANGELOG.md                                                  |    1 +
 crates/document-engine/src/lib.rs                             |    4 +-
 crates/document-engine/src/prompt.rs                          |   14 +-
 crates/document-engine/src/spec.rs                            |   63 ++-
 crates/document-engine/src/validate.rs                        |   12 +-
 crates/document-kits/src/generate.rs                          |   18 +
 crates/document-kits/src/kit.rs                               |   18 +
 crates/document-kits/src/pdfpro.rs                            |  100 +++-
 crates/document-kits/src/wordpro.rs                           |    8 +-
 crates/document-kits/tests/e2e_docs_generate_pdf.rs           |    1 +
 docs/architecture/pdfpro-specification.md                     |   97 +++-
 docs/decisions/0021-fase-5-etapa-5-pdfpro-excelpro.md         |  136 +++++-
 docs/modules/document-kits.md                                 |   42 ++
 docs/status.md                                                |    2 +-
 workers/document-worker/README.md                             |   28 +-
 workers/document-worker/bootstrap.ps1                         |  105 +++-
 workers/document-worker/document-worker.py                    |  521 +++++++++++++++++++-
 workers/document-worker/manifest.json                         |   16 +-
 workers/document-worker/pyproject.toml                        |    4 +-
 workers/document-worker/tests/test_pdf_audit.py               |  540 +++++++++++++++++++++
 workers/document-worker/tools/generate_srgb_icc.py            |  211 ++++++++
```

## Histórico relevante

- PR #15 (PR 1) — skeleton + ADR-0021 — merged.
- PR #17 (PR 2) — render real + bump atômico do `DocumentFormat::Pdf` — merged em main (`6547a66`).
- PR #16 (PR 3 original) — mergeado em `fase-5/etapa-5-pdfpro-excelpro` por engano (base não trocada) — `1dc47b4`.
- **Esta PR (#18)** — leva o delta do PR 3 (já validado localmente e com a doc ok) para main com squash.
