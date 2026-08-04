# PR 24 (Etapa 5 da Fase de Ligação): testes E2E atravessando o caminho de produção

## Contexto (transparente)

A Etapa 2.B (PR #23, mergeada em `4a59b4b`) fechou o **caminho do
modelo** (ChatOrchestrator → ToolRegistry → docs.generate → kit →
WorkerToolDispatcher → WorkerInvoker → document-worker) e
consolidou 3 ADRs. Mas o **caminho de produção end-to-end** ainda
não tinha sido provado em CI automatizado: os tests existentes
em `crates/document-kits/tests/e2e_*` exercitam o kit + IPC via
`WorkerToolDispatcher` direto, **não** atravessam o
`ChatOrchestrator`. O `cargo test --workspace` cobria o motor
mas não o caminho "modelo decide tool → ChatOrchestrator
roteia → kit invoca → worker responde → run fecha".

A Etapa 5 da Fase de Ligação fecha esse gap com 5 testes E2E em
`crates/e2e/tests/` (nova crate `frederico-e2e` no workspace).

## Decisão sobre a estrutura dos E2E (a conversa do PR)

O spec original (`docs/architecture/testing-strategy.md` §3)
dizia `Localização: tests/e2e/`. A primeira tentativa foi
manter o path `tests/e2e/` na raiz do workspace, mas o Cargo
não reconhece subdiretórios de `tests/` sem `[[test]]` entries
explícitas (e mesmo com, o overhead de `dev-dependencies`
isoladas é grande). Discussão registrada na conversa: 3 opções
(manter `tests/e2e/` via crate hospedeira virtual, criar
`crates/e2e/` como membro do workspace, ou `[[test]]` por
arquivo) → **A escolhida**: `crates/e2e/` como 14º membro do
workspace. Vantagens concretas:

- `cargo test -p frederico-e2e` roda só os E2E (sem custo do
  resto).
- `dev-dependencies` isoladas (o `frederico-app`,
  `tempfile`, `provider-engine` em modo `fake::trait_level`)
  não contaminam outros crates.
- Cada arquivo em `crates/e2e/tests/` é um alvo de teste
  automático, sem `[[test]]` manual.
- O `check-core-purity.ps1` continua sem exceção (`frederico-e2e`
  é crate de teste, não de produção; a regra de pureza do
  `frederico-app` continua valendo — se alguém adicionar
  `tauri` ao `frederico-app`, os E2E quebram).

## Decisão sobre o que os E2E cobrem (a fronteira — registrada em `testing-strategy.md` §3)

A maioria dos E2E consome o `FakeWorker` in-process (definido
em `crates/process-architecture/src/fake.rs`). O `FakeWorker`
implementa o envelope IPC sobre `tokio::sync::mpsc` — sem pipes
reais, sem Python, sem `document-worker`. **Ele exercita o
contrato do `WorkerInvoker`** (ADR-0024) e o caminho do motor
(modelo → ChatOrchestrator → ToolRegistry → kit → dispatcher →
invoker), mas **para antes do Python**: o que volta do
`invoke` é o que o `FakeWorker` devolve (`{ok: true, echo:
<args>, env_received: ...}`), não é um arquivo gerado de
verdade. Isso **prova que o motor e a casca estão bem ligados**
— exercita o bump atômico do `documents: None → Full` (ADR-0020
§3 D3), o `Arc<dyn WorkerInvoker>` no `setup`, o `ToolRegistry`
com 3 manifestos, a allowlist, o `PermissionSet`, o
`JailResolver` por conversa, o `RecordingEventSink`, a
persistência de `Message` e `Run` no SQLite, o journal de
eventos. **O que NÃO prova** é que o `document-worker` Python
real gera um `.docx` válido.

**Um único teste** (`e2e_docs_generate_with_real_worker`,
`#[ignore]`) **vai até o fim do caminho de produção**: usa o
`DocumentWorkerLauncher` real (Etapa 2.A, ADR-0023) com o
`document-worker` Python real, e gera um arquivo `.docx` de
verdade. Esse teste é `#[ignore]` por default — **não** roda
em todo PR. Ele é ativado pelo `scripts/verify-external.ps1`
(que garante o `bootstrap.ps1` antes) e conta como a
evidência "a Fase de Ligação fechou" — sem ele, a Etapa 5
fecha com `cargo test --workspace` verde, mas sem nunca ter
gerado um documento pelo caminho do produto.

## Decisão sobre a composição compartilhada (regra do ADR-0022 §D4)

Os E2E **importam** o helper `build_orchestrator` em
`tests/common/mod.rs`, que chama
`frederico_app::build_chat_orchestrator(parts)` — a **mesma
função** que a casca Tauri chama em `apps/desktop/src-tauri/src/main.rs`.
Os testes **não** montam o `ChatOrchestrator::new(...)` direto,
**não** fazem `use` em `apps/desktop/src-tauri` (que é binário).
Se alguém regredir a composição (mover código da casca pro
crate, ou vice-versa), `cargo test -p frederico-e2e` quebra
na hora — mecânico, sem depender de revisão. Essa é a regra
"mesma função que a casca" do ADR-0022 §D4, implementada
como porta de entrada única (helper) em vez de convenção
humana.

## O que entra (commits)

O PR é 1 só, com 5 commits focados:

1. `chore(deps): cria crates/e2e/ como 14º membro do workspace
   (publish=false, dev-deps)` — `crates/e2e/Cargo.toml` +
   `Cargo.toml` raiz (adiciona `crates/e2e` ao `members`).
2. `docs(testing): testing-strategy.md para parcialmente
   implementado + nova seção "Fronteira do que os E2E cobrem"`
   + "Regra da composição compartilhada"` — promove o
   cabeçalho do spec + 2 parágrafos novos. Localização muda
   de `tests/e2e/` para `crates/e2e/tests/` (com explicação
   no doc).
3. `feat(e2e): crate frederico-e2e — helper build_orchestrator
   + ScriptedProvider + 4 testes (files.read, degradation,
   jail, fake worker)` — o coração do PR. Inclui
   `tests/common/mod.rs` (helpers), `tests/e2e_files_read.rs`,
   `tests/e2e_degradation_declared.rs`,
   `tests/e2e_jail_per_conversation.rs`,
   `tests/e2e_docs_generate_with_fake_worker.rs`. 4 testes
   passam em `cargo test -p frederico-e2e` (sem runtime
   Python).
4. `feat(e2e): e2e_docs_generate_with_real_worker (#[ignore];
   gera .docx real via document-worker Python)` — único
   teste que atravessa o caminho completo. Marcado
   `#[ignore]` por default; ativado pelo `verify-external.ps1`
   (Step 7).
5. `chore(ci): verify-external.ps1 Step 7 ativa o teste de
   worker real` + `docs(status + fase-ligacao): narrativa +
   status + changelog` — finalização documental.

## Observações sobre o `e2e_degradation_declared` (o teste que mais protege contra regressão)

Esse teste prova que a Etapa 2.B instituiu o **bump atômico**
do `documents: None → Full` (ADR-0020 §3 D3) **funciona**:
sem invoker, o catálogo só tem `files.read`. Quando o
provedor tenta chamar `docs.generate` (simulando prompt
injection), o `executor.rs:702` faz
`self.registry.get(&tool_id).ok_or(UnknownTool)?` ANTES do
`validate_tool_call` — o `Err` propaga pro
`ChatOrchestrator` (linha 318 do `orchestrator.rs`), que
mapeia pra `RunStatus::Failed` e emite no sink. O
`Message.error` carrega `"run abortado: erro do executor"`. A
alternativa silenciosa (deixar `docs.generate` executar com
tool fake, ou registrar o tool sem o permission) seria o
exato modo de falha "documento falso entregue como
verdadeiro" que a Fase de Ligação existe pra evitar. **Esse
teste trava qualquer regressão** nessa simetria capability +
permission.

## Observações sobre o `e2e_jail_per_conversation`

2 conversas (`A`, `B`) com workspaces diferentes. `A` tenta
ler `secret.txt` de `B` via `../<cid_b>/secret.txt`. O
`Jail::resolve` rejeita com `TOOL_JAIL_VIOLATION` no Passo 1
(`Component::ParentDir`). **O conteúdo do `secret.txt` não
vaza** nem pro journal nem pro conteúdo da mensagem
assistant. Cobertura: regressão §I3 do threat model.

## Observações sobre o `e2e_docs_generate_with_real_worker`

O `DocumentWorkerLauncher` é lazy: spawna o `python.exe` na
primeira `invoke()`. O `frederico_app::LauncherConfig::default()`
configura 30s de timeout de invoke, 10s de ready_timeout.
Cold-start do Python no CI: ~5-10s (com `runtime/` cacheado
em `actions/cache@v4`); budget de 60s cobre com folga. O
`WordProKit::render` chama `WorkerInvoker::invoke` (via
`impl WorkerInvoker for DocumentWorkerLauncher`, ADR-0024
§D4), que spawna o Python e devolve o `path/size_bytes/...`
do `.docx` gerado. **O teste não só prova que o
`DocumentWorkerLauncher` funciona** — prova que a Fase de
Ligação inteira está bem ligada: composição (`frederico-app`)
+ tools (`document-kits`) + dispatcher (`tool-registry`) +
invoker (`DocumentWorkerLauncher` lazy + restart) + worker
real (Python) **produz um arquivo `.docx` válido pelo caminho
do produto**. Sem isso, a Etapa 5 fecha sem nunca ter
gerado um documento.

## Pendências

- **Etapa 3** — `MemoryExtractor` + embedding adapter reais
  (só depende de Fase 4 Etapa 5, já fechada).
- **Etapa 4** — decidir `frederico-agent-engine` (pendência
  de Fase 6).
- **Etapa 6** — gate CI que valida "E2E que atravessa a casca"
  por fase. Vai procurar os testes por path:
  `crates/e2e/tests/` — **fixado**, não mover sem ADR.
- **E2E com worker real por kit** — a Etapa 5 cobre `docx`
  ponta-a-ponta. Falta `xlsx` e `pdf` — pendência pra Etapa
  6 da fase ou Fase 6.
- **Subir o binário Tauri** (decidir `tauri-driver` vs.
  Playwright vs. custom) — sai do escopo da Fase de
  Ligação, é a próxima fase intermediária ou a Fase 9 com
  a máquina limpa do `testing-strategy.md` §5.

## Achado do primeiro CI (o que valeu a fase)

A primeira execução do `verify (Windows)` no PR #24
**deu vermelho em 0.74s** — exatamente o que a Fase de
Ligação existe para detectar. Antes da Etapa 5, os testes
em `crates/document-kits/tests/e2e_*` e em
`crates/process-architecture/tests/external_doc_worker.rs`
exercitavam o kit + IPC direto, **não** atravessavam o
`DocumentWorkerLauncher` que a casca Tauri usa. A Etapa 5
construiu o caminho completo (`build_orchestrator` →
`ChatOrchestrator` → `ToolRegistry` → kit →
`WorkerToolDispatcher` → `DocumentWorkerLauncher` →
Python worker) — e o primeiro run real encontrou um
defeito de modelagem que estava latente desde a Fase 5
Etapa 2.A (PR #22).

### O defeito

O `DocumentWorkerLauncher::invoke` checa
`health_snapshot()` antes do `invoke` real:

```rust
let health = handle_clone.health_snapshot().await;
if matches!(health.health, WorkerHealth::Unhealthy) {
    return Err(WorkerError::InvokeFailed(ProcessError::Unhealthy { ... }));
}
```

Mas o `WorkerHealth::Unhealthy` é o `#[default]` do enum
E o valor inicial de `fresh_health_snapshot()`. O snapshot
só vira `Ok` no primeiro `Pong` recebido pelo ator
(`manager.rs:839`); o handshake `worker.hello`/`app.ack`
do `spawn_external` **não** emite `Pong`. Resultado: a
primeira `invoke` via `DocumentWorkerLauncher` via
`DocumentWorkerLauncher::invoke` via caminho do produto
sempre rejeitava com `Unhealthy`, mesmo com worker vivo e
respondedor. **A Fase 5 inteira fechou verde sem tocar
nesse caminho** — Etapas 1, 2.A, 2.B, 2.B+X, 2.B+Y, 3, 4
e 5 PR 1/2/3 testavam o `WorkerHandle::invoke` direto (que
bypassa a checagem de saúde) ou usavam o `FakeWorker`
in-process (que tem o ator já pingado).

### O fix

`crates/app/src/launcher.rs`:

1. Helper `pub(crate) async fn ensure_first_pong(handle: &WorkerHandle) -> Result<(), ProcessError>` —
   se a saúde é stale (`Unhealthy`), faz um `ping`; o
   `Pong` que volta atualiza o snapshot para `Ok`. Falha
   do `ping` é propagada (worker realmente morto — a
   checagem `Unhealthy` subsequente trata).
2. `DocumentWorkerLauncher::invoke` chama
   `let _ = ensure_first_pong(&handle_clone).await;`
   **antes** da checagem de saúde.
3. Teste de regressão em
   `crates/app/src/launcher.rs::tests::ensure_first_pong_initializes_stale_health_snapshot`:
   usa o `FakeWorker` in-process (barato, roda no CI
   comum, não exige runtime Python) e prova que (1) o
   snapshot começa `Unhealthy` (stale), (2) o helper
   atualiza para `Ok`, (3) é idempotente. Se a modelagem
   do `WorkerHealth` mudar, este teste quebra — é o sinal
   pra revisar a pendência.

### A pendência (não só o sintoma)

O defeito real **não** é "falta um ping" — é que
`WorkerHealth::Unhealthy` é usado como estado inicial,
confundindo "nunca observei" com "observei e está ruim".
Qualquer outro consumidor que chame `health_snapshot()`
antes do primeiro `Pong` recebe a mesma resposta falsa.
A modelagem deveria ter um `WorkerHealth::Unknown`
(nunca observado) distinto de `Unhealthy` (observado e
ruim), e a guarda deveria recusar só em `Unhealthy`.
**O `ensure_first_pong` é o trabalho que a modelagem
deveria fazer sozinha.** Pendência nomeada em
`docs/modules/process-architecture.md` §"Pendências para
a próxima sessão" item 4 — quando o `process-architecture`
for reaberto, introduzir o `Unknown` e fazer o helper
sumir.

### Outro achado: `ToolManifest::allowed_paths` não-populado

Investigação durante o fix (condição 4 do user): o
`ToolManifest::allowed_paths` E o
`WorkerToolDispatcher::allowed_paths` estão **vazios**
no `docs.generate`:

- `crates/app/src/composition.rs:224` —
  `WorkerToolDispatcher::new(invoker, vec![])` com
  comentário "A Etapa 6 da fase-ligação (UI de
  configuração) ou a Fase 9 (empacotamento) podem
  popular esse vetor".
- `crates/document-kits/src/generate.rs:124-144` — o
  `ToolManifestBuilder` do `docs.generate` **não**
  chama `.allowed_path(...)` na chain. O
  `manifest.allowed_paths` fica `vec![]` (default).

Consequência: o `validate_against_allowlist` em
`worker_dispatch.rs:232-234` retorna `Ok(())` quando a
allowlist é vazia (sem validação — `caller decide`). E o
`DocsGenerateTool::execute` (generate.rs:246) chama
`self.dispatcher.check_path(output_path_str)` que é
no-op com vetor vazio. **A barreira de path que o
ADR-0018 §Decisão 4 documentou como "a forte entra na
Etapa 3 com `ToolManifest::allowed_paths`" não está
ligada no caminho de produção.** O `output_path` do
`docs.generate` passa direto pro Python worker, que
aplica a barreira fraca do `validate_path`
(rejeitar `..` + path absoluto ou relativo-ao-`cwd` +
diretório pai gravável). Resultado prático: o
`output_path: "C:\Users\conta\Documents\qualquer.docx"`
(absoluto, sem `..`, diretório gravável) **passa** —
o `docs.generate` pode escrever em qualquer path
absoluto do disco do usuário, não só no workspace da
conversa.

A Etapa 3 da Fase 5 documentou em
`composition.rs:213-223` que "A Etapa 6 da
fase-ligação (UI de configuração) ou a Fase 9
(empacotamento) podem popular esse vetor". **Nenhuma
das duas foi implementada** — é a pendência 1 do
`process-architecture.md` ("Etapa 3 (ToolRegistry +
kits DocumentSpec): ToolManifest::allowed_paths para
path safety forte"). O fix correto é popular a allowlist
no `build_default_tools` com o `<workspaces_root>`
(que o `WorkspaceTempdir` já conhece) — **mesma
filosofia do `ensure_first_pong`**: muda o caminho de
produção, não afrouxa validação. Fora do escopo da
Etapa 5 (a Etapa 5 fechou o gap de teste; o gap de
segurança é uma Etapa própria).

### O placar

A Fase de Ligação encontrou, no primeiro CI real, um
defeito que impedia qualquer geração de documento no
`.exe` do usuário. A Fase 5 inteira fechou verde sem
tocar nesse caminho (Etapas 1, 2.A, 2.B, 2.B+X, 2.B+Y,
3, 4, 5 PR 1/2/3 somam ~7 PRs e 0 exercícios do
`DocumentWorkerLauncher` com worker vivo via caminho
do produto). É exatamente o argumento de existência da
fase, agora com evidência: **a Fase de Ligação existe
porque os testes das fases anteriores não atravessam
o caminho do produto** — e a Etapa 5 provou que
afirmação.

## Aviso importante ao user (registrado em `docs/modules/e2e.md`)

> "Vale lembrar que a decisão de fundo (o provedor falso,
> golden files versus gerador determinístico) já é o
> ADR-0008. Isto aqui é aplicá-la, não decidi-la de novo."

E a frase do user sobre o `crates/e2e/`:

> "Vale lembrar que a decisão de fundo (o provedor falso,
> golden files versus gerador determinístico) já é o
> ADR-0008. Isto aqui é aplicá-la, não decidi-la de novo."

A Etapa 5 **aplicou** o ADR-0008 (via `ScriptedProvider` em
nível de trait) e o ADR-0022 §D4 (compartilhar composição).
Nenhuma decisão nova estrutural — só a execução concreta.
