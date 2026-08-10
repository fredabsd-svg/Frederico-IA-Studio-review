# Fase 7, Etapa 5 (`files.write` + `files.edit` + `files.list` no ToolRegistry) — narrativa

<!--
Estado: concluída
Verificado contra o código em: 2026-08-10
PR: #45 (mergeado) + #46 (a abrir)
Fase correspondente: 7 (Etapa 5)
-->

Narrativa de processo da **Etapa 5 da Fase 7** (Modo
Desenvolvedor — núcleo: execução isolada). Foco:
`files.write` + `files.edit` + `files.list` no `ToolRegistry`,
sob o `Jail` da Fase 3, com semântica de sobrescrita atômica,
backup, audit com hashes, e race defense via `expected_sha256`.

Esta narrativa complementa o `CHANGELOG.md` (efeito pro
usuário) com a história técnica — o que aconteceu em cada
commit, quais decisões foram tomadas no caminho, e o que se
aprendeu.

## O que esta Etapa entrega

- **3 ferramentas novas no `frederico-tool-registry::tools`**:
  `FilesListTool` (`tool_id: "files.list"`, `category: Files`,
  `risk_level: Safe`, **sem** `requires_user_approval`),
  `FilesWriteTool` (`tool_id: "files.write"`, mesma categoria,
  `risk_level: Moderate`, `requires_user_approval: true`),
  `FilesEditTool` (`tool_id: "files.edit"`, mesma categoria/risk).
  A casca Tauri e o modo servidor §5.5 ganham 3 ferramentas
  in-process que **manipulam arquivos do workspace**.
- **`Jail::resolve_or_create_parents`** (novo método,
  ADR-0035 D5) — valida jail e cria diretórios intermediários
  em uma operação. 8 unit tests.
- **3 E2E novos em `crates/e2e/tests/`** —
  `e2e_files_list_under_jail.rs` (2 tests, caminho completo
  via `ChatOrchestrator`), `e2e_files_write_under_jail.rs`
  (10 tests, `validate_tool_call` + `FilesWriteTool::execute`
  direto), `e2e_files_edit_idempotent.rs` (14 tests, mesmo
  padrão). Total **26 testes E2E novos**.

## Por que 2 PRs, não 3 (regra do user 2026-08-10)

A estratégia inicial era 3 PRs separados (`files.list`,
`files.write`, `files.edit`). O user corrigiu pra 2:

> **`files.write` e `files.edit` vão juntos — compartilham
> a mesma máquina de escrita atômica, backup e auditoria, e
> pedem a mesma lente de revisão. Separá-los faria o segundo
> PR ser quase só ajuste do primeiro.**
>
> - **PR 1** — `files.list`: leitura, sem aprovação, já
>   commitado e verde. Entra rápido e sai do caminho.
> - **PR 2** — `files.write` + `files.edit`: as primeiras
>   ferramentas do projeto que **destroem dados do usuário**.
>   O motivo de isolar o PR 2 é o precedente do
>   `allowed_paths`: foi num PR pequeno e focado que apareceu
>   a barreira desligada *e* quebrada. Enterrada num diff
>   maior, teria passado.

**Quatro coisas que o PR 2 tem que ter:**

1. **Atomicidade de verdade** — temp no mesmo dir + fsync
   arquivo + fsync dir + rename (rename entre volumes falha
   no Windows, registrado no ADR-0035 D7). Se o app morrer
   no meio, o arquivo do usuário fica **intacto ou completo,
   nunca truncado**.
2. **Aprovação obrigatória no manifesto** — `requires_user_approval:
   true` + `risk_level: Moderate`. Sobrescrever arquivo é
   a operação mais destrutiva do catálogo até hoje; não pode
   executar sem o usuário ver o caminho.
3. **`files.edit` tem que falhar se o conteúdo mudou** — o
   `expected_sha256` no tool_call é o SHA-256 que o caller
   viu no `files.read` anterior; se o `actual_sha256` não
   bate, **recusa** em vez de aplicar no lugar errado.
