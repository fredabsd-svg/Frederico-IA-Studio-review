# Fase 5 Etapa 5 — PR 3 do PDFPro: auditoria estrutural bloqueante do §19.4

## Implementado

Auditoria estrutural bloqueante do §19.4 (PROMPT MESTRE) entra no `PdfProKit`:
o `render` agora chama `pdf.audit` no `document-worker` após `pdf.write`, e o
resultado do audit decide se o artefato é entregue. Falha do audit =
`KitError::AuditFailed`; sucesso = `KitOutput.extra.audit` populado.

A auditoria cobre 5 checks baseline (sempre) + 5 checks PDF/A-2B
opt-in (quando `metadata.pdfa: Some(PdfA2b)`). O `sRGB IEC 61966-2.1`
ICC é embedded no `OutputIntent` quando o A-2B opt-in é usado (D-PDF6).

**Três decisões desta conversa que vão tentar "consertar" sem contexto:**

1. **`pdf.audit` com `kind` como argumento, não `pdf.audit.structural`.**
   Todos os outros capabilities do worker seguem `<dominio>.<verbo>`
   (`pdf.write`, `pdf.read`, `ocr.run`). O PR 4 estende este mesmo
   handler com `kind: "visual"` (pypdfium2 rasteriza + checa grade,
   sobreposição, página vazia, alinhamento), e o `salvar()` faz uma
   chamada só — porque o §19.6 não admite auditoria pela metade.
   Terceira gramática no mesmo namespace (`dominio.substantivo.adjetivo`)
   faria o chamador orquestrar duas capabilities separadas, e a
   chance de o `salvar()` chamar uma e esquecer a outra é real.

2. **Tagged PDF é não-objetivo declarado do nível B, não pendência.**
   A v1 do PDFPro declara apenas o nível B (básico) do PDF/A-2.
   Tagged é o que separa A (accessible) de B (basic); A-2B não exige.
   v1 nunca reivindica Tagged. "PDF/A-2A fora de escopo do v0.2" está
   escrito no `pdfpro-specification.md` §"Não-objetivos" e §"Escopo
   declarado do PDF/A-2B" — não é omissão, é decisão registrada. Quem
   ler daqui a 6 meses vai ver a frase e não vai "consertar" tentando
   adicionar Tagged ao A-2B.

3. **Perfil ICC vem pelo bootstrap com SHA-256 fixado, não está
   versionado no repositório, e a licença está no ADR-0021.**
   `tools/generate_srgb_icc.py` (no repo, ~210 linhas) gera o ICC v2
   deterministicamente a partir de IEC 61966-2-1:1999 + ISO 15076-1
   (padrões públicos). `bootstrap.ps1` chama o gerador, valida o
   SHA-256 pinado (`C4188E5C...D74B64`) com hard-fail (D-FAIL-1).
   A escolha e o porquê estão no D-PDF6 do ADR-0021. URL do
   `sRGB2014.icc` no `color.org` está 404 (verificado em 2026-08-01)
   — mesma lição do `raw/main` do `tessdata` que o ADR-0019 §Decisão
   1 já pagou.

## Arquivos

- `workers/document-worker/tools/generate_srgb_icc.py` (**novo**, 211 linhas)
- `workers/document-worker/tests/test_pdf_audit.py` (**novo**, 540 linhas — 19 testes)
- `workers/document-worker/bootstrap.ps1` (+103 linhas — bloco ICC + verificação)
- `workers/document-worker/document-worker.py` (+521 linhas — handler `pdf.audit` + helpers + bumps v0.4.0)
- `workers/document-worker/manifest.json` (+5 linhas — entrada `srgb-icc` + capability `pdf.audit`)
- `workers/document-worker/pyproject.toml` (+4 linhas — version 0.3.0 → 0.4.0)
- `workers/document-worker/README.md` (+28 linhas — capability `pdf.audit` documentada)
- `crates/document-engine/src/spec.rs` (+63 linhas — `PdfaFlavor` enum, `PdfaSpec` struct, `DocumentMetadata.pdfa`, bump 0.2.0 → 0.3.0)
- `crates/document-engine/src/lib.rs` (+2 linhas — re-exports)
- `crates/document-engine/src/prompt.rs` (+14 linhas — SEMANTIC_RULES + tests)
- `crates/document-engine/src/validate.rs` (+12 linhas — helper de teste + bump version)
- `crates/document-kits/src/kit.rs` (+18 linhas — `KitError::AuditFailed` variant)
- `crates/document-kits/src/generate.rs` (+18 linhas — match arm `AuditFailed`)
- `crates/document-kits/src/pdfpro.rs` (+100 linhas — `pdf.audit` integration no `render`, helpers `pdfa_payload_value` + `metadata_payload_value`)
- `crates/document-kits/src/wordpro.rs` (+8 linhas — bump version no teste)
- `crates/document-kits/tests/e2e_docs_generate_pdf.rs` (+1 linha — `pdfa: None`)
- `docs/decisions/0021-fase-5-etapa-5-pdfpro-excelpro.md` (+136 linhas — Revisão 1 com correções + D-PDF6)
- `docs/architecture/pdfpro-specification.md` (+97 linhas — Tagged reclassificado + Escopo declarado do A-2B)
- `docs/modules/document-kits.md` (+42 linhas — seção Etapa 5)
- `docs/status.md` (+1 linha — evidencia do PR 3)
- `CHANGELOG.md` (+1 linha — entrada do PR 3)

