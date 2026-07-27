<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 2
-->

> Última verificação: 2026-07-27. Reflete as Etapas 1, 2, Leva 3, Etapa 5 (UI), Hardening 1 (DPAPI real), Hardening 3 (`provider-recorder` + sanitização), Hardening 4 (contract tests off-PR) e Hardening 5 (recovery E2E) da Fase 2 — **Fase 2 concluída**.
>
> **Etapa 1** entregou o esqueleto: trait `ProviderAdapter`, enum
> `StreamEvent`, parser de SSE, fakes (transport-level + trait-level),
> trait `CredentialStore` com `FakeCredentialStore`, contratos IPC
> `ProviderList`/`ProviderSetCredential`/`ProviderDeleteCredential`,
> e o lint estendido (regra do ADR-0007).
>
> **Etapa 2** entregou o catálogo embutido (`frederico-model-catalog`),
> os adapters reais (`OpenAiCompatAdapter` + `AnthropicAdapter`), e
> a migração `0002_chat_core.sql` com 5 tabelas e 5 repositórios.
>
> **Leva 3 (esta atualização)** entregou o `ChatOrchestrator` — a
> cola entre adapter, catálogo, storage, RunRegistry e EventSink.
> Pontos principais:
> - `EventSink` trait + `NoopEventSink` / `RecordingEventSink`
>   (em memória) / `TauriEventSink` (na casca).
> - `RunRegistry` — mapping `RunId` → `CancellationToken`.
> - `ProviderMap` — `ProviderId` → `Arc<dyn ProviderAdapter>`.
> - `ChatOrchestrator::send_message` — persiste user msg primeiro
>   (regra de teste), cria assistant msg e run, dispara o loop de
>   stream em background. Retorna `(user_msg, run_id)`.
> - Loop de stream com `tokio::select!` entre o próximo evento do
>   adapter, `cancel.cancelled()`, e `tokio::time::sleep(60s)`
>   (watchdog). Cada evento é **persistido no journal antes** de
>   ser emitido (regra do §"Journal de eventos").
> - `ChatOrchestrator::cancel_run` + `get_events` (reload de janela).
> - `error_to_view` — mapeamento `ProviderError` → `ProviderErrorView`
>   (PT-BR com ação), tabela do spec §"Política de erro de provedor".
> - Casca Tauri atualizada: constrói o `ChatOrchestrator` com
>   adapters reais, `TauriEventSink` ligado ao AppHandle, e
>   `ipc_dispatch` cobre todas as 18 ops do contrato IPC
>   (`ProviderList`/`Set`/`Delete`, `ModelCatalogList`/`ForProvider`,
>   `ConversationCreate`/`List`/`Get`/`Rename`/`SetModel`/`Delete`,
>   `MessageSend`, `RunGetEvents`, `RunCancel`).
>
> **Etapa 5 (UI)** entregou a casca React. Camada `services/` é a
> única que faz `invoke` no Tauri e `listen` em eventos. Componentes
> React nunca importam `@tauri-apps/api` diretamente. Rotas:
> - `/chat` (com `:id` opcional) — lista de conversas, header com
>   troca de modelo, mensagens com streaming otimista
>   (`accumRef` + cursor `streamingMessageId`), composer
>   (Enter envia / Shift+Enter quebra linha), botão Parar
>   (`RunCancel`), recarregar-janela-meio-stream via `RunGetEvents`
>   pra cada mensagem com `status=streaming` no mount.
> - `/settings` — tabela de provedores conhecidos do catálogo
>   (fonte da verdade), cadastro/edição/remoção de credencial.
> - `/sobre` — atualizado pra Fase 2.
>
> **Cobertos** na Etapa 5 + Hardening 1, 3, 4, 5 (Fase 2
> **concluída**): casca UI em React com `services/` como única
> camada de invoke/listen, rotas `/chat` e `/settings` com
> streaming otimista + reload mid-stream + cancelamento + erros
> PT-BR com ação; **o impl DPAPI real do `WindowsCredentialStore`**
> via `CredWriteW`/`CredReadW`/`CredDeleteW`/`CredEnumerateW`
> (crate `windows` v0.58, `TargetName`
> `Frederico-IA-Studio:provider:<id>`, `CRED_PERSIST_LOCAL_MACHINE`,
> mapeamento HRESULT→win32 com `& 0xFFFF`); o **`provider-recorder`**
> binário que grava fixture com header obrigatório + sanitização
> regex; os **contract tests off-PR** que pulam limpo sem env var;
> e os **recovery tests E2E** que validam a regra do "Journal de
> eventos" — o SQLite é a fonte de verdade e sobrevive ao drop
> do orquestrador. Cobertura: 6 integration tests + 3 unit tests
> em `crates/security/tests/windows_credential_store.rs` e
> `crates/security/src/windows.rs` (DPAPI); 14 unit tests em
> `frederico_provider_engine::sanitize`; 2 integration tests em
> `crates/provider-engine/tests/fixtures_sanitize.rs` (gate de
> CI); 3 contract tests (gateados em env var); 3 recovery tests
> (journal atravessa drop do orquestrador, status consistente
> pós-recovery, user msg antes do I/O).
>
> Cobertura total: **133 testes verdes** no workspace.

