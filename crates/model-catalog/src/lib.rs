//! `frederico-model-catalog` — catálogo embutido de modelos.
//!
//! Ver [ADR-0006](../decisions/0006-model-catalog-crate.md) e o spec
//! [`docs/architecture/chat-and-providers.md`](../architecture/chat-and-providers.md).
//!
//! O catálogo é versionado em `data/catalog.json`, validado em build,
//! embutido no binário via `include_str!`, e exposto por uma API
//! tipada (`find_model`, `list_for_provider`, `pricing_for`).
//!
//! Atualizações são PRs normais — uma revisão, um merge, próximo
//! release. Não há rede em runtime.
//!
//! ## Etapa 3 da Fase 6 (ADR-0030)
//!
//! A partir da Etapa 3, o crate também carrega o **registro de
//! especialistas** (`SpecialistDefinition` + `SpecialistRegistry`),
//! versionado em `data/specialists/default.toml` e bundled no
//! binário. Ver [`specialist`] e [`registry`]. O registro é o
//! complemento user-facing do catálogo: o catálogo diz "que modelos
//! existem", o registro diz "que papéis (com quais ferramentas e
//! restrições) esses modelos podem exercer". Mesma estratégia
//! (bundled + override em `~/.config/frederico/specialists.toml`,
//! zero rede em runtime, versão pinada).

use std::sync::OnceLock;

use frederico_core::{ModelId, ProviderId};
use serde::{Deserialize, Serialize};

pub mod fusao;
pub mod registry;
pub mod specialist;

pub use fusao::{fundir, ModeloEfetivo, ModeloRemotoNormalizado, Origem, RespostaDoProvedor};
pub use registry::{DefaultSpecialistRegistry, RegistryError, SpecialistRegistry};
pub use specialist::{
    parse_specialists_toml, SpecialistDefinition, SpecialistId, SpecialistMaxSteps,
    SpecialistSummary, SpecialistTimeoutMs, SpecialistTokenBudget,
};

/// Hash BLAKE3 (ou fallback FNV-64 se `blake3` crate não estiver
/// disponível) do catálogo canônico, exposto pelo `build.rs`.
pub const CATALOG_HASH: &str = env!("CATALOG_HASH");

/// Modalidade de entrada/saída de um modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Tools,
    JsonMode,
    ParallelToolCalls,
    Vision,
    PromptCaching,
    ReasoningContent,
}

/// Capacidades declaradas de um modelo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalitySet {
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
}

impl ModalitySet {
    #[must_use]
    pub fn has_input(&self, m: Modality) -> bool {
        self.input.contains(&m)
    }
    #[must_use]
    pub fn has_output(&self, m: Modality) -> bool {
        self.output.contains(&m)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub capabilities: Vec<Capability>,
}

impl CapabilitySet {
    #[must_use]
    pub fn has(&self, c: Capability) -> bool {
        self.capabilities.contains(&c)
    }
}

/// Preço por **milhão** de tokens. Sem ponto flutuante no banco
/// (regra do [ADR-0006](../decisions/0006-model-catalog-crate.md)).
///
/// **O nome do campo mente sobre a unidade, e o comentário anterior
/// mentia sobre a base.** Ele dizia "por mil tokens", enquanto o
/// `cost_microcents` divide por `1_000_000` e o campo do JSON se
/// chama `pricing_per_million` — é por milhão. E a unidade não é
/// microcent (10⁻⁶ de centavo): é **10⁻⁵ de dólar**, ou seja
/// milicentavo. Conferido contra seis entradas do catálogo cujo
/// preço público se conhece — GPT-4o mini a US$ 0,15/1M gravado como
/// `15000`, Claude 3.5 Haiku a US$ 0,80/1M como `80000`.
///
/// O nome fica como está porque renomeá-lo toca schema, JSON, banco
/// e migração — trabalho com risco próprio, que não cabe junto de
/// uma atualização de catálogo. Fica registrado para quem for
/// calcular preço a partir daqui não errar por três ordens de
/// grandeza.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceTable {
    pub input_microcents: u64,
    pub output_microcents: u64,
}

