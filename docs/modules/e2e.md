<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-04
Fase correspondente: Fase de Ligação (entre Fase 5 e Fase 6), Etapa 5
-->

# `frederico-e2e`

Crate de **testes E2E** do Frederico IA Studio. Hospeda os testes
que atravessam o caminho de produção do app sem subir a casca
Tauri: `modelo → ChatOrchestrator → ToolRegistry → kit →
WorkerToolDispatcher → WorkerInvoker → document-worker → arquivo`.

Ver [`docs/architecture/testing-strategy.md`](../architecture/testing-strategy.md) §3
e a narrativa de processo
[`docs/releases/fase-ligacao/pr-fase-ligacao-etapa-5.md`](../releases/fase-ligacao/pr-fase-ligacao-etapa-5.md).

## 1. O que este módulo faz

Centraliza os E2E em um crate dedicado (`frederico-e2e`) por três
razões combinadas (Etapa 5 da Fase de Ligação, 2026-08-04):

- **A casca e os E2E chamam a mesma função de composição**
  (`frederico_app::build_chat_orchestrator`). Sem um crate
  dedicado que importe `frederico-app`, a fronteira "os testes
  usam a mesma função que a casca" depende de disciplina
  humana, não de compilador. Com o crate dedicado, o
  `cargo test -p frederico-e2e` falha se a função de
  composição mudar a ponto de quebrar o caminho — a
  divergência entre "o que o teste exercita" e "o que a casca
  usa em produção" volta a ser possível de detectar
  mecanicamente.
- **`dev-dependencies` isoladas.** O `frederico-app` (que é
  núcleo puro), o `tempfile`, o `provider-engine` em modo
  `fake::trait_level` — tudo isso é dependência de teste,
  não de produção. Manter as dev-deps num crate separado
  impede que algum desses vá parar por engano nas deps de
  produção de outro crate.
- **`cargo test -p frederico-e2e` roda só os E2E.** O
  `--workspace` continua rodando os unit + integration + E2E
  juntos (o gate do CI), mas durante o desenvolvimento local
  dá pra isolar os E2E sem custo do resto.

### A fronteira que importa (não a pasta)

O que faz esses testes serem E2E **não é** o nome
"frederico-e2e" — **é o fato de consumirem
`frederico_app::build_chat_orchestrator`**, a mesma função
que a casca Tauri chama. Se alguém começar a adicionar
testes de unidade aqui, esse teste vira mentiroso (passa
verde mas não prova nada do caminho de produção). A regra é
mecânica:

- Os testes em `crates/e2e/tests/` **devem** montar o
  `ChatOrchestrator` via `frederico_app::build_chat_orchestrator(parts)`.
  Nada de montar o `ChatOrchestrator::new(...)` direto, nada
  de `use` em `apps/desktop/src-tauri` (que é binário).
- O `helpers::build_orchestrator(...)` em
  `tests/common/mod.rs` é a única porta de entrada — quem
  quiser um teste novo **importa** o helper, não duplica a
  montagem.

A Etapa 6 da fase-ligação vai criar o gate "E2E que
atravessa a casca por fase" — ele vai procurar esses testes
por caminho. **Não mudar o caminho `crates/e2e/tests/`
sem ADR.**

## 2. O que ele expõe

**Nenhuma API pública para outros crates** (é crate de teste
com `publish = false`).

**Estrutura interna (Etapa 5):**

- `tests/common/mod.rs` — helper `pub fn build_orchestrator`
  que constrói o `ChatOrchestrator` completo a partir de
  `Database::open_in_memory()`, `RecordingEventSink`,
  `FakeProviderAdapter` programável via `with_events`,
  `FileSystemJailResolver` apontando para um `tempdir` por
  teste, `SystemClock`, e o `invoker: Option<Arc<dyn
  WorkerInvoker>>` que decide se entra `DocumentWorkerLauncher`
  real ou `FakeWorker` in-process. Devolve o
  `ChatOrchestrator` (wrapped em `Arc` — `send_message` exige
  `self: &Arc<Self>`) + handles pros componentes que o
  caller precisa asserir (sink, db).
