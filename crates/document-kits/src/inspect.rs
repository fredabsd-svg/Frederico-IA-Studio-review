//! `DocsInspectTool` — round-trip parcial de `.docx` /
//! `.xlsx` para `DocumentSpec` (Etapa 4 da Fase 5).
//!
//! ## Por que existe
//!
//! O `docs.generate` (Etapa 3) é one-way: o modelo emite um
//! `DocumentSpec`, o kit renderiza um arquivo. Sem o
//! inspect, o modelo não tem como se auto-verificar ("a
//! aba Painel é a primeira?", "a linha de TOTAL está
//! presente?", "o formato de moeda foi aplicado?"). A
//! Etapa 4 fecha esse ciclo.
//!
//! ## Modo padrão: resumo (não despejar conteúdo bruto)
//!
//! Padrão de chamada do modelo: `{"path": "..."}` —
//! devolve nomes de sheets, intervalo, header,
//! `n_rows`/`n_cols`, amostra de 5 linhas, `has_total`,
//! `column_formats` por coluna. Sem `range` = modo
//! resumo. **Nunca** despeja planilha de 5000 linhas no
//! contexto do modelo.
//!
//! ## Cobertura (parcial — documentada, não silenciada)
//!
//! Round-trip preserva apenas os blocos que o `.docx` /
//! `.xlsx` consegue reconstruir estruturalmente: para
//! `.docx`, `Heading 1/2/3` (style), `Paragraph`,
//! `Table` (do `docx.read`); para `.xlsx`, `Table` por
//! sheet (com mapeamento `sheets: [{block_index, name}]`).
//! `Cover`, `Kpis`, `Callout`, `Quote`, `Steps`, `Chart`,
//! `Image`, `Code`, `Footer`, `Signatures`, `BackCover`,
//! `Toc`, `KeyValue`, `List` → `coverage.lost` (honesto:
//! o round-trip é parcial).
//!
//! ## ADR-0020 (Etapa 4)
//!
//! D3 — `docs.inspect` cobre .xlsx também (default =
//! modo resumo; `range` opcional pra detalhe).

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_document_engine::{DocumentBlock, DocumentSpec, DocumentType, SpecVersion};
use frederico_tool_registry::{
    DispatchError, JsonSchema, RiskLevel, Tool, ToolCategory, ToolManifest, ToolManifestBuilder,
    ToolResult, WorkerToolDispatcher,
};
use serde_json::{json, Value};

use crate::format::DocumentFormat;

/// Cobertura do round-trip. `preserved` lista os blocos
/// que o inspect conseguiu reconstruir do arquivo;
/// `lost` lista os que o inspect **não** consegue
/// reconstruir e o motivo (campo `reason` opcional).
///
/// Documentar o que se perde é o defeito "ferramenta que
/// mente" prevenido — o modelo sabe o que falta
/// antes de tentar reescrever o spec.
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    /// Blocos preservados (strings tipo "heading",
    /// "paragraph", "table"). Os nomes batem com
    /// `DocumentBlock` em snake_case.
    pub preserved: Vec<&'static str>,
    /// Blocos perdidos. O inspect **não** consegue
    /// reconstruir Cover, Kpis, Callout, Quote, Steps,
    /// Chart, Image, Code, Footer, Signatures,
    /// BackCover, Toc, KeyValue, List de .docx /
    /// .xlsx (não tem como distinguir lista de
    /// parágrafo; chart/cover/etc. são semânticos, não
    /// estruturais).
    pub lost: Vec<&'static str>,
}

