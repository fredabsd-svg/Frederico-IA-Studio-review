//! `WordProKit` — o kit de Word da Etapa 3 da Fase 5.
//!
//! ## Escopo v0.1 ("WordPro mínimo")
//!
//! Esta versão entrega a **ponte** entre o `DocumentSpec` e o
//! handler `docx.write` do `document-worker` Python. O
//! contrato do handler é:
//!
//! ```json
//! {
//!   "path": "C:\\...\\out.docx",
//!   "title": "string",
//!   "sections": [
//!     { "heading": "string", "paragraphs": ["string", ...] },
//!     ...
//!   ]
//! }
//! ```
//!
//! O kit traduz cada bloco do `DocumentSpec` para uma
//! combinação `heading` + `paragraphs` no payload. Blocos
//! que o `docx.write` **não tem** cobertura direta (Tabela,
//! Kpis, Chart, Image, Code, ...) caem num **fallback de
//! texto formatado** (ex: tabela vira linhas
//! tab-separadas). Honesto: o kit **não finge** que renderiza
//! uma planilha dentro do Word — ele produz texto que o
//! humano consegue ler. A fidelidade tipográfica (cores,
//! grade, imagens embutidas) é trabalho do `python-docx`
//! **estendido** que entra em uma etapa futura (Etapa 6 —
//! identidade visual).
//!
//! ## ADR-0018 §Decisão 1
//!
//! O `document-worker` v0.3.0 é uma **biblioteca de
//! primitivas de I/O**. Os 7 handlers sobrevivem à Etapa 3
//! sem reescrita — o kit é o tradutor. Esta é a forma
//! "handler = primitiva, kit = renderer" travada no ADR.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use frederico_document_engine::{DocumentError, DocumentSpec};
use frederico_process_architecture::WorkerHandle;
use frederico_tool_registry::{
    JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder,
};
use serde_json::{json, Value};

use crate::format::DocumentFormat;
use crate::kit::{Kit, KitError, KitOutput};