# Chat e provedores

Este spec descreve o desenho do motor de chat da Fase 2: adaptadores de provedor, catálogo de modelos, credenciais, conversas, streaming, custo, cancelamento e o contrato com a casca Tauri. Ele é a fonte de verdade do "como" da Fase 2; o [`docs/status.md`](../status.md) é a fonte do "quando".

A Fase 1 fechou com a casca Tauri + IPC + SQLite inicial + casca. Esta fase constrói em cima dessa fundação sem violar as invariantes do [`software-architecture.md`](./software-architecture.md), do [`process-architecture.md`](./process-architecture.md) e do [`security-threat-model.md`](./security-threat-model.md).

## Decisões estruturais vigentes

Esta fase é sustentada por quatro ADRs novos. Toda referência a "ADR" sem número é a um deles.

- [ADR-0005](../decisions/0005-provider-engine-crate.md) — crate `provider-engine`, trait `ProviderAdapter`, 3 formatos (OpenAI-compat, Anthropic, simulated), Gemini fora.
- [ADR-0006](../decisions/0006-model-catalog-crate.md) — crate `model-catalog`, catálogo embutido via `build.rs`.
- [ADR-0007](../decisions/0007-credential-store-trait.md) — `CredentialStore` em `frederico-security` com DPAPI, regra "nunca shim de texto puro".
- [ADR-0008](../decisions/0008-fake-provider-strategy.md) — provedor simulado dual (golden files + gerador), no nível do transporte.

## Modelo de domínio

Identificadores opacos em `frederico-core` (regras do [`software-architecture.md`](./software-architecture.md) §"Contratos"). Novos na Fase 2: `MessageId`, `AssistantId` é o mesmo que `MessageId` quando se trata da resposta do modelo, `RunId` (já existe), `ProviderId` (já existe), `ModelId` (já existe), `EventId` (novo, sequencial por mensagem — ver §"Journal de eventos").

### Conversation

```rust
struct Conversation {
    id: ConversationId,
    title: Option<String>,           // editável pelo usuário
    provider_id: ProviderId,         // provedor ativo
    model_id: ModelId,              // modelo ativo (dentro do provider)
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    total_cost_microcents: u64,     // acumulado
}
```

Modelo e provedor são **por conversa** (escolha do usuário ao criar, mutável depois). O motor de execução da Fase 3 lê daqui.

### Message

```rust
struct Message {
    id: MessageId,
    conversation_id: ConversationId,
    role: Role,                     // User | Assistant | System
    content: String,                // conteúdo final (assistant: texto montado)
    status: MessageStatus,          // ver abaixo
    run_id: Option<RunId>,          // presente só em Assistant com run
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    cost_microcents: u64,           // 0 para User/System
    error: Option<ProviderErrorView>, // PT-BR com ação sugerida
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

enum Role { User, Assistant, System }
enum MessageStatus {
    Pending,                        // criada, run ainda não disparou
    Streaming,                      // run ativo, recebendo eventos
    Completed,                      // run terminou com Done
    Failed,                         // run terminou com Error
    Cancelled,                      // usuário pediu stop
    Timeout,                        // watchdog 60s sem evento
}
```

Append-only: mensagens nunca são editadas. Correções do usuário viram mensagens novas (Fase 4, "Memória e continuidade").

### Run (esqueleto — máquina de estados completa é Fase 3)

```rust
struct Run {
    id: RunId,
    conversation_id: ConversationId,
    message_id: MessageId,          // 1:1 com a Message Assistant
    status: RunStatus,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    cancellation_requested_at: Option<DateTime<Utc>>,
}

enum RunStatus {
    Created,                        // mensagem do usuário persistida, request ainda não saiu
    Running,                        // request em voo
    Completed,
    Failed,
    Cancelled,
    Timeout,
}
```

