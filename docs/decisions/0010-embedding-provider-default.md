# 0010 — Provider de embeddings default: OpenRouter (pluggable, sem modelo local na inicialização)

## Contexto

A Fase 4 (Memória e continuidade) introduz o retrieval híbrido
([`memory-architecture.md`](../architecture/memory-architecture.md)). O
scoring combina busca lexical (FTS5 do SQLite) com busca semântica
(cosine similarity sobre embeddings). O `PROMPT MESTRE` §10.13 fixa
três restrições duras sobre embeddings:

1. **Embeddings por provedor configurado por padrão.** Modelo local
   ONNX apenas sob demanda.
2. **Modelo local nunca na inicialização do app.** Baixar binário
   grande no startup degrada tempo de abertura e exige empacotamento
   de运行时 no instalador.
3. **Recuperação semântica com prazo máximo de 2 s.** Estourado, a
   execução segue sem ela e o fato é registrado — memória nunca trava
   uma resposta. **Busca lexical FTS5 funciona mesmo sem embeddings**
   disponíveis.

A Fase 2 fechou com o `frederico-provider-engine` consumindo o provedor
configurado pelo usuário (OpenAI, OpenRouter, DeepSeek, Mistral, NIM,
Ollama, LM Studio, Anthropic, fake). O `provider-engine` já tem
`CredentialStore` (DPAPI no Windows, ADR-0007) e adapters de chat —
mas não tem adapter de **embedding**. A Fase 4 precisa decidir quem
provê embeddings e em que formato.

A pergunta central deste ADR é **onde mora o adapter de embeddings**.
Três dimensões:

- **Gateway default:** qual provedor o usuário configura uma vez e
  usa tanto pra chat (Fase 2) quanto pra embeddings (Fase 4)?
- **Modelo default:** qual modelo de embedding é o default se o
  usuário não escolheu?
- **Pluggability:** como um usuário com requisitos diferentes (empresa
  com VPC privada, provedor self-hosted) substitui o default sem
  fork?

## Decisão

### 1. `EmbeddingProvider` trait no `frederico-memory`

A trait fica no `frederico-memory::embedding`:

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Identificador do provedor (e.g. "openrouter", "openai", "local-onnx").
    fn provider_id(&self) -> &str;
    /// Dimensões do vetor (e.g. 1536, 3072).
    fn dimensions(&self) -> usize;
    /// Embute um lote de textos. Devolve `EmbeddingError` em falha —
    /// o `Retriever` traduz em "retrieval sem semântica" (cai pra
    /// lexical-only) e registra o fato (§10.13).
    async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
}
```

A trait é **a única porta** entre o `Retriever` e qualquer backend de
embeddings. O `Retriever` aceita `Arc<dyn EmbeddingProvider>` e decide
— em runtime, por chamada — se usa semântica (provider disponível) ou
cai pra lexical-only (provider ausente / falhou / timeout).

A Etapa 2 da Fase 4 entrega o adapter concreto; este ADR só fixa a
forma da trait.

### 2. Gateway default: OpenRouter

OpenRouter é o gateway default, igual ao que a Fase 2 já usa pra
chat ([ADR-0007](../decisions/0007-credential-store-trait.md) e
`provider-engine`). Razão prática: **uma chave dá acesso a
dezenas de modelos de embedding** (OpenAI `text-embedding-3-small` /
`text-embedding-3-large`, Mistral `mistral-embed`, Cohere, Voyage, etc)
com o mesmo `CredentialStore` que o chat já consome. O usuário que
configurou OpenRouter pra chat não configura nada novo pra embedding.

`provider_id` default do adapter: `"openrouter"`. URL base
`https://openrouter.ai/api/v1/embeddings`. Header
`Authorization: Bearer <key>`. Reusa o `CredentialStore` da Fase 2
(mesmo `TargetName` `Frederico-IA-Studio:provider:openrouter`).

### 3. Modelo default: `openai/text-embedding-3-small`

`openai/text-embedding-3-small` (1536 dimensões) é o default. Razões:

- **Custo:** $0.02 / 1M tokens. Bem mais barato que `3-large` ($0.13).
- **Qualidade:** benchmark MTEB na faixa dos melhores da categoria
  pra textos curtos e médios; acima de 512 tokens, o ganho do `3-large`
  é marginal pro caso de uso de memória (frases curtas, instruções,
  factos).
- **Velocidade:** 1536 dim encolhe o índice e o cálculo de cosine.

O `Retriever` consome `Arc<dyn EmbeddingProvider>` e chama
`provider.dimensions()` antes do primeiro `embed` para validar que a
tabela `memory_embeddings(memory_id, dim, vec_blob)` foi gravada com
a mesma dimensionalidade. Mudança de modelo exige **reindexação**
(ADR-0013) — o app não recalcula embeddings em hot path.

A Etapa 2 introduz o campo `embedding_model` na config do usuário
(`provider_configs` ou nova tabela `memory_config` — a Etapa 2
decide). Default `"openai/text-embedding-3-small"`. Configurável
pelo painel de memória (Fase 4 Etapa 5).

### 4. Pluggability

Três adapters pluggáveis são canônicos:

- **`OpenRouterEmbeddingAdapter`** (default; usa o endpoint
  `/embeddings` da OpenAI-compat).
- **`OpenAiDirectEmbeddingAdapter`** (pra quem prefere OpenAI direto;
  mesma interface).