/// Tradução **pura** (sem I/O) de `DocumentSpec` → payload
/// do handler `docx.write` do `document-worker` v0.3.0.
///
/// Função livre (não método) para ser testável sem
/// `WorkerHandle`. O `WordProKit::translate` é wrapper fino
/// em torno desta.
///
/// ## Contrato do payload gerado
///
/// ```json
/// {
///   "capability": "docx.write",
///   "path": "string",
///   "title": "string",
///   "sections": [
///     { "heading": "string", "paragraphs": ["string", ...] },
///     ...
///   ]
/// }
/// ```
///
/// Espelha o que `external_doc_worker.rs:307-314` envia pro
/// worker real. Mudou o payload aqui, tem que mudar o teste
/// E2E (e vice-versa).
///
/// ## Algoritmo
///
/// Walk pelos blocos:
/// - `Cover`: o `title` do payload vem do
///   `metadata.title` ou do `Cover.title` (prioridade).
///   `subtitle` vira primeiro parágrafo.
/// - `Heading { text, .. }`: fecha a seção corrente (se
///   tiver conteúdo) e abre nova seção com esse heading.
/// - `PageBreak`: idem, mas sem heading.
/// - `Paragraph`, `List`, `Table`, `Kpis`, `Callout`,
///   `Quote`, `Steps`, `Chart`, `Image`, `Code`,
///   `Divider`, `Spacer`, `Footer`, `Signatures`,
///   `BackCover`, `Toc`, `KeyValue`: vão pra
///   `paragraphs` da seção corrente.
pub fn translate_spec_to_docx_payload(
    spec: &DocumentSpec,
    output_path: &Path,
) -> Result<Value, DocumentError> {
    let mut sections: Vec<Value> = Vec::new();
    let mut current_paragraphs: Vec<String> = Vec::new();
    let mut current_heading: Option<String> = None;

    let flush =
        |sections: &mut Vec<Value>, heading: &mut Option<String>, paragraphs: &mut Vec<String>| {
            if heading.is_some() || !paragraphs.is_empty() {
                sections.push(json!({
                    "heading": heading.take().unwrap_or_default(),
                    "paragraphs": std::mem::take(paragraphs),
                }));
            }
        };

    for block in &spec.blocks {
        match block {
            frederico_document_engine::DocumentBlock::Cover(c) => {
                if let Some(sub) = &c.subtitle {
                    current_paragraphs.push(sub.clone());
                }
            }
            frederico_document_engine::DocumentBlock::Heading {
                level: _,
                text,
                number: _,
            } => {
                flush(&mut sections, &mut current_heading, &mut current_paragraphs);
                current_heading = Some(text.clone());
            }
            frederico_document_engine::DocumentBlock::Paragraph { text, style: _ } => {
                current_paragraphs.push(text.clone());
            }
            frederico_document_engine::DocumentBlock::List { ordered: _, items } => {
                // Lista vira UMA paragraph com items joined
                // por "\n". O `docx.write` da v0.3.0 não
                // renderiza bullets tipográficos; `- ` é o
                // fallback legível.
                let joined: Vec<String> = items.iter().map(|i| format!("- {}", i.text)).collect();
                current_paragraphs.push(joined.join("\n"));
            }
            frederico_document_engine::DocumentBlock::Table {
                headers,
                rows,
                total: _,
                currency: _,
                percent: _,
                thousands: _,
                title,
                source: _,
            } => {
                let title_line = title.clone().unwrap_or_else(|| "Tabela".to_string());
                let header_line = headers.join("\t");
                let row_lines: Vec<String> = rows
                    .iter()
                    .map(|r| r.iter().map(|c| c.as_str()).collect::<Vec<_>>().join("\t"))
                    .collect();
                let block = format!("{title_line}\n{header_line}\n{}", row_lines.join("\n"));
                current_paragraphs.push(block);
            }
            frederico_document_engine::DocumentBlock::Kpis { items } => {
                for kpi in items {
                    let mut line = format!("{}: {}", kpi.label, kpi.value);
                    if let Some(delta) = &kpi.delta {
                        line.push_str(&format!(" ({delta}"));
                        if let Some(dl) = &kpi.delta_label {
                            line.push_str(&format!(" {dl}"));
                        }
                        line.push(')');
                    }
                    current_paragraphs.push(line);
                }
            }
            frederico_document_engine::DocumentBlock::Callout { kind, text } => {
                let prefix = match kind {
                    frederico_document_engine::CalloutKind::Info => "[INFO]",
                    frederico_document_engine::CalloutKind::Alert => "[ALERTA]",
                    frederico_document_engine::CalloutKind::Critical => "[CRÍTICO]",
                    frederico_document_engine::CalloutKind::Success => "[OK]",
                };
                current_paragraphs.push(format!("{prefix} {text}"));
            }
            frederico_document_engine::DocumentBlock::Quote(q) => {
                let mut line = format!("\"{}\"", q.text);
                if let Some(a) = &q.attribution {
                    line.push_str(&format!(" — {a}"));
                }
                current_paragraphs.push(line);
            }
            frederico_document_engine::DocumentBlock::Steps { items } => {
                for (i, step) in items.iter().enumerate() {
                    current_paragraphs.push(format!("{}. {}", i + 1, step.title));
                    if let Some(desc) = &step.description {
                        current_paragraphs.push(format!("   {desc}"));
                    }
                }
            }
            frederico_document_engine::DocumentBlock::Chart { .. } => {
                current_paragraphs
                    .push("[Gráfico: ver PDF/Excel para representação visual]".to_string());
            }
            frederico_document_engine::DocumentBlock::Image(img) => {
                if let Some(cap) = &img.caption {
                    current_paragraphs.push(format!("[Imagem: {cap}]"));
                } else {
                    current_paragraphs.push(format!("[Imagem: {}]", img.alt));
                }
            }
            frederico_document_engine::DocumentBlock::Code(c) => {
                for line in c.content.lines() {
                    current_paragraphs.push(format!("    {line}"));
                }
            }
            frederico_document_engine::DocumentBlock::Divider => {
                current_paragraphs.push("---".to_string());
            }
            frederico_document_engine::DocumentBlock::Spacer { height_cm: _ } => {
                current_paragraphs.push(String::new());
            }
            frederico_document_engine::DocumentBlock::PageBreak => {
                flush(&mut sections, &mut current_heading, &mut current_paragraphs);
            }
            frederico_document_engine::DocumentBlock::Footer {
                text,
                page_numbers: _,
            } => {
                current_paragraphs.push(format!("[Rodapé: {text}]"));
            }
            frederico_document_engine::DocumentBlock::Signatures { pairs } => {
                for p in pairs {
                    current_paragraphs.push(String::new());
                    current_paragraphs.push("___________________________".to_string());
                    current_paragraphs.push(p.name.clone());
                    if let Some(role) = &p.role {
                        current_paragraphs.push(role.clone());
                    }
                }
            }
            frederico_document_engine::DocumentBlock::BackCover { contacts } => {
                current_paragraphs.push(contacts.name.clone());
                if let Some(email) = &contacts.email {
                    current_paragraphs.push(email.clone());
                }
                if let Some(phone) = &contacts.phone {
                    current_paragraphs.push(phone.clone());
                }
                if let Some(addr) = &contacts.address {
                    current_paragraphs.push(addr.clone());
                }
            }
            frederico_document_engine::DocumentBlock::Toc => {
                current_paragraphs.push("[Sumário: disponível em versão futura]".to_string());
            }
            frederico_document_engine::DocumentBlock::KeyValue { entries } => {
                for (k, v) in entries {
                    current_paragraphs.push(format!("{k}: {v}"));
                }
            }
        }
    }
    flush(&mut sections, &mut current_heading, &mut current_paragraphs);

    let title = spec
        .metadata
        .title
        .clone()
        .or_else(|| {
            spec.blocks.iter().find_map(|b| match b {
                frederico_document_engine::DocumentBlock::Cover(c) => Some(c.title.clone()),
                _ => None,
            })
        })
        .unwrap_or_else(|| format!("Documento {}", spec.spec_version.0));

    Ok(json!({
        "capability": "docx.write",
        "path": output_path.to_string_lossy(),
        "title": title,
        "sections": sections,
    }))
}