A Fase 2 implementa a transição `Created → Running → {Completed, Failed, Cancelled, Timeout}` e a persistência. A Fase 3 adiciona `Paused`, `AwaitingApproval`, `Recovering` e a lógica de checkpoints do [`agent-state-machine.md`](./agent-state-machine.md).

### ProviderConfig

```rust
struct ProviderConfig {
    provider_id: ProviderId,
    display_name: String,           // "OpenAI", "OpenRouter", "Anthropic", ...
    configured: bool,               // existe credencial cadastrada
    last_ok_at: Option<DateTime<Utc>>,
    last_error_at: Option<DateTime<Utc>>,
    last_error: Option<String>,     // PT-BR, sem a chave
}
```

A UI nunca vê a chave em si — só `configured: bool` e o status. Isso é o cumprimento da regra do ADR-0007.

### ModelDescriptor (referência ao ADR-0006)

Definido no `crates/model-catalog/`. A UI consome via `model_catalog::list_for_provider(provider)`. Resumo:

```rust
struct ModelDescriptor {
    provider: ProviderId,
    model: ModelId,
    display_name: String,
    context_window: u32,
    modalities: ModalitySet,
    capabilities: CapabilitySet,
    pricing_per_million: PriceTable,    // input, output em microcents
    provider_metadata: serde_json::Value,
}
```

## Contrato do `provider-engine`

### `ProviderAdapter` (referência ao ADR-0005)

```rust
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> AdapterCapabilities;
    fn cost_model(&self) -> CostModel;

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError>;
    fn stream(&self, request: ChatRequest) -> Result<BoxStream<'static, StreamEvent>, ProviderError>;
    fn cancel(&self, handle: RunHandle) -> Result<(), ProviderError>;
}
```

- `ChatRequest` carrega `model: ModelId`, `messages: Vec<ChatMessage>`, `tools: Vec<ToolDescriptor>` (estrutura, sem execução), `temperature`, `max_tokens` e `cancel: CancellationToken` (`tokio_util::sync::CancellationToken`).
- `StreamEvent` é enum fechado:

  ```rust
  enum StreamEvent {
      Delta { content: String },
      Usage { prompt_tokens: u32, completion_tokens: u32 },
      ToolCall { id: String, name: String, arguments_json: String },
      Done { stop_reason: StopReason },    // Stop | Length | ToolCalls | Error
      Error(ProviderError),
      Cancelled,
  }
  ```

  A enumeração é fechada por design — fronteira do trait. Adicionar provedor novo que emita um evento novo exige variante nova do enum (mudança explícita, revisável).

- `ProviderError` carrega estrutura, não string:

  ```rust
  struct ProviderError {
      kind: ProviderErrorKind,
      upstream_status: Option<u16>,
      upstream_message: Option<String>,    // mensagem do provedor, sem a chave
      retry_after: Option<Duration>,
  }

  enum ProviderErrorKind {
      Auth,                                // 401 — chave inválida/ausente
      Payment,                             // 402 — sem crédito
      Forbidden,                           // 403 — chave sem acesso ao modelo
      NotFound,                            // 404 — modelo inexistente
      RateLimited,                         // 429 — com retry_after
      Server,                              // 5xx — instabilidade do provedor
      Network,                             // falha de transporte
      Cancelled,
      Timeout,                             // watchdog
      Unknown,
  }
  ```

  O `apps/desktop` traduz `ProviderError` para PT-BR com ação sugerida (Etapa 5). O núcleo nunca formata string para o usuário final.

### Mapeamento HTTP → `ProviderErrorKind`

O mapping é responsabilidade de cada adapter (não do orquestrador), porque o vocabulário HTTP varia entre provedores:

| Status | OpenAI-compat | Anthropic | Gemini (futuro) |
|---|---|---|---|
| 401 | `Auth` | `Auth` | `Auth` |
| 402 | `Payment` | n/a | n/a |
| 403 | `Forbidden` | `Forbidden` | `Forbidden` |
| 404 | `NotFound` (modelo) | `NotFound` (modelo) | `NotFound` |
| 429 | `RateLimited` (com `retry_after`) | `RateLimited` (com `retry_after`) | `RateLimited` |
| 5xx | `Server` | `Server` | `Server` |
| sem resposta | `Network` | `Network` | `Network` |