4. **Testes de negação, não só de caminho feliz** — escrita
   fora do jail, sobrescrita sem approval, edição ambígua, e
   o teste que prova que o arquivo original sobrevive a uma
   falha no meio.

Aplicado literalmente: PR #45 (files.list, mergeado) e
PR #46 (files.write + files.edit, a abrir).

## 4 regras críticas honradas

### 1. Atomicidade de verdade (D1 do ADR-0035)

`temp_path = path.<uuid_v4>.tmp` no **mesmo dir** (rename
atômico, mesmo filesystem) → `write_all` + `file.sync_all()`
(fsync) + `dir.sync_all()` (fsync dir) → `fs::rename`
(atômico no mesmo volume). Limite 10 MB hard. **Teste de
regressão** `crash_between_write_and_rename_leaves_original_intact`
(em `tools/files_write.rs::tests`) prova que path original
fica INTACTO quando o rename falha. **Pendência D7:**
rename entre volumes falha no Windows (erro
`ERROR_NOT_SAME_DEVICE` 17). `temp_path` no mesmo dir evita
o problema em 99% dos casos; pra cobrir 100%, a Etapa 5+
vai detectar cross-volume e retornar erro claro
("cross-volume write não suportado, configure workspace
no mesmo volume").

### 2. Aprovação obrigatória no manifesto

`manifest.requires_user_approval(true)` + `risk_level(Moderate)`.
Gate é o `validate_tool_call` Passo 9 (do `validate.rs` do
Phase 3). Sem `ApprovalDecision { approved: true, ... }`
passada pelo caller, retorna `ApprovalRequired` (NÃO chama
`execute`). **Por que moderate, não critical:** "write é
mutação irreversível, mas não execução de código" (Etapa 4
do Phase 7, Etapa 5 da Fase de Ligação). Critical é só pra
`exec.shell` (Etapa 6) — fronteira entre `ls` e `rm -rf` é
invisível pro `PermissionSet`.

### 3. `files.edit` recusa se o conteúdo mudou (regra do user)

`expected_sha256` no tool_call é o SHA-256 que o caller viu
no `files.read` anterior. Se `actual_sha256 != expected`, o
tool **recusa** com `conteúdo mudou desde a leitura: caller
disse X, arquivo é Y. Releia o arquivo (files.read) e refaça
o edit com o novo expected_sha256. Sem isso, o edit seria
aplicado silenciosamente no lugar errado.` Defesa contra
race read-modify-write — o modelo que leu `config.toml`
minutos atrás pode estar sobrescrevendo mudanças de outra
invocação no meio, corrompendo o arquivo silenciosamente.

**Teste de regressão** `files_edit_expected_sha256_mismatch_refuses_edit_and_leaves_file_intact`
(em `e2e_files_edit_idempotent.rs`) prova o cenário
realista: `read` no round 1 → outra invocação altera o
arquivo (race) → `edit` com `expected_sha256` do round 1 →
**recusa + arquivo INTACTO**.

### 4. Testes de negação, não só de caminho feliz

- **Path safety 3x** (em `files_write.rs::tests` e
  `files_edit.rs::tests` + E2E `e2e_files_write_under_jail.rs`
  e `e2e_files_edit_idempotent.rs`): `..` (path traversal),
  absoluto, UNC. **Prova:** o Jail rejeita antes de tocar
  o disco; o `parent dir do workspace` não tem o arquivo
  leaked.
- **Overwrite sem approval** (D2): `overwrite: false` (default)
  + arquivo existente = `OverwriteRequired`, arquivo original
  INTACTO. Teste `files_write_overwrite_false_refuses_existing_file`
  + `files_write_overwrite_creates_backup_with_previous_content`.
- **Edição ambígua** (D4): `find` casa 2+ vezes sem
  `replace_all: true` = `AmbiguousMatch`, arquivo original
  INTACTO. Teste
  `files_edit_ambiguous_match_without_replace_all_is_error`.
- **Falha no meio da escrita**: rename falha (cenário
  simulado via teste de regressão) = arquivo original
  INTACTO, `temp_path` limpo, erro propagado. Teste
  `crash_between_write_and_rename_leaves_original_intact`.

