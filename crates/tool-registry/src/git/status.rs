//! `git.status` — o que mudou no workspace da conversa.

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `git.status`.
pub struct GitStatusTool {
    pub manifest: ToolManifest,
}

impl Default for GitStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitStatusTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("git.status"), "git")
            .version("0.1.0")
            .display_name("Status do Git")
            .description(
                "Lista os arquivos com mudança pendente no repositório do workspace da conversa: \
                 modificados, novos, apagados, renomeados, não rastreados e em conflito. Cada item \
                 traz `staged`, que diz se a mudança já está no índice (entraria no próximo commit) \
                 ou só na árvore de trabalho. Não recebe caminho de repositório — opera sempre no \
                 workspace da conversa.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Safe)
            // Schema fechado e sem propriedade nenhuma: não há
            // argumento que aponte para outro repositório.
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "branch": {"type": ["string", "null"], "description": "Branch corrente, ou null em HEAD destacado."},
                    "mudancas": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "caminho": {"type": "string", "description": "Caminho relativo à raiz do workspace."},
                                "estado": {"type": "string", "description": "nao_rastreado | novo | modificado | apagado | renomeado | conflito"},
                                "staged": {"type": "boolean", "description": "true se a mudança já está no índice."}
                            },
                            "required": ["caminho", "estado", "staged"]
                        }
                    },
                    "limpo": {"type": "boolean", "description": "true se não há mudança pendente."}
                },
                "required": ["branch", "mudancas", "limpo"]
            })))
            .requires_file_read(true)
            .capability("git.read")
            .timeout_ms(10_000)
            .build()
            .expect("manifesto de git.status bem-formado")
    }
}

// `tool_id` — mesmo helper do `FilesListTool`: evita repetir a
// string do id no `execute` e mantém manifesto como fonte única.
impl GitStatusTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, _arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let repo = match super::abrir_repo(&tool_id, ctx) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let mudancas = match repo.status() {
            Ok(m) => m,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };
        let itens: Vec<serde_json::Value> = mudancas
            .iter()
            .map(|m| {
                json!({
                    "caminho": m.caminho,
                    "estado": m.estado.como_str(),
                    "staged": m.staged,
                })
            })
            .collect();
        let output = json!({
            "branch": repo.branch_atual(),
            "mudancas": itens,
            "limpo": mudancas.is_empty(),
        });
        // `accessed_paths` vazio: a auditoria de caminho existe para
        // registrar leitura de arquivo do usuário, e `status` lê
        // metadados do próprio repositório. Listar aqui a árvore
        // inteira inflaria a trilha sem acrescentar informação.
        ToolResult::ok(tool_id, output, Vec::new())
    }
}
