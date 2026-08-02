# 0022 — `JailResolver` (Etapa 1 da Fase de Ligação) + `frederico-app` como camada de composição pura

## Contexto

O `apps/desktop/src-tauri/src/main.rs` atual (commit `eab413a`,
origem da Fase de Ligação) tem três problemas conectados na
casca que vazam pra todo o produto:

1. **`ToolRegistry::new()` é vazio** na linha 199 do `main.rs`.
   Nenhum manifesto é registrado, então o Passo 1 do
   `validate_tool_call` (spec
   [`tool-registry-specification.md`](../architecture/tool-registry-specification.md)
   §"Validação antes de execução") reprova qualquer `tool_call` com
   `ToolErrorCode::TOOL_NOT_FOUND` — inclusive `files.read`, que está
   na lista de `tools` (instanciado em `Vec<Arc<dyn Tool>>`) mas não
   tem manifesto.
2. **`PermissionSet::default()` no `orchestrator.rs:260`** é deny-all
   hardcoded. Mesmo com manifesto registrado, o Passo 5 reprovaria
   (`file_read: FileReadPermission::None`).
3. **`jail = Jail::new(std::env::current_dir()?)`** (linha 203 do
   `main.rs`) aponta pro diretório de trabalho do processo. Qualquer
   tool em execução lê qualquer arquivo do PC que o usuário tenha
   acesso. A ameaça I3 (path traversal) está mitigada pelo `Jail`
   em si — mas a fronteira de isolamento é o cwd, não a conversa,
   o que vazaria arquivos entre conversas distintas se o
   `FilesReadTool` resolvesse `..` dentro de `workspaces/a/` para
   chegar a `workspaces/b/`.

A Etapa 1 da Fase de Ligação
([`docs/architecture/process-architecture.md`](../architecture/process-architecture.md)
+ prompt da fase) exige conectar à casca o que já foi construído
nos crates, sem acrescentar funcionalidade nova. Para o item (3)
em específico, a regra é: **jail por conversa** (um por
`ConversationId`), porque é a única configuração que isola
realmente o que o usuário trata como "escopo" no produto — é
exatamente a fronteira que a memória chama de "escopo" no
[`memory-architecture.md`](../architecture/memory-architecture.md)
§"Modelo de escopo" e que motivou o tratamento da I4
(vazamento cruzado).

A Etapa 7 (modo desenvolvedor) vai definir o resolvedor
definitivo do workspace via `frederico-security` (Job Objects do
Windows + AllowVolumeAccess). Esta ADR documenta a **versão
mínima correta** que a Etapa 1 entrega, registra a evolução
como pendência, e fixa a forma do contrato para que a Etapa 7
substitua a implementação sem mudar a interface.

## Decisão

Quatro decisões, todas no commit `fase-ligacao/conectar-motor-a-casca`
(Etapa 1 da Fase de Ligação, branch a partir de `main`):

### D1. Novo crate `frederico-app` (workspace member)

- Caminho: `crates/app/`.
- Função: **camada de composição**. Detém o que é "montar o app"
  e nada do que é "rodar a UI" — a casca Tauri continua sendo a
  casca. É o ponto de entrada que os E2E da raiz (`tests/e2e/`,
  Etapa 5) usam, em vez de tentarem `use` de `apps/desktop/src-tauri`
  (que é binário e não é importável por testes externos).
- **Puro** (`unsafe_code = "forbid"`, sem `tauri`, sem `windows`):
  passa em `scripts/check-core-purity.ps1` automaticamente. Esta
  pureza é o que permite reaproveitar `frederico_app::build_chat_orchestrator`
  no modo servidor do §5.5 (VPS / headless) sem fork — o
  `build_chat_orchestrator` recebe as mesmas dependências
  injetadas, e roda em qualquer runtime `tokio` que tenha acesso
  ao DB e à rede. Manter essa pureza é regra do projeto, não
  acidente: se alguém "simplificar" puxando `tauri` para dentro
  do `frederico-app`, o modo servidor perde esse caminho.

### D2. `JailResolver` trait + `FileSystemJailResolver` (default)

- Trait `JailResolver` no `frederico_app::jail`:

  ```rust
  pub trait JailResolver: Send + Sync {
      fn resolve(&self, conversation_id: &ConversationId) -> Jail;
  }
  ```

