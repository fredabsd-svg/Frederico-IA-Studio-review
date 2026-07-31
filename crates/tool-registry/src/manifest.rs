//! O `ToolManifest` e seus enums auxiliares.
//!
//! Espelha o spec `tool-registry-specification.md` §"Contrato do
//! manifesto". Todos os campos são públicos — o manifesto é o
//! **contrato** entre o registro, o validador, o executor e o modelo.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use frederico_core::ToolId;
use serde::{Deserialize, Serialize};

use crate::error::ToolErrorCode;

/// Schema JSON validado por [`jsonschema`]. Mantido como
/// `serde_json::Value` em vez de uma struct tipada porque o schema é
/// arbitrário por ferramenta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSchema(pub serde_json::Value);

impl JsonSchema {
    /// Schema vazio (`true` aceita qualquer coisa). Útil em testes e
    /// para ferramentas que ainda não definiram schema.
    #[must_use]
    pub fn any() -> Self {
        Self(serde_json::json!(true))
    }

    /// Valida um valor contra o schema. Devolve `Err(ToolError)` com
    /// `code = SchemaInvalid` se falhar, incluindo a primeira
    /// mensagem do validador (o `jsonschema` reporta todas as
    /// violações — usamos a primeira para manter o erro curto).
    pub fn validate(&self, value: &serde_json::Value) -> Result<(), crate::error::ToolError> {
        let validator = jsonschema::JSONSchema::compile(&self.0).map_err(|e| {
            crate::error::ToolError::new(
                ToolErrorCode::SchemaInvalid,
                format!("schema de manifesto inválido: {e}"),
            )
        })?;
        if let Err(mut errors) = validator.validate(value) {
            if let Some(first) = errors.next() {
                return Err(crate::error::ToolError::new(
                    ToolErrorCode::SchemaInvalid,
                    format!("valor não casa com o schema: {first}"),
                )
                .with_details(serde_json::Value::String(first.to_string())));
            }
        }
        Ok(())
    }
}

/// Risco declarado da ferramenta. Spec §7.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Leitura de arquivo, busca — sem efeito colateral.
    Safe,
    /// Escrita local, criação de diretório.
    Moderate,
    /// Execução de processo, instalação, mutação irreversível.
    High,
    /// Acesso a credenciais, rede externa, exfiltração.
    Critical,
}

impl RiskLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Moderate => "moderate",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RiskLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "safe" => Ok(Self::Safe),
            "moderate" => Ok(Self::Moderate),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            other => Err(format!("RiskLevel desconhecido: {other:?}")),
        }
    }
}

/// Categoria da ferramenta. Spec §7.1 + §7.11.
/// Lista fechada (REGRAS §1.9 e §"Categorias" do
/// `tool-permission-model.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Files,
    Exec,
    Web,
    GitHub,
    Memory,
    Docs,
    Brasil,
}

impl ToolCategory {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Exec => "exec",
            Self::Web => "web",
            Self::GitHub => "github",
            Self::Memory => "memory",
            Self::Docs => "docs",
            Self::Brasil => "brasil",
        }
    }
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Modo de provisionamento de tool calling do provedor. Spec §7.1.
/// `NativeTools` = o provedor suporta `tools:` nativamente (OpenAI,
/// Anthropic, etc.); `TextEmulation` = o adapter roteiriza o tool
/// calling via texto (experimental, atrás de flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    NativeTools,
    TextEmulation,
}

impl ProviderMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeTools => "native-tools",
            Self::TextEmulation => "text-emulation",
        }
    }
}

/// Plataformas suportadas. Spec §7.1. v1 é Windows-only, mas a enum
/// fica preparada pra v1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

/// Disponibilidade. Spec §7.1. Diferente de `WorkerHealth`:
/// `Availability` é o **estado declarado** pelo manifesto (config),
/// `WorkerHealth` é o estado **observado** em runtime (worker vivo ou
/// morto).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Disponível pra ser chamada.
    Available,
    /// Desligada por configuração (default de ferramenta perigosa é
    /// deny — spec §"Default é deny").
    Disabled,
    /// Dependência ausente (binário não instalado, rede indisponível).
    Missing,
    /// Worker morreu ou healthcheck falhou. Diferente de `Missing`:
    /// a dependência existe, mas está doente.
    Unhealthy,
}

