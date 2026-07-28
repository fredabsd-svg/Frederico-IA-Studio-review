//! Converte `ToolManifest` em `ToolDescriptor` (o tipo que o
//! `ProviderAdapter` consome no `ChatRequest`).
//!
//! Cada provedor (OpenAI, Anthropic, etc.) tem seu próprio
//! formato. O `OpenAiCompatAdapter` aceita
//! `{"type": "function", "function": {"name", "description",
//! "parameters"}}`. O `AnthropicAdapter` aceita
//! `{"name", "description", "input_schema"}`. A Etapa 4 cobre
//! o formato OpenAI-compat (que cobre OpenAI, OpenRouter,
//! DeepSeek, Mistral, NIM, Ollama, LM Studio). Anthropic é
//! hardeings ou Etapa 4.1.
//!
//! O `ToolDescriptor` é a forma **interna** (não tem
//! `parameters_schema` que casa com o provedor); o adapter
//! faz a tradução final. Pra isso, o `ToolDescriptor` carrega
//! o schema JSON do manifesto e a Etapa 4 adiciona o
//! `name` do manifest (`ToolId`).

use frederico_tool_registry::{ToolManifest, ToolRegistry};

/// Estrutura intermediária entre `ToolManifest` e o que cada
/// adapter consome. A Etapa 4 só precisa do `name` e do
/// `parameters_schema` (OpenAI-compat).
#[derive(Debug, Clone)]
pub struct ToolDescriptorForProvider {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

/// Converte os manifestos selecionados em `ToolDescriptor`s
/// prontos pro provedor. Usa `ToolRegistry::effective_tools`
/// (a interseção filtrada do spec `tool-registry-specification.md`
/// §"Interseção de inventário por execução") — passando
/// `allowed_for_run` como filtro. O resultado é o que vai no
/// `ChatRequest::tools` que vai pro `OpenAiCompatAdapter`.
#[must_use]
pub fn tools_to_descriptors(
    registry: &ToolRegistry,
    allowed_for_run: &[frederico_core::ToolId],
) -> Vec<ToolDescriptorForProvider> {
    registry
        .effective_tools(allowed_for_run)
        .into_iter()
        .map(to_descriptor)
        .collect()
}

fn to_descriptor(manifest: ToolManifest) -> ToolDescriptorForProvider {
    ToolDescriptorForProvider {
        name: manifest.id.to_string(),
        description: manifest.description,
        parameters_schema: manifest.input_schema.0,
    }
}
