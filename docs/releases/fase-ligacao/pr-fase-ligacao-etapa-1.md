# PR 21 (Etapa 1 da Fase de Ligação): caminho de produção agora consome o que a suíte dos crates já provava

## Contexto (transparente)

A Fase 5 fechou o `document-worker` (kit DocumentSpec, 3 formatos
de arquivo, 8 handlers, auditoria estrutural bloqueante) mas o
caminho de produção do app **ainda não consumia nada disso**. O
diagnóstico do prompt da Fase de Ligação listou 3 buracos:

1. `ToolRegistry::new()` ficava vazio (manifestos não chegavam
   ao registro) — divergência "manifesto à mão vs. tool real"
   do §5.2 do projeto anterior.
2. `PermissionSet::default()` deny-all hardcoded no
   `orchestrator.rs:260` — bloqueava o Passo 5 do
   `validate_tool_call` antes mesmo do jail entrar em jogo.
3. `Jail::new(std::env::current_dir()?)` no
   `apps/desktop/src-tauri/src/main.rs:203` — jail no cwd do
   app, não por conversa. Mesma classe de I4 da memória
   (vazamento entre escopos).

Esta PR fecha os 3 itens com 5 commits encadeados + 1 fix-up
de teste (commit 6, do rebase contra `eab413a`) + 1
registro no CHANGELOG.

## A saga do rebase (lição de processo, 3ª ocorrência)

A branch `fase-ligacao/conectar-motor-a-casca` foi criada em
2026-08-01 e ramificou de `d518226` (PR #15 = PDFPro skeleton
merged). Naquele momento, os PRs #17 (PDFPro v0.1 real),
#19 (auditoria estrutural) e #20 (promoção Fase 5) ainda
estavam em voo. Quando o último entrou no main (`eab413a`,
2026-08-02), a branch já estava congelada com 7 commits
locais e base errada.

**Sintoma:** `git diff origin/main..HEAD` mostrou
-5.053/+3.107 linhas, com a auditoria estrutural do PR #19
sumindo (1.207 linhas a menos em `pdfpro.rs`, 540 em
`test_pdf_audit.py`, 1.730 em `document-worker.py`).
Mesclar como estava seria reverter o PR #19 por outro
caminho — exatamente o que a regra "PRs empilhadas: só abra
a próxima depois que a anterior entrou em main" advertiu.