## `Jail::resolve_or_create_parents` (ADR-0035 D5)

Novo método em `crates/tool-registry/src/workspace.rs`:

1. Mesma validação textual de `resolve_allowing_nonexistent`
   (rejeita `..`, absoluto, UNC) — barreira de jail não muda.
2. Se path completo já existe, delega pro
   `resolve_allowing_nonexistent` (que valida jail).
3. Se não, sobe hierarquia até achar ancestral existente
   (até `root_canonical`, que sempre existe por construção
   no `Jail::new`).
4. Canonicaliza o ancestral, valida `starts_with(root_canonical)`
   (symlinks pra fora detectados).
5. `create_dir_all(parent_of_final)` (idempotente).
6. Retorna path final.

**8 unit tests** em `workspace.rs::tests`:
- `create_parents_rejects_path_traversal` (JAIL)
- `create_parents_rejects_absolute_path` (JAIL)
- `create_parents_rejects_unc_path` (JAIL)
- `create_parents_delegates_to_resolve_when_path_exists`
  (caminho comum de "create_parents: true" em path que
  existe — substituição de arquivo)
- `create_parents_creates_intermediate_dirs`
  (`sub/utils/helper.py` em workspace com só `sub/`)
- `create_parents_creates_deeply_nested_dirs` (`a/b/c/d/file.txt`
  em workspace com só `a/`)
- `create_parents_works_at_workspace_root`
  (`newdir/newfile.txt` em workspace)
- `create_parents_is_idempotent` (chamar 2x não falha)

## `build_default_tools` + `build_default_allowed_for_run` bump atômico

A composição é **bump atômico** (ADR-0020 §3 D3: capability
+ permission atômicas). O branch `fase-7-etapa-5-files-write-edit`
foi criado a partir de `da9e98f2` (Etapa 3 merged, Etapa 4
em PR) — então a versão do `build_default_tools` é 1-arg
(`invoker: Option<Arc<dyn WorkerInvoker>>`), sem `exec_deps`.
**4 tools sem runtime** (`files.read` + `files.list` +
`files.write` + `files.edit`) / **6 tools com runtime**
(+ `docs.generate` + `docs.inspect`). Allowlist simétrica.
Quando o PR #46 for mergeado em `main`, o `git rebase` da
Etapa 5 sobre a Etapa 4 traz a versão 2-arg
(`invoker, exec_deps`) e os 2 tools do exec
(`exec.python` + `exec.node`). O rebase resolve o
`<<<<<<< Updated upstream` automaticamente (commit do
rebase `git fetch origin && git rev-parse origin/main`
+ `git checkout -b <nome> <sha-fresco>`, regra de PRs
empilhadas).

## Por que `validate_tool_call` + `Tool::execute` direto nos E2E

O `RunExecutor` enfileira o `ApprovalRequired` na
`approval_queue` e finaliza o run como `Cancelled` (caminho
B "pausar o run e continuar após resposta" não está
implementado — Etapa 6.2 da Fase 3). Pra não acoplar os E2E
a uma UI de aprovação que ainda não existe, o teste faz:

1. **`validate_tool_call` direto** (Passo 9 honrado: sem
   `ApprovalDecision` → `ApprovalRequired`; com → `Approved`).
2. **`FilesWriteTool::execute` / `FilesEditTool::execute`
   direto** com o `Jail` real do workspace temporário.

Isso prova o comportamento real (atomicidade, backup, hash,
path safety, `expected_sha256` race defense) sem depender
do loop `approval_queue`.

**Pendência nova:** caminho B do `approval_queue` (Etapa
6.2 da Fase 3) — necessário pra fazer E2E end-to-end do
`files.write`/`files.edit` via `ChatOrchestrator` no futuro.

## Decisões e trade-offs

- **`overwrite: false` por default (D2)**: o tool_call mais
  comum (criar arquivo novo) **funciona sem pergunta**; o
  tool_call destrutivo (sobrescrever) **pede opt-in
  explícito**. Sem isso, o usuário seria interrompido em
  90% dos calls (criar novo é o caso comum, não sobrescrever).
