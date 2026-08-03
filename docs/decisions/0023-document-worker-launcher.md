# 0023 — `DocumentWorkerLauncher` (Etapa 2.A da Fase de Ligação) + caminho do `document-worker` em runtime

## Contexto

A Etapa 1 da Fase de Ligação (PR #21, mergeada em `c8922dc`) fechou a
composição da casca Tauri via `frederico_app::build_chat_orchestrator` e
substituiu o `Jail::new(current_dir())` por `FileSystemJailResolver`. O
caminho de produção do Frederico agora executa o que a suíte dos crates
já provava — **exceto** que o `frederico-document-kits` (3 kits: WordPro,
ExcelPro, PDFPro + 2 tools: `DocsGenerateTool` + `DocsInspectTool`) ainda
não está plugado. O `frederico_document_kits` foi construído na Fase 5
(PRs #11/#12/#15/#17/#19), mas o `WorkerManager` que aciona o
`document-worker` Python sidecar não é instanciado pela casca — o
`apps/desktop/src-tauri/src/main.rs` para em `tools: vec![files_read]`.

A Etapa 2 da Fase de Ligação fecha esse gap. O escopo declarado
(`docs/releases/fase-ligacao/README.md`) é: "ligar `frederico-document-kits`
como dep da casca + registrar `docs.generate` + `docs.inspect` no
`build_tool_registry` + bump atômico do `documents` permission (de
`None` para `DocumentPermission::Full`)".

## Ramificações descobertas no plano

O plano original (proposto antes desta Etapa) era ligar
`frederico-document-kits` em 1 PR com `WorkerManager::spawn_external`
direto. Quatro ramificações não previstas forçaram a divisão em 2 PRs
(recomendado pelo relatório da conversa da Etapa 2):

1. **Não há `bundle.resources` no `tauri.conf.json`.** O caminho do
   `document-worker.exe` em produção (`.exe` instalado) não tem
   mecanismo de resolução — o Tauri não empacota nada hoje além dos
   assets do frontend. Empacotar o `document-worker` (~250 MB) via
   `bundle.resources` é **Fase 9 do PROMPT MESTRE** (empacotamento
   NSIS completo), não fase-ligação.
2. **`WorkerManager::shutdown(self)` consome `self`.** Não pode ser
   reciclado — wrapper precisa destruir e recriar a cada morte.
3. **`WorkerManager` não tem restart on death automático.** O
   `health_snapshot` existe, mas ninguém monitora. O caller precisa
   detectar morte e recriar.
4. **Detecção de runtime ausente** é não-trivial: o caminho de dev
   (`workers/document-worker/runtime/`) só existe se o `bootstrap.ps1`
   rodou. Sem bootstrap, o `spawn_external` falha em 10s no
   `ready_timeout`. A regra "degradação declarada, nunca substituição
   silenciosa" do ADR-0022 §D2 diz que **não pode** haver fallback
   silencioso pro `FakeWorker` (que retornaria `handler_stub` —
   "documento falso entregue como verdadeiro" no contexto do user).

## Decisões

### D1 — `resolve_document_worker_runtime()` com precedência declarada

Função pura, sem I/O, sem efeitos colaterais — recebe uma
`&RuntimeContext` (paths candidatos) e devolve
`Option<RuntimeLocation>`. Precedência fixa (a primeira que achar
vence):

1. **Variável de ambiente `FREDERICO_DOCUMENT_WORKER_RUNTIME`** — uma
   `PathBuf` absoluta pro diretório que contém o `python.exe` e o
   `document-worker.py`. Usada em testes e em setups não-padrão
   (ex.: desenvolvedor com Python instalado em outro path).
2. **Recursos do app** — `runtime/document-worker/` relativo ao
   `tauri::AppHandle::path().resolve("document-worker", Resource)`.
   Em dev, o `PathResolver` retorna o diretório onde o Tauri esperaria
   os recursos; o caminho só fica populado em produção quando a
   `bundle.resources` do `tauri.conf.json` for configurada (Fase 9).
3. **Caminho de dev no repositório** — `workers/document-worker/runtime/`
   relativo ao `CARGO_MANIFEST_DIR` do app. Funciona em dev quando o
   `bootstrap.ps1` rodou.

A função checa a presença de **3 artefatos** no diretório candidato
antes de aceitar: `python.exe` (ou `python3.exe`), `document-worker.py`,
e a subpasta `Lib/site-packages` com as deps instaladas. Se faltar
qualquer um, o candidato é rejeitado e a próxima opção é tentada.
**Ausência = indisponibilidade** (D2 abaixo), não erro.

O ponto crítico de D1 é que o **código não muda quando a Fase 9
chegar**. A Fase 9 popula a opção 2 (recursos do app) declarando
`bundle.resources` no `tauri.conf.json` + ajustando o `bootstrap.ps1`
para instalar em caminho empacotável. A função de resolução
permanece a mesma; só a precedência 2 começa a retornar `Some(_)` em
produção. **Zero código novo** quando o empacotamento for
resolvido.

### D2 — Ausência do runtime = tools fora do `Vec` (degradação declarada)

Quando `resolve_document_worker_runtime()` devolve `None` (nenhuma
das 3 opções tem o runtime completo), a função
`build_default_tools(launcher, registry)` no `frederico-app` retorna
apenas `FilesReadTool`. `DocsGenerateTool` e `DocsInspectTool` **não
entram no `Vec<Arc<dyn Tool>>`**, e portanto:

- `build_tool_registry(&tools)` não tem manifestos dessas 2 tools
  registradas — o **modelo não as enxerga** no schema.
- `allowed_for_run` não tem `ToolId::new("docs.generate")` nem
  `ToolId::new("docs.inspect")` — o `RunExecutor` rejeita invocação
  com `ToolNotAllowed` se o modelo tentar (bypass impossível).
- `documents: DocumentPermission::Full` em `initial_permission_set()`
  é **bumpado condicionalmente**: se o launcher está disponível, vai
  pra `Full`; se não, fica em `None` (não muda o `default()` deny).
  **Bump atômico do permission junto com o capability registrado** —
  mesmo princípio do ADR-0020 §3 D3 (bump capability + permission
  atômicas).

A UI recebe um canal de diagnóstico novo: o `tauri::command`
`DocumentWorkerStatus()` devolve `{ available: bool, resolved_path:
Option<String>, reason: Option<String> }`. A tela de diagnóstico
mostra "document-worker: disponível/indisponível, caminho
resolvido". **O inventário passa a variar por ambiente**, e o
operador precisa conseguir responder "por que sumiu o
`docs.generate`?" sem depurar — o `reason` carrega a mensagem PT-BR
("runtime ausente em env, recursos, dev") para o caso `available: false`.

### D3 — `DocumentWorkerLauncher` (lazy start + restart on death com teto)

Novo tipo no `frederico-app`:

```rust
pub struct DocumentWorkerLauncher {
    state: Arc<Mutex<LauncherState>>,
    config: LauncherConfig,
    runtime: RuntimeLocation, // resolvido em D1; se None, new() falha
    // ...
}

enum LauncherState {
    NotStarted,                              // lazy — primeira invoke
    Alive { manager: WorkerManager, handle: WorkerHandle },
    Restarting { attempts: u8, last_error: ProcessError, last_attempt_at: Instant },
    Dead,                                    // excedeu teto de tentativas
}

pub struct LauncherConfig {
    pub max_restart_attempts: u8,    // default 3
    pub restart_backoff: Duration,   // default 1s, 2s, 4s (exponencial)
    pub ready_timeout: Duration,     // default 10s, mesmo do ExternalSpawnConfig
}
```

Comportamento:

- **Lazy start:** o `ManagerState` começa em `NotStarted`. A primeira
  chamada `invoke(args)` spawna o worker via `spawn_external` com a
  `RuntimeLocation` resolvida em D1. Se o spawn falha, retorna
  `WorkerError::SpawnFailed` com a mensagem do `ProcessError`.
- **Restart on death:** a cada `invoke`, checa `health_snapshot` —
  se `alive == false`, transita pra `Restarting`, mata o manager
  antigo com `shutdown()` (que é o caminho oficial), e tenta criar
  um novo. **Sempre mata o antigo antes de criar o novo** — worker
  em ciclo de falha gerando processos Python órfãos é o pior modo de
  falha possível num app desktop (worker zumbi consumindo memória,
  sockets nomeados vazando, etc.).
- **Teto de 3 tentativas com recuo exponencial** (1s, 2s, 4s).
  Excedeu → transita pra `Dead`. `invoke` subsequente retorna
  `WorkerError::PermanentlyDead` sem tentar mais. A casca pode
  mostrar "document-worker: falhou 3 vezes, reinicie o app" no
  diagnóstico. A UI pode ter um botão "tentar reiniciar" que reseta
  o state pra `Restarting` e tenta de novo.
- **`Drop` chama `shutdown`** se o state é `Alive`. Como o
  `WorkerManager::shutdown(self)` consome `self`, o `Drop` precisa
  usar `tokio::runtime::Handle::current().block_on()` para shutdown
  síncrono — aceitável porque o `Drop` do `DocumentWorkerLauncher`
  só roda no app exit, e o shutdown é best-effort (timeout 5s já
  existe no `WorkerManager::shutdown`).
- **Kill tree no app exit:** o `apps/desktop/src-tauri/src/main.rs`
  registra um `tauri::Manager::on_window_event` handler para
  `WindowEvent::CloseRequested` que chama
  `launcher.shutdown_blocking()` antes de retornar. **Garante que
  nenhum Python órfão fica rodando depois que a janela fecha.**

### D4 — Pendência nomeada com escopo e consequência

Não é suficiente dizer "depende da Fase 9". A pendência fica
registrada no `docs/status.md` e no `docs/releases/fase-ligacao/README.md`
com escopo e consequência explícitos:

> **O `.exe` instalado do Frederico não gera documentos até o
> `document-worker` ser empacotado como `bundle.resources` do Tauri
> (ou a alternativa da D6 abaixo).** Em produção (`.exe` instalado
> sem bundle.resources), a opção 2 do resolvedor retorna `None`, a
> opção 3 do resolvedor também (`CARGO_MANIFEST_DIR` aponta pro
> diretório de instalação do app, não pro repo), e o resultado é
> `runtime ausente` — `docs.generate` e `docs.inspect` não aparecem
> no schema do modelo, e a UI mostra a mensagem de diagnóstico.
> Esta pendência **fecha na Fase 9 do PROMPT MESTRE** (empacotamento
> NSIS completo) ou na D6 abaixo (instalador leve + bootstrap
> lazy em `%APPDATA%`).

Alguém lendo a tabela de fases em 6 meses precisa entender que **a
Fase 5 fechou os 3 kits DocumentSpec no motor, mas o caminho de
produção até o usuário final (`.exe`) ainda não gera documentos** —
a fase de Ligação fecha a integração no motor, não o
empacotamento.

### D5 — `status.md` honesto: E2E de `docs.generate` da Etapa 5 roda com runtime de dev

A Etapa 5 da fase-ligação (`tests/e2e/` na raiz atravessando a
casca) é a primeira a testar o caminho de produção end-to-end.
Quando ela rodar, o `WorkerLauncher` resolve o runtime **na opção 3
do resolvedor** (caminho de dev no repo) — não na opção 2
(recursos do app). O `cargo test` exercita o **kit** e o **IPC**, não
o **caminho empacotado**.

O `docs/status.md` registra isso explicitamente: "Etapa 5 da fase-
ligação prova o kit e o IPC, não o empacotamento. O caminho até o
`.exe` instalado fica pendente na Fase 9 do PROMPT MESTRE." Senão a
fase de Ligação fecha reivindicando mais do que entregou, que é
exatamente o vício que ela veio corrigir.

### D6 (opção registrada para Fase 9) — Instalador leve + bootstrap lazy em `%APPDATA%`

Empacotar ~250 MB dentro do NSIS não é o único desenho. A
alternativa comum em apps desktop modernos:

- **Instalador NSIS leve** (~10 MB) — só o app `frederico-desktop.exe`
  + binários do frontend.
- **Primeira execução** — o app detecta runtime ausente e roda o
  `bootstrap.ps1` em background, baixando Python embeddable + libs
  + Tesseract (com SHA-256 fixo, mesmo código do bootstrap atual)
  para `%APPDATA%\studio\frederico\ia\document-worker-runtime\`.
- **Atualizações** — o app checa a versão do runtime no startup e
  roda o `bootstrap.ps1` novamente se mudou (ex.: upgrade do Tesseract
  5.4 → 5.5). O `bootstrap.ps1` é idempotente.
- **Resolução** — `FREDERICO_DOCUMENT_WORKER_RUNTIME` aponta pro
  `%APPDATA%` resolvido (ou `PathResolver` mapeia `document-worker`
  pra `%APPDATA%` direto).

**Provavelmente sai mais barato que mexer em `bundle.resources`**
porque o `bootstrap.ps1` já existe, já verifica SHA-256, já é
idempotente, e o overhead de download lazy é aceitável num app que
não roda 24/7 (download de 1 vez, com progress bar na primeira
execução). A Fase 9 pode escolher entre D6 e `bundle.resources`
conforme a restrição de banda do usuário-alvo.

Esta opção **fica registrada aqui** enquanto o contexto está fresco
— não é decisão tomada, é alternativa documentada pra Fase 9
considerar.

## Consequências

- O `frederico-document-kits` finalmente aparece no caminho de
  produção do app — em dev, imediatamente; em produção, depois da
  Fase 9.
- O `JailResolver` da Etapa 1 continua válido: o worker opera dentro
  do jail por conversa, sem exfiltração de path.
- A UI precisa de uma tela de diagnóstico nova (canal `DocumentWorkerStatus`).
  O frontend React vai consumir o `tauri::command`. **Não** é escopo
  desta Etapa o design da tela — só o `tauri::command` e o hook no
  React. A UI em si fica pra Etapa 6 da fase-ligação ("regra de
  definição de pronto" + compressão de docs/status/CHANGELOG) ou
  pra uma Etapa subsequente dedicada a UI.
- O `WorkerManager::shutdown(self)` continuar consumindo `self` é
  aceitável aqui — o `DocumentWorkerLauncher` é o owner do ciclo
  de vida. Mudar a assinatura do `WorkerManager::shutdown` para
  `&mut self` (reciclável) seria trabalho de fase de Ligação
  posterior (Etapa 5 talvez), não desta.

## Divisão Etapa 2.A vs 2.B

**O que a Etapa 2.A fecha** (este PR):

1. `resolve_document_worker_runtime` (D1) — função pura no
   `frederico-app`, testada em 10+ unit tests.
2. `DocumentWorkerLauncher` (D3) — owner do ciclo de vida do
   worker, com lazy start + restart on death com teto + reset.
3. `tauri::command DocumentWorkerStatus` — diagnóstico do
   launcher (alive/source/path/message). UI consome via React.
4. `tauri::command DocumentWorkerInvoke(payload)` — caminho
   de invoke direto do launcher (sem passar pelo
   `ChatOrchestrator`). O frontend React chama isso quando
   o usuário pede "gerar documento" via botão da UI.
5. Window event handler — `CloseRequested` chama
   `launcher.shutdown_blocking()` (best-effort).
6. Permissões e `ToolRegistry` da casca **continuam como na
   Etapa 1** (sem bump, sem tools extras). O modelo ainda
   não enxerga `docs.generate`/`docs.inspect` no schema.
   Isso é **intencional** — ver Etapa 2.B abaixo.

**O que a Etapa 2.B fecha** (PR seguinte, **separa da 2.A**):

1. **Integração com `ToolRegistry`**: o
   `WorkerToolDispatcher::new(handle: WorkerHandle, ...)`
   recebe um `WorkerHandle` **concreto** (não trait), e o
   `DocumentWorkerLauncher` é lazy (sem `WorkerHandle`
   até a primeira `invoke`). Pra integrar, é preciso um
   **adapter** `LauncherDispatcher` que implemente a
   mesma interface do `WorkerHandle::invoke` mas delegue
   pro `launcher.invoke()`. Isso muda a forma do
   `WorkerToolDispatcher` (trait `WorkerHandleLike` em vez
   de struct concreto), e por consequência o
   `frederico-process-architecture` (mexida na Fase 5
   fechada, e mexer em Fase 5 fechada é trabalho de fase
   de Ligação).
2. **Bump atômico do `documents: Full`** quando o
   launcher está disponível (D2 + ADR-0020 §3 D3).
3. **`build_default_tools` integrado**: retorna
   `FilesReadTool` + `DocsGenerateTool` + `DocsInspectTool`
   quando o launcher está disponível. Os helpers
   `initial_permission_set_for_capable_launcher` e
   `build_default_allowed_for_run` (que já existem
   testados) entram em uso.
4. **E2E da Etapa 5 da fase-ligação** exercita o
   caminho end-to-end (modelo chama `docs.generate` →
   launcher spawn lazy → worker Python processa → arquivo
   gerado).

**Por que Etapa 2.A não fecha o ciclo todo:** o `WorkerHandle`
é a `struct` central do IPC do `process-architecture`, e
mexer nela é trabalho que toca Fase 5 fechada. A regra do
projeto (REGRAS §1.7) é "mudança de contrato exige ADR + bump
atômico do enum junto" — o `WorkerHandle` mudou de forma
(struct → trait). É trabalho grande, e fazer isso dentro da
Etapa 2.A mistura fase-ligação com Fase 5. Manter a Etapa
2.A mínima (launcher + status + invoke direto) preserva a
divisão de fases. A Etapa 2.B é a "integração" propriamente
dita, e vai precisar do seu próprio ADR (provavelmente
ADR-0024) quando abrir.

**Status honesto da Etapa 2.A** (D5): o launcher existe e
funciona, mas o **caminho de produção** (`.exe` instalado)
ainda não tem `bundle.resources` (D4), e o caminho **do
modelo** (ChatOrchestrator → ToolRegistry) ainda não inclui
`docs.generate` no schema. O que a Etapa 2.A **prova** é o
ciclo de vida do worker + o resolvedor de runtime + o
bypass de invoke direto. O que a Etapa 2.A **NÃO prova** é
o caminho completo do modelo até o `.pdf` no disco do
usuário — isso é Etapa 2.B + Fase 9.

## Alternativas consideradas

1. **`spawn_in_process` (FakeWorker) no caminho de produção.** Vetado
   pelo user: "se a casca registrar `docs.generate` apontando para o
   FakeWorker, o usuário pede um relatório e recebe um resultado
   simulado. Documento falso entregue como verdadeiro é a falha mais
   cara possível num app de contabilidade."
2. **Híbrido com warning no log.** Vetado pelo user pelo mesmo motivo,
   com argumento adicional: "um warning num log não impede o modelo de
   dizer 'segue seu relatório em anexo'." Viola o ADR-0022 §D2
   (degradação declarada, nunca substituição silenciosa).
3. **1 PR só com `bundle.resources` mexido.** Possível tecnicamente,
   mas mistura a fase-ligação com a Fase 9 do PROMPT MESTRE.
   Aumenta superfície de bug e fura a divisão de fases.
4. **Adiar Etapa 2 até a Fase 9 começar.** Mais conservador, mas
   deixa o `docs.generate` fora do schema do modelo por um tempo
   indefinido — o "caminho de produção agora consome o que a suíte
   dos crates já provava" da Etapa 1 fica meio no ar.

## Pendências

- **D4 nomeada com escopo e consequência**: `.exe` instalado não
  gera documentos até `document-worker` ser empacotado. Fecha na
  Fase 9 do PROMPT MESTRE (D6 = instalador leve + bootstrap lazy
  em `%APPDATA%` é a alternativa preferida pelo autor deste ADR).
- **E2E da Etapa 5** roda com runtime de dev (D5). Vai precisar de
  tag explícita no `docs/status.md` quando fechar.
- **`WorkerManager::shutdown(self)` consumir `self`** é aceitável
  aqui mas pode virar mudança de assinatura em fase posterior
  (Etapa 5 da fase-ligação talvez).
- **Tela de diagnóstico** (UI) consome o `tauri::command`
  `DocumentWorkerStatus()` mas o design da tela não é escopo desta
  Etapa.

## Histórico de revisão

- 2026-08-03 — versão inicial. Convergência da conversa da Etapa 2
  da fase-ligação (Opção 2.1 do relatório + 5 condições do user).