A Etapa 5 da Fase 2 fecha o `error_view.rs` no `apps/desktop` com as mensagens PT-BR e ações. As ações são coisas como: "Cadastre uma chave em Configurações → Provedores", "Sem crédito, veja o painel do provedor", "Modelo indisponível, escolha outro na lista", etc.

### Cancelamento

- O `ChatRequest` carrega um `CancellationToken` (de `tokio_util::sync::CancellationToken`).
- O adapter monitora o token; quando disparado, interrompe a leitura do `reqwest` response stream e emite `StreamEvent::Cancelled`.
- O `provider-engine` expõe um `RunRegistry` que mantém o mapping `RunId → CancellationToken` para o command da casca.
- Watchdog de 60s sem evento: o orquestrador consulta o `Clock` injetado; se passaram 60s desde o último `StreamEvent`, dispara o `CancellationToken` e marca a mensagem como `Timeout`. Sem `Clock` real — o tempo é virtual em testes (ADR-0008 §3.2).

## Catálogo de modelos (referência ao ADR-0006)

- Arquivo versionado em `crates/model-catalog/data/catalog.json`.
- Schema em `crates/model-catalog/data/schema.json` (JSON-Schema draft 2020-12).
- `build.rs` valida o JSON, escreve cópia em `OUT_DIR/catalog.json`, computa `catalog_hash` (BLAKE3) e expõe via `env!`. Runtime usa `include_str!`.
- API: `find_model`, `list_for_provider`, `list_all`, `pricing_for`. `ModelDescriptor` é o tipo público.
- v1 cobre: OpenAI (gpt-4o, gpt-4o-mini), Anthropic (claude-3-5-sonnet, claude-3-5-haiku), OpenRouter (subset representativo), DeepSeek (deepseek-chat), Mistral (mistral-large), NVIDIA NIM (modelo referência), Ollama/LM Studio (placeholders), `simulated/fake-model-v1`. Lista no ADR-0006 §Decisão.
- Erro explícito: `pricing_for` retorna `None` para modelo sem preço. O `provider-engine` rejeita a request com `ProviderError::Unknown("modelo sem preço cadastrado")` em vez de estimar.

## Credenciais (referência ao ADR-0007)

- `CredentialStore` no trait `Platform` (em `frederico-security`): `get`, `set`, `delete`, `list_providers`. `SecretString` do crate `secrecy` (erro de compilação sem `expose_secret`).
- Windows: `CredWriteW`/`CredReadW` via `windows-rs`, gateado `#[cfg(windows)]`.
- Testes: `FakeCredentialStore` (HashMap em memória, atrás de `Arc<Mutex<...>>`).
- **Regra verificável "nunca shim de texto puro"** (ADR-0007 §Decisão):
  1. `provider-engine` não importa `std::env::var`, `dotenv` nem `dotenvy`. Lint em `scripts/check-core-purity.ps1`.
  2. `provider-engine` não parseia config de provedor de arquivo. Config de provedor vem do `CredentialStore`.
  3. Teste de contrato que monkey-patcha env, roda um ciclo completo com `FakeCredentialStore` e verifica que (a) chave veio do trait, (b) env do adapter não contém a chave.
  4. Subscriber de `tracing` que filtra valores que casam com prefixos conhecidos (`sk-`, `sk-ant-`, `gsk_`, `or-`) e falha o teste se casar.
- Fronteira com UI: `AppOp::ProviderStatus { provider }` devolve `{ configured, last_ok, last_error }`. UI nunca vê a chave.

## Journal de eventos de mensagem (a peça de "recarregar a janela no meio do stream")

> Esta seção existe porque "se você deixa para depois, descobre tarde que o contrato de eventos está errado" (feedback do usuário). A Fase 2 fecha o contrato desde o primeiro commit; a Etapa 5 da casca é que exerce o cenário.

Cada `Message` do tipo Assistant tem um **journal de eventos** persistido em SQLite (`message_events`), com sequência monotônica por mensagem. Eventos chegam do adapter, são **persistidos antes** de serem emitidos para a janela, e ficam disponíveis para a UI em qualquer momento.

```rust
struct MessageEvent {
    id: i64,                        // auto-increment
    message_id: MessageId,
    seq: u32,                       // monotônico por message_id
    kind: StreamEventKind,          // delta | usage | tool_call | done | error | cancelled
    data: serde_json::Value,        // payload do evento
    created_at: DateTime<Utc>,
}
```

### Contrato de persistência + emissão

Para cada `StreamEvent` do adapter:

1. **Persistir** em `message_events` dentro de uma transação SQLite curta. Se a persistência falhar, o run é abortado (`ProviderError` com `kind: Network` mapeado para erro de storage).
2. **Emitir** `tauri::Window::emit("run://<run_id>/event", payload)` para a janela aberta. Se a janela estiver fechada, `emit` falha silenciosamente — o evento já está persistido.
3. **Acumular** no buffer in-memory do `Message.content` (apenas para `Delta`).

A ordem importa: persistência antes de emissão garante que a janela possa ser recarregada sem perder eventos.

### Recarga de janela no meio do stream

Cenário: o usuário está no meio de uma resposta, o app trava (ou ele dá reload manual), a janela reabre.

1. UI chama `Conversation.Get { conversation_id }` → recebe lista de `Message` em ordem.
2. Para a `Message` com `status: Streaming`, UI chama `Run.GetEvents { message_id, since_seq: 0 }` → recebe todos os eventos do journal em ordem.
3. UI re-renderiza o conteúdo a partir dos eventos (deltas acumulados, usage final, status atual).
4. UI chama `Run.Subscribe { run_id }` (subscrição Tauri de eventos) para continuar recebendo os próximos.
5. Se o run termina enquanto a janela estava morta, o `Message.status` vira `Completed` (ou `Failed`/`Cancelled`/`Timeout`) e a UI, ao re-renderizar, vê o estado final.

O ponto crítico: a janela é **consumidora** do journal, não produtora. O journal é a fonte de verdade. A janela pode morrer e voltar quantas vezes quiser — o conteúdo é reconstruído a partir do que está em SQLite.

### Reconexão (Etapa 5)

A UI implementa reconexão automática: ao montar a tela, verifica se há `Message` com `status: Streaming`. Se houver, lê o journal e subscreve. Se houver falha de subscrição (Tauri event channel fechado), re-tenta com backoff de 100ms, 250ms, 500ms (cap em 2s). O tempo total de "ficar sem atualização" enquanto a janela está aberta é < 2s em qualquer cenário coberto pelos testes.

## Schema SQLite — migração `0002_chat_core.sql`

```sql
CREATE TABLE conversations (
  id TEXT PRIMARY KEY,
  title TEXT,
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  total_cost_microcents INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_conversations_updated_at ON conversations(updated_at DESC);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
  content TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL CHECK(status IN ('pending', 'streaming', 'completed', 'failed', 'cancelled', 'timeout')),
  run_id TEXT,
  prompt_tokens INTEGER,
  completion_tokens INTEGER,
  cost_microcents INTEGER NOT NULL DEFAULT 0,
  error TEXT,                            -- JSON do ProviderErrorView
  created_at TEXT NOT NULL,
  finished_at TEXT
);

CREATE INDEX idx_messages_conversation_created
  ON messages(conversation_id, created_at);

CREATE TABLE message_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(message_id, seq)
);

CREATE INDEX idx_message_events_message_seq
  ON message_events(message_id, seq);

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK(status IN ('created', 'running', 'completed', 'failed', 'cancelled', 'timeout')),
  started_at TEXT NOT NULL,
  finished_at TEXT,
  cancellation_requested_at TEXT,
  UNIQUE(message_id)
);

CREATE TABLE provider_configs (
  provider_id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  configured INTEGER NOT NULL DEFAULT 0,
  last_ok_at TEXT,
  last_error_at TEXT,
  last_error TEXT
);
```

A migração roda junto com a `0001_initial.sql` na abertura do banco. O `frederico-storage` adiciona `ConversationRepository`, `MessageRepository`, `MessageEventRepository`, `RunRepository`, `ProviderConfigRepository`. Cada um é append-only onde aplicável (mensagens e eventos).

## Operações de IPC

Novas variantes de `AppOp` em `packages/shared-contracts/`. Os payloads são versionados por JSON-Schema (a definir no spec `shared-contracts` da Fase 2, ou inline aqui se ficar pequeno).

### Configuração de provedores

- `Provider.List` → `Vec<ProviderConfig>` (configurados ou não).
- `Provider.SetCredential { provider, value: SecretString }` → `Result<()>`. UI envia só no momento do cadastro.
- `Provider.DeleteCredential { provider }` → `Result<()>`. Revoga.
- `Provider.TestConnection { provider, model }` → `Result<{ ok: bool, latency_ms, model_available }>`. Roda uma request mínima (`max_tokens: 1`).
- `Provider.Status { provider }` → `Result<ProviderConfig>`. Opaque para a UI.