/// Resumo de uma sheet do .xlsx (Etapa 4). Devolvido
/// no `InspectOutput::sheets` (apenas para .xlsx).
///
/// É a "informação estrutural" que prova correção sem
/// despejar o conteúdo bruto (D3 do plano: "o que prova
/// correção é estrutural: a aba de painel é a primeira?
/// cada Table do spec virou uma aba? a linha de TOTAL
/// existe? o formato numérico da coluna de moeda está
/// aplicado?").
#[derive(Debug, Clone)]
pub struct SheetSummary {
    /// Nome da sheet (sanitizado — sem chars proibidos
    /// pelo Excel).
    pub name: String,
    /// Intervalo usado (ex: "A1:C5" do openpyxl).
    pub used_range: String,
    /// Cabeçalhos (1ª linha, exatas como no arquivo).
    pub headers: Vec<String>,
    /// Número de linhas de dados (excluindo header).
    pub n_rows: usize,
    /// Número de colunas (len de `headers`).
    pub n_cols: usize,
    /// Amostra das primeiras `sample_rows` linhas.
    pub first_rows: Vec<Vec<String>>,
    /// `true` se a última linha de dados começa com
    /// "Total" (heurística simples — cobre a maioria
    /// dos casos do `Table.total` do ExcelPro v0.1).
    pub has_total: bool,
    /// `cell.number_format` da 1ª célula não-vazia de
    /// cada coluna, mapeado pra alias semântico
    /// (`"BRL"`, `"PCT"`, `"THOUSANDS"`, `"INT"`) ou
    /// string cru do Excel.
    pub column_formats: HashMap<String, String>,
}

/// Saída do inspect (unificada para .docx e .xlsx).
#[derive(Debug, Clone)]
pub struct InspectOutput {
    /// `DocumentSpec` parcial reconstruído (heading /
    /// paragraph / table preservados, resto = loss
    /// declarado em `coverage.lost`).
    pub spec: DocumentSpec,
    /// Cobertura do round-trip (preserved / lost).
    pub coverage: Coverage,
    /// Resumo por sheet do .xlsx (vazio para .docx).
    pub sheets: Vec<SheetSummary>,
}

/// Argumentos do inspect (extraídos do `args` JSON).
#[derive(Debug, Clone)]
pub struct InspectArgs {
    /// Path do arquivo (validado contra allowlist).
    pub path: String,
    /// Formato (opcional; default = inferido pela
    /// extensão).
    pub format: Option<DocumentFormat>,
    /// Sheet específica (opcional; só pra .xlsx).
    pub sheet: Option<String>,
    /// `sample_rows` (opcional; default 5, max 20).
    pub sample_rows: Option<usize>,
    /// Modo detalhe (opcional; `range` é uma flag
    /// por enquanto — v0.1 do inspect não aplica o
    /// range; só devolve o summary completo).
    pub range: Option<String>,
}

impl InspectArgs {
    /// Parse dos args do `Tool::execute`. Erro
    /// estruturado se `path` ausente.
    fn from_value(value: &Value) -> Result<Self, String> {
        let path = value
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "argumento 'path' ausente ou não-string".to_string())?
            .to_string();
        let format = match value.get("format").and_then(|v| v.as_str()) {
            Some("docx") => Some(DocumentFormat::Docx),
            Some("xlsx") => Some(DocumentFormat::Xlsx),
            Some(other) => {
                return Err(format!(
                    "format '{other}' não é um DocumentFormat conhecido"
                ));
            }
            None => None,
        };
        let sheet = value
            .get("sheet")
            .and_then(|v| v.as_str())
            .map(String::from);
        let sample_rows = value
            .get("sample_rows")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let range = value
            .get("range")
            .and_then(|v| v.as_str())
            .map(String::from);
        Ok(Self {
            path,
            format,
            sheet,
            sample_rows,
            range,
        })
    }
}

/// `DocsInspectTool` — tool separado (não faz parte do
/// `docs.generate`). Roteia para `docx.read` ou
/// `xlsx.read` do worker, dependendo do formato.
pub struct DocsInspectTool {
    dispatcher: WorkerToolDispatcher,
    tool_id: ToolId,
    manifest: ToolManifest,
}

