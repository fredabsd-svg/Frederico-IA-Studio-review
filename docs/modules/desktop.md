<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-10
Fase correspondente: 1
-->

# Módulo `desktop` (casca Tauri)

> Diretório: [`apps/desktop/`](../../apps/desktop/)
> Binário: `frederico-desktop` (Rust + Tauri 2)
> Frontend: React + TypeScript + Vite

## O que faz

A casca do Frederico IA Studio. É a única parte do app que conhece
Tauri e a plataforma Windows. Configura a janela, expõe comandos IPC
para o frontend, e delega toda a lógica para os crates do núcleo
(`frederico-core`, `frederico-storage`, `frederico-diagnostics`,
`frederico-security`, `frederico-shared-contracts`).

A Fase 1 entrega duas operações IPC:
- `app_version()` — devolve `frederico_core::APP_VERSION` como string.
- `ipc_dispatch(request)` — despacha um `IpcRequest` (do
  `frederico-shared-contracts`) para o handler correspondente.
  Operações da Fase 1: `ping` e `get_app_info`.

### Startup recovery (Etapa 6+ do fase de Ligação, 2026-08-10)

Se `Database::open` falhar no `.setup` da casca (ex.: banco criado por
uma versão anterior do app com `Migrate(VersionMismatch(N))` — o SHA-384
do arquivo de migração no disco **não bate** com o gravado em
`_sqlx_migrations.checksum`), o app **não** entra em pânico. Em vez
disso, mostra um diálogo nativo do Windows via `tauri-plugin-dialog`
com:

1. A causa específica (`Migrate(VersionMismatch(N))` /
   `Dirty(N)` / `VersionMissing(N)` / `VersionNotPresent(N)` /
   `VersionTooOld/Old(N)` / `Execute(N)` / `Source(...)` — variantes de
   `sqlx::migrate::MigrateError` re-exportadas por
   `frederico_storage::MigrateError`).
2. O caminho completo do banco.
3. O caminho de recuperação específico (backup + delete do .db
   pra `VersionMismatch`; reinstalar pra `VersionMissing`; etc.).
4. A nota de que o diagnóstico completo está no **stderr do app**
   (capture com `cargo run 2> erro.log` ou via Event Viewer do
   Windows) — referência a `frederico-mind.log` que existia
   em drafts anteriores foi removida porque o arquivo não é
   escrito em nenhum lugar do código atual.

A função está em `apps/desktop/src-tauri/src/main.rs::handle_startup_db_error`.
O fluxo é:

- O `tracing::error!` é emitido **antes** de mostrar o dialog (garante
  que o diagnóstico está no stderr mesmo se o dialog falhar).
- O `blocking_show` é chamado em uma **thread separada** com
  `std::panic::catch_unwind` (em headless, o plugin
  `tauri-plugin-dialog-2.7.2` **panica** em vez de retornar `Err` —
  o `catch_unwind` captura e loga via `tracing::warn!`).
- O thread principal espera até **3s** pelo dialog fechar via
  `mpsc::channel::recv_timeout`. Se timeout (headless), loga warning
  e segue. Sem esse timeout, o CI ficaria pendurado esperando alguém
  fechar uma janela que ninguém vê.
- O setup chama `std::process::exit(1)` em vez de `return Err(...)`.
  `tauri::Builder` trata Err do setup como **pânico do main thread**
  (`tauri-2.11.5/src/app.rs:1425` — "Failed to setup app") que
  polui o stderr com stack trace e confunde o smoke test. O
  `std::process::exit(1)` mata direto. Trade-off: destructors não
  rodam (DB não fecha, sockets não fecham) — aceitável porque o
  `Database::open` falhou e o estado já está inconsistente.

A detecção dessa classe de erro é feita pelo test
`apps/desktop/src-tauri/tests/db_open_failure.rs::database_open_fails_when_data_dir_is_a_file`
(PR #48 v4, in-process). Tentativas anteriores (v2 com
`rusqlite` + checksum zerado, v3 com arquivo como data dir)
spawnavam o binário e eram flaky em CI (Windows Server 2022
é mais lento no init do Tauri runtime, excedendo a janela
de 5s do test). A v4 elimina a dependência do Tauri runtime:
cria um tempdir, escreve um arquivo comum dentro, computa
o path do banco como `<arquivo>/frederico.db` (parent é
arquivo), chama `Database::open` via
`tokio::runtime::Builder::new_current_thread().block_on(...)`
e verifica que retorna `Err(StorageError)` com a mensagem
mencionando o path do banco e a falha de I/O subjacente.
Cobre a mesma pré-condição do `.setup` callback: o
`Database::open` deve retornar erro estruturado em path
inválido, pra que o `handle_startup_db_error` seja chamado.
Sem esse test, uma regressão no `Database::open` (ex.: alguém
trocar `create_dir_all` por algo que silenciosamente ignora
a falha) passaria no caminho feliz do smoke mas quebraria
o caminho de erro na produção — o binário panicaria de novo
(regressão do `.expect()` original).