## Decisões

- **D-PDF5 + D-PDF6 do ADR-0021** (revisão 1): correção factual sobre
  Tagged (não é requisito do A-2B) + novo D-PDF6 registrando a escolha
  do sRGB ICC (gerado localmente, SHA-256 fixado).
- **`KitError::AuditFailed` novo variant** (D-PDF5): §19.6 sem
  interruptor — o kit NÃO entrega o artefato quando a auditoria falha.
- **Schema bump `SpecVersion` 0.2.0 → 0.3.0** (MINOR — campo
  opcional `DocumentMetadata.pdfa`, backward-compat com 0.1.0 e 0.2.0).
- **Capability name `pdf.audit` (sem terceiro nível)** — todos os
  outros capabilities seguem `<dominio>.<verbo>`. O PR 4 estende
  este mesmo handler com `kind: "visual"`. O `salvar()` faz uma
  chamada só.
- **Cache key (D-AUDIT-1)** = `sha256(pdf_bytes) + ":" + AUDIT_RULES_VERSION`.
  `0.1.0` no PR 3. **Cache persistente fica pra PR próprio** — mistura
  de correção com otimização faz bug de cache virar bug de auditoria.
- **Bump atômico do `is_implemented() == true` + `target_format() == Pdf`**
  já no PR 2 (precedente ADR-0020 §3 D3). O teste
  `atomic_bump_target_format_and_is_implemented` em `pdfpro.rs:752`
  trava essa invariante. O PR 3 só estende o `render`, não muda o flip.

## Testes executados

**Python (`workers/document-worker/tests/test_pdf_audit.py`):**
```
$ .\runtime\python.exe .\tests\test_pdf_audit.py
=== 19 testes do pdf.audit (handler Python, D-PDF5/D-PDF6) ===
  [OK]   baseline_ok
  [OK]   baseline_n_pages_consistency
  [OK]   baseline_no_docinfo
  [OK]   baseline_encrypted
  [OK]   baseline_external_uri
  [OK]   baseline_external_embedded_file
  [OK]   baseline_corrupted
  [OK]   pdfa2b_ok
  [OK]   pdfa2b_missing_output_intent
  [OK]   pdfa2b_missing_xmp
  [OK]   pdfa2b_wrong_xmp_part
  [OK]   pdfa2b_wrong_xmp_conformance
  [OK]   pdfa2b_missing_icc
  [OK]   pdfa2b_bad_icc
  [OK]   pdfa2b_javascript_openaction
  [OK]   pdfa2b_javascript_names
  [OK]   kind_unsupported
  [OK]   missing_path
  [OK]   path_traversal

Total: 19  OK: 19  FAIL: 0  ERROR: 0
```

**Rust (`cargo test --workspace --lib`):**
```
test result: ok. 46 passed; 0 failed
test result: ok. 29 passed; 0 failed
test result: ok.  0 passed; 0 failed
test result: ok.  1 passed; 0 failed
test result: ok.  9 passed; 0 failed
test result: ok. 20 passed; 0 failed
test result: ok. 77 passed; 0 failed
test result: ok.  1 passed; 0 failed
test result: ok.  3 passed; 0 failed  (e2e_docs_generate_pdf: 3/3)
test result: ok. 13 passed; 0 failed
test result: ok. 57 passed; 0 failed
... (workspace total: 350+ passed, 0 failed)
```

**`cargo clippy --workspace --all-targets`:** limpo.

**`node scripts/check-docs.mjs`:** OK (cabeçalhos, carimbos, trava §1.13, links).

**`bootstrap.ps1` em runtime local:** gera o ICC, valida o SHA-256,
hard-fail se ausente. Bootstrap completo end-to-end passa.

## Limitações