- `FileSystemJailResolver` é a única implementação da Etapa 1.
  Recebe `workspaces_root: PathBuf` no construtor (resolvido
  pela casca como `<data_local_dir>/workspaces/`).
  `resolve(cid)` faz `mkdir -p` idempotente em
  `workspaces_root.join(cid.to_string())` e devolve `Jail::new(path)`.
  **Falha de `mkdir` é erro duro**, propagado como
  `ToolError::PathSetup { conversation_id, source }` legível pro
  usuário (não warn + fallback pra `temp_dir`, que seria
  degradação silenciosa num caminho de isolamento — exatamente o
  que o `JailResolver` foi criado pra impedir).

- O resolvedor definitivo da Etapa 7
  (`SecurityJailResolver` via `frederico-security`, com Job
  Objects e AllowVolumeAccess) implementa o mesmo trait.
  A interface é estável: trocar a implementação não exige mudar
  `ChatOrchestrator`, `RunExecutor` nem `FilesReadTool`.

### D3. `ToolContext` carrega o `conversation_id` por tool_call

- Novo tipo em `frederico_tool_registry::tools`:

  ```rust
  #[derive(Debug, Clone)]
  #[non_exhaustive]
  pub struct ToolContext {
      pub conversation_id: ConversationId,
      pub run_id: RunId,
      pub message_id: MessageId,
  }
  ```

  `#[non_exhaustive]` (§5 do PROMPT MESTRE + precedente do
  `MemoryHit` no `frederico-memory`): acrescentar campo no
  contexto não é breaking change pra quem constrói o valor
  (não pode usar struct literal fora do crate), mas a Etapa 7
  pode adicionar `workspace: Option<WorkspaceSnapshot>` etc.
  sem nova quebra.

- `Tool::execute` muda de assinatura:

  ```rust
  // antes (Fase 3, Etapa 4)
  async fn execute(&self, arguments: &serde_json::Value) -> ToolResult;

  // depois (Fase de Ligação, Etapa 1)
  async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult;
  ```

  Breaking change consciente: único `Tool` concreto hoje é o
  `FilesReadTool`, e o `RunExecutor` é o único caller
  (além dos testes do crate). Registrada no `CHANGELOG.md` da
  Etapa 1 com a seção "Alterado — breaking".

- O `RunExecutor` resolve `conversation_id` **uma vez por run**
  (na construção do executor, lendo do `Run` carregado), não por
  `tool_call`. O `conversation_id` é imutável durante o run (a
  conversa não muda de ID no meio da execução), então carregá-lo
  uma vez elimina query por chamada e ponto de falha em caminho
  quente. O `ToolContext` é construído uma vez por tool_call com
  os IDs em mão, custo O(1) sem I/O.

### D4. Composição via `frederico_app`, casca consome

- A casca `apps/desktop/src-tauri/src/main.rs` substitui a
  montagem inline (provedores + runs + sink + db + clock + catalog
  + tool_registry + jail + tools + permission_set + memory_extractor)
  por uma única chamada
  `frederico_app::build_chat_orchestrator(parts)`. As funções
  `build_tool_registry(tools)`, `initial_permission_set()`,
  `FileSystemJailResolver::new(workspaces_root)` moram no
  `frederico-app` e são as **mesmas** que os E2E da Etapa 5 vão
  chamar — `tests/e2e/` na raiz importa de `frederico_app`, não
  do `src-tauri` (que é binário).

- `ChatOrchestrator` ganha dois campos novos:
  - `permissions: PermissionSet` (substitui o `default()` no
    `tokio::spawn` do `send_message`).
  - `jail_resolver: Arc<dyn JailResolver>` (substitui `Jail`).
  - Mantém a decisão aprovada na conversa: campo na struct, não
    parâmetro de `new()` (que já tem 11 args).

- `initial_permission_set()` da Etapa 1 retorna o **mínimo
  necessário** para a Etapa 1 passar: `file_read: WorkspaceOnly`,
  todo o resto deny (incluindo `documents: None`). `documents` é
  bumpado pra `DocumentPermission::Full` **junto com o registro
  de `docs.generate`/`docs.inspect` na Etapa 2**, no mesmo commit
  (regra do §1.3 + precedente do ADR-0020 §3 D3: bump atômico
  de enum + capability).