impl DocsInspectTool {
    /// Constrói o tool. O `dispatcher` é o mesmo
    /// `WorkerToolDispatcher` do `DocsGenerateTool`
    /// (mesma barreira de path, mesmo worker).
    #[must_use]
    pub fn new(dispatcher: WorkerToolDispatcher) -> Self {
        let manifest = Self::build_manifest();
        Self {
            dispatcher,
            tool_id: ToolId::new("docs.inspect"),
            manifest,
        }
    }

    /// Tool id exposto ao modelo.
    #[must_use]
    pub fn tool_id(&self) -> &ToolId {
        &self.tool_id
    }

    /// Infere o formato pela extensão do path. Default
    /// quando `format` é None.
    fn infer_format(path: &str) -> Result<DocumentFormat, String> {
        let p = Path::new(path);
        match p.extension().and_then(|e| e.to_str()) {
            Some("docx") => Ok(DocumentFormat::Docx),
            Some("xlsx") => Ok(DocumentFormat::Xlsx),
            Some(other) => Err(format!(
                "extensão '{other}' não é suportada pelo docs.inspect (use .docx ou .xlsx)"
            )),
            None => Err("path sem extensão — não dá pra inferir o formato".to_string()),
        }
    }

    /// Dispatch para `docx.read` ou `xlsx.read` no
    /// worker. Retorna a response crua.
    async fn invoke_read(
        &self,
        path: &str,
        format: DocumentFormat,
        sheet: Option<&str>,
        sample_rows: usize,
        range: Option<&str>,
    ) -> Result<Value, DispatchError> {
        let mut args = json!({
            "capability": match format {
                DocumentFormat::Docx => "docx.read",
                DocumentFormat::Xlsx => "xlsx.read",
            },
            "path": path,
            "sample_rows": sample_rows,
        });
        if let Some(s) = sheet {
            args["sheet"] = json!(s);
        }
        if let Some(r) = range {
            args["range"] = json!(r);
        }
        self.dispatcher.dispatch(args, &["path"]).await
    }

    /// Reconstrói um `DocumentSpec` parcial a partir
    /// do output do `docx.read`.
    fn build_docx_spec(
        paragraphs: &[String],
        tables: &[Vec<Vec<String>>],
    ) -> (DocumentSpec, Coverage) {
        let mut coverage = Coverage::default();
        let mut blocks = Vec::new();
        // Walk pelos paragrafos. Headings (style.name
        // `Heading 1/2/3` no python-docx) viram
        // `Heading { level, text, number: None }`.
        // O `docx.read` v0.3.0 nao expoe o style
        // (devolve so `paragraphs: [str]`), entao a
        // heuristica e: paragrafos que COMECAM
        // com a keyword "Heading N" sao tratados
        // como heading. Em v0.1 do inspect, isso e
        // aproximacao — a Etapa 4.x pode estender o
        // docx.read pra devolver paragraphs COM
        // style.
        for p in paragraphs {
            let trimmed = p.trim_start();
            // Detecta heading N (1, 2, ou 3).
            let heading_level: Option<u8> = if trimmed.starts_with("Heading 1 ") {
                Some(1)
            } else if trimmed.starts_with("Heading 2 ") {
                Some(2)
            } else if trimmed.starts_with("Heading 3 ") {
                Some(3)
            } else {
                None
            };
            if let Some(level) = heading_level {
                // Extrai o texto apos o prefixo "Heading N ".
                let prefix_len = "Heading 1 ".len()
                    + if level == 2 {
                        1
                    } else if level == 3 {
                        2
                    } else {
                        0
                    };
                let text = trimmed[prefix_len..].to_string();
                coverage.preserved.push("heading");
                blocks.push(DocumentBlock::Heading {
                    level,
                    text,
                    number: None,
                });
                continue;
            }
            // Caso contrario, paragrafo normal.
            coverage.preserved.push("paragraph");
            blocks.push(DocumentBlock::Paragraph {
                text: p.clone(),
                style: None,
            });
        }
        for table in tables {
            if table.is_empty() {
                continue;
            }
            // 1a linha = headers, resto = rows.
            let headers: Vec<String> = table[0].clone();
            let rows: Vec<Vec<String>> = table[1..].to_vec();
            coverage.preserved.push("table");
            blocks.push(DocumentBlock::Table {
                headers,
                rows,
                total: None,
                currency: None,
                percent: false,
                thousands: false,
                title: None,
                source: None,
            });
        }
        // O que nao foi preservado — lost.
        // (Em v0.1 do inspect, tudo que nao
        // heading/paragraph/table e lost.)
        coverage.lost = vec![
            "cover",
            "kpis",
            "callout",
            "quote",
            "steps",
            "chart",
            "image",
            "code",
            "footer",
            "signatures",
            "backcover",
            "toc",
            "keyvalue",
            "list",
        ];
        let spec = DocumentSpec {
            spec_version: SpecVersion::default(),
            doc_type: DocumentType::Report, // o inspect nao distingue — Report default
            style: frederico_document_engine::DocumentStyle::default(),
            language: "pt-BR".to_string(),
            blocks,
            metadata: frederico_document_engine::DocumentMetadata::default(),
            confidentiality: None,
        };
        (spec, coverage)
    }