/// `WordProKit` v0.1 — entrega mínima da Fase 5 Etapa 3.
///
/// `Arc<WorkerHandle>` interno: o kit chama o worker
/// diretamente. O `DocsGenerateTool` **também** tem o
/// handle (pra validar paths antes), mas a chamada final é
/// do kit — a fronteira do "eu sou o tradutor + chamador"
/// fica clara aqui.
pub struct WordProKit {
    handle: Arc<WorkerHandle>,
    manifest: ToolManifest,
}

impl WordProKit {
    /// Cria o kit. O `handle` é o `WorkerHandle` do
    /// `document-worker` (clonado do `AppState`).
    #[must_use]
    pub fn new(handle: Arc<WorkerHandle>) -> Self {
        Self {
            handle,
            manifest: Self::build_manifest(),
        }
    }

    /// Traduz o `DocumentSpec` para o payload do
    /// `docx.write`. Função **pura** (sem I/O) — testável
    /// sem worker.
    ///
    /// Blocos com cobertura direta (`Cover`, `Heading`,
    /// `Paragraph`, `List`, `Callout`, `Quote`, `Steps`,
    /// `Code`, `Divider`, `Spacer`, `PageBreak`,
    /// `Signatures`, `BackCover`): viram `sections` no
    /// payload. Blocos sem cobertura direta (`Table`,
    /// `Kpis`, `Chart`, `Image`, `Toc`, `Footer`): viram
    /// representações textuais no `paragraphs` da seção
    /// corrente (ou são pulados, com `tracing::warn!`).
    ///
    /// Estratégia: caminhamos pelos blocos; `Heading` e
    /// `PageBreak` abrem uma nova seção; o resto vai pra
    /// `paragraphs` da seção corrente.
    pub fn translate(
        &self,
        spec: &DocumentSpec,
        output_path: &Path,
    ) -> Result<Value, DocumentError> {
        translate_spec_to_docx_payload(spec, output_path)
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new("docs.generate", "docs")
            .version("0.1.0")
            .display_name("Gerar documento")
            .description(
                "Gera um documento profissional (.docx) a partir de um DocumentSpec \
                 declarativo. Recebe o spec validado e delega ao kit apropriado. \
                 v0.1: WordPro apenas (Etapa 3 da Fase 5). ExcelPro e PDFPro \
                 serão adicionados em Etapas 4 e 5.",
            )
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Moderate)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "spec": {
                        "type": "object",
                        "description": "DocumentSpec validado (schema + regras semânticas). \
                                        O schema detalhado é gerado pelo document-engine."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Caminho absoluto do arquivo .docx a ser gerado. \
                                        Deve estar dentro de um diretório permitido \
                                        (validado pelo ToolManifest.allowed_paths)."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["docx"],
                        "description": "Formato do documento. O enum é gerado a partir \
                                        dos kits implementados — v0.1 contém apenas 'docx'."
                    }
                },
                "required": ["spec", "output_path", "format"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path final do arquivo."},
                    "size_bytes": {"type": "integer", "description": "Tamanho do arquivo gerado."},
                    "format": {"type": "string", "description": "Formato (eco do input)."},
                    "sections_written": {"type": "integer", "description": "Número de seções gravadas."}
                },
                "required": ["path", "size_bytes", "format", "sections_written"]
            })))
            .requires_file_write(true)
            .capability("docx.write")
            .capability("document.generate")
            .timeout_ms(30_000)
            .build()
            .expect("manifesto de docs.generate bem-formado")
    }
}

