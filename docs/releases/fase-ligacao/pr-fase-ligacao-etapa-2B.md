# PR 23 (Etapa 2.B da Fase de Ligação): trait `WorkerInvoker` + integração dos 3 kits no `ToolRegistry` + bump atômico do `documents: None → Full`

## Contexto (transparente)

A Etapa 2.A (PR #22, mergeada em `c0abe54`) introduziu o
`DocumentWorkerLauncher` (ADR-0023) com lazy start + restart on
death com teto + kill tree no app exit. O ADR-0023 §"Divisão
Etapa 2.A vs 2.B" deixou a Etapa 2.B registrada com escopo
claro: "o `WorkerHandle` é a `struct` central do IPC do
`process-architecture`, e mexer nela é trabalho que toca Fase 5
fechada. (...) É trabalho grande, e fazer isso dentro da Etapa
2.A mistura fase-ligação com Fase 5. (...) A Etapa 2.B é a
'integração' propriamente dita, e vai precisar do seu próprio
ADR (provavelmente ADR-0024) quando abrir."

A Etapa 2.B reavalia a proposta do `LauncherDispatcher` wrapper
no meio da implementação. **3 ramificações não previstas no
ADR-0023 forçaram uma decisão diferente**: (a) o `WorkerHandle`
e o `Launcher` têm ciclo de vida muito diferente (eagerly spawned
vs lazy); (b) o `ProcessError` carrega info do
`WorkerHealthSnapshot` do `process-architecture`, mas o
contrato genérico precisa morar no `core` (regra de pureza);
(c) a regra "contratos genéricos de worker não têm nada a ver
com registro de ferramentas — colocar no `tool-registry` criaria
uma dependência errada do `document-kits` em `tool-registry`".

A solução é o **trait `WorkerInvoker` no `frederico-core`** com
**erro `InvokeError` próprio**. O `WorkerHandle` (Fase 5) e o
`DocumentWorkerLauncher` (Etapa 2.A) implementam via adapters
separados. O caminho do `LauncherDispatcher` wrapper do ADR-0023
é descartado.

## ADR-0024 (novo) — 6 decisões

- **D1** — `WorkerInvoker` no `core` (não no `tool-registry`).
  Regra do user: contratos genéricos de worker não pertencem
  ao registro de ferramentas. Inversão hierárquica (camada de
  renderização dependendo de camada de catálogo) é o tipo de
  aresta que ninguém desfaz depois.
- **D2** — `InvokeError` próprio do `core` (não `ProcessError`).
  6 variantes: 5 mapeiam 1:1 do `ProcessError` (Protocol /
  Transport / Timeout / Cancelled→Unhealthy / Unhealthy /
  Platform); `PermanentlyDead` é específico do launcher (a UI
  precisa chamar `reset()` antes de tentar de novo).
- **D3** — `impl WorkerInvoker for WorkerHandle` no
  `process-architecture` (regra do Rust: orphan rule —
  `WorkerHandle` mora no `process-architecture`, o `impl` precisa
  estar lá). **Helper `process_to_invoke_error` inline
  duplicado em 3 lugares** (`process-architecture`, `app`, deletado
  do `tool-registry`): ~25 linhas duplicadas, preço de manter o
  grafo de dependências limpo (o `core` não pode importar
  `process-architecture` pela regra de pureza).
- **D4** — `impl WorkerInvoker for DocumentWorkerLauncher` no
  `app`. O `Arc<dyn WorkerInvoker>` é construído **localmente**
  no `setup` (não mora no `AppState` — seria `dead_code`). O
  mesmo `Arc<DocumentWorkerLauncher>` atende aos dois papéis:
  invoker (pro `build_default_tools`) e tipo concreto (pros
  `tauri::command document_worker_*` que precisam de `status()`
  e `reset()`).
- **D5** — Bump atômico do contrato em 1 PR: 4 commits
  consecutivos mas **obrigatórios** (cada um é uma camada do
  grafo: `core` → `process-architecture` → `tool-registry` →
  `document-kits`). A invariante preservada: **em qualquer
  commit intermediário o workspace não compila** (a ordem é
  obrigatória). Força o reviewer a olhar a mudança como
  atômica, não como 4 mudanças separadas.
- **D6** — Guarda automatizada `scripts/check-fase-5-untouched.ps1`.
  A Etapa 2.B **integra** os 3 kits ao `ToolRegistry`, mas
  **não mexe** no `document-worker` Python da Fase 5. O script
  compara `git diff --stat origin/main..HEAD` em 3 arquivos
  sensíveis do worker (`test_pdf_audit.py`, `document-worker.py`,
  `generate_srgb_icc.py`); se algum mudou, exit 1 com mensagem
  explicando o atravessamento de fronteira. "Valeria virarem
  passo de script em vez de comando manual" virou passo de
  script — roda em todo CI e em todo pre-push manual.

## O que entra (4 commits do contrato + 1 commit da casca + 1 docs/scripts)

### Commits 1-4 (bump atômico do contrato, ADR-0024 §D5)

- **Commit 1 (`954e79b`)** — `feat(core): trait WorkerInvoker +
  erro proprio InvokeError`. `crates/core/src/worker_invoker.rs`
  novo, 3 unit tests. Re-exports em `lib.rs`. Adiciona
  `serde_json` + `async-trait` em `Cargo.toml`.
- **Commit 2 (`1c777b8`)** — `feat(process-architecture): impl
  WorkerInvoker for WorkerHandle (regra de pureza mantida)`.
  `crates/process-architecture/src/worker_invoker_impl.rs` novo,
  4 unit tests. Adiciona `frederico-core` em `Cargo.toml`.
  Helper `process_to_invoke_error` inline (duplicado em 3
  lugares — `process-architecture`, `app`, deletado do
  `tool-registry` — pra evitar ciclo).
- **Commit 3 (`5d0d26f`)** — `feat(tool-registry):
  WorkerToolDispatcher aceita Arc<dyn WorkerInvoker> (bump
  atomico do contrato)`. `DispatchError::Process(ProcessError)`
  → `DispatchError::Invoke(InvokeError)`. Acessor `handle()` →
  `invoker()`.
- **Commit 4 (`8800d26`)** — `feat(document-kits): 3 kits +
  KitError aceitam Arc<dyn WorkerInvoker> e InvokeError (bump
  atomico)`. 10 arquivos: `WordPro`/`ExcelPro`/`PdfPro` `new(handle:
  Arc<dyn WorkerInvoker>)`, `kit.rs` `Process(InvokeError)`,
  generate/inspect `DispatchError::Process(_)` →
  `DispatchError::Invoke(_)`. 4 E2E tests atualizados pra
  `WorkerToolDispatcher::new(Arc::new((*handle).clone()), vec![])`.

### Commit 5 (atualização dos tests do `frederico-app`)

`crates/app/src/composition.rs` — 2 tests atualizados pra nova
assinatura `Option<Arc<dyn WorkerInvoker>>`:

- `build_default_tools_with_invoker_returns_three_tools`
  (renomeado de `_with_runtime_still_returns_files_read_only`)
  — helper `fake_invoker()` constrói um `WorkerHandle` real via
  `WorkerManager::spawn_in_process(FakeWorkerConfig::default(),
  WorkerSpawnConfig::default())` (mesmo padrão dos E2E do
  `document-kits/src/generate.rs:489-494`). Assert de 1 tool
  → 3 tools.
- `build_default_allowed_for_run_with_invoker_includes_documents`
  (renomeado de `_with_runtime_includes_documents`) — usa o
  mesmo `fake_invoker()` e checa que a allowlist contém os 3
  `ToolId`s.

### Commit 6 (casca Tauri + scripts + docs)

- `apps/desktop/src-tauri/src/main.rs` — bump atômico do
  `documents: None → Full` (ADR-0020 §3 D3): quando o
  `DocumentWorkerLauncher` está disponível, as 3 funções de
  composição (`build_default_tools`,
  `build_default_allowed_for_run`, `initial_permission_set*`)
  recebem a **mesma** `Option<Arc<dyn WorkerInvoker>>` —
  quando `Some`, os 2 tools do `document-worker` entram no
  `ToolRegistry`, os 2 `ToolId`s entram na allowlist, e
  `documents` vira `Full`; quando `None`, em nenhum dos três
  lugares. **A simetria é o que garante que o modelo nunca
  vê um tool que não consegue invocar** (degradação declarada,
  não substituição silenciosa).
- `scripts/check-fase-5-untouched.ps1` (novo) — guarda
  automatizada (ADR-0024 §D6).
- `docs/decisions/0024-worker-invoker-trait.md` (novo) — 6
  decisões, 3 alternativas consideradas (especialmente o
  `LauncherDispatcher` wrapper do ADR-0023 que foi
  descartado).
- `CHANGELOG.md` — entrada "Fechado — Fase de Ligação,
  Etapa 2.B" no topo do "Não publicado", com o resumo de 5
  commits e o D4 (`.exe` instalado) nomeado explicitamente.
- `docs/releases/fase-ligacao/README.md` — atualizado pra
  marcar Etapa 2.B como fechada, índice com PR #23.

## Bump atômico do `documents: None → Full` (ADR-0020 §3 D3)

A casca Tauri é o **ponto de simetria** entre as 3 funções de
composição. Quando o `DocumentWorkerLauncher` está disponível:

- `build_default_tools(Some(invoker))` retorna
  `[FilesReadTool, DocsGenerateTool, DocsInspectTool]`.
- `build_default_allowed_for_run(Some(invoker))` retorna
  `["files.read", "docs.generate", "docs.inspect"]`.
- `initial_permission_set_for_capable_launcher()` retorna
  `documents: Full`.

Quando o launcher é `None` (runtime ausente em produção sem
`bundle.resources`, ou em dev sem `bootstrap.ps1`):

- `build_default_tools(None)` retorna só `[FilesReadTool]`.
- `build_default_allowed_for_run(None)` retorna só
  `["files.read"]`.
- `initial_permission_set()` retorna `documents: None`
  (default deny).

**A simetria é o que garante que o modelo nunca vê um tool
que não consegue invocar.** Se o `ToolRegistry` registrasse
`docs.generate` mas a allowlist não tivesse o `ToolId`, o
`RunExecutor` rejeita invocação com `ToolNotAllowed`. Se a
allowlist tivesse o `ToolId` mas o `PermissionSet` negasse
`documents`, o `validate_tool_call` rejeita. **Tudo atômico.**

## Status honesto

A Etapa 2.B fecha o **caminho do modelo**:
`ChatOrchestrator → ToolRegistry → docs.generate → kit
(WordPro/ExcelPro/PdfPro) → WorkerToolDispatcher → WorkerInvoker
(WorkerHandle ou DocumentWorkerLauncher) → document-worker`. O
`docs.generate` agora aparece no schema do modelo, e o
`docs.inspect` também. Suíte do `frederico-app` continua
**32/32 verde**, workspace (excluindo `process-architecture` com
2 testes de OCR flaky) **533/533 verde**.

O que a Etapa 2.B **NÃO fecha**:

- **E2E atravessando a casca** (`tests/e2e/` na raiz) — vai
  ser Etapa 5 da fase-ligação. Os tests E2E existentes em
  `crates/document-kits/tests/e2e_*` exercitam o kit + IPC
  via `WorkerToolDispatcher` direto, **não** atravessam a
  casca Tauri. O caminho de produção end-to-end (modelo →
  casca → worker → arquivo) ainda não foi provado em
  CI automatizado.
- **Caminho empacotado (.exe)** — D4 do ADR-0023, fecha na
  Fase 9 do PROMPT MESTRE. Em produção, o resolvedor de
  runtime retorna `None` (sem `bundle.resources`), a UI
  mostra "indisponível" no diagnóstico.

## Lições de processo

- **Bump atômico do contrato em 1 PR (4 commits obrigatórios)**
  funcionou como previsto. O reviewer vê a mudança como
  atômica (1 PR), e a história (4 commits por camada do grafo)
  fica no log. A invariante "em qualquer commit intermediário
  o workspace não compila" força o review a olhar a mudança
  inteira, não só o último commit.
- **Helper `process_to_invoke_error` duplicado em 3 lugares**
  é o preço de manter o grafo limpo. Documentado em ADR-0024
  §D3 e no próprio helper — quem ler daqui a 6 meses vai
  entender a razão. O risco de divergência é baixo (mudança
  rara).
- **`Arc<dyn WorkerInvoker>` local no `setup` (não no
  `AppState`)** evita `dead_code` warning. O mesmo
  `Arc<DocumentWorkerLauncher>` atende aos 2 papéis (invoker
  pro `ToolRegistry`, tipo concreto pros `tauri::command
  document_worker_*`).
- **Guarda automatizada `check-fase-5-untouched.ps1`** —
  "valeria virarem passo de script em vez de comando manual"
  virou passo de script. Roda em todo CI e em todo pre-push
  manual. É o tipo de coisa que ninguém desfaz depois.

## Pendências

- **D4 do ADR-0023** (`.exe` instalado) — fecha na Fase 9.
- **E2E atravessando a casca** (Etapa 5 da fase-ligação) —
  vai precisar de tag explícita no `docs/status.md` quando
  fechar (D5 do ADR-0023: "Etapa 5 da fase-ligação prova o
  kit e o IPC, não o empacotamento").
- **`PermanentlyDead` no `InvokeError`** não tem mapeamento
  1:1 pro `ProcessError` (o `WorkerHandle` nunca está
  "permanentemente morto" — o caller recria o `WorkerManager`
  quando morre). Documentado em ADR-0024 §"Pendências".
  Aceitável.
