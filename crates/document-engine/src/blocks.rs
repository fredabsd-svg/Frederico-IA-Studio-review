//! O `DocumentBlock` e seus sub-tipos — o catálogo de blocos que o
//! `DocumentSpec` aceita. O catálogo é **finito e versionado**:
//! `spec_version: SemVer` no `DocumentSpec` declara qual catálogo o
//! JSON usa. Etapa 1 entrega a v0.1 (20 blocos, derivado do stub em
//! `docs/architecture/document-engine-architecture.md`).
//!
//! Cada bloco é uma `serde`-tagged enum variant; `schemars` gera o
//! JSON Schema de cada sub-tipo automaticamente, e o `build.rs`
//! consolida tudo no arquivo `document_spec.schema.json` versionado
//! (REGRAS §1.9 — gerado vence manual).
//!
//! Invariantes semânticas que **não** cabem no JSON Schema ficam em
//! `validate.rs` (ex: `Kpis` aceita 2 a 4 cartões — schema aceita
//! qualquer array não-vazio).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Capa. Pode ter título, subtítulo, autor, data, e imagem de fundo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cover {
    /// Título principal (obrigatório).
    pub title: String,
    /// Subtítulo (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Autor (opcional — não confundir com `metadata.author`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Data de emissão (string livre — formato depende do `style`;
    /// "Tinta & Latão" usa dd/MM/aaaa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// Item de lista numerada ou com marcadores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListItem {
    /// Texto do item.
    pub text: String,
    /// Sub-itens aninhados (recursivo — JSON Schema trata via
    /// `$ref` cíclico gerado pelo `schemars`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ListItem>,
}

/// Especificação da linha de total de uma tabela.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TotalSpec {
    /// Rótulo do total (ex: "Total geral", "Subtotal").
    pub label: String,
    /// Fórmula ou valor (string — a engine decide como avaliar).
    pub expression: String,
}

/// Cartão de KPI (2 a 4 por bloco `Kpis`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KpiCard {
    /// Rótulo do indicador.
    pub label: String,
    /// Valor (string — números são formatados pela engine conforme
    /// `style` e `locale`).
    pub value: String,
    /// Variação (ex: "+12%" YoY). Opcional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    /// Rótulo do delta (ex: "vs. 2025"). Aparece se `delta` aparece.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_label: Option<String>,
}

/// Tipo de caixa de destaque. Cor/ícone é decidido pelo `style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalloutKind {
    /// Informação neutra (azul discreto).
    Info,
    /// Alerta (amarelo).
    Alert,
    /// Crítico (vermelho, sem emoji).
    Critical,
    /// Sucesso (verde).
    Success,
}

/// Citação com atribuição.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Quote {
    /// Texto citado.
    pub text: String,
    /// Atribuição (ex: "Maria Souza, CEO da Acme").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// Passo numerado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Step {
    /// Título do passo.
    pub title: String,
    /// Descrição (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Série de dados de um gráfico.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChartSeries {
    /// Nome da série.
    pub name: String,
    /// Pontos — string para acomodar moeda, percentual e número puro
    /// com a formatação que a engine aplica.
    pub values: Vec<String>,
}

/// Tipo de gráfico. `Bar` inclui vertical e horizontal (a engine
/// decide pela proporção altura/largura ou por flag explícita).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    /// Barras (vertical ou horizontal — decidido pela engine).
    Bar,
    /// Linha.
    Line,
    /// Pizza.
    Pie,
}

/// Imagem inline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImageBlock {
    /// Caminho da imagem no workspace (jail aplicado pelo executor).
    pub path: String,
    /// Texto alternativo (obrigatório para acessibilidade).
    pub alt: String,
    /// Legenda exibida abaixo da imagem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Largura em centímetros (opcional — engine escolhe se ausente).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_cm: Option<f32>,
}

/// Bloco de código com syntax highlight opcional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodeBlock {
    /// Linguagem (ex: "rust", "python", "sql"). `None` = sem highlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Conteúdo do código (preserva indentação e caracteres).
    pub content: String,
    /// Rótulo exibido acima do bloco (ex: nome do arquivo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Par de assinatura (quem assina + função).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignaturePair {
    /// Nome de quem assina.
    pub name: String,
    /// Função/cargo (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Local (ex: "São Paulo - SP"). Opcional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// Contato para a contracapa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContactInfo {
    /// Nome (pessoa ou organização).
    pub name: String,
    /// E-mail (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Telefone (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Endereço (opcional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Tipo de marca de confidencialidade. A engine renderiza a marca
