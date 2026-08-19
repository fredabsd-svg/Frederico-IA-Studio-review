//! `milestone.list` — os marcos do projeto da conversa.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_project_engine::ProjectEngine;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

use super::MarcoDeps;

/// A ferramenta `milestone.list`.
pub struct MilestoneListTool {
    pub manifest: ToolManifest,
    deps: MarcoDeps,
}

impl MilestoneListTool {
    #[must_use]
    pub fn new(deps: MarcoDeps) -> Self {
        Self {
            manifest: Self::build_manifest(),
            deps,
        }
    }

    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("milestone.list"), "marcos")
            .version("0.1.0")
            .display_name("Listar marcos")
            .description(
                "Lista os marcos do projeto do workspace desta conversa, do mais novo para o mais \
                 antigo. Cada item traz nome, descrição, commit, quando foi criado e se foi \
                 automático (criado pelo app antes de uma restauração). Não recebe projeto — \
                 opera sempre no projeto do workspace desta conversa.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Safe)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "marcos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "nome": {"type": "string"},
                                "descricao": {"type": "string"},
                                "commit_id": {"type": "string"},
                                "automatico": {"type": "boolean", "description": "true se criado pelo app antes de uma restauração."},
                                "criado_em": {"type": "string"}
                            },
                            "required": ["nome", "descricao", "commit_id", "automatico", "criado_em"]
                        }
                    },
                    "total": {"type": "integer"}
                },
                "required": ["marcos", "total"]
            })))
            .requires_file_read(true)
            .capability("milestone.read")
            .timeout_ms(10_000)
            .build()
            .expect("manifesto de milestone.list bem-formado")
    }
}

#[async_trait]
impl Tool for MilestoneListTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, _arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let projeto_id = match super::projeto_da_conversa(&tool_id, &self.deps, ctx).await {
            Ok(id) => id,
            Err(e) => return e,
        };

        let engine = ProjectEngine::new(&self.deps.pool);
        let marcos = match engine.listar_marcos(projeto_id).await {
            Ok(m) => m,
            Err(e) => return super::erro_para_resultado(tool_id, &e),
        };

        let itens: Vec<serde_json::Value> = marcos
            .iter()
            .map(|m| {
                json!({
                    "nome": m.nome,
                    "descricao": m.descricao,
                    "commit_id": m.commit_id,
                    "automatico": m.automatico,
                    "criado_em": m.criado_em,
                })
            })
            .collect();
        ToolResult::ok(
            tool_id,
            json!({ "marcos": itens, "total": marcos.len() }),
            Vec::new(),
        )
    }
}