impl PriceTable {
    /// Custo em microcents para um par de tokens.
    #[must_use]
    pub fn cost_microcents(&self, prompt: u32, completion: u32) -> u64 {
        let input = (u64::from(prompt) * self.input_microcents) / 1_000_000;
        let output = (u64::from(completion) * self.output_microcents) / 1_000_000;
        input + output
    }
}

/// Metadado de um modelo. Catálogo embutido expõe um vetor desses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub provider: ProviderId,
    pub model: ModelId,
    pub display_name: String,
    pub context_window: u32,
    pub modalities: ModalitySet,
    pub capabilities: CapabilitySet,
    pub pricing_per_million: PriceTable,
}

impl ModelDescriptor {
    #[must_use]
    pub fn cost_microcents(&self, prompt: u32, completion: u32) -> u64 {
        self.pricing_per_million.cost_microcents(prompt, completion)
    }
}

/// Catálogo. Clone barato: apenas clona o `Vec<ModelDescriptor>`
/// (a estrutura é pequena — algumas dezenas de entradas no v1).
#[derive(Debug, Clone)]
pub struct Catalog {
    models: Vec<ModelDescriptor>,
}

impl Catalog {
    /// Carrega o catálogo embutido. Retorna `&'static` para o
    /// caminho rápido, mas `Catalog: Clone` permite cópias para
    /// passar por `Arc` ou injetar em workers de teste.
    #[must_use]
    pub fn load() -> &'static Self {
        static CATALOG: OnceLock<Catalog> = OnceLock::new();
        CATALOG.get_or_init(Catalog::from_embedded_json)
    }

    /// Monta um catálogo a partir de descritores já resolvidos.
    ///
    /// É o construtor que o refresh de boot usa: a fusão do
    /// embutido com o que os provedores responderam ([ADR-0052])
    /// produz `Vec<ModeloEfetivo>`, e o motor precisa de um
    /// `Catalog` para validar o modelo escolhido.
    ///
    /// **Por que isso existe:** sem ele, a UI listava a lista
    /// fundida e o motor validava contra a embutida — as duas
    /// discordavam sobre quais modelos existem, e escolher um
    /// modelo novo dava `ModelNotFound` no meio da conversa.
    ///
    /// [ADR-0052]: ../../../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
    #[must_use]
    pub fn from_models(models: Vec<ModelDescriptor>) -> Self {
        Self { models }
    }

    /// Carrega a partir de uma string JSON (útil para testes com
    /// catálogo customizado).
    #[must_use]
    pub fn from_json(json: &str) -> Self {
        let raw: RawCatalog =
            serde_json::from_str(json).expect("catalog.json é válido (build.rs garante)");
        Self {
            models: raw.models.into_iter().map(Into::into).collect(),
        }
    }

    fn from_embedded_json() -> Self {
        let json = include_str!(env!("CATALOG_JSON_PATH"));
        Self::from_json(json)
    }

    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    #[must_use]
    pub fn find_model(&self, provider: &ProviderId, model: &ModelId) -> Option<&ModelDescriptor> {
        self.models
            .iter()
            .find(|m| &m.provider == provider && &m.model == model)
    }

    #[must_use]
    pub fn list_for_provider(&self, provider: &ProviderId) -> Vec<&ModelDescriptor> {
        self.models
            .iter()
            .filter(|m| &m.provider == provider)
            .collect()
    }

    #[must_use]
    pub fn list_all(&self) -> Vec<&ModelDescriptor> {
        self.models.iter().collect()
    }

    #[must_use]
    pub fn pricing_for(&self, provider: &ProviderId, model: &ModelId) -> Option<PriceTable> {
        self.find_model(provider, model)
            .map(|m| m.pricing_per_million)
    }
}

/// Forma crua do `catalog.json` antes de ser convertida para os tipos
/// de domínio.
#[derive(Debug, Deserialize)]
struct RawCatalog {
    models: Vec<RawModelDescriptor>,
}

#[derive(Debug, Deserialize)]
struct RawModelDescriptor {
    provider: String,
    model: String,
    display_name: String,
    context_window: u32,
    modalities: RawModalitySet,
    capabilities: Vec<String>,
    pricing_per_million: RawPriceTable,
}

