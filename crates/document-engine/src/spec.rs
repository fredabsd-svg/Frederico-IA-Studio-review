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
/// `0.1.0`. **Etapa 5 bump**: `0.2.0` adiciona o campo
/// `DocumentMetadata.watermark: Option<WatermarkSpec>` (opcional,
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` —
/// backward-compat com 0.1.0). Mudança de catálogo continua MINOR;
/// remoção de campo ou mudança de semântica incompatível seria
/// MAJOR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SpecVersion(pub String);

impl Default for SpecVersion {
    fn default() -> Self {
        // Bump pra 0.2.0 na Etapa 5 (ADR-0021). O JSON Schema gerado
        // em runtime via `schemars` reflete o novo campo
        // `watermark` automaticamente.
        Self("0.2.0".to_string())
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

/// Posição da marca d'água visual na página. Opt-in via
/// `DocumentMetadata.watermark` (`PROMPT MESTRE` §5.3 + §16.5;
/// ADR-0021 §D-PDF2). A combinação com `DocumentStyle::Sobrio` é
/// rejeitada pelo validador (`validate_semantic` regra 8) — modo
/// Sóbrio é para registráveis, e tarja visual atravessando
/// instrumento da Junta é erro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WatermarkPosition {
    /// Centro da página, fonte grande (default 72pt), opacidade baixa.
    /// Caso de uso padrão "CONFIDENCIAL" atravessando a página.
    Center,
    /// Diagonal do canto inferior esquerdo ao superior direito
    /// (rotação 45°). Cobre a página inteira.
    Diagonal,
    /// Canto inferior direito, fonte menor (default 14pt).
    /// Visível mas discreto.
    BottomRight,
    /// Canto superior direito, fonte menor (default 14pt).
    TopRight,
}

/// Especificação da marca d'água visual. Opt-in (D-PDF2 do
/// ADR-0021). O `DocumentSpec.confidentiality` é separado — vai
/// como metadado / cabeçalho / nota de rodapé conforme o `style`.
/// A marca d'água visual é uma camada por cima do conteúdo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WatermarkSpec {
    /// Texto da marca. Ex: "CONFIDENCIAL", "USO INTERNO", "RASCUNHO".
    /// Comprimento máximo recomendado: 32 chars (renderizado em fonte
    /// grande no centro, fica ilegível se passar disso).
    pub text: String,
    /// Posição na página.
    pub position: WatermarkPosition,
    /// Opacidade de 0.0 a 1.0. `None` = 0.15 (visível mas não
    /// obstrutivo). PDF/A-2 aceita transparência; PDF/A-1 não —
    /// irrelevante na v1 (PDF/A-1 não está implementado), mas
    /// registrado.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    /// Tamanho da fonte em pontos. `None` = default conforme
    /// `position` (72pt para Center/Diagonal, 14pt para Corner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

/// Metadados do documento. Não afeta a renderização do
/// **conteúdo** (vai para propriedades do arquivo:
/// `docProps/core.xml` no `.docx`, metadados PDF, etc.) — mas o
/// campo `watermark` (Etapa 5, ADR-0021 §D-PDF2) é renderizado
/// como overlay visual **opt-in** quando o kit suporta. O
/// `DocumentSpec.confidentiality` continua sendo o portador do
/// nível de confidencialidade como metadado / cabeçalho / nota de
/// rodapé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
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
    /// Marca d'água visual **opt-in** (Etapa 5, ADR-0021
    /// §D-PDF2). `None` = sem marca visual (default). O
    /// validador rejeita a combinação com
    /// `DocumentStyle::Sobrio` (`validate_semantic` regra 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<WatermarkSpec>,
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
