//! `DocsGenerateTool` — o **único** tool que o modelo vê.
//!
//! Roteador: recebe `{spec, output_path, format}` e
//! despacha pro kit certo (v0.1: WordPro). O `format` no
//! schema é **gerado** a partir de
//! `KitRegistry::implemented_formats()` — adicionar
//! `DocumentFormat::Xlsx` exige um kit implementado; o
//! modelo nunca vê um formato que não funciona.
//!
//! ## O que este tool faz
//!
//! 1. Valida `output_path` contra a allowlist do
//!    `WorkerToolDispatcher` (path safety forte).
//! 2. Parsea o `spec` (já validado pelo `DocumentSpec`
//!    schema do `document-engine`, mas re-validamos aqui
//!    por defesa em profundidade — `Tool::execute` é
//!    chamado após `validate_tool_call`, e este tool
//!    pode ser chamado direto em testes).
//! 3. Resolve o `format` pedido contra o `KitRegistry`.
//!    Se o formato não está implementado, devolve
//!    `ToolResult::err` (não é panic — é resposta
//!    estruturada pro modelo).
//! 4. Chama `kit.render(spec, output_path)`.
//! 5. Devolve `ToolResult::ok` com `path`, `size_bytes`,
//!    `format`, `sections_written` (eco do worker).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_document_engine::{
    validate_against_schema, validate_semantic, DocumentError, DocumentSpec,
};
use frederico_tool_registry::{
    DispatchError, JsonSchema, RiskLevel, Tool, ToolCategory, ToolContext, ToolManifest,
    ToolManifestBuilder, ToolResult, WorkerToolDispatcher,
};
use serde_json::{json, Value};

use crate::format::DocumentFormat;
use crate::kit::KitError;
use crate::registry::KitRegistry;

/// O tool único exposto ao modelo. Roteador puro: o
/// trabalho de tradução é dos kits; este aqui só
/// valida, roteia e formata a resposta.
pub struct DocsGenerateTool {
    registry: Arc<KitRegistry>,
    dispatcher: WorkerToolDispatcher,
    /// Tool id (constante — `docs.generate`).
    tool_id: ToolId,
    /// Manifesto pré-construído (gerado uma vez no `new`
    /// — o schema do `format` é derivado do registry
    /// naquele momento).
    manifest: ToolManifest,
}

impl DocsGenerateTool {
    /// Constrói o tool. O `registry` deve estar
    /// "fechado" (todos os kits registrados) — depois
    /// do `new`, o `manifest` é congelado e não muda
    /// (mesmo se o registry mudar externamente).
    pub fn new(registry: Arc<KitRegistry>, dispatcher: WorkerToolDispatcher) -> Self {
        let manifest = Self::build_manifest(&registry);
        Self {
            registry,
            dispatcher,
            tool_id: ToolId::new("docs.generate"),
            manifest,
        }
    }

    /// Constrói o `ToolManifest` com o enum `format`
    /// **gerado** a partir de
    /// `KitRegistry::implemented_formats()`. Se o
    /// registry muda depois, o manifesto fica
    /// desatualizado — por isso o `new` "congela" no
    /// registry atual.
    fn build_manifest(registry: &KitRegistry) -> ToolManifest {
        let formats: Vec<String> = registry
            .implemented_formats()
            .iter()
            .map(|f| f.as_str().to_string())
            .collect();

        // Se o registry não tem kits implementados, o
        // schema é vazio (sem `format`). O modelo não
        // pode chamar `docs.generate` sem saber o
        // formato — falha de configuração, não de
        // execução.
        let format_schema = if formats.is_empty() {
            json!({
                "type": "string",
                "description": "(nenhum kit implementado — ToolRegistry deve recusar `docs.generate` antes desta validação)"
            })
        } else {
            json!({
                "type": "string",
                "enum": formats,
                "description": "Formato do documento. Gerado a partir dos kits implementados no KitRegistry."
            })
        };

        let input_schema = json!({
            "type": "object",
            "properties": {
                "spec": {
                    "type": "object",
                    "description": "DocumentSpec validado (schema + regras semânticas). \
                                    O schema detalhado é gerado pelo document-engine."
                },
                "output_path": {
                    "type": "string",
                    "description": "Caminho absoluto do arquivo de saída. \
                                    Validado contra o ToolManifest.allowed_paths antes do invoke."
                },
                "format": format_schema
            },
            "required": ["spec", "output_path", "format"],
            "additionalProperties": false
        });

        let builder = ToolManifestBuilder::new(ToolId::new("docs.generate"), "docs")
            .version("0.1.0")
            .display_name("Gerar documento")
            .description(Self::description_for(&registry.implemented()))
            .category(ToolCategory::Docs)
            .risk_level(RiskLevel::Moderate)
            .input_schema(JsonSchema(input_schema))
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
            .timeout_ms(30_000);

        builder
            .build()
            .expect("manifesto de docs.generate bem-formado")
    }

