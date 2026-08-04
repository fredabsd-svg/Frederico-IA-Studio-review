# Fase de Ligação — Etapa 5.X (patch-allowed-paths)

**Tipo:** patch de segurança (não é uma das 6 etapas da Fase de
Ligação; é um patch no caminho de produção que as Etapas 2.B/5
deixaram com barreira desligada).

**Status:** fechado (5 de 6 etapas da Fase de Ligação; Etapa 5.X
não conta como uma das 6).

## TL;DR

A Etapa 3 da Fase 5 (set/2025) introduziu o
`WorkerToolDispatcher` com allowlist no **construtor** e
`validate_against_allowlist` com fail-open em dois lugares (vetor
vazio passava; `use_canonical=false` strippava verbatim do
`allowed` mas não do `canonical`). A composição
(`app/src/composition.rs`) passou `vec![]` na construção. O
`docs.generate` (ligado ao caminho de produção desde a Etapa 2.B
da Fase de Ligação, PR #23) validava `output_path` contra
allowlist vazia → passava sem validar. O `output_path` mal
formado pelo modelo apontava pra qualquer path absoluto do
disco; a única barreira era o `document-worker.py` rejeitar
`..`, que **não cobre caminho absoluto** (ex.: `C:\Windows\System32\out.docx`).

**A barreira estava desligada E quebrada desde a Etapa 3.** O
fail-open não só deixou passar — ele escondeu que o mecanismo
nunca funcionou. O bug do verbatim (latente no `validate_against_allowlist`)
só não foi pego porque o fail-open bypassava a função inteira.

Este PR é o argumento definitivo contra defaults fail-open:
**eles não só deixam passar, eles escondem que o mecanismo
nunca funcionou.** Quando a barreira foi ligada (commit 2),
o bug do verbatim apareceu imediatamente (cenário 2 do
`path_safety`).

## O que mudou

### Commit 1 — `fix(tool-registry): validate_against_allowlist nega allowlist vazia (fail-closed)`

A função pura `validate_against_allowlist(path, &[])` retornava
`Ok(())` (sem validar). Agora retorna `Err(PathNotAllowed)`. O
`dispatch()` remove o `if !self.allowed_paths.is_empty()` que
puxava a checagem. Teste `path_with_empty_allowlist_passes`
vira `path_with_empty_allowlist_denies` (regressão contra
"simplificação" que voltaria ao fail-open).

**"Se algum caller precisar de sem-restrição legítimo, é variante
explícita (`PathPolicy::Unrestricted` num PR próprio)."** Sem
caller hoje; abstração especulativa não cabe aqui.

### Commit 2 — bump atômico: dispatcher recebe allowlist por chamada; `docs.generate` valida contra o jail da conversa

1. `WorkerToolDispatcher::new(invoker)` — allowlist removida do
   struct. Justificativa: a base varia por conversa
   (`workspaces/<cid>/`) e o dispatcher é compartilhado entre
   tool_calls e entre conversas. Era por chamada, não por
   composição.

2. `dispatch(args, path_fields, allowed: &[PathBuf])` — passa
   allowlist por chamada. `check_path(path_str, allowed: &[PathBuf])`
   idem. Fail-closed (commit 1).

3. `Jail::root_canonical() -> &Path` (novo getter, `workspace.rs`)
   — o `Jail::new` já canonicalizava internamente (`root_canonical`)
   e comparava o canonical do `resolve` contra ele. O getter
   expõe o canonical pra call sites que precisam de "a allowlist
   é o jail da conversa" — assim o `Tool::execute` e o
   `validate_against_allowlist` operam na **mesma** canonicalização
   e o `Path::starts_with` component-wise é confiável mesmo com
   case misto do Windows.

4. `DocsGenerateTool::execute` (`document-kits/src/generate.rs`):
   três passos em série.
   - **(a)** `ctx.jail.resolve_allowing_nonexistent(output_path)` —
     barreira primária, mesma Etapa 1. Cobre `..`, absoluto, UNC,
     symlink (via canonicalize do pai). Erro = "output_path fora
     do workspace da conversa".
   - **(b)** `std::fs::symlink_metadata` — **mitigação parcial**
     contra symlink-on-output (TOCTOU entre check e write do
     Python; não cobre o caso do arquivo não existir, que é o
     caso normal). Rótulo explícito: **NÃO é barreira**. Barreira
     de verdade exige `O_NOFOLLOW` / `O_CREAT|O_EXCL` no `open` do
     Python — pendência 5 do `process-architecture.md`.
   - **(c)** `dispatcher.check_path(canonic, &[root_canonical])` —
     defesa em profundidade, fail-closed.

5. `DocsInspectTool::execute` (`document-kits/src/inspect.rs`):
   mesma estrutura, mas usa `Jail::resolve` (não
   `_allowing_nonexistent` — o `docs.inspect` **lê** arquivo
   existente). Sem mitigação symlink (o `Jail::resolve` já pega
   via canonicalize do path inteiro).

### Bug colateral (descoberto ao ligar a barreira)

`validate_against_allowlist` em `use_canonical=false` (path não
existe) strippava o verbatim `\\?\` do `allowed` mas **não** do
`canonical` — path com `\\?\` + allowed com `\\?\` → `starts_with`
falhava mesmo path dentro do jail. Bug latente desde a Etapa 3
(não foi pego antes porque a composição passava `&[]` =
fail-open bypassava tudo). Fix: `strip_windows_verbatim` também
no `canonical` do ramo `normalize_lexically`. Regressão coberta
por `path_with_verbatim_prefix_and_nonexistent_strips_correctly`
(`tool-registry/src/worker_dispatch.rs`).

### Commit 3 — `test(document-kits): path_safety — 6 cenários`

`crates/document-kits/tests/path_safety.rs` (novo). Integration
test do `Tool::execute` end-to-end + 1 teste direto do
`check_path` com allowlist vazia. Usa `FakeWorker` in-process
(sem Python, sem `bootstrap.ps1`) — roda no CI comum.

Cenários:

1. **allow_relative_path** — `"out.docx"` → barreira aceita;
   `kit.render` (FakeWorker) devolve `ok: true`. **Não** asserta
   `is_file()` porque o FakeWorker não toca no FS — a confirmação
   de que o **arquivo real é criado no canônico do jail** é o
   cenário do E2E real com Python (commit 4 do PR,
   `e2e_docs_generate_with_real_worker`).
2. **reject_absolute_path_outside_jail** — `C:\Windows\System32\out.docx`
   (Windows) ou `/etc/passwd` (Unix) → `JailViolation`. **Cobre o
   que os 2 testes de unidade em `generate.rs::tests` perderam**
   quando foram ajustados de absoluto pra relativo. Comentário
   no teste explicita a matriz: "ajustar o teste ao comportamento
   novo é legítimo aqui, mas o que não pode acontecer é a
   cobertura do caso absoluto fora do jail desaparecer no
   caminho".
3. **reject_parent_traversal** — `"../<outro_cid>/secret.docx"`
   → `'..'` rejeitado no loop de componentes do
   `Jail::resolve_allowing_nonexistent`.
4. **allow_mixed_case_filename** — workspace em
   `Workspace-Lower-Case-<nonce>/`, `output_path: "Output.DOCX"`
   (case diferente do FS). Prova que os dois lados da comparação
   vêm da mesma canonicalização do `Jail::new` e que o
   `Path::starts_with` é confiável.
5. **reject_symlink_output** — `link.txt` → symlink pra
   `/etc/hosts` (Unix) ou `C:\Windows\System32\drivers\etc\hosts`
   (Windows). A mitigação no `execute` (passo 2) detecta.
   Rótulo explícito: mitigação parcial, NÃO barreira.
6. **check_path_fail_closed_on_empty_allowlist** — chama
   `check_path` direto com allowlist vazia → `Err(PathNotAllowed)`.
   Regressão contra "simplificação" do `validate_against_allowlist`.

### Commit 5 — `chore(gitignore): atualiza comentário tmp_*`

Comentário antigo (Etapa 3 da Fase 5) descrevia o `tmp_*` como
"padrão usado pelos E2E do document-kits". Agora é profilático
(classe recorrente de subproduto de teste) — o `real_minimal.docx`
específico foi consertado pela origem (bump atômico do commit 2).

### Commit 4 — bump atômico: asserção extra do `e2e_docs_generate_with_real_worker` + comentário aspiracional corrigido

(Detalhes abaixo — o commit é o último por dependência do runtime
Python + `bootstrap.ps1`.)

## Lições aprendidas

### 1. Defaults fail-open escondem que o mecanismo nunca funcionou

O bug do verbatim era latente desde a Etapa 3. Se o
`validate_against_allowlist` tivesse sido fail-closed (e a
composição tivesse passado allowlist não-vazia), a função
teria rodado de verdade, o `starts_with` teria falhado, e o
bug teria sido pego em 5 minutos.

A lição é a **forma** do argumento: **default fail-open não é
só "deixa passar" — é "esconde que o mecanismo nunca foi
exercitado"**. Mecanismos de validação que nunca rodaram
parecem funcionar até o dia que precisam; quando precisam, é
tarde. Daí a regra do projeto (que vale cross-project):
**default de validação = fail-closed**; "sem restrição" é
opt-in explícito.

### 2. Comentário no código que descreve barreira que o código não impõe é aspiracional por falta de gate

O comentário em `e2e_docs_generate_with_real_worker.rs:264` (na
versão pré-PR) dizia: "O `output_path` veio relativo ao
workspace da conversa; junta com o `<workspaces_root>/<cid>/`
pra ter o path absoluto". Descrevia um comportamento que **não
existia** quando foi escrito. A barreira que "vinculava" o
output_path ao workspace não estava implementada.

A Regra 1 do `REGRAS-DO-PROJETO.md` (gate documental) só
alcança docs; comentário no código é narrativa, e narrativa
pode mentir sem ninguém perceber. O `git blame` mostra quem
escreveu; o `git log` mostra quando; nenhum mostra se o que
está escrito é verdade.

A lição é **estreita e útil** (não tentamos formalizar como
regra nova no `REGRAS-DO-PROJETO.md` — o user sinalizou que
seria inverificável até o padrão aparecer 3 vezes):
**comentário não descreve barreira que o código não impõe; se
a barreira não existe ainda, o comentário diz que não existe**.
A primeira ocorrência foi esse `e2e_docs_generate_with_real_worker.rs:264`.
A próxima vez que aparecer, conta; a terceira vira regra.

(Os comentários do código que **estão** ancorados em teste —
como o do `Jail::resolve_allowing_nonexistent` agora, que
descreve o que **acontece** e tem o `path_safety::scenario_2`
provando — não aspiram. É a diferença entre "o código deveria
fazer X" e "o código faz X, conforme teste Y".)

### 3. Reutilizar abstração existente em vez de reimplementar paralelamente

O código original (Etapa 3) tentava fazer path safety com
`starts_with` sobre string canonicalizada pelo próprio
`WorkerToolDispatcher`. O `Jail` (Etapa 1) já fazia isso
melhor — com `realpath`, symlink, UNC, caso canônico do
Windows, suíte de testes cobrindo todos os vetores. A Etapa
5.X **reusa o `Jail` em vez de reimplementar**. Duas barreiras
paralelas é como uma delas fica pra trás (a Etapa 3 provou
isso: o bug do verbatim ficou no `validate_against_allowlist`
porque ninguém olhava pra ele — todo mundo olhava pro `Jail`).

A lição: **a barreira certa mora no lugar certo**;
reimplementar por "mais simples" cria dois lugares e um deles
fica desatualizado.

## Comportamento visível que mudou

**`docs.inspect` agora recusa inspecionar arquivo fora do
workspace da conversa.** Antes a barreira era no-op; agora o
`Jail::resolve` rejeita. Inspeção sem jail seria porta lateral
para ler qualquer arquivo do disco (incluindo `C:\Users\conta\.ssh\id_rsa`,
`C:\Windows\System32\config\SAM`, etc.) — restrição nova é
correta, mas é visível pra quem usa.

**Quando as "Pastas do PC" chegarem (Etapa 6 da Fase de
Ligação, ou Fase 9 do PROMPT MESTRE), elas entram na allowlist**
(allowlist = `root_canonical` por conversa hoje; "Pastas do PC"
será uma allowlist explícita por origem do path). Até lá, o
limite é o workspace da conversa.

## Pendências

- **Pendência 5 do `process-architecture.md` (NOVA)**:
  escrita segura no worker Python. Mitigação symlink atual é
  TOCTOU + não cobre caso do arquivo não existir. Barreira de
  verdade: `O_NOFOLLOW` / `O_CREAT|O_EXCL` no `open` dos
  handlers `docx.write` / `xlsx.write` / `pdf.write`. ADR nova
  necessária.
- **Pendência 4 do `process-architecture.md`**: `WorkerHealth::Unknown`
  distinto de `Unhealthy`. Sem mudança de comportamento no
  caminho atual; `ensure_first_pong` convive.
- **Pastas do PC (Etapa 6 / Fase 9)**: allowlist ampliada.
- **Etapas 3, 4 e 6 da Fase de Ligação**: independentes desta.

## Validações

- `cargo build --workspace` verde
- `cargo test --workspace --lib` 476/476 verde
- `cargo test -p frederico-document-kits --test path_safety` 6/6 verde
- `cargo fmt --all -- --check` limpo
- `cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock` limpo

(Validação do E2E real com Python — `e2e_docs_generate_with_real_worker`
— depende de `bootstrap.ps1`. CI noturno via `verify-external.ps1`
step 7.)