A motivação é a regra do `sqlx::migrate!`: **migração aplicada é
imutável** — checksum SHA-384 do arquivo tem que bater com o gravado
em `_sqlx_migrations.checksum`. Edits posteriores viram migração nova,
mas o app tem que lidar com usuários que estão migrando entre commits.

**Correção de diagnóstico (sessão 2026-08-10, user input):** os
arquivos de migração no repositório **NUNCA foram editados**.
`git log -- crates/storage/migrations/` mostra exatamente 1 commit por
arquivo (Fase 1 criou `0001_initial.sql` em `5f7b8ca`; Fase 2 criou
`0002_chat_core.sql` em `b262304`; Fase 3 criou `0003..0006` em
`b88491c`; etc., nenhum amend). A divergência que disparou o
`Migrate(VersionMismatch(1))` no início desta sessão veio de
**edições locais nos arquivos `.sql` que só existiam na cópia
descartada** (`C:\Users\conta\OneDrive\Documentos\Studio review\
Frederico-IA-Studio-review` e/ou `C:\src\Frederico` — abandonados
quando o user fez o clone limpo em `C:\src\Frederico-IA`).
**Não há defeito de migração no projeto.** O tratamento de erro
continua valendo pra qualquer usuário com banco de versão anterior
(ex.: rodou build de outro commit, depois voltou) — porque o
`sqlx::migrate!` recusa mismatch independente da causa.

## O que expõe

### Rust (`apps/desktop/src-tauri/`)

- `pub fn main()` — inicializa logs, abre o banco, registra comandos Tauri, abre a janela.
- `fn data_local_dir() -> PathBuf` — resolve o path do data dir. **Override via `FREDERICO_DATA_DIR` env var** (criado 2026-08-10 — ver "Decisões não óbvias" abaixo). Sem a env var, usa `ProjectDirs::from("studio", "frederico", "ia")`.
- `fn resolve_db_path() -> PathBuf` — junta `data_local_dir()` com `frederico.db`.
- `fn handle_startup_db_error(handle, db_path, err) -> String` — startup recovery (mostra dialog nativo com causa + caminho de recuperação, loga via `tracing::error!`, espera até 3s pelo dialog fechar).
- `#[tauri::command] fn ipc_dispatch(request, state) -> IpcResponse`
- `#[tauri::command] fn app_version() -> String`

### Environment variables

- **`FREDERICO_DATA_DIR`** (override de `data_local_dir()`) — usado pelo smoke test pra apontar o binário pra um `tempfile::tempdir()` em vez de `%LOCALAPPDATA%`. **Não usar em produção** — apps de user real não setam essa env var, então o path default (ProjectDirs) é preservado. Convenção: env vars com prefixo `FREDERICO_` são o ponto de override público da casca (já existia `FREDERICO_DOCUMENT_WORKER_RUNTIME` da Etapa 2.A do fase de Ligação, mesmo papel pro `document-worker`).
- **`FREDERICO_DOCUMENT_WORKER_RUNTIME`** (Etapa 2.A, existente) — override do path do `document-worker` runtime.

### Permissions (`apps/desktop/src-tauri/capabilities/default.json`)

- `core:default` (lifecycle + window).
- `dialog:default` (necessário pro startup recovery — Etapa 6+).

### Frontend (`apps/desktop/src/`)

- `services/api.ts` — **única** camada que faz `invoke` no Tauri (ADR-0003).
- `services/contracts.ts` — tipos espelhados do `frederico-shared-contracts`. Virará arquivo gerado na Fase 2 (REGRAS §1.9).
- `routes/Home.tsx` — busca `app_info` via IPC e mostra o resultado.
- `routes/About.tsx` — texto estático.
- `App.tsx` — layout com `HashRouter` e 2 rotas.

## De quem depende / quem depende dele

- **Depende de:** Tauri 2, `tauri-plugin-dialog`, `tokio`, todos os crates do núcleo, `directories`, `tracing`.
- **Usado por:** o usuário final (janela desktop).

## Decisões não óbvias / armadilhas