- **`NoopEmbeddingAdapter`** (sempre disponível; usado pela Etapa 1
  da Fase 4 — devolve `Err(EmbeddingError::Unavailable)` e força o
  caminho lexical-only. É o que faz o §10.13 "FTS5 funciona sem
  embeddings" cair num teste determinístico).

Modelo local ONNX **não** é adapter de v1. Se o usuário quiser, é
trabalho da Fase 4.x.y (depois da Etapa 6), e deve ser **opt-in
explícito** com download de modelo on-demand (nunca no startup —
§10.13).

### 5. Erro e timeout

A trait `EmbeddingProvider::embed` retorna `EmbeddingError` com
variantes:

```rust
pub enum EmbeddingError {
    /// Credencial ausente / provedor não configurado. O `Retriever`
    /// registra o fato e cai pra lexical-only.
    Unavailable,
    /// HTTP 4xx/5xx ou parsing falhou. Retry até 2x com backoff
    /// exponencial curto (200ms, 800ms); depois `Unavailable`.
    Transport(String),
    /// Timeout (> 1.5s — `Retriever` tem orçamento total de 2s,
    /// precisa de 500ms pra cosine + ordenação). Retry não tenta
    /// mais — cai pra lexical-only.
    Timeout,
}
```

A regra de **"memória nunca trava uma resposta"** (§10.13) é
enforçada pelo `Retriever`, não pela trait. A trait devolve erro; o
`Retriever` decide o que fazer.

## Alternativas descartadas

- **Só OpenAI direto (sem OpenRouter).** Descartada: 1 chave por
  provedor vs. 1 chave pra vários. OpenRouter é o gateway default da
  Fase 2 — Fase 4 segue o mesmo padrão (consistência operacional
  menor atrito de configuração).
- **Modelo local ONNX obrigatório.** Descartada: §10.13 proíbe
  init; empacotar binário grande no instalador Windows degrada a
  abertura do app; a Etapa 4 da Fase 4 cobre 95% do uso com
  embeddings remotos.
- **Modelo local ONNX opcional, mas no startup.** Descartada: mesma
  proibição do §10.13. Opt-in explícito só, com download on-demand.
- **Embeddings por modelo local em background (após startup).**
  Descartada: ambiguidade de "inicialização" — se a primeira
  chamada do `Retriever` for em < 1s após o app abrir, ainda é
  startup pro usuário. Modelos locais ficam pra Fase 4.x.y.
- **Embeddings como parte do `provider-engine`.** Descartada: o
  `provider-engine` é adapter de **chat** (SSE streaming de
  completions), não de embeddings. Misturar os dois quebra a
  separação `process-architecture.md` e cria ciclo de dependência
  (o `Retriever` é o caller, e o `provider-engine` já depende de
  várias coisas; inversão de deps desnecessária).
- **Crate novo `frederico-embeddings`.** Descartada: 1 crate com
  50 linhas de trait + 1 adapter é overhead. Embeddings é detalhe
  do `memory`. Crate separado só se um segundo consumidor aparecer
  (Fase 4.x.y, modelo local, ou se o chat quiser embeddings pra
  outra coisa).

## Consequências

**Mais fácil:**

- A trait é fina (1 método `embed`) e o `NoopEmbeddingAdapter` é
  trivial — a Etapa 1 da Fase 4 pode exercitar o caminho
  lexical-only **antes** da Etapa 2 existir adapter real. O gold-set
  baseline da Etapa 1 já é executado com `NoopEmbeddingAdapter`,
  provando §10.13.
- OpenRouter como gateway default reaproveita credencial existente.
  Usuário que já configurou Fase 2 não toca em nada pra Fase 4
  funcionar com embeddings.
- Dimensionalidade conhecida em tempo de configuração: o `Retriever`
  não tem que lidar com embeddings de tamanho variável.
- A `OpenRouterEmbeddingAdapter` é a "irmã" da
  `OpenAiCompatAdapter` do `provider-engine` (mesma forma de request
  OpenAI-compat, mesma auth) — Etapa 2 implementa em ~1 hora.

**Mais difícil:**

- Mudança de provedor de embedding exige **reindexação**
  (ADR-0013). O `Retriever` precisa ler embeddings de um único
  namespace (não misturar `text-embedding-3-small` com
  `mistral-embed` na mesma memória). O `embedding_model` da config
  vira chave de partição.
- OpenRouter adiciona latência (~50ms a mais que OpenAI direto).
  Mitigação: o orçamento de 2s do `Retriever` é folgado; em prática
  embeddings de 1k tokens custam ~200ms com OpenRouter. Se a
  latência virar problema real, o usuário troca pra `OpenAiDirect`
  via painel (Etapa 5).
- O `NoopEmbeddingAdapter` é "silencioso demais" — se a credencial
  OpenRouter expirar, o `Retriever` cai pra lexical sem avisar
  ninguém. Mitigação: a `Retriever` registra via `tracing::warn!`
  toda vez que cai pra lexical; o painel de memória (Etapa 5)
  mostra um banner "embeddings desabilitados" se o contador > 0
  nas últimas 24h.
- A Etapa 2 da Fase 4 é **bloqueada** pela Etapa 1 (a Etapa 1
  entrega o trait + `NoopEmbeddingAdapter`; a Etapa 2 entrega o
  `OpenRouterEmbeddingAdapter` real). Isso está refletido no plano
  da Fase 4.