- **`create_parents: false` por default (D5)**: o tool_call
  mais comum (criar arquivo em dir existente) **funciona
  sem pergunta**; o tool_call que cria estrutura nova
  (`src/utils/helper.py` quando `src/utils/` não existe)
  **pede opt-in explícito**.
- **Find literal, não regex (D4)**: regex silenciosamente
  casa mais do que o usuário espera (`.` `*` `+` `?` `(` `[`
  `\`), e 95% do uso de `files.edit` é "substituir
  definição de função" / "mudar string de config" / "ajustar
  import". Regex fica pra Fase 8 (com `files.regex_edit` +
  UI de "test pattern").
- **`expected_sha256` é opcional, mas Etapa 5+ UI (ADR-0034 D5)
  vai exigir**: na v1, omitir = "aceito risco de race". A UI
  da Etapa 5+ da Fase 3 vai sempre passar o hash do
  `files.read` anterior. Sem isso, o modelo corrompe arquivo
  silenciosamente (regra do user).
- **Backup em colisão**: `<path>.bak` se livre;
  `<path>.bak.<timestamp ISO 8601>` (ex.: `20260810T104200Z`)
  em colisão. Em colisão improvável (2 escritas no mesmo
  segundo), sufixa com nanos — vai pro `.bak.<ts>.<nanos>`.
- **Audit com hashes, não conteúdo** (D6): SHA-256 hex
  lowercase 64 chars (mesmo formato `git hash-object` /
  `sha256sum`). Hashes no `result_json` do `ToolResult`
  (Passo 10 captura no `AuditEntry`). Hashes, não
  conteúdo — o log de auditoria vaza prova de "algo foi
  escrito", não a credencial que estava dentro (fecha T1
  do threat model).

## Pendências da Etapa 5 (registradas pra Etapa 5+ do Phase 7)

1. **Cross-volume write** (D7 do ADR-0035): rename entre
   volumes falha no Windows (`ERROR_NOT_SAME_DEVICE` 17).
   `temp_path` no mesmo dir evita em 99% dos casos. Pra
   cobrir 100%, Etapa 5+ detecta cross-volume via
   `fs::metadata(temp_path.parent()).dev() !=
   fs::metadata(path.parent()).dev()` e retorna erro claro.
2. **Lock por arquivo** (Etapa 8): race read-modify-write
   entre `files.edit` paralelo no mesmo path — a última
   escrita vence. A Etapa 8 (com UI de projeto) introduz
   lock por arquivo se virar problema real.
3. **UI de `expected_sha256` sempre passar** (Etapa 5+ da
   Fase 3): hoje é opcional, omitir = "aceito risco de
   race". A UI da Etapa 5+ da Fase 3 vai sempre passar o
   hash do `files.read` anterior (regra do ADR-0034 D5).
4. **`files.regex_edit`** (Fase 8): regex silenciosamente
   casa mais do que o usuário espera — fica pra Fase 8 com
   UI de "test pattern" (preview do match antes de aplicar).
5. **`files.move` / `files.delete`** (Etapa 5+ do Phase 7
   ou Fase 8): v1 cobre `read` + `list` + `write` + `edit`.
   Move e delete são destrutivos ainda mais perigosos
   (delete não tem backup trivial; move tem atomicidade
   cross-volume). Ficam pra Etapa 5+ ou Fase 8.
6. **Caminho B do `approval_queue`** (Etapa 6.2 da Fase 3)
   — pendência também registrada da Etapa 4. Hoje o run
   finaliza como `Cancelled` quando bate em `ApprovalRequired`;
   pra fazer E2E end-to-end via `ChatOrchestrator`, esse
   caminho precisa existir.

## Histórico de revisão

- 2026-08-10 — Etapa 5 fechada em 2 PRs. PR #45 (files.list)
  mergeado. PR #46 (files.write + files.edit) a abrir.
  4 regras críticas honradas (atomicidade, aprovação,
  `expected_sha256` race defense, testes de negação).
  26 E2E novos em `crates/e2e/tests/`. Suíte workspace
  verde local.