- `tauri::async_runtime::block_on` é usado em `setup` para abrir o banco de forma síncrona antes da janela abrir. Trocar para `tokio::spawn` exigiria lidar com inicialização tardia dos comandos.
- O `services/` do frontend é a **única** camada que importa `@tauri-apps/api/core`. Componentes nunca chamam `invoke` diretamente. Lint futuro no CI deve verificar isso.
- O CSP em `tauri.conf.json` é restritivo. Recursos adicionais exigem atualização consciente.
- **`FREDERICO_DATA_DIR` é o ponto de override do data dir** — **NUNCA** o binário deve abrir `%LOCALAPPDATA%` em contexto de test. O smoke test usa `tempfile::tempdir()` e seta essa env var antes de spawnar o binário, garantindo **zero contato com o banco de produção do user**. Sem essa env var, qualquer `cargo test` destruiria conversas/memórias/runs reais (verificado na sessão 2026-08-10: o `frederico.db` do user foi de 376KB pra 0 bytes durante a investigação). Lição: **nunca use o path de produção em tests** — tempdir sempre.
- **`tracing_subscriber::fmt::layer()` precisa de `.with_writer(std::io::stderr)` explícito** — o default é `stdout`, e o smoke test `smoke_startup` só captura stderr via `Stdio::piped()`. Sem o writer explícito, o `tracing::error!` do startup recovery não aparece no test, e a nova classe de erro não é detectada (volta pro falso "smoke verde" que motivou a criação do test). Garantido em `frederico-diagnostics/src/lib.rs::init`.
- **`sqlx::migrate::MigrateError` é `#[non_exhaustive]`** — match exaustivo nas variantes precisa de fallback `_ => ...`. Variantes cobertas no `handle_startup_db_error` (2026-08-10): `VersionMismatch`, `Dirty`, `VersionMissing`, `VersionNotPresent`, `VersionTooOld`, `VersionTooNew`, `ExecuteMigration`, `Execute`, `Source`. Variantes futuras do `sqlx` caem no fallback com mensagem genérica.
- **`tauri::Builder` trata `Err` do setup como pânico do main thread** — verificado em `tauri-2.11.5/src/app.rs:1425`. O startup recovery **não** usa `return Err(...)`; usa `std::process::exit(1)` direto, que mata o processo sem passar pelo runtime. Sem isso, o stderr ganha um "Failed to setup app" panic que polui o diagnóstico e confunde o smoke test.
- **`tauri-plugin-dialog` panica em headless** (verificado em `tauri-plugin-dialog-2.7.2/src/lib.rs:358` na sessão 2026-08-10: `called \`Result::unwrap()\` on an \`Err\` value`). O `handle_startup_db_error` envolve o `blocking_show` com `std::panic::catch_unwind` pra capturar o panic em headless, loga via `tracing::warn!`, e prossegue (o `tracing::error!` lá em cima já tem o diagnóstico completo).

## Como testar isoladamente

```pwsh
# Compilação
cargo build -p frederico-desktop

# Lint do frontend
cd apps/desktop
npm run typecheck

# Build do frontend (gera apps/desktop/dist/)
npm run build

# Smoke test do startup (cobre o panic do .setup,
# a classe de erro "startup recovery", e garante
# que o test NUNCA toca %LOCALAPPDATA% — usa
# tempfile::tempdir() + FREDERICO_DATA_DIR)
cargo test -p frederico-desktop --test smoke_startup -- --nocapture
```

**Cenário de teste do caminho de erro (startup recovery):** o
`database_open_fails_when_data_dir_is_a_file` em
`tests/db_open_failure.rs` cobre isso **automaticamente** —
in-process, sem spawn de binário. Cria um tempdir, escreve um
arquivo dentro, e verifica que `Database::open` retorna erro
estruturado. Não precisa de comando manual.

**Cenário manual (apenas pra investigar regressão):** se quiser
reproduzir localmente o cenário "banco de versão anterior na
produção" (NÃO recomendado — destrutivo), o caminho era:

```pwsh
# NÃO RECOMENDADO: tritura o banco de produção.
# Em vez disso, use o test automático acima.
$bak = "$env:LOCALAPPDATA\frederico\ia\data\frederico.db.bak"
$db  = "$env:LOCALAPPDATA\frederico\ia\data\frederico.db"
Move-Item $db "$db.good" -Force
Move-Item $bak $db -Force
# Roda o app — deve mostrar o dialog de recovery.
# NÃO feche o dialog até anotar a mensagem.
# Restaura depois.
Move-Item $db $bak -Force
Move-Item "$db.good" $db -Force
```

## O que este módulo **não** faz

- Não tem chat, tools, memória, documentos. É a casca mínima.
- O `handle_startup_db_error` cobre só as variantes atuais de
  `sqlx::migrate::MigrateError` — variantes futuras caem no fallback
  com mensagem genérica (ainda assim, o user vê um dialog, não um
  pânico silencioso).
- O instalador empacota via NSIS (target declarado em `tauri.conf.json`); a Fase 1 inclui o build do bundle, mas a Fase 9 cuida da produção final.
