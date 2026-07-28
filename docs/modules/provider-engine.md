# `frederico-provider-engine`

## O que faz

Adapter de provedores de LLM (OpenAI-compat, Anthropic) e a infraestrutura de teste transport-level (golden files + scripted) e trait-level (FakeProviderAdapter). É a fronteira do motor de chat do Frederico IA Studio com o mundo externo (HTTP, SSE, credenciais).

## O que expõe

- `ProviderAdapter` — trait que todo provedor implementa.
- `StreamEvent` — enum fechado dos eventos que chegam durante o stream (`Delta`, `Usage`, `ToolCall`, `Done`, `Error`, `Cancelled`).
- `ProviderError` — erro estruturado com `ProviderErrorKind` (Auth, Payment, Forbidden, NotFound, RateLimited, Server, Network, Cancelled, Timeout, Unknown).
- `ChatRequest` / `ChatResponse` — entrada e saída do modo não-streaming.
- `parser::SseParser` / `sse_stream` — parser de SSE usado por todos os adapters e pelos fakes.
- `fake::transport` — fakes transport-level (golden file, scripted) — entregam bytes ao parser real.
- `fake::trait_level::FakeProviderAdapter` — fake que implementa `ProviderAdapter` diretamente, para testes rápidos acima do parser.

## De quem depende

- `frederico-core` — `ProviderId`, `ModelId`, `RunId`, etc.
- `frederico-security` — `Platform::credentials()` para ler chaves. **Nenhuma** leitura de env, dotenv, ou config em arquivo.
- `tokio`, `tokio-util` (CancellationToken), `tokio-stream`, `futures` (BoxStream).
- `reqwest` (rustls-tls) — HTTP client.
- `eventsource-stream` — parser SSE.
- `serde`, `serde_json`, `thiserror`, `tracing`, `bytes`, `http`.

## Quem depende dele

- (Fase 2 Etapa 2) o orquestrador de chat em `frederico-provider-engine::orchestrator` (a ser criado).
- (Fase 2 Etapa 5) a casca Tauri, via `packages/shared-contracts` para `ProviderList` e `ProviderSetCredential`.

## Decisões não óbvias e armadilhas conhecidas

- **Falsifique no nível do transporte, não do trait** ([ADR-0008](../decisions/0008-fake-provider-strategy.md) §3.1). O `FakeProviderAdapter` em `fake::trait_level` é o atalho errado para testar o parser; use `fake::transport` com golden file.
- **`StreamEvent` é enum fechado** ([ADR-0005](../decisions/0005-provider-engine-crate.md)). Adicionar variante nova exige revisão explícita.
- **Credenciais vêm do `CredentialStore`** ([ADR-0007](../decisions/0007-credential-store-trait.md)). O construtor de um adapter concreto recebe `Arc<dyn CredentialStore>` (ou equivalente), nunca `String`/`&str` com a chave. Erro de compilação se você tentar.
- **Watchdog via `Clock` injetado**. O `ChatRequest::event_timeout` é um `Duration` "real" mas o orquestrador consulta o `Clock` do `frederico-security` para saber quando estourou. Testes usam `FakeClock` + `tokio::time::pause()`.
- **UTF-8 partido entre chunks** é cenário coberto pelo fixture `openai/utf8_split.jsonl`. Se o parser regredir nesse caso, o teste `parser_handles_utf8_split_across_chunks` falha.
- **Sentinela `[DONE]`** do OpenAI-compat é traduzido para `StreamEvent::Done { stop_reason: Stop }`. Não vaza no stream do consumidor.

## Como testar isoladamente

```bash
cargo test -p frederico-provider-engine
```

Os testes vivem em `#[cfg(test)] mod tests` em cada arquivo do crate. Os fixtures vivem em `crates/provider-engine/fixtures/<provider>/<scenario>.jsonl` e são carregados por `fake::transport::load_golden_file`.

Para adicionar um cenário novo:

1. Crie `crates/provider-engine/fixtures/<provider>/<scenario>.jsonl`.
2. Primeira linha: `{"_fixture_header": {provider, model, scenario, recorded_at, recorder_version, source_endpoint}}`.
3. Linhas seguintes: cada uma é uma string JSON com o chunk SSE bruto (com `\n` escapado como `\\n`).
4. Adicione um teste em `src/fake/transport.rs` (ou no adapter correspondente) que carrega o fixture e verifica os eventos parseados.

