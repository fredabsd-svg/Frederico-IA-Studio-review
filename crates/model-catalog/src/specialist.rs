//! `SpecialistDefinition` — a peça user-facing do catálogo de
//! modelos.
//!
//! Ver [`docs/architecture/subagent-architecture.md` §"Contrato"](../architecture/subagent-architecture.md)
//! e o [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md).
//!
//! ## O que é um especialista
//!
//! Um `SpecialistDefinition` é a descrição de quem o modelo
//! principal pode delegar. Carrega: id (`"revisor"`, `"pesquisador"`),
//! nome legível, descrição, propósito, modelo default,
//! capabilities permitidas, allow/deny de tools, e o sub-budget
//! (steps, timeout, tokens, custo).
//!
//! A struct vive no `frederico-model-catalog` (junto do
//! `ModelDescriptor`) porque é uma **extensão natural do
//! catálogo**: o catálogo diz "que modelos existem", o
//! specialist registry diz "que papéis (com quais ferramentas e
//! restrições) esses modelos podem exercer". A `default_model`
//! referencia o `ModelId` do catálogo, e o caller resolve o
//! provedor via `Catalog::find_model` quando o subagente de fato
//! roda (Etapa 4).
//!
//! ## Bundled vs override
//!
//! 8 `SpecialistDefinition`s são **bundled** no binário via
//! `data/specialists/default.toml` (embedded pelo `build.rs` —
//! mesmo padrão do `data/catalog.json` da Etapa 2). O usuário
//! pode desabilitar/reescrever esses em
//! `~/.config/frederico/specialists.toml` (override) — o
//! [`registry::DefaultSpecialistRegistry`] carrega bundled +
//! override e expõe via [`registry::SpecialistRegistry::get`].
//!
//! **Adicionar** novos IDs no override é permitido (o usuário
//! pode definir um especialista custom). **Invadir** IDs bundled
//! é proibido — o ADR-0030 §D1 mantém os 8 bundled como
//! "default razoável" e o override complementa, não substitui
//! silenciosamente. Detalhes em
//! [`registry::DefaultSpecialistRegistry::override_allowed`].

use std::fmt;

use frederico_core::{ModelId, ToolId};
use serde::{Deserialize, Serialize};

/// Identificador de um especialista. String bem-conhecida (não é
/// UUID porque o conjunto é finito e versionado com o app —
/// mesma decisão do `ProviderId` / `ModelId` em
/// [`frederico_core`]).
///
/// **Convenção** (regra do §9.1 do PROMPT MESTRE + ADR-0030 §D1):
/// kebab-case, ASCII, sem espaços. Exemplos bundled: `"revisor"`,
/// `"pesquisador"`, `"testador"`, `"validador"`, `"sumador"`,
/// `"arquiteto"`, `"critico"`, `"executor"`. Override do usuário
/// pode ter IDs próprios (qualquer string que case com
/// `[a-z0-9-]+`), mas o `SpecialistRegistry::get` retorna
/// `Err(RegistryError::UnknownSpecialist { requested, valid })`
/// se o ID não estiver registrado (zero fallback silencioso,
/// §9.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecialistId(pub String);

impl SpecialistId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SpecialistId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SpecialistId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SpecialistId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Sub-orçamento de steps que o subagente pode executar antes
/// de bater no `BudgetEnforcer` do `RunExecutor`. Parte da
/// alocação do pai (ADR-0027 §D5 — `BudgetAllocation`).
///
/// Valor `0` é inválido em runtime (o subagente não conseguiria
/// nem completar a primeira chamada de modelo); o parse tolera
/// mas o `SubagentRunner` da Etapa 4 rejeita com erro
/// estruturado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecialistMaxSteps(pub u32);

impl Default for SpecialistMaxSteps {
    fn default() -> Self {
        // Default razoável: 30 steps. O budget do pai é 50
        // (default da Fase 3 Etapa 4); o subagente recebe uma
        // fração. 30 cobre leitura + raciocínio + 1 tool call +
        // sumarização pra 90% dos casos de uso desenhados no
        // §9.1 do PROMPT MESTRE.
        Self(30)
    }
}