impl Availability {
    #[must_use]
    pub const fn is_callable(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Saúde observada do worker. Spec §7.1. Vem de healthcheck ativo
/// (Etapa 5). `ToolRegistry` ainda não consulta workers na Etapa 2 —
/// o `WorkerHealth::Ok` é o default razoável para ferramentas
/// in-process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealth {
    Ok,
    Degraded,
    Unhealthy,
}

/// O manifesto da ferramenta. Spec §7.1.
///
/// Construído via [`ToolManifestBuilder`] — campos obrigatórios são
/// `id`, `namespace`, `version`, `display_name`, `description`,
/// `input_schema`, `output_schema`, `category`, `risk_level`,
/// `supported_platforms`, `supported_provider_modes`, `timeout_ms`.
/// O resto tem defaults razoáveis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: ToolId,
    pub namespace: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub input_schema: JsonSchema,
    pub output_schema: JsonSchema,
    pub category: ToolCategory,
    pub capabilities: Vec<String>,
    pub risk_level: RiskLevel,
    pub requires_network: bool,
    pub requires_file_read: bool,
    pub requires_file_write: bool,
    pub requires_process_execution: bool,
    pub requires_user_approval: bool,
    pub supported_platforms: Vec<Platform>,
    pub supported_provider_modes: Vec<ProviderMode>,
    pub timeout_ms: u32,
    pub cancellable: bool,
    pub availability: Availability,
    pub health_message: Option<String>,
    /// `None` = executa no app principal; `Some(worker_id)` =
    /// executa em sidecar. Etapa 2 não usa; Etapa 5+ popula.
    pub worker_id: Option<String>,
    /// Allowlist de diretórios nos quais a ferramenta pode
    /// ler/escrever (Etapa 3 da Fase 5).
    ///
    /// Ferramentas in-process (`files.read`, `files.write` etc.)
    /// continuam validadas pelo `Jail` do workspace — mais
    /// estrito. **Ferramentas worker-backed** (a partir do
    /// `docs.generate` da Etapa 3) validam contra esta allowlist
    /// no `WorkerToolDispatcher` antes de chamar
    /// `WorkerHandle::invoke` (defesa em profundidade: o
    /// `validate_path` do Python worker é a barreira mínima; esta
    /// é a barreira forte — o worker é **externo** ao
    /// `frederico-tool-registry` e não pode ser confiado pra
    /// revalidar).
    ///
    /// Default: vazio = sem restrição explícita além da do
    /// worker. Ferramentas que aceitam `path` arbitrário do
    /// chamador **devem** popular este campo no manifesto.
    /// Validação: cada path é canonicalizado em runtime (com
    /// `canonicalize`) e comparado por `starts_with`; caminhos
    /// que não existam são rejeitados (não vale a pena adiar
    /// a checagem para "se o arquivo não existe, deixa").
    pub allowed_paths: Vec<PathBuf>,
}

impl ToolManifest {
    /// `true` se a ferramenta pode ser chamada **neste exato
    /// momento** considerando apenas seu próprio manifesto
    /// (disponibilidade e saúde). Não considera permissões, jail ou
    /// schema — isso é trabalho do `validate_tool_call`.
    #[must_use]
    pub fn is_callable_now(&self, worker_health: WorkerHealth) -> bool {
        self.availability.is_callable() && matches!(worker_health, WorkerHealth::Ok)
    }
}

/// Builder fluente para [`ToolManifest`]. Torna o registro de
/// ferramentas legível: `ToolManifest::builder("files.read",
/// "files").display_name("Ler arquivo").risk_level(RiskLevel::Safe)
///   .input_schema(...).build()`.
pub struct ToolManifestBuilder {
    manifest: ToolManifest,
}

impl ToolManifestBuilder {
    /// Constrói o builder com `id` e `namespace` (campos
    /// obrigatórios). `display_name` e `description` são validados
    /// em [`build`](Self::build) — não Vazios, porque o `description`
    /// é o que o modelo lê.
    pub fn new(id: impl Into<ToolId>, namespace: impl Into<String>) -> Self {
        Self {
            manifest: ToolManifest {
                id: id.into(),
                namespace: namespace.into(),
                version: "0.1.0".to_string(),
                display_name: String::new(),
                description: String::new(),
                input_schema: JsonSchema::any(),
                output_schema: JsonSchema::any(),
                category: ToolCategory::Files,
                capabilities: Vec::new(),
                risk_level: RiskLevel::Safe,
                requires_network: false,
                requires_file_read: false,
                requires_file_write: false,
                requires_process_execution: false,
                requires_user_approval: false,
                supported_platforms: vec![Platform::Windows],
                supported_provider_modes: vec![ProviderMode::NativeTools],
                timeout_ms: 30_000,
                cancellable: true,
                availability: Availability::Available,
                health_message: None,
                worker_id: None,
                allowed_paths: Vec::new(),
            },
        }
    }