**Caminho escolhido:** `git rebase origin/main` (rebase
padrão, branch é local-only, sem força-push em branch
compartilhada). 2 conflitos, ambos no `CHANGELOG.md`
esperados pelo prompt da fase ("CHANGELOG e status são
conflito de texto trivial: mantenha as duas entradas"):

1. Conflito entre `## [Não publicado]` e o `### Alterado —
   breaking` do commit 4a. **Resolução:** mantenho as duas
   seções, com o commit 4a virando subseção dentro do "Não
   publicado".
2. Conflito entre o `### Fechado — Fase de Ligação, Etapa 1`
   do commit 7 e o `### Alterado — breaking` do commit 4a.
   **Resolução:** o `### Fechado` entra antes do
   `### Alterado`, mantendo a ordem cronológica da leitura.

Resultado: 7 commits coesos preservados, 39 arquivos
modificados (+2.852 / -183 linhas), **zero reversão** da
auditoria estrutural do PR #19. O diff nos 4 arquivos
críticos da auditoria (`pdfpro.rs`, `test_pdf_audit.py`,
`document-worker.py`, `generate_srgb_icc.py`) é **vazio** —
o PR #19 continua intacto.

**Causa raiz:** a branch saiu de `d518226` quando três PRs
ainda estavam em voo. **Regra reforçada (3ª ocorrência):**
branch nova sai de `origin/main` recém-buscado, sempre. A
versão da lição era "PR empilhada tem que ter a base trocada
para main antes do merge", mas a Etapa 1 da Fase de Ligação
provou que a versão **preventiva** (ramo base sempre em main
fresco) é o que evita o rebase custoso.

## O que entra (5 commits da Etapa 1 + 1 fix-up + 1 CHANGELOG)

### Commit 1 (`42fdae8`) — `docs: move narrativas de PRs da Fase 5 para docs/releases/fase-5/`

Limpa a raiz do `docs/` (4 arquivos `pr*-description.md`
soltos viram `pr-NN-titulo.md` indexados por `README.md`).
Conteúdo de processo (não spec) vai pra `docs/releases/`,
preservando as lições (sobretudo a saga PR #16→#19) sem
poluir a raiz. Decide o padrão `docs/releases/fase-N/`
que esta própria PR segue.

### Commit 2 (`faee4ca`) — `feat(app): adiciona crate frederico-app (camada de composicao pura) + ADR-0022`

`crates/app/` como workspace member, sem `tauri`/`windows`
(verificado por `scripts/check-core-purity.ps1`). A decisão
de manter o crate puro por construção é o que permite
reusar `build_chat_orchestrator` no modo servidor do PROMPT
MESTRE §5.5 (VPS / headless) sem fork.

ADR-0022 documenta 4 decisões desta Etapa:
- **D1**: `frederico-app` puro por construção (passa no
  purity gate).
- **D2**: `JailResolver` trait + `FileSystemJailResolver`
  (Etapa 1).
- **D3**: `ToolContext` `#[non_exhaustive]` carrega
  `conversation_id` por tool_call (resolvido uma vez no
  `RunExecutor`, não por call).
- **D4**: composição via `frederico_app`, casca consome.

Skeleton mínimo: `lib.rs` documenta a intenção (dois
módulos: `jail` no commit 3, `composition` no 4b) e expõe
uma função `version()`. Lógica de composição propriamente
dita entra nos commits 3, 4a, 4b e 5. Manter o skeleton
mínimo serve a dois propósitos: (a) `cargo test
--workspace` continua verde desde o commit 2; (b) a revisão
pode ler a forma do contrato antes do comportamento entrar.

### Commit 3 (`2ba7b62`) — `feat(app): adiciona JailResolver trait + FileSystemJailResolver (erro duro, sem fallback)`

O ponto de entrada para o workspace per-conversa.
`JailResolver` trait recebe um `&ConversationId` e devolve
um `Jail` construído em cima de
`<workspaces_root>/<conversation_id>/`. A implementação
default (`FileSystemJailResolver`) cria o diretório sob
demanda via `mkdir -p` idempotente.

**Decisão crítica:** falha ao resolver o jail é erro duro.
O código antigo tinha fallback para `temp_dir` no
`apps/desktop/src-tauri/src/main.rs:203` — degradação
silenciosa num caminho de isolamento: se o workspace da
conversa não pode ser criado, os arquivos do usuário vão
para um diretório compartilhado, e o jail por conversa
deixa de existir. Este commit remove o fallback: o
`FileSystemJailResolver::resolve` propaga
`JailResolverError` quando o `mkdir` ou o `Jail::new`
falham, com o `conversation_id` e o `io::Error` no enum.
O caller mapeia para `ToolResult::err` com mensagem PT-BR.

A interface do trait é estável. A Etapa 7 (modo
desenvolvedor) substitui por `SecurityJailResolver` (via
`frederico-security` com Job Objects + AllowVolumeAccess)
sem mudar `ChatOrchestrator`, `RunExecutor` nem
`FilesReadTool`. A troca é drop-in.

7 testes novos (total 8 no crate):
- `resolve_creates_workspace_dir`
- `resolve_is_idempotent`
- `resolve_creates_independent_dirs_per_conversation`
  (regressão contra vazamento entre escopos)
- `jail_returned_accepts_path_inside_workspace`
- `jail_returned_rejects_parent_dir` (regressão do I3
  com `..`)
- `jail_returned_rejects_absolute_windows_path`
  (regressão I3 com caminho absoluto Windows)
- `version_is_set`

### Commit 4a (`db03884`) — `feat(tool-registry): Tool::execute recebe ToolContext + RunExecutor usa JailResolver`

Breaking change da Etapa 1. Implementa D2, D3 e parte de
D4 do ADR-0022 (com ajuste em D2 documentado no ADR).

**O que entra:**

1. `frederico-tool-registry::JailResolver` trait (no
   toolkit, não no `frederico-app` — ajuste do ADR-0022
   §D2 registrado com nota "Alterado em relação ao plano
   original") + `JailResolverError` + `StaticJailResolver`
   (helper para testes / fase de transição, sempre devolve
   o mesmo `Jail`).
2. `frederico-tool-registry::ToolContext` (com
   `#[non_exhaustive]`) carregando `conversation_id`,
   `run_id`, `message_id` e `Jail` resolvido.
3. `Tool::execute` muda de `async fn execute(&self, args)`
   para `async fn execute(&self, ctx: &ToolContext, args)`.
   Breaking change consciente: os 4 `Tool` concretos
   (`FilesReadTool`, `DocsGenerateTool`, `DocsInspectTool`,
   e a chamada em `e2e_docs_generate_pdf.rs` que **não
   foi atualizada pelo commit original** — virou o fix-up
   do commit 6 depois do rebase) foram atualizados.
4. `RunExecutor::new` recebe `Arc<dyn JailResolver>` em
   vez de `Jail` direto. O `Jail` efetivo é resolvido
   **uma vez por run** no início do `run()` via
   `RunRepo::get(run_id)` (extrai `conversation_id`) +
   `JailResolver::resolve(&conversation_id)`. Cacheado
   em `cached_jail` no `RunExecutor` para uso pela
   validação (Passo 7 do `validate_tool_call`) e pelo
   `ToolContext` por tool_call. Custo O(1) por chamada,
   sem I/O no caminho quente.
5. `ChatOrchestrator` ganha campo `jail_resolver:
   Arc<dyn JailResolver>` (substitui `jail: Jail`). A
   composição via `frederico_app::build_chat_orchestrator`
   é o commit 5 (esta commit deixa a casca usando
   `FileSystemJailResolver` direto, sem
   `build_chat_orchestrator` — composição "limpa" no
   commit 5).
6. **Casca Tauri** substitui
   `Jail::new(std::env::current_dir()?)` (linha 203,
   pré-existente) por
   `FileSystemJailResolver::new(<data_local_dir>/workspaces/)`.
   Falha de `mkdir` é erro duro (sem fallback para
   `temp_dir`).

**Bug encontrado pelo rebase (commit 6):** o
`e2e_docs_generate_pdf.rs` (criado no PR #17, posterior a
`d518226` que era a base original da branch) não foi
atualizado quando o commit 4a quebrou `Tool::execute` em
`&ctx, &args`. O autor do 4a atualizou os outros 3 testes
E2E mas não este, porque o arquivo ainda não existia no
HEAD da branch naquele momento. O rebase contra `eab413a`
trouxe o arquivo para dentro do diff, expondo a
desatualização. **Commit 6 desta PR** adiciona o helper
`dummy_ctx()` (mesmo do `e2e_docs_generate.rs`) e propaga
`&dummy_ctx()` nas 3 chamadas.

### Commit 4b (`485792d`) — `feat(app): build_tool_registry + initial_permission_set em frederico-app`

Funções de composição da Etapa 1 que o commit 4a preparou
mas não consumiu. A casca Tauri ainda monta o
`ChatOrchestrator` inline; o commit 5 substitui isso por
`frederico_app::build_chat_orchestrator`.

**O que entra:**

1. `frederico_app::composition::build_tool_registry(tools:
   &[Arc<dyn Tool>]) -> ToolRegistry` — itera sobre
   `tool.manifest()` e registra cada manifesto no
   `ToolRegistry`. Como o método `manifest()` é
   obrigatório na trait `Tool`, é impossível ter tool sem
   manifesto (o divergente de inventário do §5.2 do projeto
   anterior está fechado mecanicamente). `tools: &[]`
   devolve um `ToolRegistry` vazio (estado antes do
   `docs.generate` ser registrado na Etapa 2). **Mesma
   função que a casca e os E2E da raiz chamam** (regra do
   prompt da fase).
2. `frederico_app::composition::initial_permission_set()
   -> PermissionSet` — configuração fixa e explícita da
   Etapa 1: `file_read: WorkspaceOnly` (habilita o
   `files.read` dentro do jail por conversa, Etapa 1
   commit 4a), todo o resto deny incluindo `documents:
   None`. O bump para `DocumentPermission::Full` entra no
   commit da Etapa 2 que registra `docs.generate` +
   `docs.inspect`, no mesmo commit (bump atômico do
   ADR-0020 §3 D3: capability + permissão atômicas).
   Substitui o `PermissionSet::default()` deny-all
   hardcoded no `orchestrator.rs:260`.
3. `frederico_app::composition::ChatOrchestratorParts` —
   struct que agrupa os 11 args do `ChatOrchestrator::new`
   (decisão da conversa da Etapa 1: "campo na struct, não
   parâmetro de `new()`"). O `build_chat_orchestrator(parts)`
   entra no commit 5.

5 testes novos (total 12 no crate):
- `build_tool_registry_empty_returns_empty`
- `build_tool_registry_with_one_tool_registers_manifest`
- `build_tool_registry_with_same_tool_twice_does_not_dedupe`
  (comportamento do `HashMap::insert` no `register` —
  substitui em vez de duplicar, intencional pra hot-reload
  futuro)
- `initial_permission_set_enables_files_read_only` (smoke
  dos 18 campos do `PermissionSet`)
- `initial_permission_set_differs_from_default` (regressão
  contra o `default()` deny-all)

### Commit 5 (`e5e1728`) — `feat(desktop): casca consome frederico_app::build_chat_orchestrator; ChatOrchestrator recebe permission_set real`

Fecha a Etapa 1: a casca Tauri agora usa o caminho de
composição `frederico_app::build_chat_orchestrator(parts)`
em vez de montar o `ChatOrchestrator` inline com 12 args
posicionais. O `PermissionSet` real (carregado de
`initial_permission_set()`) substitui o
`PermissionSet::default()` deny-all hardcoded.

**O que entra:**

1. `ChatOrchestrator` ganha campo `permissions:
   PermissionSet` — substitui o `PermissionSet::default()`
   deny-all que estava no `tokio::spawn` do `send_message`
   (linha 260 do `orchestrator.rs`, pré-existente em `main`
   commit `eab413a`). O `ChatOrchestrator::new` agora
   recebe `permission_set: PermissionSet` como 11° argumento
   (antes do `memory_extractor`).
2. `frederico_app::composition::build_chat_orchestrator(parts)
   -> ChatOrchestrator` — função de composição pura (sem
   I/O) que monta o `ChatOrchestrator` a partir de
   `ChatOrchestratorParts`. **Esta é a única forma de
   construir o `ChatOrchestrator` na Fase de Ligação**:
   a casca Tauri e o modo servidor §5.5 chamam esta
   função, e os E2E da raiz (`tests/e2e/`, Etapa 5)
   também. Garante a regra do prompt da fase: "os testes
   usam a mesma função da casca".
3. **Casca Tauri** substitui a montagem inline (8+ args
   posicionais) por um único bloco que monta
   `ChatOrchestratorParts` e chama
   `build_chat_orchestrator`. O `tool_registry` é
   construído a partir de `build_tool_registry(tools)` —
   não há mais `ToolRegistry::new()` solto.
4. **Tests do `execution-engine`** (4 arquivos:
   `integration_orchestrator.rs`, `provider-engine/tests/
   recovery.rs`, mais 9 ajustes em tests de
   `approval_queue`, `audit`, `cancel`, `e2e`,
   `recovery`, `watchdog`) atualizados para passar o
   `permission_set` ao `ChatOrchestrator::new` ou usar
   `static_jail_resolver` no `RunExecutor::new`. Os tests
   usam `PermissionSet::default()` (deny-all) — são tests
   de caminho de erro, não da feature de permissão em si.

### Commit 6 (`cc8e6b0`) — `test(document-kits): propaga &dummy_ctx em e2e_docs_generate_pdf apos breaking change de Tool::execute`

Fix-up introduzido pelo rebase (explicado no commit 4a
acima). Adiciona `dummy_ctx()` no
`crates/document-kits/tests/e2e_docs_generate_pdf.rs` e
propaga `&dummy_ctx()` nas 3 chamadas `tool.execute(...)`.
Suite do `frederico-document-kits` continua 84/84 verde.

### Commit 7 (`b3c42d6`) — `docs(changelog): registra Etapa 1 da Fase de Ligacao como fechada`

Entrada "Fechado" no topo do `CHANGELOG.md` cobrindo os 5
commits da Etapa 1 + o sumário do que mudou + as pendências
que vão pras próximas etapas (2-7) + o aviso dos 2 testes
de OCR pré-existentes que continuam vermelhos (Tesseract
ausente, não relacionado a esta fase).

## Decisões (ADR-0022 + ajustes de plano)

1. **D1 — `frederico-app` puro por construção.** Passa no
   `check-core-purity.ps1` automaticamente. Reusável no
   modo servidor §5.5 (VPS / headless) sem fork. A
   alternativa "frederico-app" dentro de `apps/desktop/`
   acoplava a composição à casca Tauri e exigia abstração
   por trás pra reusar no servidor — overhead de YAGNI.
2. **D2 — `JailResolver` trait mora no
   `frederico-tool-registry` (não no `frederico-app` como
   o plano original do ADR-0022 §D2 dizia).** Ajuste
   arquitetural registrado com nota "Alterado em relação
   ao plano original: motivo" no ADR-0022 §D2. A
   `FilesReadTool` precisa de uma referência ao trait, e
   o `frederico-tool-registry` não pode depender do
   `frederico-app` (seria ciclo). O trait é abstração
   pura do toolkit; a impl é decisão de composição.
3. **D3 — `ToolContext` `#[non_exhaustive]` carrega o
   `Jail` resolvido (não só os IDs).** O `Jail` efetivo
   é resolvido **uma vez por run** no `RunExecutor`, não
   por tool_call. Custo O(1) por chamada, sem I/O no
   caminho quente. **Risco (b) documentado:** não
   consultar `conversation_id` por tool_call (overhead
   desnecessário).
4. **D4 — composição via `frederico_app`, casca
   consome.** A casca Tauri nunca monta `ChatOrchestrator`
   inline na Fase de Ligação. `build_chat_orchestrator(parts)`
   é a única porta de entrada — a casca e os E2E da
   raiz chamam a mesma função.
5. **Erro duro, sem fallback para `temp_dir`.**
   `FileSystemJailResolver` propaga `JailResolverError`
   quando o `mkdir` falha. Mesma classe de bug do
   "interruptor de auditoria" do PR #19: degradação
   silenciosa num caminho de isolamento é pior que erro
   duro.
6. **`ChatOrchestrator::new` recebe `permission_set`
   como 11° argumento, não como campo em struct
   intermediário.** Decisão tomada na conversa da Etapa
   1: "campo na struct, não parâmetro de `new()`" — o
   `ChatOrchestratorParts` é a struct que agrupa os 11
   args, e `build_chat_orchestrator(parts)` é a função
   pura que monta. Manter a struct + a função
   Composition (em vez de aceitar 11 args soltos) é o
   que permite o `build_tool_registry` + o
   `initial_permission_set` ficarem testáveis
   independentemente.
7. **Reescrita do ADR-0022 §D2** durante a Etapa 1:
   o `JailResolver` trait mudou de lugar
   (`frederico-app` → `frederico-tool-registry`). Em vez
   de esconder, registramos com nota explícita "Alterado
   em relação ao plano original: motivo" no ADR. Mesma
   postura do ADR-0021 quando o PR 1 da Fase 5
   reverteu o bump atômico do enum (`5c39bac`).

## Limitações e riscos (honestos)

1. **Casca Tauri ainda não chama `MemoryExtractor` com
   `CompletionProvider` real** (OpenRouter + gpt-4o-mini)
   — fica para a Etapa 3. Sem o completion real, a
   extração de memória roda com stub.
2. **Casca Tauri ainda não injeta o embedding adapter
   real** (OpenRouter + `text-embedding-3-small`) — fica
   para a Etapa 3. Sem o embedding real, o índice vetorial
   é mock.
3. **`frederico-agent-engine` ainda tem `apply_transition`
   não decidido** (manter / remover / promover) — fica
   para a Etapa 4.
4. **Não há `tests/e2e/` na raiz atravessando a casca**
   — fica para a Etapa 5. Os tests atuais validam o
   motor (`crates/`) e os handlers (`workers/`), mas
   ninguém testa a casca chamando o `build_chat_orchestrator`
   end-to-end.
5. **`docs.generate` / `docs.inspect` ainda não estão
   registrados no `build_tool_registry` da casca** —
   fica para a Etapa 2. Sem isso, o modelo **não tem
   `documents` no permission set** (continua
   `documents: None` no `initial_permission_set()`).
6. **2 testes E2E de OCR no
   `frederico-process-architecture` continuam
   vermelhos** (`e2e_ocr_run_with_real_image`,
   `e2e_pdf_read_with_ocr_fallback_on_scanned`) —
   pré-existente, precisam de Tesseract instalado via
   `bootstrap.ps1` em contexto Admin. Verificado no
   commit `eab413a` (main) também falha. Cobertura
   desses cenários em CI depende do job "Bootstrap
   document-worker" do `.github/workflows/ci.yml`.

## Validação (local, antes do push)

- `cargo test --workspace --exclude
  frederico-process-architecture`: **315 passed, 0
  failed, 0 ignored** em 25 grupos (excluindo os 2
  pré-existentes de OCR).
- `cargo fmt --all -- --check`: limpo.
- `cargo clippy --workspace --all-targets --exclude
  frederico-process-architecture -- -D warnings -D
  clippy::await_holding_lock`: limpo.
- `node scripts/check-docs.mjs`: OK (cabeçalhos,
  carimbos, trava §1.13, links internos).
- `./scripts/check-core-purity.ps1`: OK — `frederico-app`
  puro por construção.
- Suíte do `frederico-document-kits` continua 84/84
  verde (35 Etapa 3 + 49 Etapa 4).
- **CI ainda não rodou pra esse delta específico** —
  a PR é a primeira vez que o delta da Etapa 1 vai ser
  exercitado end-to-end em CI. **Por isso a importância
  de usar SQUASH no merge**: preserva a atomicidade da
  Etapa 1.

## Próximas etapas (registradas em `docs/releases/fase-ligacao/README.md`)

- **Etapa 2:** ligar `frederico-document-kits` como dep
  da casca + registrar `docs.generate` + `docs.inspect`
  no `build_tool_registry` + bump atômico do `documents`
  permission.
- **Etapa 3:** injetar embedding adapter real (OpenRouter
  + `text-embedding-3-small`) + `CompletionProvider` real
  no `LlmMemoryClassifier` (OpenRouter + gpt-4o-mini).
- **Etapa 4:** decidir `frederico-agent-engine` (promover
  `apply_transition` ou remover o crate).
- **Etapa 5:** `tests/e2e/` na raiz atravessando a casca.
- **Etapa 6:** nova regra de "definição de pronto" no
  `REGRAS-DO-PROJETO.md` + gate no CI + compressão de
  `docs/status.md`/`CHANGELOG.md`.
- **Etapa 7 (modo desenvolvedor):** `SecurityJailResolver`
  via `frederico-security` (Job Objects +
  AllowVolumeAccess) substitui `FileSystemJailResolver`
  (interface do trait é estável, troca é drop-in).

## Instrução de merge

**MERGE COM SQUASH** (não "Merge commit", não "Rebase
and merge"). Razão: o source branch contém 7 commits de
implementação + 1 fix-up de teste; squash preserva a
atomicidade da Etapa 1 e mantém o `main` com um único
commit de bump por fase/etapa, igual à Fase 5 (PR #20).

## Arquivos modificados (40 files, +2900 / −198)

```
 CHANGELOG.md                                       | 196 +++++++++++-
 Cargo.lock                                         |  19 ++
 Cargo.toml                                         |   2 +
 apps/desktop/src-tauri/Cargo.toml                  |   1 +
 apps/desktop/src-tauri/src/main.rs                 |  81 +++--
 crates/app/Cargo.toml                              |  23 ++
 crates/app/src/composition.rs                      | 263 ++++++++++++++++
 crates/app/src/jail.rs                             | 337 +++++++++++++++++++++
 crates/app/src/lib.rs                              |  93 ++++++
 crates/document-kits/src/generate.rs               |  66 ++--
 crates/document-kits/src/inspect.rs                |   6 +-
 crates/document-kits/tests/e2e_docs_generate.rs    |  37 ++-
 crates/document-kits/tests/e2e_docs_generate_pdf.rs|  48 ++++++--
 crates/document-kits/tests/e2e_docs_generate_xlsx.rs|  37 ++-
 crates/document-kits/tests/e2e_docs_inspect.rs     |  28 +-
 crates/execution-engine/src/executor.rs            |  88 +++++-
 crates/execution-engine/src/orchestrator.rs        |  39 ++-
 crates/execution-engine/tests/approval_queue.rs    |   4 +-
 crates/execution-engine/tests/audit.rs             |   4 +-
 crates/execution-engine/tests/cancel.rs            |  12 +-
 crates/execution-engine/tests/e2e.rs               |  43 +--
 crates/execution-engine/tests/integration_orchestrator.rs | 4 +-
 crates/execution-engine/tests/recovery.rs          |  10 +-
 crates/execution-engine/tests/watchdog.rs          |   2 +-
 crates/provider-engine/tests/recovery.rs           |   5 +-
 crates/tool-registry/Cargo.toml                    |   1 +
 crates/tool-registry/src/jail_resolver.rs          | 203 +++++++++++++
 crates/tool-registry/src/lib.rs                    |  15 +
 crates/tool-registry/src/tools/files_read.rs       | 107 ++++---
 crates/tool-registry/src/tools/mod.rs              |  84 ++++-
 docs/architecture/tool-registry-specification.md   |  81 ++++-
 docs/decisions/0022-jail-resolver-v1.md            | 283 +++++++++++++++++
 docs/modules/app.md                                | 190 ++++++++++++
 docs/modules/execution-engine.md                   |  31 +-
 docs/modules/tool-registry.md                      |  25 +-
 docs/releases/fase-5/README.md                     |  37 +++
 docs/releases/fase-5/pr-17-pdfpro-v01-real.md      |  83 +++++
 docs/releases/fase-5/pr-19-auditoria-estrutural.md | 301 ++++++++++++++++++
 docs/releases/fase-5/pr-19-correcao-base.md        | 112 +++++++
 docs/releases/fase-5/pr-20-promocao-fase5-concluida.md| 82 +++++
 docs/releases/fase-ligacao/README.md               |  ??
 docs/releases/fase-ligacao/pr-fase-ligacao-etapa-1.md | ??
```

## Histórico relevante

- **Esta PR (#21)** — Etapa 1 da Fase de Ligação.
- Próxima PR (Etapa 2) — abre depois que esta entrar em
  main (regra de PRs empilhadas, 3ª ocorrência confirmada).