    /// Constrói `SheetSummary` para cada sheet do
    /// .xlsx a partir da response do worker.
    fn build_sheet_summaries(sheets_json: &[Value]) -> Vec<SheetSummary> {
        let mut summaries = Vec::new();
        for sh in sheets_json {
            let name = sh
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let used_range = sh
                .get("used_range")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let headers: Vec<String> = sh
                .get("headers")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default();
            let n_rows = sh.get("n_rows").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let n_cols = sh.get("n_cols").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let first_rows: Vec<Vec<String>> = sh
                .get("first_rows")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .map(|row| {
                            row.as_array()
                                .map(|r| {
                                    r.iter()
                                        .map(|v| v.as_str().unwrap_or("").to_string())
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            // has_total: heuristica — a ultima linha
            // de dados comeca com "Total" (case
            // insensitive).
            let has_total = sh
                .get("rows")
                .and_then(|v| v.as_array())
                .and_then(|rows| rows.last())
                .and_then(|last| {
                    last.as_array()
                        .and_then(|cells| cells.first().and_then(|c| c.as_str()))
                })
                .map(|s| s.to_lowercase().contains("total"))
                .unwrap_or(false);
            let column_formats: HashMap<String, String> = sh
                .get("column_formats")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();
            summaries.push(SheetSummary {
                name,
                used_range,
                headers,
                n_rows,
                n_cols,
                first_rows,
                has_total,
                column_formats,
            });
        }
        summaries
    }

    fn build_manifest() -> ToolManifest {
        let input_schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path absoluto do arquivo .docx ou .xlsx. \
                                    Validado contra a allowlist do ToolManifest."
                },
                "format": {
                    "type": "string",
                    "enum": ["docx", "xlsx"],
                    "description": "Formato do arquivo. Opcional — default = \
                                    inferido pela extensao (.docx ou .xlsx)."
                },
                "sheet": {
                    "type": "string",
                    "description": "Sheet especifica (so pra .xlsx). \
                                    Opcional — default = todas as sheets."
                },
                "sample_rows": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Numero de linhas de amostra no \
                                    output (default 5, max 20). \
                                    Evita despejar planilha de 5000 \
                                    linhas no contexto do modelo."
                },
                "range": {
                    "type": "string",
                    "description": "Intervalo A1:D10 (opcional). \
                                    v0.1 do inspect: apenas flag — \
                                    nao aplica o range; sempre \
                                    devolve o summary completo."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        });

        let output_schema = json!({
            "type": "object",
            "properties": {
                "spec": {
                    "type": "object",
                    "description": "DocumentSpec parcial (heading/paragraph/table \
                                    preservados, resto = coverage.lost)."
                },
                "coverage": {
                    "type": "object",
                    "properties": {
                        "preserved": {"type": "array", "items": {"type": "string"}},
                        "lost": {"type": "array", "items": {"type": "string"}}
                    }
                },
                "sheets": {
                    "type": "array",
                    "description": "Resumo por sheet (.xlsx apenas). \
                                    Para .docx, fica vazio.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "used_range": {"type": "string"},
                            "headers": {"type": "array", "items": {"type": "string"}},
                            "n_rows": {"type": "integer"},
                            "n_cols": {"type": "integer"},
                            "first_rows": {"type": "array"},
                            "has_total": {"type": "boolean"},
                            "column_formats": {"type": "object"}
                        }
                    }
                }
            }
        });