## Alternativas descartadas

### A. Manter `Jail` único e resolver path no `Tool::execute` via `arguments["conversation_id"]`

Rejeitada. O `conversation_id` viraria parte do contrato da
chamada de ferramenta (vazamento de abstração), qualquer tool
passaria a poder lê-lo dos argumentos e o `validate_tool_call`
teria que tratá-lo como campo reservado. Pior: dois tools
diferentes que recebem o mesmo `arguments` (em cópias distintas
do `serde_json::Value`) podem divergir sobre o que fazer com
esse campo. O `ToolContext` resolve isso sem contaminar o schema.

### B. `HashMap<ConversationId, Jail>` cacheado no `AppState`

Rejeitada. Cresce com o uso; precisa de invalidação quando a
conversa é deletada (e o `frederico-storage` expõe
`ConversationRepo::delete` mas a casca não tem o hook
atualmente). É o mesmo problema de cache de path sem o ganho:
a `Jail::new` é barata e o `mkdir -p` é idempotente.

### C. Deixar o jail raiz em `workspaces/` sem subdiretório por conversa

Rejeitada. **Vazamento entre conversas** — exatamente a classe
de ataque que a I4 trata na memória. "Jail é barreira, não
diretório específico" é o argumento que transforma isolamento
em nada. A I3 (path traversal via `..`) está coberta pelo
`Jail` em si, mas I4 não: o jail raiz em `workspaces/` permite
`files.read {"path": "b/secret.txt"}` da conversa A ler arquivos
da conversa B, que é o oposto do que o usuário espera quando
trata "conversa" como escopo.

### D. Pular o `JailResolver` e usar o `frederico-security` direto na Etapa 1

Rejeitada. O `frederico-security` ainda não tem o resolvedor
definitivo (Job Objects + AllowVolumeAccess); ele está
planejado pra Etapa 7 (modo desenvolvedor). Pular o trait e
implementar direto no `frederico-security` agora criaria
acoplamento da casca com `frederico-security` antes da
Etapa 7, o que obrigaria a refatorar a casca duas vezes.

### E. Deixar `std::env::current_dir()` enquanto a Etapa 7 não chega

Rejeitada. A Etapa 1 da Fase de Ligação existe exatamente
porque as Fases 3, 4 e 5 ficaram com a fiação desligada
"temporariamente" e três releases fecharam assim. Registrar
essa dívida como "pendência pra Etapa 7" sem corrigir agora
é o mesmo erro, em escala menor.

## Consequências

**Mais fácil:**

- Jail por conversa real, sem vazamento entre escopos. A
  mesma garantia que a memória tem (I4) passa a existir no
  filesystem (I3 + I4 combinados no caminho de tool).
- Camada de composição pura (`frederico-app`), reaproveitável
  no modo servidor §5.5 sem fork.
- `build_tool_registry(tools)` itera sobre `tool.manifest()`,
  então **impossível ter tool sem manifesto ou manifesto sem
  tool** — a divergência de inventário do §5.2 está fechada
  mecanicamente.
- `ToolContext #[non_exhaustive]` significa que a Etapa 7 (e a
  Etapa 4 — `frederico-agent-engine` resolve `RunState`)
  acrescentam campos sem nova quebra.

**Mais difícil:**

- Breaking change em `Tool::execute` (passou de 1 arg pra 2).
  Registrada no `CHANGELOG.md` da Etapa 1 com a seção
  "Alterado — breaking". Único `Tool` concreto hoje (`FilesReadTool`).
- `RunExecutor` ganha um argumento novo no construtor
  (o `Arc<dyn JailResolver>`) e uma query de `Run` por run
  (não por tool_call) pra carregar o `conversation_id`.
- `ChatOrchestrator` cresce dois campos (`permissions`,
  `jail_resolver`). Mitigado: campo na struct (não em `new`),
  e a composição mora em `frederico_app::build_chat_orchestrator`.

**Pendência registrada:**

- A Etapa 7 (modo desenvolvedor) substitui o
  `FileSystemJailResolver` por `SecurityJailResolver` via
  `frederico-security` (Job Objects + AllowVolumeAccess).
  A interface (`JailResolver` trait) é estável — a troca
  não exige mudar `ChatOrchestrator`, `RunExecutor` nem
  `FilesReadTool`.