### Catálogo

- `ModelCatalog.List` → `Vec<ModelDescriptor>` (tudo do catálogo embutido).
- `ModelCatalog.ForProvider { provider }` → `Vec<ModelDescriptor>` (filtrado).

### Conversas

- `Conversation.Create { provider, model, title? }` → `Conversation`.
- `Conversation.List` → `Vec<Conversation>` (resumido: id, título, modelo, atualizado em, custo).
- `Conversation.Get { id }` → `Conversation` + `Vec<Message>` (com eventos do journal embutidos para mensagens em `streaming`).
- `Conversation.Delete { id }` → `Result<()>`.
- `Conversation.Rename { id, title }` → `Result<()>`.
- `Conversation.SetModel { id, provider, model }` → `Result<()>`.

### Execução de mensagem (chat)

- `Message.Send { conversation_id, content }` → `Message` (User criada, status `pending`). Dispara o run em background e retorna imediatamente. O frontend subscreve via Tauri events.
- `Run.Subscribe { run_id }` → ativa o canal de eventos para esse run. Já é idempotente; subscribe duplo é no-op.
- `Run.Pause { run_id }` → Fase 3. Na Fase 2 responde `Err("não implementado")`.
- `Run.Resume { run_id }` → Fase 3. Na Fase 2 responde `Err("não implementado")`.
- `Run.Cancel { run_id }` → `Result<()>`. Dispara o `CancellationToken` do adapter.
- `Run.GetEvents { message_id, since_seq }` → `Vec<MessageEvent>`. Usado na recarga de janela.

### Eventos Tauri (streaming)

- `run://<run_id>/event` → payload `{ seq, kind, data }`. Disparado para cada `StreamEvent` do adapter, após a persistência.
- `run://<run_id>/status` → payload `{ status, finished_at? }`. Disparado na transição final do run.

## Política de custo

- `PriceTable` em microcents: `u64`. Sem ponto flutuante no banco.
- Cada `StreamEvent::Usage` carrega `prompt_tokens` e `completion_tokens`. O orquestrador multiplica pelo `PriceTable` do modelo ativo e atualiza (a) o `cost_microcents` da `Message`, (b) o `total_cost_microcents` da `Conversation`.
- Modelo sem preço cadastrado → o orquestrador aborta o run antes de qualquer I/O de rede, com `ProviderError::Unknown("modelo sem preço cadastrado")` mapeado para `ErrorView { code: "model_no_price", action: "Adicione o preço em docs/architecture/.../chat-and-providers.md#catálogo e regere o binário" }`. Erro explícito, não estimativa silenciosa.
- Exibição: UI formata `cost_microcents` em moeda humana (BRL por padrão) usando o câmbio do `clock` da última atualização (a taxa fica como TODO da Fase 8 — performance/copiloto).

## Política de erro de provedor (PT-BR com ação sugerida)

Tabela mínima de mapping na Etapa 5 da Fase 2. Cada erro gera um `ProviderErrorView`:

```rust
struct ProviderErrorView {
    code: &'static str,              // "auth_invalid", "no_credit", etc.
    title: String,                   // PT-BR, curto
    detail: String,                  // PT-BR, 1-2 frases
    action: Option<String>,          // PT-BR, frase de ação
    retry_after: Option<Duration>,
}
```

Exemplos:

| `kind` | `code` | `title` | `action` |
|---|---|---|---|
| `Auth` | `auth_invalid` | "Chave de API inválida" | "Abra Configurações → Provedores, confira a chave e salve de novo." |
| `Payment` | `no_credit` | "Sem crédito no provedor" | "Veja o painel de billing do provedor para adicionar saldo." |
| `Forbidden` | `forbidden` | "Sem acesso a este modelo" | "Sua chave não tem acesso ao modelo {model}. Escolha outro ou peça acesso ao provedor." |
| `NotFound` | `model_not_found` | "Modelo não encontrado" | "O modelo {model} não está disponível. Escolha outro na lista." |
| `RateLimited` | `rate_limited` | "Limite de requisições atingido" | "Aguarde {retry_after} e tente de novo." (sem `action` se for 0) |
| `Server` | `provider_error` | "Provedor instável" | "Tente de novo em alguns instantes." |
| `Network` | `network_error` | "Falha de rede" | "Confira sua conexão e tente de novo." |
| `Cancelled` | `cancelled` | "Interrompido" | (nenhuma) |
| `Timeout` | `timeout` | "Provedor sem resposta" | "O provedor não respondeu em 60s. Tente de novo ou troque de modelo." |

