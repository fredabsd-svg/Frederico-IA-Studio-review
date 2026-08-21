//! Trait `ProviderAdapter` — a fronteira entre o motor de chat e os
//! provedores de LLM.
//!
//! Ver [ADR-0005](../decisions/0005-provider-engine-crate.md) e
//! [`crate::types`] para os tipos públicos.

use async_trait::async_trait;
use frederico_core::{ModelId, ProviderId};
use futures::stream::BoxStream;

use crate::types::{ChatRequest, ChatResponse, StreamEvent};

/// Handle opaco de um run em andamento. Usado por
/// [`ProviderAdapter::cancel`] para sinalizar o [`CancellationToken`]
/// interno. O ID é o mesmo [`frederico_core::RunId`] mas tratado de
/// forma opaca pelo adapter (o adapter não conhece o tipo `RunId` —
/// recebe um `u128`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunHandle(pub u128);

impl RunHandle {
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for RunHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RunHandle({:032x})", self.0)
    }
}

/// Capacidades declaradas de um adapter. Usado pelo orquestrador para
/// escolher o adapter certo para um pedido e pela UI para mostrar
/// modelos disponíveis.
#[derive(Debug, Clone)]
pub struct AdapterCapabilities {
    /// O adapter implementa `stream` (todos da v1).
    pub supports_stream: bool,
    /// O adapter emite `ToolCall` (todos da v1; execução é Fase 3).
    pub supports_tools: bool,
    /// O adapter emite eventos `Usage` durante o stream (a maioria sim,
    /// alguns só no fim).
    pub supports_usage_in_stream: bool,
}

/// Tabela de preços por 1 milhão de tokens, em **microcents** (1 cent =
/// 1.000.000 microcents). `u64` para evitar ponto flutuante no banco.
///
/// Um modelo sem preço cadastrado é rejeitado antes de qualquer I/O de
/// rede (ver [ADR-0006](../decisions/0006-model-catalog-crate.md)
/// §Decisão e [`crate::types::ChatRequest`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct CostModel {
    pub input_microcents_per_million: u64,
    pub output_microcents_per_million: u64,
}

impl CostModel {
    /// Calcula o custo em microcents para um par de tokens.
    #[must_use]
    pub fn cost_microcents(&self, prompt: u32, completion: u32) -> u64 {
        // u64 * u32 não estoura em faixa realista (1M tokens * 1B microcents
        // = 10^15, dentro de u64). A divisão por 1_000_000 é inteira.
        let input = (u64::from(prompt) * self.input_microcents_per_million) / 1_000_000;
        let output = (u64::from(completion) * self.output_microcents_per_million) / 1_000_000;
        input + output
    }
}

/// Fronteira do motor de chat com provedores de LLM. O orquestrador
/// conhece `dyn ProviderAdapter`; cada provedor concreto (OpenAI-compat,
/// Anthropic, simulated) implementa essa trait.
///
/// **A trait não recebe credenciais em texto puro.** A credencial é
/// obtida via trait `CredentialStore` do `frederico-security` e
/// injetada no construtor do adapter concreto. Ver
/// [ADR-0007](../decisions/0007-credential-store-trait.md) §Decisão.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// ID do provedor (e.g. `"openai"`, `"anthropic"`, `"simulated"`).
    fn id(&self) -> ProviderId;

    /// Capacidades declaradas.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Tabela de preços. Vem do [`crate::types::ChatRequest`] em
    /// produção (atrelada ao `ModelDescriptor`); este método é a
    /// **capacidade padrão** do adapter, usada para fallback quando o
    /// modelo não tem preço explícito.
    fn cost_model(&self) -> CostModel;

    /// Lista os modelos que o provedor declara ter, para o refresh
    /// de catálogo no boot ([ADR-0052] §D1).
    ///
    /// **Default recusa em vez de devolver lista vazia.** Um adapter
    /// que não sabe listar e devolvesse `Ok(vec![])` seria lido pela
    /// fusão como "o provedor não tem modelo nenhum", e a lista
    /// embutida dele desapareceria da UI. Recusar preserva o
    /// embutido, que é o comportamento certo para quem não sabe
    /// responder.
    ///
    /// [ADR-0052]: ../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
    async fn listar_modelos(
        &self,
    ) -> Result<Vec<crate::openai_compat::ModeloRemoto>, crate::types::ProviderError> {
        Err(crate::types::ProviderError::network(format!(
            "o adapter do provedor '{}' não sabe listar modelos",
            self.id().as_str()
        )))
    }

    /// Resposta não-streaming. Usado por `Provider.TestConnection`
    /// (`max_tokens: 1`) e por clientes que não querem stream.
    async fn complete(
        &self,
        request: ChatRequest,
    ) -> Result<ChatResponse, crate::types::ProviderError>;

    /// Stream de eventos. O adapter é dono do `BoxStream`; o consumidor
    /// (`provider-engine` orquestrador) o consome até `Done` ou `Error`.
    fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, StreamEvent>, crate::types::ProviderError>;

    /// Sinaliza o `CancellationToken` interno. Idempotente. O run
    /// associado emite `StreamEvent::Cancelled` na próxima oportunidade.
    fn cancel(&self, handle: RunHandle) -> Result<(), crate::types::ProviderError>;

    /// Lista os modelos que este adapter sabe atender, no formato
    /// `(model_id, display_name)`. O catálogo completo (preços,
    /// capacidades) vem do `frederico-model-catalog` (Leva 2).
    fn known_models(&self) -> Vec<(ModelId, &'static str)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_model_zero_for_zero_tokens() {
        let m = CostModel::default();
        assert_eq!(m.cost_microcents(0, 0), 0);
    }

    #[test]
    fn cost_model_1k_in_1k_out_at_3_per_million() {
        // Preço: 1 cent por 1M tokens de input, 2 cents por 1M de output.
        // 1 cent = 1_000_000 microcents. Logo:
        //   1k tokens input  = 1_000 * 1_000_000 / 1_000_000 = 1000 microcents
        //   1k tokens output = 1_000 * 2_000_000 / 1_000_000 = 2000 microcents
        //   total             = 3000 microcents
        let m = CostModel {
            input_microcents_per_million: 1_000_000,
            output_microcents_per_million: 2_000_000,
        };
        assert_eq!(m.cost_microcents(1000, 1000), 3000);
        // 1M tokens input = 1_000_000 microcents (1 cent).
        assert_eq!(m.cost_microcents(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn run_handle_display_is_stable() {
        let h = RunHandle::new(0xdead_beef);
        let s = h.to_string();
        assert!(s.starts_with("RunHandle(00000000"));
        assert!(s.ends_with("deadbeef)"));
    }
}