/// Timeout (em ms) do subagente. **Independente do timeout do
/// pai** (decisão da Etapa 1 da Fase 6, documentada em
/// `subagent-architecture.md` §"CancellationToken" como
/// "pendência pra Etapa 4 revisar"). Cada subagente tem o seu;
/// o pai não compartilha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecialistTimeoutMs(pub u32);

impl Default for SpecialistTimeoutMs {
    fn default() -> Self {
        // 5 minutos. Maior que o do pai (10min default da Fase
        // 3) só não porque o subagente é um Especialista
        // pontual, não um agente completo.
        Self(300_000)
    }
}

/// Teto opcional de tokens (input + completion somados) que o
/// subagente pode consumir. `None` = sem teto explícito (o teto
/// do pai continua valendo via `BudgetAllocation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecialistTokenBudget(pub u64);

/// Definição completa de um especialista. Carregada do
/// `default.toml` (bundled) ou do `~/.config/frederico/specialists.toml`
/// (override). Veja o módulo e o [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md)
/// §D1 para o shape exato e as decisões.
///
/// **Defaults explícitos** (todos deny / 0 / None exceto onde
/// marcado): o subagente só tem o que o `default.toml` ou o
/// override **explicitamente** declarar. Default deny é o
/// mesmo princípio do `PermissionSet::default()` do
/// `frederico-tool-registry` (spec `tool-permission-model.md`
/// §"Invariantes").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialistDefinition {
    /// ID bem-conhecido (kebab-case). Único por registry.
    pub id: SpecialistId,

    /// Nome legível pelo usuário. Exibido na UI do Modo
    /// Equipe (Etapa 6) e no dropdown do `SpecialistPicker`
    /// (Etapa 3). Em PT-BR por convenção do projeto.
    pub name: String,

    /// Descrição curta (1-2 frases) do que o especialista
    /// faz. Exibido no tooltip/descrição do `SpecialistPicker`
    /// e na sidebar do Modo Equipe.
    pub description: String,

    /// Propósito completo (parágrafo). Mais longo que a
    /// `description`; é o que o modelo principal lê pra
    /// decidir se delega. Aparece no `system_prompt` do
    /// subagente quando o `SubagentRunner` da Etapa 4 montar o
    /// contexto.
    pub purpose: String,

    /// Modelo default do catálogo (`frederico-model-catalog`).
    /// Resolvido pra `(ProviderId, ModelId)` via
    /// `Catalog::find_model` no momento do spawn (Etapa 4) —
    /// o registry só carrega o `ModelId` porque o mesmo
    /// `ModelId` pode estar disponível em provedores
    /// diferentes (ex.: `"gpt-4o-mini"` em openai e
    /// openrouter).
    pub default_model: ModelId,

    /// Capabilities que o modelo default **precisa ter** pra
    /// esse papel ser exercido. Ex.: `["tools", "json_mode"]`
    /// pra um especialista que valida JSON. O
    /// `DefaultSpecialistRegistry` confere no carregamento e
    /// loga warning se o `default_model` não tiver a
    /// capability — não é hard-fail porque o usuário pode
    /// ter configurado um modelo local sem `tools` por
    /// design.
    pub allowed_model_capabilities: Vec<String>,

    /// Allowlist de `ToolId`s. O subagente só pode chamar
    /// tools nesta lista. **Deny explícito** em
    /// `denied_tools` tem precedência (sobrescreve allow).
    /// Ambos vazios = subagente não pode chamar nenhuma tool
    /// (vai operar só com texto — útil pro `sumador` e o
    /// `critico`).
    pub allowed_tools: Vec<ToolId>,

    /// Denylist de `ToolId`s. Precedência sobre
    /// `allowed_tools`. Útil pra "tudo exceto terminal" sem
    /// ter que listar cada tool individual.
    pub denied_tools: Vec<ToolId>,

    /// Sub-budget de steps. `None` = o subagente herda o
    /// teto do pai direto (sem alocação explícita). Default
    /// razoável: 30. Veja [`SpecialistMaxSteps`].
    #[serde(default)]
    pub max_steps: Option<SpecialistMaxSteps>,

    /// Timeout do subagente em ms. Independente do pai.
    /// Default razoável: 5min. Veja [`SpecialistTimeoutMs`].
    #[serde(default)]
    pub timeout_ms: Option<SpecialistTimeoutMs>,

    /// Teto de tokens (input + completion somados). `None`
    /// = sem teto explícito (o do pai prevalece).
    #[serde(default)]
    pub token_budget: Option<SpecialistTokenBudget>,

    /// Teto de custo em microcents (etapa 4 usa pra
    /// `BudgetAllocation`). `None` = sem teto explícito.
    #[serde(default)]
    pub cost_budget_microcents: Option<u64>,
}