        ToolManifestBuilder::new(ToolId::new("docs.inspect"), "docs")
            .version("0.1.0")
            .display_name("Inspecionar documento")
            .description(
                "Round-trip parcial de .docx/.xlsx para DocumentSpec. \
                 Padrao = modo resumo (nomes, intervalo, header, \
                 contagens, amostra de 5 linhas, has_total, \
                 column_formats). Cobertura: heading/paragraph/table \
                 preservados; cover/kpis/callout/chart/etc. vao pra \
                 coverage.lost (honesto: o round-trip e parcial). \
                 v0.1 da Etapa 4 da Fase 5.",
            )
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Safe) // So le; nao escreve
            .input_schema(JsonSchema(input_schema))
            .output_schema(JsonSchema(output_schema))
            .capability("docx.read")
            .capability("xlsx.read")
            .capability("document.inspect")
            .timeout_ms(30_000)
            .build()
            .expect("manifesto do docs.inspect bem-formado")
    }
}

#[async_trait]
impl Tool for DocsInspectTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, arguments: &Value) -> ToolResult {
        // 1. Parse dos args.
        let args = match InspectArgs::from_value(arguments) {
            Ok(a) => a,
            Err(msg) => return ToolResult::err(self.tool_id.clone(), msg),
        };

        // 2. Resolve formato.
        let format = match args.format {
            Some(f) => f,
            None => match Self::infer_format(&args.path) {
                Ok(f) => f,
                Err(msg) => return ToolResult::err(self.tool_id.clone(), msg),
            },
        };

