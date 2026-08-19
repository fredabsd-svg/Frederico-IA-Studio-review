//! `milestone.create` — salva o estado do workspace com um nome.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_project_engine::ProjectEngine;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

use super::MarcoDeps;

/// A ferramenta `milestone.create`.
pub struct MilestoneCreateTool {
    pub manifest: ToolManifest,
    deps: MarcoDeps,
}

impl MilestoneCreateTool {
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
        ToolManifestBuilder::new(ToolId::new("milestone.create"), "marcos")
            .version("0.1.0")
            .display_name("Criar marco")
            .description(
                "Salva o estado atual do workspace desta conversa como um marco nomeado — uma \
                 etiqueta no histórico do Git do projeto. Exige que o workspace esteja sob Git e \
                 seja um projeto registrado. Nome repetido é recusado. **Não apaga marco**: a \
                 operação não existe.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Moderate)
            // Aprovação por invocação (ADR-0034): escreve tag e commit
            // no repositório do usuário.
            .requires_user_approval(true)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "nome": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 100,
                        "description": "Nome do marco. Vira etiqueta no Git, então sem espaço, \
                                        `~`, `^`, `:`, `?`, `*`, `[`, `..` nem traço inicial."
                    },
                    "descricao": {
                        "type": "string",
                        "maxLength": 1000,
                        "description": "O que este estado representa. Vira a mensagem da etiqueta."
                    }
                },
                "required": ["nome"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "nome": {"type": "string"},
                    "commit_id": {"type": "string"},
                    "criado_em": {"type": "string"}
                },
                "required": ["nome", "commit_id", "criado_em"]
            })))
            .requires_file_write(true)
            .capability("milestone.write")
            .timeout_ms(30_000)
            .build()
            .expect("manifesto de milestone.create bem-formado")
    }
}

#[async_trait]
impl Tool for MilestoneCreateTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let Some(nome) = arguments.get("nome").and_then(serde_json::Value::as_str) else {
            return ToolResult::err(tool_id, "`nome` é obrigatório");
        };
        let descricao = arguments
            .get("descricao")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        let projeto_id = match super::projeto_da_conversa(&tool_id, &self.deps, ctx).await {
            Ok(id) => id,
            Err(e) => return e,
        };

        let engine = ProjectEngine::new(&self.deps.pool);
        // A conversa de origem fica registrada no marco — é o que o
        // banco guarda e o Git não tem onde guardar.
        let conversa = ctx.conversation_id.as_uuid().to_string();
        match engine
            .criar_marco(
                projeto_id,
                nome,
                descricao,
                &super::autor_do_app(),
                Some(&conversa),
            )
            .await
        {
            Ok(m) => ToolResult::ok(
                tool_id,
                json!({
                    "nome": m.nome,
                    "commit_id": m.commit_id,
                    "criado_em": m.criado_em,
                }),
                Vec::new(),
            ),
            Err(e) => super::erro_para_resultado(tool_id, &e),
        }
    }
}