/// Resumo leve de um especialista — o que a UI consome no
/// `SpecialistPicker` e na listagem do Modo Equipe. Carrega só
/// o que é seguro serializar pro frontend (sem campos
/// sensíveis, sem paths internos).
///
/// **Por que existe como tipo separado** (e não como
/// `SpecialistDefinition` com `#[serde(skip)]` nos campos
/// sensíveis): o Tauri command `ListSpecialists` precisa de
/// um tipo de view estável pra serializar — e a serialização
/// do `SpecialistDefinition` direto pra UI exporia
/// `cost_budget_microcents` e outros campos que são
/// decisão de orquestrador, não de UI. Mesma divisão dos
/// outros `*View` do `shared-contracts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialistSummary {
    pub id: SpecialistId,
    pub name: String,
    pub description: String,
    /// Capabilities do `default_model` (não da
    /// `allowed_model_capabilities` — o que o modelo default
    /// declarado **tem**, o que é útil pra UI renderizar
    /// badges "tem tools" / "tem visão" / etc.).
    pub default_model_capabilities: Vec<String>,
    /// `default_model.as_str()` — só o nome pra UI; a
    /// resolução completa provedor+modelo é do
    /// `Catalog::find_model` (Etapa 4).
    pub default_model: String,
    /// Tags de capacidade que a UI pode usar pra filtragem
    /// (ex.: dropdown "só os que têm tools"). Mesmo conteúdo
    /// de `default_model_capabilities` mas com o shape que a
    /// UI prefere.
    pub capability_tags: Vec<String>,
}

impl SpecialistSummary {
    /// Constrói a partir do `SpecialistDefinition` + as
    /// capabilities do `default_model` resolvidas via catálogo
    /// (passadas pelo caller — o registry não conhece o
    /// catálogo, só o definition; quem compõe é o
    /// `build_specialist_registry` em `composition.rs`).
    #[must_use]
    pub fn from_definition(
        def: &SpecialistDefinition,
        default_model_capabilities: Vec<String>,
    ) -> Self {
        Self {
            id: def.id.clone(),
            name: def.name.clone(),
            description: def.description.clone(),
            default_model_capabilities: default_model_capabilities.clone(),
            default_model: def.default_model.as_str().to_string(),
            capability_tags: default_model_capabilities,
        }
    }
}

