# 0005 — `provider-engine` como crate do núcleo

## Contexto

A Fase 2 (Chat e provedores) introduz conversa real com provedores de LLM. O catálogo realista de formatos diverge de forma não-trivial:

- **OpenAI-compat** — um único formato HTTP/SSE, mas com várias instâncias: OpenAI, OpenRouter, DeepSeek, Mistral, NVIDIA NIM, Ollama, LM Studio. A diferença entre eles é a URL base, o cabeçalho de autenticação e o subconjunto do catálogo de modelos. O OpenRouter em particular é o gateway de maior retorno por esforço: uma chave dá acesso a centenas de modelos.
- **Anthropic** — formato genuinamente diferente: blocos de conteúdo, `tool_use` como tipo próprio, SSE com eventos `content_block_delta` em vez de `delta`. Exercitar o motor contra esse formato evita que a abstração nasça torta em cima de um único fornecedor.
- **Gemini** — o mais divergente (parts, `functionDeclarations`, esquema próprio). É o adaptador mais caro e o que menos acrescenta agora, porque o que ele faz os outros já fazem. Fica para a v1.1.
- **Simulated** — provedor falso para testes com fitas e scripts de patologia (ver ADR-0008).

A Fase 1 fechou com a casca Tauri + IPC, mas o núcleo ainda não tem abstração de provedor: sócrates genéricos (`core`, `storage`, `diagnostics`, `security`) e o envelope `IpcRequest`/`IpcResponse` com as operações `GetAppInfo` e `Ping`. O `software-architecture.md` lista `provider-engine` como crate "previsto", sem decisão formal de fronteira.

Sem um ADR, o `provider-engine` corre o risco de virar um catch-all de formatos (formato OpenAI como modelo único, com campos opcionais "para Gemini" e "para Anthropic") — abstração que parece simples no primeiro mês e quebra no segundo, quando algum provedor introduz um campo novo.

## Decisão

Criar o crate `crates/provider-engine/` no workspace do Cargo, sem dependência de plataforma (sem `tauri`, sem `windows` — ADR-0003), e expor a trait `ProviderAdapter`:

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

- `ChatRequest` carrega `model: ModelId`, `messages: Vec<ChatMessage>`, `tools: Vec<ToolDescriptor>` (estrutura, sem execução — execução é Fase 3), `temperature`, `max_tokens` e `cancel: CancellationToken` (de `tokio-util`).
- `StreamEvent` é um enum fechado: `Delta { content: String }`, `Usage { prompt_tokens, completion_tokens }`, `ToolCall { id, name, arguments_json }` (esqueleto), `Done { stop_reason }`, `Error(ProviderError)`, `Cancelled`. A enumeração é fechada por design — a fronteira não vaza formato de provedor.
- `RunHandle` é opaco (`Uuid`) e mapeia 1:1 para um `CancellationToken` interno; o método `cancel` sinaliza o token, e o adapter interrompe a request `reqwest` em andamento.
- HTTP client: `reqwest` com `rustls-tls` (sem OpenSSL — mais simples no ambiente Windows, alinhado com o toolchain GNU da Fase 1).
- Parser SSE: `eventsource-stream` (escolha pragmática; aceita o body stream do `reqwest` e emite eventos com fronteiras já delimitadas).

### Adaptadores concretos da v1

- `OpenAiCompatAdapter` — parametrizado por `(base_url: Url, auth_header: fn(SecretString) -> (String, String))`. Cobre OpenAI, OpenRouter, DeepSeek, Mistral, NVIDIA NIM, Ollama, LM Studio. O `auth_header` é o único ponto divergente por provedor: OpenRouter usa `Authorization: Bearer`, Anthropic-Compat (vía OpenRouter) tem variantes, NIM usa `Bearer` mas o header pode ser customizado. Manter essa parametrização é o que permite tratar sete provedores como um.
- `AnthropicAdapter` — implementa o endpoint `messages` com content blocks, `tool_use` e SSE `content_block_*`. Sem fallback de formato.
- `FakeProviderAdapter` (golden file) e `ScriptedProviderAdapter` (patologia) — ver ADR-0008. **Não implementam o trait**; produzem bytes que o parser real consome.
- **Gemini: fora da v1.** Sem arquivos, sem fixture, sem código. Volta como ADR-000X na v1.1 quando o motor já estiver exercitado contra OpenAI-compat e Anthropic.

### Limites explícitos do que o crate não faz na v1

- Não executa `tool_call` (Fase 3 — Motor de execução e ferramentas).
- Não persiste mensagens (vive no `frederico-storage`; `provider-engine` apenas emite os eventos).
- Não conhece nada da casca Tauri — emissão de eventos para a UI é responsabilidade do command no `apps/desktop/src-tauri/`.
- Não tem cache de resposta.

## Alternativas descartadas

- **Um crate por provedor** (`provider-openai`, `provider-anthropic`, `provider-mistral`, ...). Descartada pela regra de pragmatismo do ADR-0002: explode a contagem de crates sem ganho de fronteira, já que todos OpenAI-compat compartilham código. O agrupamento por **formato** (e não por fornecedor) é a divisão que sobrevive à chegada de um provedor novo.
- **Mega-adapter com detecção de formato** (`fn detect_format(base_url) -> FormatKind`). Descartada: a Anthropic tem blocos de conteúdo e SSE genuinamente diferente, e o Gemini (quando entrar) tem parts e `functionDeclarations`. Detectar formato na request e adaptar a resposta é exatamente o tipo de abstração que nasce torta — o caminho comum vira o menor denominador, e a primeira exceção vira `if` especial.
- **Adapter como plugin dinâmico** carregado em runtime. Descartada: fora de escopo da v1 (a Fase 0 fechou isso como adiamento — ver `development-roadmap.md` §Adiamentos).
- **Usar SDKs prontos** (`async-openai`, `anthropic-sdk-rs`). Descartada: superfície de API externa a mais para auditar, atraso de versão quando um provedor muda o formato, e lock-in em decisões de design de terceiros. Para o subconjunto que precisamos, um adapter focado é menor, mais controlável e mais fácil de testar contra fita.

## Consequências

**Mais fácil:**

- Trocar provedor vira configuração (URL + chave), não código.
- Um único teste de parser de SSE cobre todos os OpenAI-compat.
- Cada adapter é testado isoladamente com seu próprio conjunto de fitas.
- A enum `StreamEvent` é a fronteira — se um provedor emite algo novo, ou vira variante nova do enum (mudança explícita, revisável) ou não entra.

**Mais difícil:**

- Adicionar um provedor novo exige implementar um adapter (custo de integração conhecido).
- Risco real: a trait vazar detalhes de um provedor. A revisão de PR precisa olhar isso — "isso pertence ao trait ou ao adapter?" é uma pergunta obrigatória em cada mudança de interface.
- O `OpenAiCompatAdapter` parametrizado por `auth_header` é um pequeno truque que precisa de comentário bom no código; sem ele, parece que esqueci de tratar o OpenRouter.
- Gemini fica para a v1.1 — quem pedir o adaptador agora recebe a resposta "está no roadmap; sem previsão na v1".