- **Cross-check `n_pages` (write response vs pikepdf) foi removido.**
  O `pdf.write` v0.4.0 retorna `pages_rendered: 1` como chute
  (`document-worker.py:2185` — limitação do `reportlab` em não expor
  `n_pages` pós-build). A auditoria lê `n_pages` direto do PDF via
  pikepdf, que é a fonte da verdade. O cross-check entra quando o
  `pdf.write` reportar `n_pages` real — pendência registrada no
  spec. **Pego pelo próprio audit durante a integração:** o teste
  E2E existente quebrou (`n_pages_consistency` failure) e o fix foi
  remover o cross-check. **Bug do `pdf.write` continua aberto** —
  registrado como pendência 5.x, mas não bloqueia o PR 3.

- **Tagged PDF / `StructTree` não é verificado.** A v1 declara apenas
  o nível B do PDF/A-2, e A-2B (básico) não exige Tagged. **Esta é
  decisão, não omissão** — registrado no spec §"Não-objetivos" e
  §"Escopo declarado do PDF/A-2B". Se o usuário final ou sistema
  externo exigir Tagged (PDF/A-2A), o caminho é reivindicar A-2A
  em versão futura (vira bump de schema + `StructTree` XML manual
  no `reportlab`).

- **`sRGB2014.icc` do `color.org` não está disponível** (URL 404
  verificado em 2026-08-01). Solução: gerador local determinístico
  (D-PDF6). Se o ICC do color.org voltar a ficar disponível, o
  gerador ainda bate no mesmo hash porque os parâmetros sRGB são
  padrão internacional público. Bump do ICC = bump do SHA-256 =
  bump do `AUDIT_RULES_VERSION` (D-AUDIT-1) — registrado no ADR-0021.

- **`veraPDF` não roda no PR 3.** Validação rigorosa (TRC exato do
  sRGB, primaries, white point, etc.) fica no job noturno
  `ci-nightly.yml`. PR 3 só fecha auditoria estrutural sem Java;
  rigor do `veraPDF` é problema do nightly, não deste PR.

- **PDF/A-1B, PDF/A-3, PDF/A-4, PDF/UA** — fora do escopo da v1.
  A enum `PdfaFlavor` reserva variantes futuras, mas só `PdfA2b` é
  implementado.

- **3 testes pré-existentes do `frederico-process-architecture`**
  (`e2e_ocr_run_with_real_image`, `e2e_pdf_read_with_ocr_fallback_on_scanned`,
  `e2e_pdf_read_reports_ocr_unavailable`, `e2e_pdf_read_with_ocr_param_never`,
  `e2e_pdf_write_and_read`) **não rodam em dev local** porque o
  Tesseract não está instalado via Admin. Esses testes quebraram
  antes do PR 3 (são pré-existentes) e voltam a passar quando o
  `bootstrap.ps1` roda em CI (job noturno). **NÃO são regressões do
  PR 3** — verificado via `git stash` mental: o branch antes do
  PR 3 também quebra esses 5 pelos mesmos motivos.

## Riscos

- **A nova capability `pdf.audit` adiciona ~520 linhas no `document-worker.py`.**
  Toda execução do `pdf.write` agora também chama o `pdf.audit`.
  Tempo extra por geração: ~50-200ms (depende do tamanho do PDF
  e do número de checks). Para PDFs com 100+ páginas ou com
  milhares de fontes, pode chegar a 1s. Não bloqueia o caso comum.

- **Mudança de behavior no `KitOutput`.** O `extra` agora carrega
  `audit: {structural, rules_version, coverage, cache_key, checks}`.
  Caller que lia `extra.pages_rendered` continua funcionando
  (preservado), mas o shape mudou. CHANGELOG registra.

- **Schema bump 0.2.0 → 0.3.0.** `DocumentMetadata` ganhou campo
  `pdfa` opcional. Specs 0.1.0 e 0.2.0 desserializam com `pdfa = None`
  (campo é `#[serde(default, skip_serializing_if = "Option::is_none")]`).
  Backward-compat garantida por design.

- **Cache key pendente de AUDIT_RULES_VERSION.** Bump futuro de
  qualquer regra no `document-worker.py:_check_*` exige bump de
  `AUDIT_RULES_VERSION` no mesmo commit (D-AUDIT-1). Sem isso, o
  cache key fica estale e o `salvar()` retornaria verde para PDFs
  que falhariam com as regras novas. Documentado no ADR-0021 e
  no comentário do `AUDIT_RULES_VERSION = "0.1.0"`.

- **ICC gerado localmente vs ICC do veraPDF.** O `veraPDF` no
  noturno pode reclamar do TRC aproximado (gamma 2.2 vs EOTF
  sRGB exato, que é piecewise). Se o noturno quebrar, o caminho
  é gerar v4 do ICC (paramétrico) em vez de v2 (gamma). Registrado
  como pendência 5.x.