## Etapa 4.1 — Anthropic tool_result + tool_call deltas em múltiplos chunks

A Etapa 4.1 fecha 2 débitos técnicos do `provider-engine` que
estavam em aberto desde a Etapa 4 (quando o `RunExecutor` começou
a fechar o loop `tool_call`):

**4.1.1 — `AnthropicAdapter` traduz `Role::Tool` em `content_block`
`tool_result`.** O Anthropic não tem `role: "tool"` — a resposta
de ferramenta é um content block
`{"type": "tool_result", "tool_use_id": <call_id>, "content": <output>}`
dentro de uma mensagem com `role: "user"`. O
`AnthropicAdapter::build_request_body` agora detecta `Role::Tool`
e gera essa estrutura; o `tool_call_id` da `ChatMessage` casa
com o `tool_use_id` do Anthropic. Antes da Etapa 4.1, o
`role_to_str` traduzia `Role::Tool` em `"user"` placeholder
(ignorando o conteúdo) — o modelo Anthropic nunca recebia o
resultado da ferramenta. Teste novo:
`build_request_body_translates_tool_role_to_tool_result_block`.

**4.1.2 — `OpenAiCompatAdapter` agrega tool_call deltas em
múltiplos chunks.** A OpenAI (e provedores compat) envia o
`tool_call` em 1+ chunks quando os argumentos são grandes: o
primeiro traz `id` + `name` + `arguments: ""`; os seguintes
trazem só `function.arguments` com pedacinhos do JSON; o
`finish_reason: "tool_calls"` sinaliza o fim. O parser SSE
(`openai_compat_translate`) é puro por chunk — não tinha
estado entre chunks pra agregar. O novo módulo `accumulator`
introduz `ToolCallDeltaAccumulator`: estrutura com
`HashMap<u32, ToolCallPartial>` (indexada pelo `index` que o
OpenAI atribui) que mantém estado entre chunks. O
`OpenAiCompatAdapter::stream` foi refatorado pra usar
`futures::stream::unfold` carregando o accumulator no estado
(processa em série — não precisa de `Mutex`/`Arc`). O
accumulator emite 1 `StreamEvent::ToolCall { id, name,
arguments_json }` consolidado quando os deltas completam
(heurística: arguments termina com `}`) OU quando o
`finish_reason: "tool_calls"` chega (drena os partials
restantes — caso comum onde o `finish_reason` chega antes do
último delta de arguments). Testes novos: 3 no módulo
`accumulator` (`single_chunk_with_complete_tool_call`,
`multiple_chunks_accumulate_arguments`,
`finish_reason_tool_calls_drains_remaining_partials`).

## O que não faz

- **Execução de `tool_call`** (Fase 3). A enum `StreamEvent::ToolCall` é emitida e persistida, mas o motor não age.
- **Pausa/retomada de run** (Fase 3). O `CancellationToken` no `ChatRequest` cobre apenas cancelamento.
- **Memória, contexto de longo prazo** (Fase 4).
- **Adapter Gemini** (v1.1). Por enquanto, a v1 cobre OpenAI-compat, Anthropic e o provedor simulado.
- **Catálogo de modelos embutido**. O `frederico-model-catalog` (Leva 2) cuida disso; este crate só consome `ModelId`/`ProviderId`.
- **Persistência de runs/mensagens**. O orquestrador (Leva 2) cuida disso via `frederico-storage`.

## Decisões

- [ADR-0005](../decisions/0005-provider-engine-crate.md) — criação do crate, trait e adapters.
- [ADR-0007](../decisions/0007-credential-store-trait.md) — credenciais via `CredentialStore`.
- [ADR-0008](../decisions/0008-fake-provider-strategy.md) — fake transport-level + trait-level.

## Referências

- [`docs/architecture/chat-and-providers.md`](../architecture/chat-and-providers.md) — spec unificado da Fase 2.
- [`docs/architecture/security-threat-model.md`](../architecture/security-threat-model.md) — T1 (credencial em log), I1 (env do sandbox).