A `ErrorView` é persistida em `messages.error` como JSON. A UI lê e exibe. Reabrir a conversa depois de um erro mostra o erro com a ação.

## Política de testes

Aplicam-se as cinco camadas do [`testing-strategy.md`](./testing-strategy.md). Para a Fase 2:

- **Unit**: traits em isolamento; `Clock` injetado; `StreamEvent` parsing (incluindo fixture de UTF-8 partido e SSE keepalive); helpers de `from_wide`/`to_wide`/`creds_to_void` em `frederico-security/src/windows.rs`; `frederico_provider_engine::sanitize` (14 testes cobrindo cada token proibido: `Authorization`, `api_key`, `x-api-key`, `Bearer `, `sk-`, `sk-ant-`, `gsk_`, `or-`).
- **Integration**: ciclo completo de uma conversa com `FakeProviderAdapter` (golden file) — `Message.Send` → eventos → `Run.Completed`. Sem janela.
- **Integration (Windows DPAPI)**: `frederico-security/tests/windows_credential_store.rs` (gateado em `#[cfg(windows)]`) fala com o **Credential Manager de verdade**: set/get roundtrip, get de inexistente→None, delete idempotente, list filtra prefixo Frederico (`Frederico-IA-Studio:provider:`), list filtra prefixo + valida que credencial semeada de outro app **não vaza** (semeada via `CredWriteW` direto), overwrite. Mutex global de serialização + IDs únicos por run garantem cleanup e ausência de colisão.
- **Integration (gate de CI de fixtures)**: `frederico-provider-engine/tests/fixtures_sanitize.rs` varre `fixtures/**/*.jsonl` no build e quebra o build se (a) algum arquivo estiver contaminado por token proibido, ou (b) algum arquivo estiver sem header reconhecido (`# …` do recorder novo ou `{"_fixture_header":…}` legado). Complementa o `provider-recorder` que já sanitiza **antes de gravar**.
- **Contrato (off-PR)**: `frederico-provider-engine/tests/openai_compat_contract.rs` e `tests/anthropic_contract.rs`. Gateados em `OPENAI_API_KEY`/`OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`: sem env var, pulam limpo (exit 0). Com env var, fazem a chamada real via adapter, drenam o `BoxStream` até `Done`, validam ≥1 `Delta` + 1 `Done` com `StopReason::Stop` ou `Length`. Runner: `scripts/run-contract-tests.ps1`. Rodam no CI noturno (cron) e manualmente por devs.
- **Recovery E2E**: `frederico-provider-engine/tests/recovery.rs`. Valida a regra do "Journal de eventos" — o SQLite é a fonte de verdade, sobrevive ao drop do `ChatOrchestrator`. `journal_persists_events_across_orchestrator_drop` dispara run, espera completar, dropa A, reabre com B, valida `MessageEventRepo::list_for_message` retorna os mesmos eventos. `cancel_idempotent_and_status_persists` valida que `cancel_run` é idempotente e o status final bate entre sink e db pós-recovery. `user_message_persisted_before_run_starts` valida a regra de teste: `Message.role=user` no db imediatamente após `send_message` retornar.
- **E2E (Etapa 5)**: recarga de janela no meio de um stream; erro de provedor exibido em PT-BR com ação; cadastro de credencial falhando com chave inválida (sem vazar em log).

## Invariantes

Verificáveis por teste (cada uma tem pelo menos um teste na camada apropriada).