#[derive(Debug, Deserialize)]
struct RawModalitySet {
    input: Vec<String>,
    output: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawPriceTable {
    input_microcents: u64,
    output_microcents: u64,
}

impl From<RawModelDescriptor> for ModelDescriptor {
    fn from(r: RawModelDescriptor) -> Self {
        let to_modality = |s: &str| match s {
            "text" => Some(Modality::Text),
            "image" => Some(Modality::Image),
            "audio" => Some(Modality::Audio),
            _ => None,
        };
        let to_capability = |s: &str| match s {
            "tools" => Some(Capability::Tools),
            "json_mode" => Some(Capability::JsonMode),
            "parallel_tool_calls" => Some(Capability::ParallelToolCalls),
            "vision" => Some(Capability::Vision),
            "prompt_caching" => Some(Capability::PromptCaching),
            "reasoning_content" => Some(Capability::ReasoningContent),
            _ => None,
        };
        Self {
            provider: ProviderId::new(r.provider),
            model: ModelId::new(r.model),
            display_name: r.display_name,
            context_window: r.context_window,
            modalities: ModalitySet {
                input: r
                    .modalities
                    .input
                    .iter()
                    .filter_map(|s| to_modality(s))
                    .collect(),
                output: r
                    .modalities
                    .output
                    .iter()
                    .filter_map(|s| to_modality(s))
                    .collect(),
            },
            capabilities: CapabilitySet {
                capabilities: r
                    .capabilities
                    .iter()
                    .filter_map(|s| to_capability(s))
                    .collect(),
            },
            pricing_per_million: PriceTable {
                input_microcents: r.pricing_per_million.input_microcents,
                output_microcents: r.pricing_per_million.output_microcents,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_embedded_json() {
        let cat = Catalog::load();
        assert!(!cat.models().is_empty(), "catálogo não pode estar vazio");
    }

    #[test]
    fn find_model_returns_known_entry() {
        let cat = Catalog::load();

        // **Não fixa um modelo de mercado.** A versão anterior deste
        // teste pinava `openai/gpt-4o`, e quebrou na primeira
        // atualização do catálogo — que é uma operação de rotina, não
        // uma mudança de contrato. Teste que quebra quando o dado é
        // atualizado corretamente mede a data do arquivo, não o
        // código.
        //
        // `simulated/fake-model-v1` é a única entrada que o produto
        // controla: existe para os testes e não sai do catálogo por
        // decisão de nenhum provedor.
        let m = cat
            .find_model(
                &ProviderId::new("simulated"),
                &ModelId::new("fake-model-v1"),
            )
            .expect("o modelo simulado deve estar no catálogo");
        assert!(m.context_window > 0);
        assert!(m.modalities.has_input(Modality::Text));
        assert!(m.capabilities.has(Capability::Tools));

        // Controle positivo do `find_model`: id que não existe
        // devolve `None`, e não a primeira entrada.
        assert!(cat
            .find_model(&ProviderId::new("simulated"), &ModelId::new("nao-existe"))
            .is_none());

        // E o catálogo tem entrada multimodal — sem prender a qual.
        assert!(
            cat.models()
                .iter()
                .any(|m| m.modalities.has_input(Modality::Image)
                    && m.capabilities.has(Capability::Vision)),
            "o catálogo perdeu toda entrada com visão"
        );
    }

    #[test]
    fn find_model_returns_none_for_unknown() {
        let cat = Catalog::load();
        assert!(cat
            .find_model(&ProviderId::new("openai"), &ModelId::new("gpt-99-ultra"))
            .is_none());
    }

    #[test]
    fn list_for_provider_returns_only_matching() {
        let cat = Catalog::load();
        let openai = cat.list_for_provider(&ProviderId::new("openai"));
        assert!(openai.len() >= 2);
        for m in openai {
            assert_eq!(m.provider.as_str(), "openai");
        }
    }

    #[test]
    fn pricing_for_returns_price_table() {
        let cat = Catalog::load();
        let p = cat
            .pricing_for(&ProviderId::new("openai"), &ModelId::new("gpt-4o-mini"))
            .expect("gpt-4o-mini tem preço");
        // $0.15 / 1M input, $0.60 / 1M output.
        assert_eq!(p.input_microcents, 15_000);
        assert_eq!(p.output_microcents, 60_000);
    }

    #[test]
    fn openrouter_subset_is_in_catalog() {
        let cat = Catalog::load();
        let or_models = cat.list_for_provider(&ProviderId::new("openrouter"));
        assert!(!or_models.is_empty(), "pelo menos um modelo OpenRouter");
        // Cada modelo tem o formato "vendor/model" no `model`.
        for m in or_models {
            assert!(
                m.model.as_str().contains('/'),
                "modelo OpenRouter deve ter 'vendor/model': {}",
                m.model.as_str()
            );
        }
    }

    #[test]
    fn local_models_have_zero_price() {
        let cat = Catalog::load();
        let ollama = cat
            .find_model(&ProviderId::new("ollama"), &ModelId::new("llama3.1:8b"))
            .expect("ollama presente");
        assert_eq!(ollama.pricing_per_million.input_microcents, 0);
        assert_eq!(ollama.pricing_per_million.output_microcents, 0);
    }

    #[test]
    fn price_table_calculates_cost() {
        let p = PriceTable {
            input_microcents: 1_000_000,  // 1 cent per 1M
            output_microcents: 2_000_000, // 2 cents per 1M
        };
        // 1k tokens = 1 microcent input + 2 microcents output = 3
        // (Wait: 1k * 1M / 1M = 1k, not 1. Let me check the impl.)
        // 1k tokens at 1 cent per 1M = 1k microcents (not 1).
        assert_eq!(p.cost_microcents(1000, 1000), 3000);
    }

    #[test]
    fn catalog_hash_is_stable() {
        // O hash é gerado no build.rs; este teste apenas garante que
        // a env var é não-vazia (caso algo esteja mal configurado).
        assert!(!CATALOG_HASH.is_empty());
    }
}

/// Handle compartilhado do catálogo **efetivo**.
///
/// Existe porque o catálogo deixou de ser fixo no boot: o refresh
/// do [ADR-0052] o substitui quando os provedores respondem, e
/// tanto a UI quanto o motor de execução precisam enxergar a
/// *mesma* lista. Antes desta ligação, a UI lia a lista fundida e
/// o `ChatOrchestrator` validava contra a embutida — a lista
/// suspensa oferecia modelos que o motor rejeitava.
///
/// A troca é do ponteiro inteiro, não de itens: um `Arc<Catalog>`
/// obtido por [`Self::current`] continua válido e coerente mesmo
/// que o refresh publique outro no meio de um run.
///
/// [ADR-0052]: ../../../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
#[derive(Debug)]
pub struct CatalogHandle {
    atual: std::sync::RwLock<std::sync::Arc<Catalog>>,
}

impl CatalogHandle {
    #[must_use]
    pub fn new(inicial: std::sync::Arc<Catalog>) -> Self {
        Self {
            atual: std::sync::RwLock::new(inicial),
        }
    }

    /// O catálogo em vigor agora.
    ///
    /// Se o lock estiver envenenado (algum thread entrou em pânico
    /// segurando-o), devolve o embutido em vez de propagar o
    /// pânico: ficar sem catálogo nenhum derrubaria todo run, e o
    /// embutido é sempre uma resposta defensável.
    #[must_use]
    pub fn current(&self) -> std::sync::Arc<Catalog> {
        match self.atual.read() {
            Ok(g) => g.clone(),
            Err(_) => std::sync::Arc::new(Catalog::load().clone()),
        }
    }

    /// Publica um catálogo novo. Ignorado se o lock estiver
    /// envenenado — perder um refresh é melhor que derrubar o app.
    pub fn replace(&self, novo: std::sync::Arc<Catalog>) {
        if let Ok(mut g) = self.atual.write() {
            *g = novo;
        }
    }
}

impl Default for CatalogHandle {
    fn default() -> Self {
        Self::new(std::sync::Arc::new(Catalog::load().clone()))
    }
}