- `tests/common/mod.rs` — helper `pub async fn wait_for_run`
  que poll o `RecordingEventSink::events_for(run_id)` até ver
  o `RunStatus` final (Completed / Failed / Cancelled /
  Timeout) ou estourar timeout.
- `tests/e2e_files_read.rs` — caminho de produção do
  `files.read`: provedor chama o tool, jail resolve o
  `tempdir/<cid>/hello.txt`, conteúdo volta pro modelo.
- `tests/e2e_degradation_declared.rs` — sem invoker, o
  catálogo só tem `files.read`. O provedor decide chamar
  `docs.generate` mesmo assim (simulando prompt
  injection). O `RunExecutor` rejeita com
  `ToolNotAllowed` (defesa contra manifest injection).
  **Bump atômico capability+permission** (ADR-0020 §3 D3).
- `tests/e2e_jail_per_conversation.rs` — duas conversas
  com workspaces diferentes; arquivo do jail B é bloqueado
  em jail A (regressão do §I3 do threat model).
- `tests/e2e_docs_generate_with_fake_worker.rs` —
  `docs.generate(docx)` com `FakeWorker` in-process;
  assere que o `WorkerInvoker` foi chamado com os args
  certos e devolveu `{ok: true, echo: ...}` (caminho do
  motor até o `WorkerInvoker`, **para antes do Python**).
- `tests/e2e_docs_generate_with_real_worker.rs` —
  `#[ignore = "requer document-worker runtime..."]`. Gera
  `.docx` real via `DocumentWorkerLauncher` apontando pro
  Python, reabre o arquivo via `docx` (biblioteca stdlib
  + xml.etree), valida hierarquia. Ativado por
  `scripts/verify-external.ps1` no CI depois do
  `bootstrap.ps1`. **Esse é o teste que prova "a Fase de
  Ligação fechou"** — sem ele, a Etapa 5 fecha com
  `cargo test --workspace` verde mas sem nunca ter gerado
  um documento pelo caminho do produto.

## 3. De quem depende e quem depende dele

**Depende de (dev-deps, todas as crates do workspace que
compõem o caminho):**

- `frederico-app` — composição (`build_chat_orchestrator`,
  `build_default_tools`, `build_default_allowed_for_run`,
  `initial_permission_set`,
  `initial_permission_set_for_capable_launcher`,
  `FileSystemJailResolver`).
- `frederico-core`, `frederico-tool-registry`,
  `frederico-provider-engine`, `frederico-storage`,
  `frederico-security`, `frederico-model-catalog`,
  `frederico-execution-engine`,
  `frederico-process-architecture`,
  `frederico-document-kits`, `frederico-document-engine`,
  `frederico-test-support` — tipos e implementações
  consumidos via `frederico-app`.
- `tokio`, `tokio-util`, `futures`, `async-trait` — async
  runtime.
- `tempfile` — `tempdir` por teste.
- `uuid`, `serde_json`, `serde`, `chrono`, `tracing` —
  utilitários.

**Quem depende dele:** ninguém. É crate de teste (`publish =
false`). Outros crates que precisarem reusar o
`build_orchestrator` em testes próprios **devem** chamar o
`frederico-app` direto, não importar de `frederico-e2e`
(isso seria ciclo, mesmo se `frederico-e2e` não fosse
`publish = false`).

## 4. Decisões não óbvias e armadilhas conhecidas

- **`publish = false`.** A crate é de teste; não vai pro
  `crates.io`, não é dep de ninguém. Se alguém quiser
  reusar o `build_orchestrator` em outro lugar, deve
  reusar via `frederico-app`.
- **Sem `src/lib.rs`.** Os helpers ficam em
  `tests/common/mod.rs` (padrão Cargo de teste de
  integração). Cada `tests/e2e_*.rs` faz `mod common;` no
  topo e usa `common::build_orchestrator(...)` direto.
  Adicionar `src/lib.rs` só se houver helpers que outros
  crates precisem reusar — e nesse caso, mover o que for
  reusável pra `frederico-app`.