    /// Gera a `description` do tool dinamicamente, listando
    /// os formatos implementados. O modelo lê isso antes
    /// de chamar.
    fn description_for(implemented: &[Arc<dyn crate::kit::Kit>]) -> String {
        let formats: Vec<String> = implemented
            .iter()
            .map(|k| k.target_format().as_str().to_string())
            .collect();
        if formats.is_empty() {
            return "Stub do `docs.generate` — nenhum kit implementado no KitRegistry. \
                    A ToolRegistry não deve registrar este tool até que ao menos um kit \
                    esteja implementado."
                .to_string();
        }
        let list = formats.join(", ");
        format!(
            "Gera um documento profissional a partir de um DocumentSpec declarativo. \
             v0.1 — formatos disponíveis: {list}. O DocumentSpec é validado em duas \
             camadas (JSON Schema + regras semânticas) pelo `document-engine` antes \
             de chegar aqui; este tool apenas roteia pro kit certo e devolve o path \
             final."
        )
    }

    /// Re-validar o `DocumentSpec` no `Tool::execute` é
    /// **defesa em profundidade**: o `validate_tool_call`
    /// já conferiu o `input_schema` (que aceita
    /// `spec: object` genérico), mas o spec tem schema
    /// próprio.
    fn revalidate_spec(value: &Value) -> Result<DocumentSpec, DocumentError> {
        validate_against_schema(value)?;
        let spec: DocumentSpec =
            serde_json::from_value(value.clone()).map_err(|e| DocumentError::Parse {
                path: "/".to_string(),
                message: e.to_string(),
            })?;
        validate_semantic(&spec)?;
        Ok(spec)
    }

    /// Parseia o `format` string → `DocumentFormat`. Erro
    /// estruturado se a string não bate com nenhum
    /// formato conhecido.
    fn parse_format(s: &str) -> Result<DocumentFormat, String> {
        match s {
            "docx" => Ok(DocumentFormat::Docx),
            "xlsx" => Ok(DocumentFormat::Xlsx),
            // Etapa 5 PR 2 (ADR-0021): `pdf` entrou no
            // mesmo commit do `PdfProKit` real + bump
            // atômico do enum (precedente do ADR-0020 §3
            // D3).
            "pdf" => Ok(DocumentFormat::Pdf),
            other => Err(format!(
                "formato '{other}' não é um DocumentFormat conhecido"
            )),
        }
    }
}