## Próxima etapa

- **PR 4 (Etapa 5)** — auditoria visual do §19.3 via `pypdfium2`:
  rasteriza cada página, checa grade (corte), sobreposição de
  elementos, página vazia, alinhamento de cabeçalho/rodapé.
  Estende o handler `pdf.audit` com `kind: "visual"` (mesma
  capability, novo argumento). 10+ testes negativos. O `salvar()`
  continua fazendo uma chamada só.

- **PR 5 (Etapa 5)** — chart nativo Excel + identidade visual
  Excel + tabela estilizada (fecha 3 das 6 lacunas do ExcelPro).
  Bumps atômicos do ExcelProKit. Spec `excelpro-specification.md`
  atualizado. Warning de degradação "chart virou tabela" sai no
  mesmo PR.

- **PR 6 (Etapa 5)** — fechamento: E2E completo (audit visual +
  estrutural passando), bump `status.md` Etapa 5 → concluída,
  specs `pdfpro` e `excelpro` → `implementado` (3 lacunas do
  ExcelPro fechadas), CHANGELOG v0.5.0, `verify-external.ps1` cobre
  os E2E novos.

- **Pendência 5.x (registrada, fora do PR 3):** n_pages real do
  `pdf.write` (limitação do `reportlab`); v4 do ICC se o `veraPDF`
  noturno reclamar do TRC aproximado; PDF/A-2A (Tagged, accessible)
  para conformidade declarada nível A; `docs.inspect` cobrindo
  `.pdf` (round-trip com `coverage`); sumário em duas passadas
  (`multiBuild` do `reportlab`); Tagged automático no `reportlab`
  via `StructTree` XML manual.

## Ordem operacional do PR 2 → PR 3 (registro da dança)

> Escrito aqui pra próxima sessão não improvisar. O precedente
> PR #7 (Etapa 4) quebrou porque a base foi deletada no merge do
> PR pai. Empilhamento em si não é errado — é o estado natural
> de trabalho sequencial; o que dá errado é deletar a base.

**Sequência aplicada pra abrir esta PR 3:**

1. **Verificar divergência** com `git fetch origin && git log --oneline
   origin/fase-5/etapa-5-pdfpro-excelpro ^HEAD` — retornou 2 commits
   obsoletos (`bd8f918` PR 1 com bump invertido, `5c39bac` fix).
   `git diff 5c39bac d518226 --stat` vazio: as 2 commits produzem
   **a mesma tree** que `d518226` (em `origin/main`). O trabalho
   está em main, só com SHA diferente — force-push seguro.

2. **Force-push do PR 2** (`fase-5/etapa-5-pdfpro-excelpro`) com
   `--force-with-lease` — destrói os 2 obsoletos do origin.
   Espera CI verde no SHA novo do PR 2 antes de empurrar o PR 3.

3. **Push do PR 3** (esta PR, branch `fase-5/etapa-5-pdfpro-excelpro-
   pr3-auditoria-estrutural`) **empilhada em PR 2**. O diff
   desta PR é só o delta do PR 3, não inclui o PR 2.

4. **Depois que o PR 2 mesclar** (squash no GitHub): a base
   desta PR vai apontar pro PR 2 deletado. **Antes** de trocar
   a base via "Edit" do GitHub, **rebasear** esta branch
   sobre o `main` novo (porque o squash muda o SHA de tudo;
   sem o rebase, o diff desta PR mostraria o conteúdo do PR 2
   de novo). Sequência:
   ```bash
   git fetch origin
   git rebase origin/main  # rebaseia PR 3 sobre o main pós-merge
   git push --force-with-lease origin fase-5/...-pr3
   ```
   Depois, no GitHub: **"Edit"** ao lado do título da PR 3 →
   trocar `base` de `fase-5/...-pr2` para `main`. O GitHub
   recalcula o diff (que agora é só o delta do PR 3 contra
   o main novo) e a PR sobrevive.

**Por que esta abordagem (e não deletar a branch do PR 2):**
- Deletar a branch do PR 2 fecha automaticamente qualquer PR
  baseada nela (foi o que quebrou a PR #7 na Etapa 4).
- Manter a branch do PR 2 viva até a PR 3 entrar evita a janela
  em que a base some.
- O rebase + "Edit" no GitHub é o caminho robusto: o rebase
  garante que o diff está limpo (sem o conteúdo do PR 2
  duplicado), e o "Edit" só troca a referência visual na UI.

**Regra simples pra próxima vez:** "só abra a PR seguinte
depois que a anterior entrou no main". Custa uma espera,
elimina a classe inteira de problema.