    #[must_use]
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.manifest.version = v.into();
        self
    }

    #[must_use]
    pub fn display_name(mut self, name: impl Into<String>) -> Self {
        self.manifest.display_name = name.into();
        self
    }

    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.manifest.description = desc.into();
        self
    }

    #[must_use]
    pub fn input_schema(mut self, schema: JsonSchema) -> Self {
        self.manifest.input_schema = schema;
        self
    }

    #[must_use]
    pub fn output_schema(mut self, schema: JsonSchema) -> Self {
        self.manifest.output_schema = schema;
        self
    }

    #[must_use]
    pub fn category(mut self, cat: ToolCategory) -> Self {
        self.manifest.category = cat;
        self
    }

    #[must_use]
    pub fn risk_level(mut self, level: RiskLevel) -> Self {
        self.manifest.risk_level = level;
        self
    }

    #[must_use]
    pub fn requires_user_approval(mut self, yes: bool) -> Self {
        self.manifest.requires_user_approval = yes;
        self
    }

    #[must_use]
    pub fn requires_file_read(mut self, yes: bool) -> Self {
        self.manifest.requires_file_read = yes;
        self
    }

    #[must_use]
    pub fn requires_file_write(mut self, yes: bool) -> Self {
        self.manifest.requires_file_write = yes;
        self
    }

    #[must_use]
    pub fn requires_network(mut self, yes: bool) -> Self {
        self.manifest.requires_network = yes;
        self
    }

    #[must_use]
    pub fn requires_process_execution(mut self, yes: bool) -> Self {
        self.manifest.requires_process_execution = yes;
        self
    }

    #[must_use]
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.manifest.timeout_ms = ms;
        self
    }

    #[must_use]
    pub fn supported_platforms(mut self, plats: Vec<Platform>) -> Self {
        self.manifest.supported_platforms = plats;
        self
    }

    #[must_use]
    pub fn supported_provider_modes(mut self, modes: Vec<ProviderMode>) -> Self {
        self.manifest.supported_provider_modes = modes;
        self
    }

    #[must_use]
    pub fn capability(mut self, cap: impl Into<String>) -> Self {
        self.manifest.capabilities.push(cap.into());
        self
    }

    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.manifest.availability = Availability::Disabled;
        self
    }

    /// Adiciona um diretório à allowlist de paths da ferramenta
    /// (Etapa 3 da Fase 5). Validação: o path é canonicalizado
    /// em runtime pelo `WorkerToolDispatcher` antes do invoke;
    /// `starts_with` é o teste. Vazio = sem restrição além da
    /// do worker. Use para ferramentas worker-backed que
    /// recebem `path` do chamador (e.g. `docs.generate`).
    #[must_use]
    pub fn allowed_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.manifest.allowed_paths.push(p.into());
        self
    }

    /// Substitui a allowlist de paths inteira. Útil para
    /// carregar a config persistida.
    #[must_use]
    pub fn allowed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.manifest.allowed_paths = paths;
        self
    }

    /// Consome o builder e devolve o manifesto. Falha se
    /// `display_name` ou `description` estiverem vazios.
    pub fn build(self) -> Result<ToolManifest, &'static str> {
        if self.manifest.display_name.trim().is_empty() {
            return Err("display_name não pode ser vazio");
        }
        if self.manifest.description.trim().is_empty() {
            return Err("description não pode ser vazio (é o que o modelo lê)");
        }
        Ok(self.manifest)
    }
}

// Helper: `ToolManifest::into_builder` para reusar nos testes.
impl ToolManifest {
    /// Atalho para `ToolManifestBuilder::new(id, namespace)` —
    /// usado em testes e em código que constrói manifestos.
    #[must_use]
    pub fn builder(id: impl Into<ToolId>, namespace: impl Into<String>) -> ToolManifestBuilder {
        ToolManifestBuilder::new(id, namespace)
    }

    /// Reverte para o builder, preservando os campos atuais. Útil
    /// em testes para "ajustar" um manifesto.
    #[must_use]
    pub fn into_builder(self) -> ToolManifestBuilder {
        ToolManifestBuilder { manifest: self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_minimal_works() {
        let m = ToolManifest::builder(ToolId::new("files.read"), "files")
            .display_name("Ler arquivo")
            .description("Lê o conteúdo de um arquivo dentro do workspace.")
            .requires_file_read(true)
            .build()
            .unwrap();
        assert_eq!(m.id, ToolId::new("files.read"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Safe);
        assert_eq!(m.timeout_ms, 30_000);
        assert!(m.requires_file_read);
    }

    #[test]
    fn builder_rejects_empty_display_name() {
        let r = ToolManifest::builder(ToolId::new("files.read"), "files")
            .description("desc")
            .build();
        assert!(r.is_err());
    }

    #[test]
    fn builder_rejects_empty_description() {
        let r = ToolManifest::builder(ToolId::new("files.read"), "files")
            .display_name("Ler arquivo")
            .build();
        assert!(r.is_err());
    }

    #[test]
    fn is_callable_now_requires_available_and_ok_health() {
        let mut m = ToolManifest::builder("x", "files")
            .display_name("x")
            .description("x")
            .build()
            .unwrap();
        assert!(m.is_callable_now(WorkerHealth::Ok));
        assert!(!m.is_callable_now(WorkerHealth::Degraded));
        m = m.into_builder().disabled().build().unwrap();
        assert!(!m.is_callable_now(WorkerHealth::Ok));
    }

    #[test]
    fn json_schema_any_accepts_everything() {
        let s = JsonSchema::any();
        for v in [
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!(42),
            serde_json::json!("ola"),
            serde_json::json!([1, 2, 3]),
            serde_json::json!({"a": 1}),
        ] {
            s.validate(&v)
                .unwrap_or_else(|e| panic!("schema any falhou: {e}"));
        }
    }

    #[test]
    fn json_schema_object_rejects_non_object() {
        let s = JsonSchema(serde_json::json!({"type": "object"}));
        assert!(s.validate(&serde_json::json!(42)).is_err());
        assert!(s.validate(&serde_json::json!({"a": 1})).is_ok());
    }
}