#[async_trait]
impl Tool for DocsGenerateTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &Value) -> ToolResult {
        // 1. Parse básico dos args. Não usa `?` porque o
        // erro aqui é `ToolResult` e a função também
        // retorna `ToolResult` — `?` exigiria
        // `FromResidual` que não temos.
        let spec_value = arguments.get("spec").cloned().unwrap_or(Value::Null);
        let output_path_str = match arguments.get("output_path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    "argumento 'output_path' ausente ou não-string",
                );
            }
        };
        let format_str = match arguments.get("format").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    "argumento 'format' ausente ou não-string",
                );
            }
        };

        // 2. **Barreira primária** (Fase de Ligação Etapa 5.X —
        // patch-allowed-paths): resolve `output_path` contra o
        // `Jail` da conversa (Etapa 1 da Fase de Ligação,
        // `tool-registry/src/workspace.rs`). O Jail é a peça
        // de segurança central do `files.read` desde a Etapa 1;
        // aqui a gente reusa a abstração em vez de
        // reimplementar checagem de `..` / absoluto / UNC /
        // symlink (que é exatamente o tipo de reimplementação
        // que fica pra trás — uma das barreiras paralelas
        // acaba divergindo).
        //
        // `resolve_allowing_nonexistent` é a versão feita sob
        // medida pra escrita: canonicaliza o **pai** (que
        // existe — é o workspace da conversa) e compõe com o
        // `file_name()` do output (que não existe — é o que
        // vamos criar). Cobre:
        // - `..` (rejeitado no loop de componentes);
        // - caminho absoluto (rejeitado via `Component::Prefix`
        //   e `RootDir`);
        // - UNC (`\\server\share`, rejeitado via
        //   `Prefix::UNC`);
        // - symlink (canonicalize do **pai** resolve e o
        //   `starts_with(root_canonical)` rejeita — mas isso
        //   só pega se o symlink está no **pai**, não no
        //   filename; vide passo 3 abaixo).
        //
        // **Por que `resolve_allowing_nonexistent` e não
        // `resolve`:** o arquivo de saída não existe ainda
        // (é o que vamos criar); `resolve` exige existência
        // (canonicalize do path inteiro falha).
        let output_path_resolved: PathBuf = match ctx
            .jail
            .resolve_allowing_nonexistent(std::path::Path::new(output_path_str))
        {
            Ok(p) => p,
            Err(e) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!("output_path '{output_path_str}' fora do workspace da conversa: {e}"),
                );
            }
        };

        // 3. **Mitigação parcial contra symlink-on-output.**
        // O passo 2 canonicaliza o **pai** mas não o
        // `file_name()` (que não existe). Cenário bypass: o
        // workspace contém `link.txt` → symlink pra
        // `/etc/hosts` (Unix) ou `C:\Windows\...` (Windows);
        // o passo 2 aceita `output_path: "link.txt"` (o pai é
        // o workspace, ok); o `kit.render` depois abre o
        // `link.txt` e o Python segue o symlink — **bypass do
        // jail**. Esta checagem pega o caso **ingênuo**
        // (symlink já existe) e tem **TOCTOU** (entre o
        // `symlink_metadata` e a escrita pelo worker, o alvo
        // pode ser trocado). **Não é barreira** — barreira de
        // verdade exige `O_NOFOLLOW` / `O_CREAT|O_EXCL` no
        // `open` do Python. Pendência nomeada em
        // `docs/modules/process-architecture.md` item 5
        // (escrita segura no worker). Rótulo aqui é
        // intencional: é o tipo de defesa que não devemos
        // chamar de barreira sem ser.
        if let Ok(meta) = std::fs::symlink_metadata(&output_path_resolved) {
            if meta.file_type().is_symlink() {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "output_path '{}' é um symlink; jail não permite",
                        output_path_resolved.display()
                    ),
                );
            }
        }

        // 4. **Defesa em profundidade** (fail-closed): o
        // `dispatcher.check_path` com a allowlist do
        // `root_canonical()` do jail. Em produção
        // falha-praticamente-nunca (passo 2 já validou), mas
        // se alguém bypassar o passo 2 no futuro (refactor,
        // atalho, novo Tool::execute), o check_path com
        // allowlist fail-closed **ainda barra**. Os dois lados
        // da comparação (`output_path_resolved` e
        // `root_canonical`) vêm da **mesma** canonicalização
        // do `Jail::new` — então `Path::starts_with`
        // component-wise é confiável mesmo com case misto do
        // Windows.
        if let Err(e) = self.dispatcher.check_path(
            output_path_resolved.to_str().unwrap_or(output_path_str),
            &[ctx.jail.root_canonical().to_path_buf()],
        ) {
            return match e {
                DispatchError::PathNotAllowed { path, allowed } => ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "output_path '{}' não está em nenhum diretório permitido: {:?}",
                        path.display(),
                        allowed
                    ),
                ),
                DispatchError::Invoke(_) => ToolResult::err(
                    self.tool_id.clone(),
                    "erro de processo na validação de path",
                ),
                DispatchError::NotAString { field, value } => ToolResult::err(
                    self.tool_id.clone(),
                    format!("campo '{field}' não é string: {value}"),
                ),
            };
        }

        // 3. Parse o `format`.
        let format = match Self::parse_format(format_str) {
            Ok(f) => f,
            Err(msg) => return ToolResult::err(self.tool_id.clone(), msg),
        };

        // 4. Encontra o kit. Se não está implementado,
        // devolve erro estruturado.
        let kit = match self.registry.find_for_format(format) {
            Some(k) => k,
            None => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "formato '{format_str}' não tem kit implementado (ou foi desabilitado). \
                         Implementação prevista: ver docs/status.md Etapa 4/5."
                    ),
                );
            }
        };

        // 5. Valida o DocumentSpec.
        let spec = match Self::revalidate_spec(&spec_value) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!("DocumentSpec inválido: {e}"),
                );
            }
        };

        // 6. Chama o kit com o path **canônico e validado**
        // (passos 2-4 acima). Antes da Etapa 5.X o `output_path`
        // vinha de `PathBuf::from(output_path_str)` (literal, sem
        // resolução), e o Python escrevia no CWD do worker — o
        // que produzia o `real_minimal.docx` solto na árvore do
        // repo. Agora o path vem do `Jail::resolve_allowing_nonexistent`
        // e o `kit.render` recebe o absoluto dentro do jail.
        let output_path = output_path_resolved;
        let kit_output = match kit.render(&spec, &output_path).await {
            Ok(o) => o,
            Err(KitError::NotImplemented { id, format, etapa }) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "kit '{id}' (formato {format}) ainda não foi implementado (Etapa {etapa} da Fase 5)"
                    ),
                );
            }
            Err(KitError::InvalidSpec(e)) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!("DocumentSpec rejeitado pelo kit: {e}"),
                );
            }
            Err(KitError::Worker(msg)) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!("kit '{}' falhou: {msg}", kit.id()),
                );
            }
            Err(KitError::Process(p)) => {
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!("kit '{}' falhou no worker: {p}", kit.id()),
                );
            }
            Err(KitError::PathNotAllowed(msg)) => {
                return ToolResult::err(self.tool_id.clone(), msg);
            }
            Err(KitError::AuditFailed {
                code,
                message,
                failed,
            }) => {
                // §19.6 sem interruptor: o PDF nao foi entregue.
                // A mensagem vai pro modelo com o codigo e a
                // lista de checks que falharam - o caller pode
                // dizer ao usuario exatamente o que foi
                // (fonte nao embedded, PDF cifrado, falta
                // OutputIntent, etc.).
                let failed_pretty = serde_json::to_string_pretty(&failed)
                    .unwrap_or_else(|_| "<unparseable>".to_string());
                return ToolResult::err(
                    self.tool_id.clone(),
                    format!(
                        "kit '{}' - auditoria bloqueante (§19.6) falhou: {code} - {message}\n\
                         checks que falharam:\n{failed_pretty}",
                        kit.id()
                    ),
                );
            }
        };

        // 7. Monta o `output` do ToolResult.
        //
        // Inclui `sheets` (mapeamento bloco → sheet, Etapa 4)
        // e `warnings` (degradações declaradas, Etapa 4 deltas:
        // "se algo for puxado para dentro do handler antes da
        // hora, que sejam os formatos numéricos, não o chart"
        // — a degradação tem que ser declarada, nunca
        // silenciosa, pro modelo poder dizer a verdade ao
        // usuário).
        let sheets_json: Vec<Value> = kit_output
            .sheets
            .iter()
            .map(|s| {
                json!({
                    "block_index": s.block_index,
                    "sheet_name": s.sheet_name,
                })
            })
            .collect();
        let mut output = json!({
            "path": kit_output.path.to_string_lossy(),
            "size_bytes": kit_output.size_bytes,
            "format": kit_output.format.as_str(),
            "sections_written": kit_output.extra.get("sections_written").cloned().unwrap_or(json!(0)),
            "sheets": sheets_json,
            "warnings": kit_output.warnings,
        });
        // Merge `extra` por último para que campos
        // específicos do kit (ex: `cells_formatted` do
        // `xlsx.write`) apareçam no topo do output. `sheets`
        // e `warnings` continuam presentes (kit não pode
        // sobrescrever).
        if let Value::Object(extra_map) = kit_output.extra {
            if let Value::Object(out_map) = &mut output {
                for (k, v) in extra_map {
                    out_map.insert(k, v);
                }
            }
        }

        ToolResult::ok(self.tool_id.clone(), output, vec![kit_output.path])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_process_architecture::FakeWorkerConfig;

    /// Cria um `KitRegistry` com só o `WordProKit`
    /// (registrado, mas o kit é o real e precisa de
    /// `WorkerHandle`). Pra testar o `DocsGenerateTool`
    /// isolado, usamos o `FakeWorker` do
    /// `process-architecture`.
    async fn build_tool_with_wordpro() -> (
        DocsGenerateTool,
        frederico_process_architecture::WorkerManager,
    ) {
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_in_process(
            FakeWorkerConfig::default(),
            frederico_process_architecture::WorkerSpawnConfig::default(),
        )
        .await
        .expect("spawn fake worker");

        let handle = Arc::new(handle);
        let wordpro = Arc::new(crate::wordpro::WordProKit::new(handle.clone()));
        let mut registry = KitRegistry::new();
        registry.register(wordpro);
        let registry = Arc::new(registry);

        let dispatcher = WorkerToolDispatcher::new(Arc::new((*handle).clone()));
        let tool = DocsGenerateTool::new(registry, dispatcher);

        (tool, manager)
    }

    /// Helper (Etapa 4): constroi o tool com WordPro E
    /// ExcelPro registrados — para testar o inventario
    /// `["docx", "xlsx"]` do `format` no schema.
    async fn build_tool_with_all_kits() -> (
        DocsGenerateTool,
        frederico_process_architecture::WorkerManager,
    ) {
        let (manager, handle) = frederico_process_architecture::WorkerManager::spawn_in_process(
            FakeWorkerConfig::default(),
            frederico_process_architecture::WorkerSpawnConfig::default(),
        )
        .await
        .expect("spawn fake worker");

        let handle = Arc::new(handle);
        let wordpro = Arc::new(crate::wordpro::WordProKit::new(handle.clone()));
        let excelpro = Arc::new(crate::excelpro::ExcelProKit::new(handle.clone()));
        let mut registry = KitRegistry::new();
        registry.register(wordpro);
        registry.register(excelpro);
        let registry = Arc::new(registry);

        let dispatcher = WorkerToolDispatcher::new(Arc::new((*handle).clone()));
        let tool = DocsGenerateTool::new(registry, dispatcher);

        (tool, manager)
    }

    #[tokio::test]
    async fn format_schema_contains_only_implemented() {
        // Registry v0.1 (Etapa 3) so tinha WordPro
        // (Docx). A Etapa 4 adiciona ExcelPro (Xlsx) —
        // o schema do `format` agora tem `["docx", "xlsx"]`
        // (em ordem alfabetica por as_str). Inventario
        // nao mente (REGRAS §1.9): so aparece o que esta
        // implementado.
        let (tool, _manager) = build_tool_with_all_kits().await;
        let schema = tool.manifest().input_schema.0.clone();
        let format_enum = schema
            .get("properties")
            .and_then(|p| p.get("format"))
            .and_then(|f| f.get("enum"))
            .and_then(|e| e.as_array())
            .expect("schema deve ter format.enum");
        let formats: Vec<String> = format_enum
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(formats, vec!["docx", "xlsx"]);
    }

    #[tokio::test]
    async fn empty_registry_produces_warning_description() {
        // Sem kits implementados, a description avisa
        // (e o enum `format` vira string sem enum —
        // schema é placeholder).
        let (_manager, handle) = frederico_process_architecture::WorkerManager::spawn_in_process(
            FakeWorkerConfig::default(),
            frederico_process_architecture::WorkerSpawnConfig::default(),
        )
        .await
        .expect("spawn fake");
        let registry = Arc::new(KitRegistry::new());
        let dispatcher = WorkerToolDispatcher::new(Arc::new(handle.clone()));
        let tool = DocsGenerateTool::new(registry, dispatcher);

        let schema = tool.manifest().input_schema.0.clone();
        let format_field = schema
            .get("properties")
            .and_then(|p| p.get("format"))
            .expect("schema deve ter format");
        // Sem enum quando vazio (placeholder).
        assert!(format_field.get("enum").is_none());
        // Description avisa.
        assert!(tool.manifest().description.contains("nenhum kit"));
    }

    #[test]
    fn parse_format_recognizes_known() {
        assert!(matches!(
            DocsGenerateTool::parse_format("docx").unwrap(),
            DocumentFormat::Docx
        ));
        assert!(matches!(
            DocsGenerateTool::parse_format("xlsx").unwrap(),
            DocumentFormat::Xlsx
        ));
        // Etapa 5 PR 2: pdf entra no parse junto com o
        // bump atômico do enum.
        assert!(matches!(
            DocsGenerateTool::parse_format("pdf").unwrap(),
            DocumentFormat::Pdf
        ));
        assert!(DocsGenerateTool::parse_format("xyz").is_err());
    }

    #[tokio::test]
    async fn rejects_unknown_format_in_args() {
        let (tool, _manager) = build_tool_with_wordpro().await;
        let r = tool
            .execute(
                &dummy_ctx(),
                &json!({
                    "spec": {},
                    "output_path": "out.docx",
                    "format": "xyz"
                }),
            )
            .await;
        assert!(!r.ok, "esperava erro, veio {:?}", r);
        assert!(r.error_message.unwrap().contains("não é um DocumentFormat"));
    }

    #[tokio::test]
    async fn rejects_missing_output_path() {
        let (tool, _manager) = build_tool_with_wordpro().await;
        let r = tool
            .execute(
                &dummy_ctx(),
                &json!({
                    "spec": {},
                    "format": "docx"
                }),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_invalid_spec() {
        let (tool, _manager) = build_tool_with_wordpro().await;
        // Spec sem `spec_version` falha no schema.
        let r = tool
            .execute(
                &dummy_ctx(),
                &json!({
                    "spec": { "doc_type": "report" }, // falta spec_version, blocks
                    "output_path": "out.docx",
                    "format": "docx"
                }),
            )
            .await;
        assert!(!r.ok);
    }
    /// Constrói um `ToolContext` dummy para testes que não dependem
    /// do jail. Usado quando o test chama `tool.execute(&ctx, &args)`
    /// direto (sem passar pelo `RunExecutor`). O jail é construído
    /// sobre o `temp_dir` do sistema.
    #[allow(dead_code)]
    fn dummy_ctx() -> frederico_tool_registry::ToolContext {
        use frederico_core::{ConversationId, MessageId, RunId};
        use frederico_tool_registry::{Jail, ToolContext};
        use uuid::Uuid;
        let workspace = std::env::temp_dir().join(format!(
            "frederico-document-kits-dummy-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(&workspace).expect("dummy_ctx: mkdir");
        let jail = Jail::new(&workspace).expect("dummy_ctx: Jail::new");
        ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            jail,
        )
    }
}