#[async_trait]
impl Kit for WordProKit {
    fn id(&self) -> &str {
        "wordpro"
    }

    fn target_format(&self) -> DocumentFormat {
        DocumentFormat::Docx
    }

    fn is_implemented(&self) -> bool {
        true
    }

    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn render(&self, spec: &DocumentSpec, output_path: &Path) -> Result<KitOutput, KitError> {
        // 1. Traduz spec → payload do handler.
        let payload = self.translate(spec, output_path)?;

        // 2. Chama o worker. O `capability` está no payload
        // (`"capability": "docx.write"`). O `WorkerHandle`
        // é opaco.
        let response = self
            .handle
            .invoke(payload)
            .await
            .map_err(KitError::Process)?;

        // 3. Traduz a response → KitOutput.
        // O `docx.write` devolve `{ok, path, size_bytes,
        // sections_written, total_paragraphs}`.
        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let code = response
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(sem mensagem)");
            return Err(KitError::Worker(format!(
                "docx.write falhou: {code} — {message}"
            )));
        }

        let path = response
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| output_path.to_path_buf());
        let size_bytes = response
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sections_written = response
            .get("sections_written")
            .cloned()
            .unwrap_or(json!(0));

        Ok(KitOutput {
            path,
            size_bytes,
            format: DocumentFormat::Docx,
            extra: json!({ "sections_written": sections_written }),
            // WordPro v0.1 nao produz sheets (modelo
            // de workbook) nem tem warnings. v0.1
            // cobre todos os 20 blocos com fallback
            // textual onde o `docx.write` v0.3.0 nao tem
            // cobertura direta (Tabela vira texto
            // tab-separado, etc.).
            sheets: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_document_engine::{
        CalloutKind, ChartKind, ChartSeries, CodeBlock, ContactInfo, Cover, DocumentBlock,
        DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, ImageBlock, KpiCard, ListItem,
        SignaturePair, SpecVersion, Step,
    };

    /// Helper: cria um spec vazio.
    fn empty_spec() -> DocumentSpec {
        DocumentSpec {
            spec_version: SpecVersion::default(),
            doc_type: DocumentType::Report,
            style: DocumentStyle::default(),
            language: "pt-BR".to_string(),
            blocks: vec![],
            metadata: DocumentMetadata::default(),
            confidentiality: None,
        }
    }

    fn out_path() -> std::path::PathBuf {
        std::env::temp_dir().join("wordpro_test.docx")
    }

    /// Helper: extrai a lista de seções do payload.
    fn sections(payload: &Value) -> Vec<Value> {
        payload
            .get("sections")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default()
    }

    fn paragraphs(section: &Value) -> Vec<String> {
        section
            .get("paragraphs")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---- Smoke --------------------------------------------------------

    #[test]
    fn empty_spec_produces_no_sections_but_has_title() {
        let spec = empty_spec();
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        assert_eq!(p["capability"], json!("docx.write"));
        // Bump 0.2.0 -> 0.3.0 na Etapa 5 (ADR-0021): o teste
        // verifica o default de `SpecVersion` (0.3.0 desde o
        // PR 3). Se voltar pra 0.2.0 por algum motivo, este
        // teste pega.
        assert_eq!(p["title"], json!("Documento 0.3.0"));
        assert!(sections(&p).is_empty());
    }

    #[test]
    fn title_priority_metadata_over_cover() {
        let mut spec = empty_spec();
        spec.metadata.title = Some("Do Metadata".to_string());
        spec.blocks.push(DocumentBlock::Cover(Cover {
            title: "Do Cover".to_string(),
            subtitle: None,
            author: None,
            date: None,
        }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        assert_eq!(p["title"], json!("Do Metadata"));
    }

    #[test]
    fn title_falls_back_to_cover() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Cover(Cover {
            title: "Do Cover".to_string(),
            subtitle: None,
            author: None,
            date: None,
        }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        assert_eq!(p["title"], json!("Do Cover"));
    }

    // ---- Cobertura direta --------------------------------------------

    #[test]
    fn heading_opens_new_section() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Heading {
            level: 1,
            text: "Intro".to_string(),
            number: None,
        });
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "p1".to_string(),
            style: None,
        });
        spec.blocks.push(DocumentBlock::Heading {
            level: 2,
            text: "Detalhe".to_string(),
            number: None,
        });
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "p2".to_string(),
            style: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["heading"], json!("Intro"));
        assert_eq!(paragraphs(&s[0]), vec!["p1"]);
        assert_eq!(s[1]["heading"], json!("Detalhe"));
        assert_eq!(paragraphs(&s[1]), vec!["p2"]);
    }

    #[test]
    fn page_break_opens_new_section_without_heading() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "p1".to_string(),
            style: None,
        });
        spec.blocks.push(DocumentBlock::PageBreak);
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "p2".to_string(),
            style: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["heading"], json!(""));
        assert_eq!(paragraphs(&s[0]), vec!["p1"]);
        assert_eq!(s[1]["heading"], json!(""));
        assert_eq!(paragraphs(&s[1]), vec!["p2"]);
    }

    #[test]
    fn list_joins_items_with_bullets() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::List {
            ordered: false,
            items: vec![
                ListItem {
                    text: "um".to_string(),
                    children: vec![],
                },
                ListItem {
                    text: "dois".to_string(),
                    children: vec![],
                },
            ],
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        assert_eq!(s.len(), 1);
        let paras = paragraphs(&s[0]);
        assert_eq!(paras.len(), 1);
        // Lista vira UMA paragraph com items separados por \n.
        assert!(paras[0].contains("- um"));
        assert!(paras[0].contains("- dois"));
    }

    #[test]
    fn callout_prefixes_kind() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Callout {
            kind: CalloutKind::Alert,
            text: "Cuidado!".to_string(),
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        let paras = paragraphs(&s[0]);
        assert!(paras[0].contains("[ALERTA]"));
        assert!(paras[0].contains("Cuidado!"));
    }

    #[test]
    fn quote_with_attribution() {
        let mut spec = empty_spec();
        spec.blocks
            .push(DocumentBlock::Quote(frederico_document_engine::Quote {
                text: "Penso, logo existo.".to_string(),
                attribution: Some("Descartes".to_string()),
            }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert_eq!(paras[0], "\"Penso, logo existo.\" — Descartes");
    }

    #[test]
    fn steps_numbered() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Steps {
            items: vec![
                Step {
                    title: "Primeiro".to_string(),
                    description: Some("faça X".to_string()),
                },
                Step {
                    title: "Segundo".to_string(),
                    description: None,
                },
            ],
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert_eq!(paras, vec!["1. Primeiro", "   faça X", "2. Segundo"]);
    }

    #[test]
    fn code_indents_lines() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Code(CodeBlock {
            language: Some("rust".to_string()),
            content: "fn main() {\n    println!(\"hi\");\n}".to_string(),
            caption: None,
        }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert_eq!(
            paras,
            vec!["    fn main() {", "        println!(\"hi\");", "    }"]
        );
    }

    // ---- Fallback textual --------------------------------------------

    #[test]
    fn table_renders_as_tab_separated() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Table {
            headers: vec!["Mês".to_string(), "Total".to_string()],
            rows: vec![
                vec!["Jan".to_string(), "100".to_string()],
                vec!["Fev".to_string(), "200".to_string()],
            ],
            total: None,
            currency: None,
            percent: false,
            thousands: false,
            title: Some("Vendas 2026".to_string()),
            source: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        let block = &paras[0];
        assert!(block.contains("Vendas 2026"));
        assert!(block.contains("Mês\tTotal"));
        assert!(block.contains("Jan\t100"));
        assert!(block.contains("Fev\t200"));
    }

    #[test]
    fn kpis_with_delta() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Kpis {
            items: vec![
                KpiCard {
                    label: "Receita".to_string(),
                    value: "R$ 1M".to_string(),
                    delta: Some("+12%".to_string()),
                    delta_label: Some("vs 2025".to_string()),
                },
                KpiCard {
                    label: "Margem".to_string(),
                    value: "30%".to_string(),
                    delta: None,
                    delta_label: None,
                },
            ],
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert_eq!(paras[0], "Receita: R$ 1M (+12% vs 2025)");
        assert_eq!(paras[1], "Margem: 30%");
    }

    #[test]
    fn chart_placeholder() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Chart {
            kind: ChartKind::Bar,
            labels: vec!["A".to_string()],
            series: vec![ChartSeries {
                name: "S1".to_string(),
                values: vec!["10".to_string()],
            }],
            title: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras[0].contains("Gráfico"));
    }

    #[test]
    fn image_fallback_uses_caption_then_alt() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Image(ImageBlock {
            path: "/x.png".to_string(),
            alt: "alt-text".to_string(),
            caption: Some("legenda".to_string()),
            width_cm: None,
        }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras[0].contains("[Imagem: legenda]"));

        spec = empty_spec();
        spec.blocks.push(DocumentBlock::Image(ImageBlock {
            path: "/x.png".to_string(),
            alt: "alt-only".to_string(),
            caption: None,
            width_cm: None,
        }));
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras[0].contains("[Imagem: alt-only]"));
    }

    #[test]
    fn toc_placeholder() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Toc);
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras[0].contains("Sumário"));
    }

    #[test]
    fn signatures_block() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Signatures {
            pairs: vec![SignaturePair {
                name: "Maria".to_string(),
                role: Some("CEO".to_string()),
                location: None,
            }],
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras.contains(&"Maria".to_string()));
        assert!(paras.contains(&"CEO".to_string()));
        assert!(paras.iter().any(|p| p.contains("___")));
    }

    #[test]
    fn backcover_block() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::BackCover {
            contacts: ContactInfo {
                name: "Acme".to_string(),
                email: Some("a@b.c".to_string()),
                phone: Some("+55 11 1234".to_string()),
                address: Some("Rua X, 1".to_string()),
            },
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras.contains(&"Acme".to_string()));
        assert!(paras.contains(&"a@b.c".to_string()));
        assert!(paras.contains(&"+55 11 1234".to_string()));
        assert!(paras.contains(&"Rua X, 1".to_string()));
    }

    #[test]
    fn keyvalue_pairs() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::KeyValue {
            entries: vec![
                ("Cliente".to_string(), "X".to_string()),
                ("Valor".to_string(), "1000".to_string()),
            ],
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let paras = paragraphs(&sections(&p)[0]);
        assert!(paras.contains(&"Cliente: X".to_string()));
        assert!(paras.contains(&"Valor: 1000".to_string()));
    }

    #[test]
    fn cover_subtitle_goes_to_first_paragraph() {
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Cover(Cover {
            title: "Título".to_string(),
            subtitle: Some("Sub".to_string()),
            author: None,
            date: None,
        }));
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "pós-capa".to_string(),
            style: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        // O Cover sozinho (sem heading) vira uma seção com
        // heading="" e paragraphs=["Sub", "pós-capa"].
        assert_eq!(s.len(), 1);
        let paras = paragraphs(&s[0]);
        assert_eq!(paras, vec!["Sub", "pós-capa"]);
    }

    #[test]
    fn full_flow_heading_paragraph_table_paragraph() {
        // O cenário do E2E da Etapa 3: capa, 2 headings,
        // parágrafo, tabela. Garante que o payload tem a
        // estrutura que o E2E vai validar.
        let mut spec = empty_spec();
        spec.blocks.push(DocumentBlock::Cover(Cover {
            title: "Relatório".to_string(),
            subtitle: None,
            author: None,
            date: None,
        }));
        spec.blocks.push(DocumentBlock::Heading {
            level: 1,
            text: "Seção 1".to_string(),
            number: None,
        });
        spec.blocks.push(DocumentBlock::Heading {
            level: 2,
            text: "Seção 2".to_string(),
            number: None,
        });
        spec.blocks.push(DocumentBlock::Paragraph {
            text: "parágrafo entre".to_string(),
            style: None,
        });
        spec.blocks.push(DocumentBlock::Table {
            headers: vec!["A".to_string(), "B".to_string()],
            rows: vec![
                vec!["1".to_string(), "2".to_string()],
                vec!["3".to_string(), "4".to_string()],
            ],
            total: None,
            currency: None,
            percent: false,
            thousands: false,
            title: Some("Tabela".to_string()),
            source: None,
        });
        let p = translate_spec_to_docx_payload(&spec, &out_path()).unwrap();
        let s = sections(&p);
        // 3 seções: heading=Seção 1 (vazia, só o heading), heading=Seção 2 (parágrafo + tabela), ...
        // Hmm, na verdade: Cover não vira seção (subtitle=vazio). Heading 1 abre seção. Heading 2 fecha anterior e abre nova. Parágrafo vai pra seção 2. Tabela vai pra seção 2.
        // Resultado: 2 seções.
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["heading"], json!("Seção 1"));
        assert_eq!(paragraphs(&s[0]), Vec::<String>::new());
        assert_eq!(s[1]["heading"], json!("Seção 2"));
        let paras = paragraphs(&s[1]);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0], "parágrafo entre");
        assert!(paras[1].contains("Tabela"));
        assert!(paras[1].contains("A\tB"));
        assert!(paras[1].contains("1\t2"));
        assert!(paras[1].contains("3\t4"));
    }
}