        // 3. Valida path contra allowlist (defesa em
        //    profundidade).
        if let Err(e) = self.dispatcher.check_path(&args.path) {
            return match e {
                DispatchError::PathNotAllowed { path, allowed } => ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "path '{}' não está em nenhum diretório permitido: {:?}",
                        path.display(),
                        allowed
                    ),
                ),
                DispatchError::Process(_) => ToolResult::err(
                    self.tool_id.clone(),
                    "erro de processo na validação de path",
                ),
                DispatchError::NotAString { field, value } => ToolResult::err(
                    self.tool_id.clone(),
                    format!("campo '{field}' não é string: {value}"),
                ),
            };
        }

        // 4. Chama o worker (docx.read ou xlsx.read).
        let sample_rows = args.sample_rows.unwrap_or(5).clamp(1, 20);
        let response = match self
            .invoke_read(
                &args.path,
                format,
                args.sheet.as_deref(),
                sample_rows,
                args.range.as_deref(),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(self.tool_id.clone(), format!("invoke falhou: {e}")),
        };

        // 5. Valida a response.
        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let code = response
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(sem mensagem)");
            return ToolResult::err(
                self.tool_id.clone(),
                format!("worker falhou: {code} — {message}"),
            );
        }

        // 6. Monta a InspectOutput.
        let (spec, coverage) = match format {
            DocumentFormat::Docx => {
                let paragraphs: Vec<String> = response
                    .get("paragraphs")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|v| v.as_str().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let tables: Vec<Vec<Vec<String>>> = response
                    .get("tables")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|t| {
                                t.as_array()
                                    .map(|rows| {
                                        rows.iter()
                                            .map(|row| {
                                                row.as_array()
                                                    .map(|cells| {
                                                        cells
                                                            .iter()
                                                            .map(|c| {
                                                                c.as_str().unwrap_or("").to_string()
                                                            })
                                                            .collect()
                                                    })
                                                    .unwrap_or_default()
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Self::build_docx_spec(&paragraphs, &tables)
            }
            DocumentFormat::Xlsx => {
                // Para .xlsx, NAO reconstroi DocumentSpec
                // (a unidade e sheet, nao bloco). O output
                // e `sheets: [SheetSummary]`. O `spec`
                // devolvido e o DocumentType::Spreadsheet
                // com blocks vazio (honesto: o inspect
                // nao reconstroi o spec de .xlsx completo;
                // so expoe o summary estrutural).
                let coverage = Coverage {
                    preserved: vec!["table"],
                    lost: vec![
                        "cover",
                        "kpis",
                        "callout",
                        "quote",
                        "steps",
                        "chart",
                        "image",
                        "code",
                        "footer",
                        "signatures",
                        "backcover",
                        "toc",
                        "keyvalue",
                        "list",
                    ],
                };
                let spec = DocumentSpec {
                    spec_version: SpecVersion::default(),
                    doc_type: DocumentType::Spreadsheet,
                    style: frederico_document_engine::DocumentStyle::default(),
                    language: "pt-BR".to_string(),
                    blocks: vec![],
                    metadata: frederico_document_engine::DocumentMetadata::default(),
                    confidentiality: None,
                };
                (spec, coverage)
            }
        };

        let sheets = match format {
            DocumentFormat::Xlsx => {
                let sheets_json: Vec<Value> = response
                    .get("sheets")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                Self::build_sheet_summaries(&sheets_json)
            }
            DocumentFormat::Docx => Vec::new(),
        };

        // 7. Serializa o output.
        let spec_json = serde_json::to_value(&spec).unwrap_or(json!({}));
        let sheets_json: Vec<Value> = sheets
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "used_range": s.used_range,
                    "headers": s.headers,
                    "n_rows": s.n_rows,
                    "n_cols": s.n_cols,
                    "first_rows": s.first_rows,
                    "has_total": s.has_total,
                    "column_formats": s.column_formats,
                })
            })
            .collect();
        let output = json!({
            "spec": spec_json,
            "coverage": {
                "preserved": coverage.preserved,
                "lost": coverage.lost,
            },
            "sheets": sheets_json,
            "format": format.as_str(),
        });

        ToolResult::ok(self.tool_id.clone(), output, vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_format_docx() {
        assert!(matches!(
            DocsInspectTool::infer_format("C:\\Users\\me\\out.docx").unwrap(),
            DocumentFormat::Docx
        ));
    }

    #[test]
    fn infer_format_xlsx() {
        assert!(matches!(
            DocsInspectTool::infer_format("C:\\Users\\me\\out.xlsx").unwrap(),
            DocumentFormat::Xlsx
        ));
    }

    #[test]
    fn infer_format_unknown_extension_errors() {
        assert!(DocsInspectTool::infer_format("C:\\Users\\me\\out.pdf").is_err());
        assert!(DocsInspectTool::infer_format("C:\\Users\\me\\out").is_err());
    }

    #[test]
    fn parse_args_minimal() {
        let args = InspectArgs::from_value(&json!({"path": "C:\\x\\out.docx"})).unwrap();
        assert_eq!(args.path, "C:\\x\\out.docx");
        assert!(args.format.is_none());
        assert!(args.sheet.is_none());
        assert!(args.sample_rows.is_none());
        assert!(args.range.is_none());
    }

    #[test]
    fn parse_args_with_xlsx() {
        let args = InspectArgs::from_value(&json!({
            "path": "C:\\x\\out.xlsx",
            "format": "xlsx",
            "sheet": "Vendas",
            "sample_rows": 10
        }))
        .unwrap();
        assert_eq!(args.path, "C:\\x\\out.xlsx");
        assert!(matches!(args.format, Some(DocumentFormat::Xlsx)));
        assert_eq!(args.sheet.as_deref(), Some("Vendas"));
        assert_eq!(args.sample_rows, Some(10));
    }

    #[test]
    fn parse_args_invalid_format_errors() {
        let r = InspectArgs::from_value(&json!({
            "path": "C:\\x\\out.docx",
            "format": "pdf"
        }));
        assert!(r.is_err());
    }

    #[test]
    fn parse_args_missing_path_errors() {
        let r = InspectArgs::from_value(&json!({}));
        assert!(r.is_err());
    }

    #[test]
    fn build_docx_spec_extracts_headings_paragraphs_tables() {
        // Smoke da logica pura (sem worker).
        let paragraphs = vec![
            "Heading 1 Visao geral".to_string(),
            "Heading 2 Detalhe".to_string(),
            "Este paragrafo fica entre os dois.".to_string(),
        ];
        let tables = vec![vec![
            vec!["Mes".to_string(), "Total".to_string()],
            vec!["Jan".to_string(), "100".to_string()],
            vec!["Fev".to_string(), "200".to_string()],
        ]];
        let (spec, coverage) = DocsInspectTool::build_docx_spec(&paragraphs, &tables);
        assert_eq!(spec.blocks.len(), 4); // 2 headings + 1 paragraph + 1 table
                                          // 2 headings preservados.
        let preserved_count = coverage
            .preserved
            .iter()
            .filter(|p| **p == "heading")
            .count();
        assert_eq!(preserved_count, 2);
        // 1 paragraph preservado.
        let paragraph_count = coverage
            .preserved
            .iter()
            .filter(|p| **p == "paragraph")
            .count();
        assert_eq!(paragraph_count, 1);
        // 1 table preservado.
        let table_count = coverage.preserved.iter().filter(|p| **p == "table").count();
        assert_eq!(table_count, 1);
        // Lost inclui cover, kpis, etc.
        assert!(coverage.lost.contains(&"cover"));
        assert!(coverage.lost.contains(&"kpis"));
    }

    #[test]
    fn build_sheet_summaries_extracts_structural_info() {
        let sheets_json = vec![json!({
            "name": "Vendas",
            "used_range": "A1:C4",
            "headers": ["Mes", "Total", "Crescimento"],
            "n_rows": 3,
            "n_cols": 3,
            "first_rows": [
                ["Jan", "100", "0.05"],
                ["Fev", "200", "0.10"]
            ],
            "rows": [
                ["Jan", "100", "0.05"],
                ["Fev", "200", "0.10"],
                ["Mar", "300", "0.15"],
                ["Total", "600", ""]
            ],
            "column_formats": {
                "1": "BRL",
                "2": "PCT"
            }
        })];
        let summaries = DocsInspectTool::build_sheet_summaries(&sheets_json);
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.name, "Vendas");
        assert_eq!(s.used_range, "A1:C4");
        assert_eq!(s.headers, vec!["Mes", "Total", "Crescimento"]);
        assert_eq!(s.n_rows, 3);
        assert_eq!(s.n_cols, 3);
        assert_eq!(s.first_rows.len(), 2);
        // has_total: ultima linha comeca com "Total".
        assert!(s.has_total);
        assert_eq!(s.column_formats.get("1").map(String::as_str), Some("BRL"));
        assert_eq!(s.column_formats.get("2").map(String::as_str), Some("PCT"));
    }

    #[test]
    fn has_total_false_when_no_total_row() {
        let sheets_json = vec![json!({
            "name": "Vendas",
            "headers": ["Mes", "Total"],
            "n_rows": 2,
            "n_cols": 2,
            "rows": [
                ["Jan", "100"],
                ["Fev", "200"]
            ]
        })];
        let summaries = DocsInspectTool::build_sheet_summaries(&sheets_json);
        assert!(!summaries[0].has_total);
    }
}
