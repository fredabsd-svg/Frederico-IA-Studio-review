# 0006 — `model-catalog` como crate do núcleo com catálogo embutido

> **Substituído parcialmente pelo [ADR-0043](0043-catalogo-embutido-com-refresh-opcional.md)** (2026-08-16). A decisão de que o catálogo é **exclusivamente** embutido foi revista: o embutido continua sendo a base e a garantia de funcionamento offline, mas passa a existir um refresh opcional e explícito contra o provedor. Todo o resto deste ADR — estrutura do `ModelDescriptor`, validação por schema no `build.rs`, `include_str!`, `catalog_hash` — continua valendo.

## Contexto

Cada provedor oferece vários modelos, e cada modelo tem um conjunto distinto de atributos que a UI e o motor precisam conhecer para tomar decisões corretas:

- `context_window` (limite de tokens de entrada — se estourar, o motor precisa truncar com aviso, não deixar o provedor rejeitar com 400).
- `modalities` (texto, imagem, áudio — entrada e saída).
- `capabilities` (function calling, JSON mode, tool_choice, parallel tool calls, ...).
- `pricing_per_million` (input/output, em microcents para evitar ponto flutuante no banco) — base do cálculo de custo do ADR-0005.

Esses dados mudam com frequência: provedores lançam modelo novo, ajustam preço, depreciam modelo antigo. Manter a lista em código (um `match` em `provider-engine`) viola `REGRAS §1.9` ("gerado vence manual") e envelhece mal.

A v1 desktop é Windows-only e roda offline-first (o projeto é explícito sobre "nada de rede obrigatória"). Buscar catálogo de um servidor central em runtime adiciona dependência de rede,边界 de confiança nova e quebra o modo offline — sacrifícios não justificados para um conjunto de dados que cabe em kilobytes.

## Decisão

Criar o crate `crates/model-catalog/` no workspace, sem dependência de plataforma. O catálogo é **embutido no binário** em build time.

### Estrutura dos dados

- Arquivo versionado em `crates/model-catalog/data/catalog.json`. Schema validado por `crates/model-catalog/data/schema.json` (JSON-Schema draft 2020-12).
- `build.rs` valida o JSON contra o schema, escreve uma cópia em `OUT_DIR/catalog.json` e expõe o caminho via env var. O runtime usa `include_str!(env!("CATALOG_JSON_PATH"))`.
- O mesmo `build.rs` computa um `catalog_hash` (BLAKE3 sobre o JSON canônico) e o expõe como constante — vai para o log de diagnóstico da app na inicialização, para que a versão do catálogo seja identificável em qualquer log de bug.

### Forma do `ModelDescriptor`

```rust
pub struct ModelDescriptor {
    pub provider: ProviderId,
    pub model: ModelId,
    pub display_name: String,
    pub context_window: u32,
    pub modalities: ModalitySet,           // text/image/audio in/out
    pub capabilities: CapabilitySet,       // tools, json_mode, parallel_calls, ...
    pub pricing_per_million: PriceTable,   // input, output (microcents)
    pub provider_metadata: serde_json::Value, // bits específicos do provedor
}

pub struct PriceTable {
    pub input_microcents: u64,
    pub output_microcents: u64,
}
```

### API do crate

```rust
pub fn find_model(provider: &ProviderId, model: &ModelId) -> Option<ModelDescriptor>;
pub fn list_for_provider(provider: &ProviderId) -> Vec<ModelDescriptor>;
pub fn list_all() -> Vec<ModelDescriptor>;
pub fn pricing_for(provider: &ProviderId, model: &ModelId) -> Option<PriceTable>;
pub const CATALOG_HASH: &str; // ex.: "blake3:b4f3..."
```

### Conteúdo da v1 (mínimo de entrada)

| Provider | Modelos |
|---|---|
| OpenAI | `gpt-4o`, `gpt-4o-mini` |
| Anthropic | `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest` |
| OpenRouter | subset representativo: `openai/gpt-4o`, `anthropic/claude-3-5-sonnet`, `meta-llama/llama-3.1-70b-instruct` (e mais dois ou três que cubram visão/JSON mode) |
| DeepSeek | `deepseek-chat` |
| Mistral | `mistral-large-latest` |
| NVIDIA NIM | um modelo referência (`meta/llama-3.1-70b-instruct` no NIM) |
| Ollama | placeholder (`llama3.1:8b` como exemplo) |
| LM Studio | placeholder (`qwen2.5-7b-instruct` como exemplo) |
| Simulated | `fake-model-v1` |

Atualizações do catálogo entram por PR normal. Cada PR atualiza `catalog.json` + (se necessário) o schema + bump do `catalog_hash` automaticamente. O reviewer confere o diff do JSON, que é a única coisa que muda.

### Erros e degradação graciosa

- `find_model` retorna `None` se o par `(provider, model)` não está no catálogo. O caller (UI, motor) trata como "modelo indisponível" e mostra a lista de modelos que existem para aquele provedor.
- Se o `build.rs` falhar validação do schema, o build quebra. Sem fallback, sem warning.

## Alternativas descartadas

- **Fetch de catálogo em runtime** (servidor central mantido pelo projeto). Descartada: adiciona dependência de rede no cold start; cria uma fronteira de confiança nova (servidor do projeto precisa ser confiável); quebra modo offline; precisa de um pipeline de atualização e versionamento próprio. Para kilobytes de dados que mudam às vezes, o trade-off é ruim.
- **Tabela no SQLite** (`model_catalog` migration). Descartada: amarra o catálogo à camada de storage; uma atualização de preço vira migração numerada no banco; revisão humana do diff fica pior; obriga migração em todo update.
- **`match` hardcoded em código** dentro de `provider-engine`. Descartada por `REGRAS §1.9`: "se o texto vive em dois lugares, um deles vai mentir". Catálogo em código envelhece e obriga release para mudar preço.
- **Arquivo lido em runtime de `config/`**. Descartada: separa a verdade entre artefato de release e arquivo de config; o instalador NSIS da Fase 1 não teria o catálogo embutido.
- **Catálogo como feature flag** (compilar com ou sem certos provedores). Descartada: o tamanho é desprezível; a complexidade do build não vale a pena.

## Consequências

**Mais fácil:**

- Atualização do catálogo é um PR — uma revisão, um merge, próximo release.
- `build.rs` valida o schema: JSON quebrado quebra o build, sem chance de chegar em produção.
- O `catalog_hash` no log de diagnóstico responde "qual catálogo o usuário estava usando?" em qualquer ticket de bug.
- O catálogo embutido é o "snapshot oficial" do momento do release; reprodutibilidade de bug por release é direta.

**Mais difícil:**

- Adicionar um modelo novo significa esperar o próximo release (ou buildar do fonte). Workflow previsível mas exige commit+merge+tag para virar disponível ao usuário.
- O `provider_metadata` é `serde_json::Value` por design — alguns adapters vão precisar ler campos específicos. A disciplina é "leia só o que te pertence"; revisão de PR precisa olhar isso.
- Gemini não entra na v1, mas o schema precisa estar pronto para receber `functionDeclarations` quando voltar (v1.1) — `capabilities` e `provider_metadata` já dão conta.
- Catálogo grande (centenas de modelos do OpenRouter) cabe no binário sem problema (algumas dezenas de KB), mas a `build.rs` precisa lidar com isso sem travar.
