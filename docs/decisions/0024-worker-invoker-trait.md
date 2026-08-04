# 0024 — Trait `WorkerInvoker` (Etapa 2.B da Fase de Ligação) + bump atômico do contrato do dispatcher e dos 3 kits

## Contexto

A Etapa 2.A da Fase de Ligação (PR #22, mergeada em `c0abe54`) fechou o
`DocumentWorkerLauncher` (ADR-0023) — o owner do ciclo de vida do
`document-worker` Python sidecar, com lazy start, restart on death com
teto, e kill tree no app exit. O ADR-0023 §"Divisão Etapa 2.A vs 2.B"
deixou a Etapa 2.B registrada com escopo claro:

> O `WorkerHandle` é a `struct` central do IPC do `process-architecture`,
> e mexer nela é trabalho que toca Fase 5 fechada. (...) É trabalho
> grande, e fazer isso dentro da Etapa 2.A mistura fase-ligação com
> Fase 5. (...) A Etapa 2.B é a "integração" propriamente dita, e vai
> precisar do seu próprio ADR (provavelmente ADR-0024) quando abrir.

O ADR-0023 também propôs o **caminho** da Etapa 2.B: criar um adapter
`LauncherDispatcher` que implemente a mesma interface do
`WorkerHandle::invoke` mas delegue pro `launcher.invoke()`. Isso muda a
forma do `WorkerToolDispatcher` (de struct concreto para trait
`WorkerHandleLike`), e por consequência mexe no `frederico-process-architecture`
(Fase 5 fechada).

A Etapa 2.B reavalia essa proposta no meio da implementação. Três
ramificações não previstas no ADR-0023 forçaram uma decisão diferente:

1. **O `WorkerHandle` (Fase 5) e o `DocumentWorkerLauncher` (Etapa 2.A)
   têm **ciclo de vida** muito diferente** — o primeiro é eagerly
   spawned, o segundo é lazy. Um `trait` com 2 implementações é a
   abstração mínima que expressa a diferença; um `LauncherDispatcher`
   wrapper (que seria o "outro lado" do `WorkerHandle`) reintroduz um
   `Arc<Mutex<...>>` interno e mais um nível de indireção, sem
   benefício claro.

2. **O `ProcessError` carrega informação do `WorkerHealthSnapshot`**
   (tipo do `process-architecture`). O `WorkerHandle` retorna
   `ProcessError` direto. O `DocumentWorkerLauncher` precisa converter
   `WorkerError` → `ProcessError` → erro exposto aos kits. A forma
   natural é **um erro novo no `core`** que é genérico o suficiente
   para ambos — o `ProcessError` continua existindo (não mexe na
   Fase 5), mas os kits veem `InvokeError` (do `core`).

3. **A regra de pureza do `core`** (REGRAS §1.10) diz que o
   `frederico-core` não pode importar `tauri`, `windows`, paths do
   sistema, **nem `process-architecture`** (que tem `#[cfg(windows)]`
   e depende de `windows-rs`). O `WorkerInvoker` precisa estar no
   `core` (regra do user: "contratos genéricos de worker não têm
   nada a ver com registro de ferramentas — colocar no
   `tool-registry` criaria uma dependência errada do `document-kits`
   em `tool-registry`"). Logo, o erro do trait também precisa estar
   no `core` — não `ProcessError`.

O caminho do `LauncherDispatcher` wrapper do ADR-0023 §"Divisão
Etapa 2.A vs 2.B" é descartado em favor de um **trait `WorkerInvoker`
no `core`** com um **erro `InvokeError` próprio**. O `WorkerHandle`
(Fase 5) e o `DocumentWorkerLauncher` (Etapa 2.A) ambos implementam
o trait via adapters separados.

## Decisões

### D1 — Trait `WorkerInvoker` no `frederico-core` (não no `tool-registry`)

```rust
// crates/core/src/worker_invoker.rs
#[async_trait]
pub trait WorkerInvoker: Send + Sync + 'static {
    async fn invoke(&self, payload: Value) -> Result<Value, InvokeError>;
}
```

**Por que no `core` e não no `tool-registry`:** o `WorkerInvoker` é
um contrato genérico de invocação de worker sidecar — não tem nada a
ver com registro de ferramentas. Colocá-lo no `tool-registry` criaria
uma dependência "errada" do `document-kits` em `tool-registry` por
conveniência (`document-kits` precisa do trait mas não do registry).
A inversão hierárquica (camada de renderização dependendo de camada
de catálogo) é o tipo de aresta no grafo que ninguém desfaz depois.
O `core` é o lugar dos tipos compartilhados (`ToolId`,
`ConversationId`, `MessageId`, `RunId`) — o `WorkerInvoker` se junta
a eles.

**Conferência pré-merge:** verifiquei que `document-kits` **não**
depende de `tool-registry` (depende só de `core`, `document-engine`,
`async-trait`, `serde_json`, `thiserror` via path). Logo, mover o
trait pro `core` é correto.

**Por que `Send + Sync + 'static`:** o `RunExecutor` carrega o
invoker por longos períodos em `Arc<dyn WorkerInvoker>`, e a casca
Tauri roda em multi-thread. Sem esses bounds, o borrow checker
rejeita o uso.

### D2 — `InvokeError` próprio do `core` (não `ProcessError`)

```rust
// crates/core/src/worker_invoker.rs
#[derive(Debug, thiserror::Error)]
pub enum InvokeError {
    #[error("erro de protocolo: {message}")]
    Protocol { message: String },
    #[error("erro de transporte: {message}")]
    Transport { message: String },
    #[error("invoke excedeu o timeout")]
    Timeout,
    #[error("worker não saudável: {message}")]
    Unhealthy { message: String },
    #[error("erro de plataforma: {message}")]
    Platform { message: String },
    #[error("worker permanentemente morto após {attempts} tentativas (limite: {max})")]
    PermanentlyDead { attempts: u8, max: u8 },
}
```

**Por que não reusar `ProcessError`:** o `ProcessError` mora no
`frederico-process-architecture` e referencia o `WorkerHealthSnapshot`
que mora lá. O `core` **não pode** importar `process-architecture`
(regra de pureza do `core`, ADR-0003 + `scripts/check-core-purity.ps1`).
O `InvokeError` é definido no `core` com 6 categorias que cobrem o
mesmo espaço do `ProcessError` (5:1, exceto `PermanentlyDead` que é
específico do launcher). As implementações convertem de `ProcessError`
(1:1) e de `WorkerError` (mapeamento documentado em
`app/src/launcher.rs::WorkerInvoker` impl).

**Por que `PermanentlyDead` é variante própria:** o `WorkerError` do
launcher tem essa variante, e ela tem semântica diferente de
`Unhealthy { message }` (a UI precisa chamar `reset()` antes de
tentar de novo — não é "tente mais uma vez"). O `ProcessError` não
tem esse caso (o `WorkerHandle` nunca está "permanentemente morto" —
se morrer, o caller recria o `WorkerManager`). Definir a variante no
`core` significa que os kits podem reagir a ela sem importar
`app` ou `process-architecture`.

**Conferência de mapeamento (1:1):**

| `ProcessError`                  | `InvokeError`                          |
|---------------------------------|----------------------------------------|
| `Protocol { message }`          | `Protocol { message }`                 |
| `Transport { message }`         | `Transport { message }`                |
| `Timeout { worker_id, .. }`     | `Timeout` (worker_id stripped)         |
| `Cancelled { worker_id, .. }`   | `Unhealthy { message: "cancelado..." }`|
| `Unhealthy { worker_id, msg }`  | `Unhealthy { message: msg }` (id stripped) |
| `Platform { message }`          | `Platform { message }`                 |

| `WorkerError` (launcher)        | `InvokeError`                          |
|---------------------------------|----------------------------------------|
| `RuntimeUnavailable`            | `Platform { message: "runtime indisponível..." }` |
| `PlatformNotSupported`          | `Platform { message: "Windows only" }` |
| `SpawnFailed(pe)`               | `process_to_invoke_error(pe)`           |
| `InvokeFailed(pe)`              | `process_to_invoke_error(pe)`           |
| `PermanentlyDead { a, m }`      | `PermanentlyDead { a, m }` (preserva)   |
| `ShutdownFailed(pe)`            | `Platform { message: format!("shutdown best-effort falhou: {pe}") }` |

O helper `process_to_invoke_error` é **duplicado intencionalmente** em
3 lugares (`process-architecture/src/worker_invoker_impl.rs`,
`tool-registry/src/worker_dispatch.rs` foi removido, e
`app/src/launcher.rs`) — ver D3 abaixo.

### D3 — `impl WorkerInvoker for WorkerHandle` no `process-architecture` (orphan rule)

A regra do Rust (orphan rule) diz: "only traits defined in the
current crate can be implemented for types defined outside of the
crate". O trait `WorkerInvoker` é definido no `core`; o `WorkerHandle`
é definido no `process-architecture`. O `impl` precisa estar em um
dos dois. A escolha: o `process-architecture` é o crate que conhece
o `WorkerHandle` intimamente, e o `core` é puro (não pode importar
`process-architecture`). Logo, o `impl` mora no
`process-architecture/src/worker_invoker_impl.rs`.

**O que muda no `process-architecture` em si:** estritamente **nada**.
O `WorkerHandle` struct continua igual (Fase 5 fechada, Etapa 3). O
`WorkerManager::invoke` continua idêntico, o modelo de ator
(ADR-0015) continua idêntico, `health_snapshot` continua idêntico.
Só adicionamos um `impl` para um trait novo (definido no `core`). É
estritamente aditivo.

**Helper `process_to_invoke_error` inline duplicado** em
`process-architecture/src/worker_invoker_impl.rs`. A alternativa
seria o `core` expor um `From<ProcessError> for InvokeError` — mas
isso obriga o `core` a conhecer `ProcessError`, que vive no
`process-architecture` (regra de pureza). A duplicação é ~25 linhas,
e cada crate que precisa do mapeamento tem o seu. **É o preço de
manter o grafo limpo**, exatamente como o
`process_to_invoke_error` espelhado em `app/src/launcher.rs` (que
converte `WorkerError` → `InvokeError` antes do `process_to_invoke_error`
interno). A mesma duplicação existe no `tool-registry` (não mais —
foi deletada quando a 1ª duplicação foi para `process-architecture`).

**Conferência de dependências:**

- `process-architecture/Cargo.toml` ganha `frederico-core` em
  `[dependencies]`. O ciclo é `core → process-architecture`
  inverte? **Não** — `core` continua sem importar `process-architecture`.
  O fluxo é `process-architecture` (que conhece `core` desde sempre)
  adiciona um `impl` pro trait novo.
- `app/Cargo.toml` ganha `async-trait` em `[dependencies]` (já
  estava em `core` e em `process-architecture`).

### D4 — `impl WorkerInvoker for DocumentWorkerLauncher` no `app`

O launcher implementa o trait no `app/src/launcher.rs` (mesma razão
do D3: o tipo concreto vive no `app`, e a orphan rule exige o `impl`
aqui). O `Arc<dyn WorkerInvoker>` que a casca constrói a partir do
`Arc<DocumentWorkerLauncher>` é a ponte pro `build_default_tools` /
`build_default_allowed_for_run` / `initial_permission_set_for_capable_launcher`.

**Por que o `AppState` guarda o `DocumentWorkerLauncher` (não o
`Arc<dyn WorkerInvoker>`):** os commands Tauri
`document_worker_status` / `document_worker_invoke` / `document_worker_reset`
precisam do tipo **concreto** (o trait `WorkerInvoker` **não** expõe
`status()` nem `reset()` — `WorkerError::PermanentlyDead` pede
`reset()` da UI). O `Arc<dyn WorkerInvoker>` é construído **localmente**
no `setup` (não mora no `AppState`) — ele só é usado pra alimentar
a composição do `ChatOrchestrator` (uma vez no startup), e guardar
no `AppState` seria `dead_code` (warning do compilador). O mesmo
`Arc<DocumentWorkerLauncher>` atende aos dois papéis.

### D5 — Bump atômico do contrato em 1 PR

A mudança de contrato do `WorkerToolDispatcher::new`,
`WordProKit::new`, `ExcelProKit::new`, `PdfProKit::new`,
`KitError::Process`, e `DispatchError::Process → DispatchError::Invoke`
foi feita **em commits consecutivos mas no mesmo PR**:

- `954e79b feat(core): trait WorkerInvoker + erro proprio InvokeError`
- `1c777b8 feat(process-architecture): impl WorkerInvoker for WorkerHandle`
- `5d0d26f feat(tool-registry): WorkerToolDispatcher aceita Arc<dyn WorkerInvoker>`
- `8800d26 feat(document-kits): 3 kits + KitError aceitam Arc<dyn WorkerInvoker> e InvokeError`

A invariante preservada: **em qualquer commit intermediário** o
workspace **não compila** (a ordem dos 4 commits é obrigatória).
Isso é intencional — força o reviewer a olhar a mudança como
atômica, não como 4 mudanças separadas. O bump atômico do
`documents: None → Full` (ADR-0020 §3 D3) entra no **commit que
atualiza a casca Tauri** (não nos 4 commits do contrato), junto
com o registro de `DocsGenerateTool` + `DocsInspectTool` no
`build_default_tools`.

**Por que 4 commits e não 1 só:** cada commit é **uma camada**
do grafo (`core` → `process-architecture` → `tool-registry` →
`document-kits`). O diff de cada commit é mínimo e cabe numa
revisão de 5min. O commit final da casca fecha o ciclo.

### D6 — Guarda automatizada: `scripts/check-fase-5-untouched.ps1`

A Etapa 2.B **integra** os 3 kits do `document-worker` ao
`ToolRegistry`, mas **não mexe no `document-worker` Python** da
Fase 5. Os 3 arquivos sensíveis do worker
(`tests/test_pdf_audit.py`, `document-worker.py`,
`tools/generate_srgb_icc.py`) **não podem** ter diff contra
`origin/main` neste PR.

A regra "valeria virarem passo de script em vez de comando manual"
(de quem revisou o ADR-0023 / pediu Etapa 2.B) virou
`scripts/check-fase-5-untouched.ps1`. O script:

1. Compara `git diff --stat origin/main..HEAD -- <path>` em cada
   um dos 3 arquivos.
2. Se algum mudou, exit 1 com mensagem explicando o
   atravessamento de fronteira.
3. Se OK, exit 0 com mensagem de sucesso.

Roda em todo CI (best-effort — `git fetch` falha sem rede, segue
com ref local) e em todo pre-push manual. **É o tipo de coisa que
ninguém desfaz depois.**

## Consequências

- O `frederico-document-kits` finalmente entra no caminho de
  produção do app (juntos com o launcher, que já estava na
  Etapa 2.A). O `ToolRegistry` registra `docs.generate` e
  `docs.inspect` quando o runtime está disponível; o
  `RunExecutor` aceita invocação via allowlist; o
  `PermissionSet.documents` vai pra `Full`. **Bump atômico
  capability + permission** (ADR-0020 §3 D3).
- O `WorkerHandle` (Fase 5) e o `DocumentWorkerLauncher` (Etapa
  2.A) implementam o **mesmo** trait `WorkerInvoker`. O
  `WorkerToolDispatcher` e os 3 kits não distinguem — o
  `Arc<dyn WorkerInvoker>` é opaco.
- O `process-architecture` ganha uma dep em `core` (já tinha —
  `core` é o tipo de `RunId` etc.), e o `app` ganha
  `async-trait` (já tinha no `core`).
- **Helper `process_to_invoke_error` duplicado em 3 lugares.**
  É o preço de manter o grafo limpo. Quem ler daqui a 6 meses
  vai achar estranho — o ADR-0024 §D3 e o comentário no próprio
  helper documentam a razão.
- O `WorkerHandle` (Fase 5) **não mudou** — só ganhou um `impl`
  pra um trait novo. O `WorkerManager` (modelo de ator do
  ADR-0015) **não mudou**.
- A UI continua com o canal `DocumentWorkerStatus` da Etapa 2.A
  (status do launcher). O bump capability + permission não muda
  a UI do diagnóstico — o que muda é que o modelo **passa a ver**
  `docs.generate` e `docs.inspect` no schema. A Etapa 6 da
  fase-ligação (UI de configuração) pode adicionar uma tela
  para ligar/desligar essas tools.

## Alternativas consideradas

1. **`LauncherDispatcher` wrapper (proposto no ADR-0023 §"Divisão
   Etapa 2.A vs 2.B")**. O wrapper seria uma `struct` com
   `Arc<DocumentWorkerLauncher>` interno, implementando
   `WorkerHandle::invoke` via delegação. **Rejeitado** porque:
   (a) precisa de um método de `clone()` que cria novo `Arc` pro
   mesmo `Arc<Mutex<...>>` interno — sim, isso funciona, mas é
   uma indireção a mais; (b) o `WorkerHandle` é uma struct, e um
   wrapper que finge ser `WorkerHandle` mas delega pra
   `launcher.invoke()` é uma **representação falsa** — o `Drop` do
   `WorkerHandle` faz shutdown, o do `LauncherDispatcher` não pode
   fazer shutdown do `Arc<Mutex<...>>` interno (que vive no
   `launcher`); (c) o `WorkerHandle` mudou de **struct pra trait**
   nessa proposta, e mexer em `process-architecture` (Fase 5
   fechada) é trabalho de fase de Ligação posterior, não desta.
   O trait `WorkerInvoker` resolve os 3 problemas de uma vez: (a)
   não precisa de wrapper; (b) o `WorkerHandle` continua struct
   (não muda); (c) o trait é definido no `core` (limpo).

2. **`ProcessError` como erro do trait**. **Rejeitado** porque o
   `core` não pode importar `process-architecture` (regra de
   pureza). O `InvokeError` é definido no `core` com 6 categorias
   que cobrem o mesmo espaço (5:1 + `PermanentlyDead` específico
   do launcher). As implementações convertem via helper inline.

3. **`WorkerInvoker` no `tool-registry` (proposta original)**.
   **Rejeitado** pela regra do user: "WorkerInvoker é um contrato
   genérico de invocação de worker — não tem nada a ver com
   registro de ferramentas. Colocá-lo no `tool-registry` criaria
   uma dependência errada do `document-kits` em `tool-registry`.
   Onde ele deveria morar: `frederico-core`." A inversão
   hierárquica (camada de renderização dependendo de camada de
   catálogo) é o tipo de aresta no grafo que ninguém desfaz
   depois. O `core` é o lugar dos tipos compartilhados.

4. **Um PR só (`commit 8800d26` + commit da casca) sem os
   commits intermediários (`954e79b`, `1c777b8`, `5d0d26f`)**.
   **Rejeitado** porque cada commit é uma camada do grafo, e o
   diff de cada um é mínimo (cabe numa revisão de 5min). O commit
   final da casca fecha o ciclo, mas os 4 commits intermediários
   dão contexto pra quem revisa.

## Pendências

- **D4 do ADR-0023 nomeada com escopo e consequência**: `.exe`
  instalado do Frederico não gera documentos até o
  `document-worker` ser empacotado como `bundle.resources` do
  Tauri (ou a alternativa D6 do ADR-0023). Fecha na Fase 9 do
  PROMPT MESTRE. **Não** é escopo da Etapa 2.B.
- **E2E da Etapa 5 da fase-ligação** (`tests/e2e/` na raiz
  atravessando a casca) ainda não está escrito. Quando rodar,
  o `WorkerInvoker` é o mesmo (o `FakeWorker` em
  `process-architecture` pode implementar o trait pra E2E
  rápidos, ou a casca pode usar um `DocumentWorkerLauncher`
  apontando pro runtime de dev). Vai precisar de tag explícita
  no `docs/status.md` quando fechar (ADR-0023 §D5).
- **`PermanentlyDead` no `InvokeError` não tem mapeamento 1:1
  pro `ProcessError`**: o `WorkerHandle` nunca está
  "permanentemente morto" (o caller recria o `WorkerManager`
  quando morre). O `ProcessError::Unhealthy` é o mais próximo,
  mas perde a info `{attempts, max}`. Aceitável — o
  `PermanentlyDead` é específico do launcher, e os adapters
  preservam os campos no `InvokeError` direto.
- **Helper `process_to_invoke_error` duplicado em 3 lugares.**
  Documentado em D3 e no próprio helper. Se um dia o
  `process-architecture` precisar de mais variantes, atualizar
  3 lugares. O risco é baixo (mudança rara) e o custo é ~25
  linhas duplicadas.

## Histórico de revisão

- 2026-08-03 — versão inicial. Convergência da conversa da
  Etapa 2.B (ramificação do plano original do ADR-0023 §"Divisão
  Etapa 2.A vs 2.B" — o `LauncherDispatcher` wrapper foi
  descartado em favor do trait `WorkerInvoker` no `core`).