/// conforme o `style` ("Tinta & Latão" usa cabeçalho destacado;
/// "Sóbrio" usa nota de rodapé sem cor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfidentialityLevel {
    /// Documento público.
    Public,
    /// Interno da organização.
    Internal,
    /// Confidencial — distribuição restrita.
    Confidential,
    /// Sigiloso — manuseio sob controle.
    Restricted,
}

/// Marca de confidencialidade anexada ao documento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfidentialityMark {
    /// Nível de confidencialidade.
    pub level: ConfidentialityLevel,
    /// Texto adicional exibido junto da marca (ex: "Auditoria —
    /// uso restrito").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// O catálogo de blocos do `DocumentSpec` v0.1.
///
/// É o coração do formato — o que o modelo emite em `DocumentSpec` é
/// uma lista de variantes desta enum. Toda variante vira uma seção
/// renderizada por algum dos três kits (Word, Excel, PDF) na Etapa
/// 3+. A enum é `#[serde(tag = "type")]` para que o JSON seja
/// auto-descritivo (`{"type": "heading", "level": 1, "text": "..."}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentBlock {
    /// Capa do documento.
    Cover(Cover),
    /// Sumário (gerado em duas passadas — `PROMPT MESTRE` §16.4).
    Toc,
    /// Cabeçalho de seção. `level` 1-3 (4+ cai pra heading 3 com nota).
    Heading {
        /// Nível 1-3.
        level: u8,
        /// Texto do cabeçalho.
        text: String,
        /// Numeração explícita (ex: "3.2"). Opcional.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        number: Option<String>,
    },
    /// Parágrafo de texto.
    Paragraph {
        /// Conteúdo do parágrafo.
        text: String,
        /// Nome de estilo opcional (ex: "Body", "Lead"). Se ausente,
        /// usa o estilo padrão do `style` corrente.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    /// Lista numerada ou com marcadores.
    List {
        /// `true` = numerada; `false` = marcadores.
        ordered: bool,
        /// Itens da lista.
        items: Vec<ListItem>,
    },
    /// Tabela.
    Table {
        /// Cabeçalhos das colunas.
        headers: Vec<String>,
        /// Linhas de dados (cada linha = uma entrada).
        rows: Vec<Vec<String>>,
        /// Linha de total opcional.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<TotalSpec>,
        /// Código de moeda (ex: "BRL") — afeta formatação.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        currency: Option<String>,
        /// `true` = formata colunas numéricas como percentual.
        #[serde(default)]
        percent: bool,
        /// `true` = aplica separador de milhar.
        #[serde(default)]
        thousands: bool,
        /// Título exibido acima da tabela.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Fonte dos dados (ex: "Contabilidade 2025"). Opcional.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// Tabela de chave-valor (ex: contrato, especificação).
    KeyValue {
        /// Lista de pares (chave, valor).
        entries: Vec<(String, String)>,
    },
    /// Conjunto de 2 a 4 KPIs. Regra semântica: 2 ≤ items.len() ≤ 4
    /// (validado em `validate.rs`).
    Kpis {
        /// Cartões. Regra: 2 a 4.
        items: Vec<KpiCard>,
    },
    /// Caixa de destaque.
    Callout {
        /// Tipo de caixa.
        kind: CalloutKind,
        /// Texto.
        text: String,
    },
    /// Citação.
    Quote(Quote),
    /// Passos numerados.
    Steps {
        /// Lista de passos (≥ 1).
        items: Vec<Step>,
    },
    /// Gráfico.
    Chart {
        /// Tipo de gráfico.
        kind: ChartKind,
        /// Rótulos do eixo X (ou categorias, no caso de pizza).
        labels: Vec<String>,
        /// Séries (≥ 1).
        series: Vec<ChartSeries>,
        /// Título exibido acima.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Imagem.
    Image(ImageBlock),
    /// Código.
    Code(CodeBlock),
    /// Linha horizontal.
    Divider,
    /// Espaço vertical (em centímetros).
    #[allow(clippy::derive_partial_eq_without_eq)]
    Spacer {
        /// Altura em cm.
        height_cm: f32,
    },
    /// Quebra de página.
    PageBreak,
    /// Rodapé (repetido em todas as páginas a partir do bloco).
    Footer {
        /// Texto do rodapé.
        text: String,
        /// `true` = numera as páginas ("1 / 12").
        #[serde(default)]
        page_numbers: bool,
    },
    /// Bloco de assinaturas.
    Signatures {
        /// Pares (nome, função, local).
        pairs: Vec<SignaturePair>,
    },
    /// Contracapa com contatos.
    BackCover {
        /// Contatos exibidos.
        contacts: ContactInfo,
    },
}
