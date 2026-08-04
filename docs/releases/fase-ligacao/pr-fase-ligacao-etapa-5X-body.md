# Fase de Ligação Etapa 5.X (patch-allowed-paths): path safety do `docs.generate`/`docs.inspect` ligado (fail-closed) + bug latente do verbatim

Patch de segurança no caminho de produção do `docs.generate` e `docs.inspect`.
A Etapa 3 da Fase 5 (set/2025) deixou a barreira de path safety
**desligada E quebrada** desde então. Este PR liga a barreira,
conserta o bug latente, cobre os dois lados (allow + reject) e
documenta a lição.

## TL;DR

**Placar:** dois defeitos empilhados que se escondiam mutuamente:

1. **Defesa desligada** (fail-open por omissão): `WorkerToolDispatcher::new`
   recebia `allowed_paths: Vec<PathBuf>` no construtor, a composição
   passava `vec![]`, e `validate_against_allowlist` retornava `Ok(())`
   com allowlist vazia. O `output_path` mal formado pelo modelo apontava
   pra qualquer path absoluto do disco; a única barreira era o
   `document-worker.py` rejeitar `..`, que **não cobre caminho absoluto**
   (`C:\Windows\System32\out.docx`).
2. **Defesa quebrada** (bug do verbatim `\\?\`): `validate_against_allowlist`
   em `use_canonical=false` (path não existe) strippava o verbatim do
   `allowed` mas **não** do `canonical` — `starts_with` falhava mesmo
   o path estando dentro do jail. Latente desde a Etapa 3 — **nunca
   foi exercitado** porque o fail-open bypassava a função inteira.

**É o argumento mais forte que o projeto tem contra defaults fail-open**:
defaults permissivos escondem o que nunca funcionou. Mecanismos de
validação que nunca rodaram parecem funcionar até o dia que precisam;
quando precisam, é tarde. Daí a regra cross-project: **default de
validação = fail-closed**; "sem restrição" é opt-in explícito.

## Comportamento visível que mudou (efeito pro usuário)

- **`docs.generate` agora recusa `output_path` absoluto** (`C:\...`,
  `/...`). Antes a barreira era no-op; agora `Jail::resolve_allowing_nonexistent`
  rejeita com `JailViolation`. Se o modelo emitir `output_path: "C:\Users\conta\out.docx"`,
  o tool devolve `ToolResult::err` em vez de escrever no CWD do Python.
  Path relativo (`output_path: "out.docx"`) resolve pro workspace da
  conversa — comportamento esperado.
- **`docs.inspect` agora recusa arquivo fora do workspace da conversa.**
  Antes a barreira era no-op; agora o `Jail::resolve` rejeita. Inspeção
  sem jail seria porta lateral pra ler qualquer arquivo do disco
  (`C:\Users\conta\.ssh\id_rsa`, `C:\Windows\System32\config\SAM`, etc.).

Quando as "Pastas do PC" chegarem (Etapa 6 / Fase 9), elas entram
na allowlist; até lá, o limite é o workspace da conversa.

## O que muda (7 commits, +1293/-214)

1. **`fix(tool-registry): validate_against_allowlist nega allowlist vazia (fail-closed)`**
   — função pura `validate_against_allowlist(path, &[])` agora retorna
   `Err(PathNotAllowed)`. `dispatch()` remove o `if !is_empty`. Teste
   `path_with_empty_allowlist_passes` vira `_denies` (regressão contra
   "simplificação" que voltaria ao fail-open).

2. **`feat: dispatcher recebe allowlist por chamada; docs.generate valida contra o jail da conversa (bump atômico)`**
   — `WorkerToolDispatcher::new(invoker)` (allowlist removida do struct,
   passada por chamada); `Jail::root_canonical` getter exposto; `DocsGenerateTool::execute`
   usa `Jail::resolve_allowing_nonexistent` como barreira primária +
   mitigação symlink explícita (rótulo "parcial", TOCTOU documentado)
   + `check_path` fail-closed como defesa em profundidade; `DocsInspectTool::execute`
   mesma estrutura com `Jail::resolve` (lê arquivo existente). **Bug
   colateral consertado:** bug do verbatim `\\?\` (strip do canonical
   no `validate_against_allowlist`). 9 files, +265/-85.

3. **`test(document-kits): path_safety — 6 cenários`**
   — `crates/document-kits/tests/path_safety.rs` (novo): allow relativo,
   reject absoluto, reject traversal, allow mixed-case, reject
   symlink, fail-closed. FakeWorker in-process (roda no CI comum).

4. **`test(e2e + document-kits): ajustes do path safety + asserção extra + comentário aspiracional corrigido`**
   — ajusta 4 tests do `document-kits/tests/e2e_*.rs` que usavam
   `temp_out_dir()` (path absoluto em outro tempdir) pra usar path
   relativo dentro do jail do `dummy_ctx`; asserção extra no
   `e2e_docs_generate_with_real_worker` (compara canônico do path
   esperado com canônico do path devolvido pelo worker) sustenta o
   cenário 1 do `path_safety`; comentário aspiracional corrigido.

5. **`chore(gitignore): atualiza comentário tmp_***`
   — `tmp_*` já estava no gitignore; comentário atualizado pra
   "classe recorrente de subproduto de teste, profilático".

6. **`docs(fase-ligacao + status + process-architecture): pendência 1 fechada`**
   — `status.md` (5 de 6 etapas); `process-architecture.md` (pendência
   1 marcada fechada, pendência 5 nomeada — escrita segura no worker
   Python); narrativa + CHANGELOG.

7. **`docs(fase-ligacao + CHANGELOG): lição --lib + placar do PR + cobertura migrada + efeito visível docs.generate`**
   — atualizações documentais após as descobertas do commit 4. Em
   particular: **lição de que `cargo test --workspace --lib` não roda
   integration tests** — sem o `cargo test --workspace` completo, o
   PR teria sido aberto com 4 testes quebrados.

## Cobertura migrou pra `path_safety::scenario_2` (não evaporou)

6 testes (2 em `generate.rs::tests` + 4 em `document-kits/tests/e2e_*.rs`)
tiveram o `output_path` ajustado de **absoluto** pra **relativo**, e
cada ajuste é "remoção de um caso que agora falha". A cobertura do
"absoluto fora do jail é rejeitado" migrou pro
`crates/document-kits/tests/path_safety.rs::scenario_2_reject_absolute_path_outside_jail`
com asserção focada em `JailViolation`. Tabela detalhada na narrativa
do PR. Regra: ao mover teste de absoluto pra relativo, a cobertura
do "absoluto/quebra é rejeitado" tem que ir pra algum lugar.

## Pendências

- **Pendência 5 do `process-architecture.md` (NOVA)**: escrita segura
  no worker Python. Mitigação symlink atual é TOCTOU + não cobre caso
  do arquivo não existir. Barreira de verdade: `O_NOFOLLOW` /
  `O_CREAT|O_EXCL` no `open` dos handlers `docx.write` / `xlsx.write`
  / `pdf.write`. ADR nova necessária.
- **Pendência 4 do `process-architecture.md`**: `WorkerHealth::Unknown`
  distinto de `Unhealthy`. Sem mudança de comportamento no caminho
  atual; `ensure_first_pong` convive.
- **Pastas do PC (Etapa 6 / Fase 9)**: allowlist ampliada.
- **Etapas 3, 4 e 6 da Fase de Ligação**: independentes desta.

## Lições aprendidas (registradas na narrativa)

1. **Defaults fail-open escondem que o mecanismo nunca funcionou** —
   a regra cross-project do projeto passa a ser: default de validação
   = fail-closed; "sem restrição" é opt-in explícito.
2. **Comentário no código que descreve barreira que o código não impõe
   é aspiracional por falta de gate** — Gate documental só alcança
   docs; comentário no código é narrativa, e narrativa pode mentir
   sem ninguém perceber. A lição: comentário não descreve barreira
   que o código não impõe; se a barreira não existe ainda, o comentário
   diz que não existe. Não formaliza como regra até o padrão aparecer
   3 vezes.
3. **Reutilizar abstração existente em vez de reimplementar paralelamente**
   — o código original reimplementava path safety no `WorkerToolDispatcher`
   paralelo ao `Jail` da Etapa 1. A Etapa 5.X reusa o `Jail`. Duas
   barreiras paralelas é como uma delas fica pra trás.
4. **`cargo test --workspace --lib` não roda integration tests** —
   sempre rodar `cargo test --workspace` (sem `--lib`) local antes
   de PR com mudança de assinatura. O CI já roda completo (o buraco
   era só na validação local).

## Validações locais

- `cargo build --workspace` verde
- `cargo test --workspace` (sem `--lib`, **34 test results**) todos
  verde:
  - 5 E2E com `FakeWorker` (`crates/e2e/tests/`)
  - `e2e_docs_generate_with_real_worker` (Python real, 1.03s) — asserção
    extra do path canônico dentro do jail passa
  - `e2e_docs_generate_docx_full_vertical` (Python real, 1.45s)
  - 3 `e2e_docs_generate_pdf_*` (Python real, 1.43s + 1.68s + 1.11s)
  - `e2e_docs_generate_xlsx` (Python real)
  - `e2e_docs_inspect_docx_roundtrip` (Python real, 1.06s)
  - 6 `path_safety` (`FakeWorker`, 0.03s)
  - 476 dos `--lib`
- `cargo test -p frederico-document-kits --test path_safety` 6/6 verde
- `cargo fmt --all -- --check` limpo
- `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock`
  limpo
- `check-core-purity.ps1` OK
- `check-fase-5-untouched.ps1` OK
- `check-docs.mjs` OK
- Python **não** engasgou com path canônico `\\?\` (preocupação inicial:
  reportlab/python-docx lidando com verbatim — na prática o `python.exe`
  e o `python-docx` no Windows lidam bem). Se algum dia reclamar,
  conserto é tirar o verbatim na fronteira com o worker (handler do
  `document-worker.py` ou no `WorkerInvoker`), **não** afrouxar a
  barreira.

## Working tree

1 untracked: `workers/document-worker/real_minimal.docx` (36673 bytes,
subproduto antigo do CI do PR #24, antes do bump atômico). Bloqueado
pela policy de safety nesta sessão; precisa ser deletado manualmente.
Não bloqueia o PR (untracked).

## Commits (7)

```
31fa509 docs(fase-ligacao + CHANGELOG): lição --lib + placar do PR + cobertura migrada + efeito visivel docs.generate
8756eb3 docs(fase-ligacao + status + process-architecture): pendencia 1 fechada; Etapa 5.X (patch-allowed-paths) fechada; pendencia 5 nova
54cd733 chore(gitignore): atualiza comentario tmp_* (Etapa 3 -> classe recorrente)
ad370e9 test(e2e + document-kits): ajustes do path safety + assercao extra + comentario aspiracional corrigido
36889d8 test(document-kits): path_safety — 6 cenarios (allow, reject abs, reject traversal, allow mixed-case, reject symlink, fail-closed)
ba6c5cb feat: dispatcher recebe allowlist por chamada; docs.generate valida contra o jail da conversa (bump atomico)
ee27f20 fix(tool-registry): validate_against_allowlist nega allowlist vazia (fail-closed)
```

Narrativa completa em `docs/releases/fase-ligacao/pr-fase-ligacao-patch-allowed-paths.md`.
