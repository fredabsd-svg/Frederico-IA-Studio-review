//! `git.log` — os últimos commits do workspace da conversa.

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

const LIMITE_DEFAULT: usize = 20;
const LIMITE_MAXIMO: usize = 200;

/// A ferramenta `git.log`.
pub struct GitLogTool {
    pub manifest: ToolManifest,
}

impl Default for GitLogTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLogTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("git.log"), "git")
            .version("0.1.0")
            .display_name("Histórico do Git")
            .description(
                "Lista os últimos commits do branch corrente, do mais novo para o mais antigo. \
                 Cada item traz id, resumo da mensagem, autor e número de pais (2 ou mais indica \
                 merge). Repositório sem nenhum commit devolve lista vazia, não erro. \
                 `limite` default 20, máximo 200.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Safe)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "limite": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": LIMITE_MAXIMO,
                        "description": "Quantos commits devolver (default 20, máximo 200)."
                    }
                },
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "commits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "SHA-1 completo do commit."},
                                "resumo": {"type": "string", "description": "Primeira linha da mensagem."},
                                "autor": {"type": "string"},
                                "pais": {"type": "integer", "description": "Número de pais; 2 ou mais indica merge."}
                            },
                            "required": ["id", "resumo", "autor", "pais"]
                        }
                    },
                    "total": {"type": "integer"}
                },
                "required": ["commits", "total"]
            })))
            .requires_file_read(true)
            .capability("git.read")
            .timeout_ms(10_000)
            .build()
            .expect("manifesto de git.log bem-formado")
    }
}

// `tool_id` — mesmo helper do `FilesListTool`: evita repetir a
// string do id no `execute` e mantém manifesto como fonte única.
impl GitLogTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[async_trait]
impl Tool for GitLogTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let limite = arguments
            .get("limite")
            .and_then(serde_json::Value::as_u64)
            .map_or(LIMITE_DEFAULT, |n| (n as usize).min(LIMITE_MAXIMO));

        let repo = match super::abrir_repo(&tool_id, ctx) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let commits = match repo.historico(limite) {
            Ok(c) => c,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let itens: Vec<serde_json::Value> = commits
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "resumo": c.resumo,
                    "autor": c.autor,
                    "pais": c.pais,
                })
            })
            .collect();
        ToolResult::ok(
            tool_id,
            json!({ "commits": itens, "total": commits.len() }),
            Vec::new(),
        )
    }
}
