//! O `DocumentSpec` — o **contrato** entre o modelo e os kits
//! (Word, Excel, PDF). O modelo emite um `DocumentSpec`; os kits
//! consomem. O JSON Schema gerado em `build.rs` é a fonte da verdade
//! do formato.
//!
//! `DocumentSpec` é **declarativo**: o modelo **não escreve código
//! de diagramação** (`PROMPT MESTRE` §16.4 e §16.5). O que o modelo
//! faz é descrever a estrutura; a engine decide tipografia, cores,
//! paginação, etc., conforme o `style`.
//!
//! ## Versionamento
//!
//! `spec_version` é `SemVer` e versiona o **catálogo de blocos** e as
//! **regras semânticas**. Mudar uma regra semântica é bump de MINOR
//! (v0.1 → v0.2). Adicionar bloco é MINOR. Remover bloco é MAJOR.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::blocks::DocumentBlock;

/// A versão do esquema (`SemVer`) que o JSON usa.
///
/// Aceita qualquer string no formato `MAJOR.MINOR.PATCH` — a
/// comparação é lexicográfica por componente. A Etapa 1 define a
/// `0.1.0`; bumps futuros acontecem quando o catálogo ou as regras
/// semânticas mudam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SpecVersion(pub String);

impl Default for SpecVersion {
    fn default() -> Self {
        Self("0.1.0".to_string())
    }
}

/// Tipo de documento. A engine pode usar isto para escolher
/// templates, cabeçalhos/rodapés padrão, e regras de paginação
/// (ex: "spreadsheet" é por planilha, não por seção).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentType {
    /// Relatório executivo.
    Report,
    /// Memorando interno.
    Memo,
    /// Contrato.
    Contract,
    /// Planilha (Excel/CSV).
    Spreadsheet,
    /// Proposta comercial.
    Proposal,
    /// Parecer técnico.
    TechnicalOpinion,
    /// Procuração.
    PowerOfAttorney,
    /// Ofício.
    OfficialLetter,
    /// Comunicado.
    Announcement,
    /// Manual / documentação.
    Manual,
    /// Documento para apresentação (slide).
    Presentation,
    /// Genérico (usado quando o tipo não é nenhum dos acima).
    Generic,
}

/// Identidade visual. "Tinta & Latão" é a identidade padrão do
/// Frederico IA Studio (`PROMPT MESTRE` §16.3). "Sóbrio" é o modo
/// para documentos registráveis (contratos, procurações, ofícios) —
/// sem cor, sem ornamento, com tipografia conservadora
/// (`PROMPT MESTRE` §16.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStyle {
    /// Identidade visual padrão do app — fontes Source Serif 4 +
    /// Source Sans 3, paleta com azul escuro / verde de sucesso /
    /// cinza claro / branco (`PROMPT MESTRE` §18.2).
    #[default]
    #[serde(alias = "tinta_e_latao", alias = "tinta-e-latao")]
    TintaELatao,
    /// Modo para registráveis — sem cor, tipografia conservadora,
    /// sem ornamento.
    Sobrio,
}

/// Metadados do documento. Não afeta a renderização — vão para
/// propriedades do arquivo (`docProps/core.xml` no `.docx`,
/// metadados PDF, etc.) e para o `DocumentMetadataView` que o
/// `docs.inspect` devolve na Etapa 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct DocumentMetadata {
    /// Título (vai pra `<dc:title>` no `.docx` e `/Title` no PDF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Autor (`<dc:creator>` / `/Author`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Organização (`<cp:company>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// Palavras-chave separadas por vírgula.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// Comentário / descrição.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// O `DocumentSpec` — o contrato raiz.
///
/// Invariantes semânticas (validadas em `validate.rs`):
///
/// 1. `blocks` não pode ser vazio.
/// 2. Se `doc_type == Spreadsheet`, todos os blocos devem ser de um
///    subconjunto compatível com Excel (`Table`, `Kpis`, `Chart`).
///    Bloqueios fora desse subconjunto são rejeitados — o `DocumentSpec`
///    é "por documento", não "por planilha", mas o kit Excel
///    precisa dessa restrição pra saber o que renderizar.
///    Detalhamento na Etapa 4 quando o ExcelPro entra.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentSpec {
    /// Versão do esquema que o JSON usa. Etapa 1: `0.1.0`.
    pub spec_version: SpecVersion,

    /// Tipo do documento.
    pub doc_type: DocumentType,

    /// Identidade visual. Default: `tinta_e_latao`.
    #[serde(default)]
    pub style: DocumentStyle,

    /// Idioma do documento (BCP-47 — ex: "pt-BR", "en-US"). Default:
    /// `pt-BR` (única locale da v1 — `PROMPT MESTRE` §5.4 e
    /// `development-roadmap.md` §"Adiamentos").
    #[serde(default = "default_language")]
    pub language: String,

    /// Lista ordenada de blocos. ≥ 1.
    pub blocks: Vec<DocumentBlock>,

    /// Metadados (opcional).
    #[serde(default)]
    pub metadata: DocumentMetadata,

    /// Marca de confidencialidade (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidentiality: Option<crate::blocks::ConfidentialityMark>,
}

fn default_language() -> String {
    "pt-BR".to_string()
}
