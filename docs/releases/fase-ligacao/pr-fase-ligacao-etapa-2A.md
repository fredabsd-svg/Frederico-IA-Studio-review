# PR 22 (Etapa 2.A da Fase de Ligação): `DocumentWorkerLauncher` + caminho de invoke direto do `document-worker`

## Contexto (transparente)

A Etapa 1 da Fase de Ligação (PR #21) fechou a composição da casca
Tauri via `frederico_app::build_chat_orchestrator` e substituiu o
`Jail::new(current_dir())` por `FileSystemJailResolver`. O caminho
de produção do Frederico agora executa o que a suíte dos crates
já provava — **exceto** que o `frederico-document-kits` (3 kits:
WordPro, ExcelPro, PDFPro + 2 tools: `DocsGenerateTool` +
`DocsInspectTool`) ainda não está plugado. O `WorkerManager` que
aciona o `document-worker` Python sidecar não é instanciado pela
casca — o `apps/desktop/src-tauri/src/main.rs` para em
`tools: vec![files_read]`.

A Etapa 2 da Fase de Ligação fecha esse gap em 2 sub-etapas
(recomendado pelo relatório da conversa da Etapa 2 + confirmado pelo
`docs/status.md`): **Etapa 2.A** (este PR) introduz o owner do
ciclo de vida do worker + caminho de invoke direto, e **Etapa 2.B**
(o próximo PR) integra os 3 kits no `ToolRegistry` via o trait
`WorkerInvoker` (ADR-0024).

## Ramificações não previstas (4) que forçaram a divisão em 2 PRs

1. **Não há `bundle.resources` no `tauri.conf.json`.** O caminho do
   `document-worker.exe` em produção (`.exe` instalado) não tem
   mecanismo de resolução — o Tauri não empacota nada hoje além
   dos assets do frontend. Empacotar o `document-worker` (~250 MB)
   via `bundle.resources` é **Fase 9 do PROMPT MESTRE**
   (empacotamento NSIS completo), não fase-ligação.
2. **`WorkerManager::shutdown(self)` consome `self`.** Não pode
   ser reciclado — wrapper precisa destruir e recriar a cada
   morte.
3. **`WorkerManager` não tem restart on death automático.** O
   `health_snapshot` existe, mas ninguém monitora. O caller
   precisa detectar morte e recriar.
4. **Detecção de runtime ausente** é não-trivial: o caminho de
   dev (`workers/document-worker/runtime/`) só existe se o
   `bootstrap.ps1` rodou. Sem bootstrap, o `spawn_external`
   falha em 10s no `ready_timeout`. A regra "degradação
   declarada, nunca substituição silenciosa" do ADR-0022 §D2
   diz que **não pode** haver fallback silencioso pro
   `FakeWorker` (que retornaria `handler_stub` — "documento
   falso entregue como verdadeiro" no contexto do user).

## O que entra (6 commits do PR #22)

### Commit 1 — `feat(app): resolve_document_worker_runtime() com precedência declarada`

Função pura no `frederico-app` (sem I/O, sem efeitos colaterais)
que recebe uma `&RuntimeContext` e devolve `Option<RuntimeLocation>`.
Precedência fixa: env var `FREDERICO_DOCUMENT_WORKER_RUNTIME` →
`app.path().resolve("document-worker", Resource)` (bundle.resources
do Tauri — populado pela Fase 9 em produção) → caminho de dev no
repo (`<repo>/workers/document-worker/runtime/`, populado pelo
`bootstrap.ps1`). 11 unit tests novos.

### Commit 2 — `feat(app): DocumentWorkerLauncher (lazy + restart on death com teto)`

Owner do ciclo de vida do worker sidecar com 3 responsabilidades:

- **Lazy start** — o `LauncherState` começa em `NotStarted`. A
  primeira `invoke` spawna via `spawn_external`. Não pesa os 2s
  de abertura do app.
- **Restart on death com teto** — a cada `invoke`, checa
  `health_snapshot`. Se `Unhealthy`, transita pra `Restarting`,
  mata o manager antigo, espera o backoff, e tenta criar um novo.
  Teto: 3 tentativas com recuo exponencial (1s, 2s, 4s). Excedeu
  → state `Dead` permanente, `invoke` retorna
  `WorkerError::PermanentlyDead`. **Sempre mata o antigo antes
  de criar o novo** — worker em ciclo de falha gerando processos
  Python órfãos é o pior modo de falha possível num app desktop.
- **Kill tree no app exit** — `Drop` chama `shutdown` best-effort
  (síncrono, com `tokio::runtime::Handle::block_on`). Garante
  que nenhum Python órfão sobrevive ao fechamento da janela.
  Limitação conhecida: o `Drop` do Rust é síncrono, e a Etapa 7
  (modo desenvolvedor) substitui por Job Objects via
  `SecurityJailResolver` que matam o child mesmo em kill -9 do
  parent.

4 unit tests no launcher. 1 helper module novo
(`app/src/launcher.rs` + `app/src/runtime.rs`).

### Commit 3 — `feat(app): 3 tauri::command novos na casca (Etapa 2.A — invoke direto)`

- `DocumentWorkerStatus()` — devolve `{ available, runtime_source,
  runtime_path, message }` PT-BR. UI consome pra mostrar
  "document-worker: disponível/indisponível" no diagnóstico.
- `DocumentWorkerInvoke(payload)` — caminho de invoke direto,
  **sem passar pelo ChatOrchestrator**. Frontend React usa
  quando o usuário clica em "gerar documento".
- `DocumentWorkerReset()` — botão "tentar reiniciar" no
  diagnóstico após `PermanentlyDead`. Idempotente (não faz nada
  se o launcher é `None`).

### Commit 4 — `feat(app): build_default_tools + build_default_allowed_for_run + initial_permission_set_for_capable_launcher (helpers de Etapa 2.B já testados e prontos)`

3 helpers novos no `frederico-app::composition` que serão
consumidos pela casca na Etapa 2.B:

- `build_default_tools(launcher: Option<&RuntimeLocation>)` —
  retorna 1 tool (`FilesReadTool`) se o launcher é `None`, ou 3
  tools (`FilesReadTool` + `DocsGenerateTool` +
  `DocsInspectTool`) se o launcher está disponível. **Bump
  atômico do capability** quando o launcher está disponível.
- `build_default_allowed_for_run(launcher: Option<&RuntimeLocation>)`
  — mesma regra: 1 `ToolId` (`files.read`) se `None`, ou 3
  `ToolId`s (`files.read` + `docs.generate` + `docs.inspect`)
  se `Some`.
- `initial_permission_set_for_capable_launcher()` — bumpar
  `documents: None → Full` quando o launcher está disponível.
  **Bump atômico do permission junto com o capability**
  (ADR-0020 §3 D3).

6 testes novos no `composition` documentam a direção da Etapa 2.B.
**A Etapa 2.A não usa esses helpers na casca** — fica pronto pra
Etapa 2.B (que muda a forma do contrato: `Option<&RuntimeLocation>`
→ `Option<Arc<dyn WorkerInvoker>>` via trait do ADR-0024).

### Commit 5 — `docs: ADR-0023 + narrativas de release`

- ADR-0023 (novo) — 4 decisões: D1 (resolvedor com precedência),
  D2 (ausência = indisponibilidade), D3 (launcher com lazy +
  restart + kill tree), D6 (alternativa instalador leve +
  bootstrap em `%APPDATA%` registrada enquanto o contexto está
  fresco).
- `docs/releases/fase-ligacao/README.md` — atualizado pra marcar
  Etapa 2.A como fechada, índice com PR #22.

### Commit 6 — `docs: CHANGELOG.md com Etapa 2.A`

Entrada "Fechado — Fase de Ligação, Etapa 2.A (...)" no topo do
"Não publicado", com o resumo de 5 commits e a D4 nomeada
explicitamente (D4: `.exe` instalado não gera documentos até
`bundle.resources` ser populado — Fase 9).

## Status honesto (D5 do ADR-0023)

A Etapa 2.A fecha o **ciclo de vida do worker** + o **bypass de
invoke direto** + o **diagnóstico da UI**. **Não fecha** o
caminho do modelo (`ChatOrchestrator → ToolRegistry → docs.generate`).
Isso é Etapa 2.B (PR #23, ADR-0024). O que a Etapa 2.A **prova**
é que o worker spawna, responde, restart em caso de morte, e mata
limpo no app exit. O que a Etapa 2.A **NÃO prova** é que o
modelo consegue chamar `docs.generate` no schema — isso é
Etapa 2.B.

O E2E da Etapa 5 da fase-ligação (`tests/e2e/` atravessando a
casca) vai exercitar o **kit** e o **IPC**, não o caminho
empacotado. O empacotamento é Fase 9.

## Lições de processo

- **Regra "PRs empilhadas: ramo base em main fresco" funcionou
  pela 3ª vez.** A branch `fase-ligacao/document-worker-launcher`
  foi criada de `c8922dc` (main após PR #21), e o `git fetch
  origin && git rev-parse origin/main` confirmou antes do
  checkout. Zero rebase, zero conflito, zero reversão.
- **3ª ocorrência da regra preventiva** registrada na memória
  do agente (rule update, 2026-08-03). Não precisa mais
  re-derivar — o padrão é sempre `git fetch && rev-parse +
  checkout de SHA-fresco`.
- **ADR-0023 §"Divisão Etapa 2.A vs 2.B"** documenta
  explicitamente o que cada sub-etapa fecha, com o argumento
  de por que não fazer tudo em 1 PR (mexer em `WorkerHandle` =
  Fase 5 fechada, e mexer em Fase 5 fechada é trabalho de fase
  de Ligação posterior).

## Pendências nomeadas com escopo e consequência (D4 do ADR-0023)

> O `.exe` instalado do Frederico não gera documentos até o
> `document-worker` ser empacotado como `bundle.resources` do
> Tauri (ou a alternativa D6 do ADR-0023 — instalador leve +
> bootstrap lazy em `%APPDATA%`). Fecha na Fase 9 do PROMPT
> MESTRE.

Quem ler `docs/status.md` em 6 meses precisa entender que a
Fase 5 fechou os 3 kits DocumentSpec no motor, mas o caminho
de produção até o usuário final (`.exe`) ainda não gera
documentos — a fase de Ligação fecha a integração no motor,
não o empacotamento.