1. `provider-engine` **não importa** `std::env::var`, `dotenv`/`dotenvy`, nem parseia config de provedor de arquivo. (Lint + teste.)
2. A `Message` do usuário é persistida em `created` **antes** de qualquer I/O de rede (regra do `testing-strategy.md` §"Exemplos"). (Teste de integração que mata o processo após `Message.Send` e encontra a mensagem no banco.)
3. Cada `StreamEvent` do adapter é **persistido** em `message_events` **antes** de ser emitido via Tauri. (Teste de integração com adapter que emite 100 deltas; após o run, todos os 100 estão no journal.)
4. O `ProviderAdapter` nunca recebe uma chave de API em texto puro — sempre via `CredentialStore` injetado. (Teste que falha se o construtor do adapter aceitar `String`/(`&str`) com a chave.)
5. Recarga de janela no meio de um run preserva o conteúdo: ao re-renderizar, a soma dos `Delta` recebidos via journal é igual à soma esperada. (E2E.)
6. Cancelamento de run derruba a request `reqwest` em ≤ 1s (tempo virtual) e o `Message.status` vira `cancelled`. (Teste de integração com `ScriptedProviderAdapter` em `StallAt`.)
7. Watchdog de 60s dispara `StreamEvent::Error(Timeout)` se nenhum evento chegar nesse intervalo. (Teste com `StallAt(60s)` e `Clock::advance(60s)`.)
8. Modelo sem preço cadastrado é rejeitado antes de qualquer I/O de rede. (Teste de integração.)
9. Catálogo embutido falha o build se o JSON não bate com o schema. (`build.rs`.)
10. Cada fixture em `fixtures/` tem o cabeçalho obrigatório e não contém chave (regex do ADR-0008). (CI — `tests/fixtures_sanitize.rs` + `provider-recorder::sanitize::check`.)
11. A UI nunca vê a chave — só `configured: bool` e status. (Inspeção do schema TS gerado.)
12. `WindowsCredentialStore` cobre set/get/delete roundtrip contra Credential Manager real, delete idempotente, e o filtro de prefixo não vaza credenciais de outros apps. (Integration tests em `crates/security/tests/windows_credential_store.rs`.)
13. O `provider-recorder` aborta com `exit 1` e deleta o arquivo se algum token proibido aparecer no stream antes do flush. (Comportamento do binário + tests em `frederico_provider_engine::sanitize`.)
14. Os contract tests off-PR pulam limpo sem env var (exit 0) e validam ≥1 `Delta` + 1 `Done` com `StopReason` válido quando rodam contra a API real. (Tests em `crates/provider-engine/tests/openai_compat_contract.rs` e `tests/anthropic_contract.rs`.)
15. O journal de eventos (`message_events`) é a fonte de verdade e sobrevive ao drop do `ChatOrchestrator` — o db B lê o mesmo conteúdo que o A persistiu antes do drop. (Tests em `crates/provider-engine/tests/recovery.rs`.)
16. O status final do run (Completed/Cancelled/Failed/Timeout) bate entre o último `RunStatus` emitido pelo `EventSink` e o `Run.status` no db, e sobrevive ao drop. (Test `cancel_idempotent_and_status_persists`.)
17. A `Message.role=user` é persistida no db **antes** de qualquer I/O de rede — `send_message` retorna só depois que a user msg está no `messages`. (Test `user_message_persisted_before_run_starts`.)

## Não-objetivos

- Execução de `tool_call` (Fase 3 — Motor de execução e ferramentas). A enum `StreamEvent::ToolCall` é emitida e persistida no journal, mas a estrutura `ToolDescriptor` é só o esqueleto; nada é executado.
- Pausa e retomada de run (Fase 3). Os ops `Run.Pause`/`Run.Resume` existem e respondem `Err("não implementado")`.
- Memória e continuidade (Fase 4).
- Documentos (Fase 5).
- Subagentes, multimodelo (Fase 6).
- Projetos, GitHub, sandbox (Fase 7).
- Gemini (v1.1) — adapter, fixture e testes quando voltar.
- Multi-chave por provedor, organização de provedores em "favoritos", agrupamento por projeto — tudo isso vira discussão da Fase 4+.
- Exportar/importar conversas (LGPD). Está no roadmap da Fase 9; o `Conversation.Delete` é o mínimo da Fase 2.
- Cota de uso (mensal, diária) — Fase 4 ou 8.
- Internacionalização além de pt-BR. Estrutura de strings preparada, mas só pt-BR é ativado.

## Decisões

- [ADR-0005](../decisions/0005-provider-engine-crate.md)
- [ADR-0006](../decisions/0006-model-catalog-crate.md)
- [ADR-0007](../decisions/0007-credential-store-trait.md)
- [ADR-0008](../decisions/0008-fake-provider-strategy.md)

## Referências

- [`software-architecture.md`](./software-architecture.md) — layout, contratos, invariantes de pureza.
- [`process-architecture.md`](./process-architecture.md) — processo, IPC, watchdog.
- [`security-threat-model.md`](./security-threat-model.md) — I1 (env não herdado), T1 (credencial em log), credenciais no Windows Credential Manager.
- [`testing-strategy.md`](./testing-strategy.md) — camadas, máquina de referência.
- [`agent-state-machine.md`](./agent-state-machine.md) — máquina de estados completa (Fase 3).
- [`tool-registry-specification.md`](./tool-registry-specification.md) — esqueleto de `ToolDescriptor` (Fase 3).