- **Caminho fixado em `crates/e2e/tests/`.** A Etapa 6 vai
  criar o gate "E2E por fase" que procura por esse path.
  Mover pra `tests/e2e/` na raiz do workspace exigiria um
  package root com dev-deps (que o workspace não tem) — o
  caminho atual é a forma idiomática do Cargo workspace.
- **`cargo test -p frederico-e2e` ignora
  `--include-ignored` por default.** O teste
  `e2e_docs_generate_with_real_worker` é `#[ignore]`;
  rodar `cargo test -p frederico-e2e` (sem flag) **não**
  exercita o caminho Python. Pra ativar:
  `cargo test -p frederico-e2e -- --include-ignored`. O
  CI faz isso via `scripts/verify-external.ps1` (depois
  do `bootstrap.ps1`).
- **A maioria dos testes para antes do Python.** Com
  `FakeWorker` in-process, o `WorkerInvoker::invoke` devolve
  `{ok: true, echo: <args>, env_received: ...}` — o que o
  fake responde, não um arquivo real. Isso prova o
  caminho do motor (modelo → casca → WorkerInvoker) mas
  **não** prova que o `document-worker` Python gera um
  arquivo válido. Próxima sessão: ler
  [`testing-strategy.md` §3](../architecture/testing-strategy.md)
  antes de adicionar teste novo — não ler "E2E verde"
  como "documento gerado de verdade" sem checar qual teste
  cobre o quê.
- **Gate de pureza não aplica.** `frederico-e2e` é crate
  de teste (não núcleo) — `check-core-purity.ps1` não
  roda aqui. **Mas:** a regra de que `frederico-app`
  continua puro (sem `tauri`, sem `windows`) é a coisa
  que mantém o `cargo test -p frederico-e2e` verde. Se
  alguém adicionar `tauri` ao `frederico-app` por
  "simplificar", o E2E quebra — gate mecânico
  (`check-core-purity.ps1` + a compilação deste crate)
  pega.

## 5. Como testá-lo isoladamente

```pwsh
# Só os E2E (sem #[ignore])
cargo test -p frederico-e2e

# E2E + o de worker real (gera .docx via Python)
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap.ps1
cargo test -p frederico-e2e -- --include-ignored

# Suíte completa do workspace (CI faz isso)
cargo test --workspace

# Verificação mecânica (fmt, clippy, pureza do núcleo, docs)
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
```

A Etapa 5 adiciona um step novo em
`scripts/verify-external.ps1` que roda o
`e2e_docs_generate_with_real_worker` (com
`--include-ignored`) depois do `bootstrap.ps1` — é o
cobre a fronteira "até o Python".

## 6. O que ele **não** faz

- **Não sobe a casca Tauri.** A decisão arquitetural
  (ADR-0022 §D4) é: testes E2E consomem a mesma função
  de composição que a casca, sem subir o binário. Subir
  a casca via `tauri-driver` é trabalho de fase futura
  (provavelmente a Etapa 6 da fase-ligação, ou a Fase 9
  com a máquina limpa do `testing-strategy.md` §5).
- **Não é o lugar pra teste de unidade.** Teste de
  unidade vai em `#[cfg(test)] mod tests` no próprio
  crate sendo testado, com `cargo test -p <crate>`. Este
  crate aqui é só pra E2E do caminho de produção.
- **Não é o lugar pra teste de performance.** O
  `testing-strategy.md` §"Desempenho" lista budget
  numéricos (tempo até janela visível, latência de
  digitação); a máquina de referência é um i5-3570. Os
  testes de performance ficam na Etapa 6 da fase
  futura, não aqui.
- **Não é o lugar pra fixtures versionadas.** Fixtures
  vão em `packages/testing-fixtures/` (spec
  `testing-strategy.md` §"Dados de teste"). Este crate
  consome `frederico-test-support` (helpers de timeout
  e estrutura), mas não define fixtures próprias.