/// Helper de parse. Desserializa de TOML (formato
/// `[[specialist]]` com `version` no topo) e devolve
/// `Vec<SpecialistDefinition>`.
///
/// **Por que função pública e não método de
/// `DefaultSpecialistRegistry`:** o `default.toml` é embedded
/// no binário pelo `build.rs` e exposto via env var
/// `SPECIALISTS_TOML_PATH` (mesmo padrão do `CATALOG_JSON_PATH`).
/// O `DefaultSpecialistRegistry::load_bundled` consome essa
/// env var. Esta função é a peça pura que o load_bundled
/// chama — testável isoladamente com fixture inline.
pub fn parse_specialists_toml(toml_text: &str) -> Result<Vec<SpecialistDefinition>, String> {
    #[derive(Deserialize)]
    struct SpecialistsFile {
        /// Versão do schema do `default.toml`. Mudanças
        /// incompatíveis exigem migração (mesma política do
        /// `model-catalog`'s `CATALOG_HASH`).
        #[allow(dead_code)]
        version: String,
        #[serde(default)]
        specialist: Vec<SpecialistDefinition>,
    }

    let file: SpecialistsFile =
        toml::from_str(toml_text).map_err(|e| format!("specialists.toml inválido: {e}"))?;
    Ok(file.specialist)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
version = "1"

[[specialist]]
id = "revisor"
name = "Revisor de Código"
description = "Revisa o diff e aponta problemas."
purpose = "Revisão de PR antes de merge."
default_model = "gpt-4o"
allowed_model_capabilities = ["code", "long-context", "tools"]
allowed_tools = ["files.read", "files.list"]
denied_tools = ["terminal"]
max_steps = 30
timeout_ms = 300000
token_budget = 50000

[[specialist]]
id = "sumador"
name = "Sumador"
description = "Sumariza o output de outros especialistas."
purpose = "Síntese final após paralelização."
default_model = "gpt-4o-mini"
allowed_model_capabilities = ["text", "long-context"]
allowed_tools = []
denied_tools = []
"#;

    #[test]
    fn parse_specialists_toml_extracts_definitions() {
        let defs = parse_specialists_toml(FIXTURE).expect("parse ok");
        assert_eq!(defs.len(), 2);

        let r = &defs[0];
        assert_eq!(r.id.as_str(), "revisor");
        assert_eq!(r.default_model.as_str(), "gpt-4o");
        assert_eq!(
            r.allowed_model_capabilities,
            vec!["code", "long-context", "tools"]
        );
        assert_eq!(r.allowed_tools.len(), 2);
        assert_eq!(r.denied_tools, vec![ToolId::new("terminal")]);
        assert_eq!(r.max_steps.expect("max_steps set").0, 30);
        assert_eq!(r.timeout_ms.expect("timeout set").0, 300_000);
        assert_eq!(r.token_budget.expect("token set").0, 50_000);
        assert_eq!(r.cost_budget_microcents, None);

        let s = &defs[1];
        assert_eq!(s.id.as_str(), "sumador");
        assert!(s.allowed_tools.is_empty());
        assert_eq!(s.max_steps, None);
        assert_eq!(s.timeout_ms, None);
    }

    #[test]
    fn parse_specialists_toml_empty_array_is_valid() {
        let toml = r#"
version = "1"
"#;
        let defs = parse_specialists_toml(toml).expect("parse ok");
        assert!(defs.is_empty());
    }

    #[test]
    fn parse_specialists_toml_rejects_invalid_syntax() {
        let bad = r#"
version = "1"
[[specialist]]
id = 123
"#;
        let err = parse_specialists_toml(bad).expect_err("deve falhar");
        assert!(err.contains("specialists.toml inválido"), "msg: {err}");
    }

    #[test]
    fn specialist_id_display() {
        let id = SpecialistId::new("revisor");
        assert_eq!(id.to_string(), "revisor");
        assert_eq!(id.as_str(), "revisor");
    }

    #[test]
    fn specialist_id_from_str() {
        let id: SpecialistId = "pesquisador".into();
        assert_eq!(id.as_str(), "pesquisador");
    }

    #[test]
    fn summary_from_definition_keeps_caller_provided_capabilities() {
        let def = SpecialistDefinition {
            id: SpecialistId::new("revisor"),
            name: "Revisor".into(),
            description: "revisa".into(),
            purpose: "revisar PR".into(),
            default_model: ModelId::new("gpt-4o"),
            allowed_model_capabilities: vec!["tools".into()],
            allowed_tools: vec![],
            denied_tools: vec![],
            max_steps: Some(SpecialistMaxSteps(30)),
            timeout_ms: Some(SpecialistTimeoutMs(300_000)),
            token_budget: Some(SpecialistTokenBudget(50_000)),
            cost_budget_microcents: None,
        };
        let caps = vec!["tools".into(), "vision".into()];
        let summary = SpecialistSummary::from_definition(&def, caps.clone());
        assert_eq!(summary.id, def.id);
        assert_eq!(summary.default_model, "gpt-4o");
        assert_eq!(summary.default_model_capabilities, caps);
        assert_eq!(summary.capability_tags, caps);
    }

    #[test]
    fn defaults_match_expectations() {
        assert_eq!(SpecialistMaxSteps::default().0, 30);
        assert_eq!(SpecialistTimeoutMs::default().0, 300_000);
    }
}
