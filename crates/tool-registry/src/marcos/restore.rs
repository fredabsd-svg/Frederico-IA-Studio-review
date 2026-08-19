//! `milestone.restore` — volta o workspace ao estado de um marco.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_project_engine::ProjectEngine;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

use super::MarcoDeps;

/// A ferramenta `milestone.restore`.
///
/// **`High` e não `Critical`**, ao contrário das ferramentas de
/// GitHub: o [ADR-0042] §D3 garante que restaurar não descarta
/// trabalho — pendências viram marco automático antes, e a
/// restauração é commit novo, não `reset`. O dano máximo é um commit
/// indesejado no histórico, que o usuário desfaz com o Git dele.
/// `Critical` fica reservado para o que não tem desfazer
/// ([ADR-0048] §D2).
///
/// [ADR-0042]: ../../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
/// [ADR-0048]: ../../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md
pub struct MilestoneRestoreTool {
    pub manifest: ToolManifest,
    deps: MarcoDeps,
}

impl MilestoneRestoreTool {
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
        ToolManifestBuilder::new(ToolId::new("milestone.restore"), "marcos")
            .version("0.1.0")
            .display_name("Restaurar marco")
            .description(
                "Volta o workspace desta conversa ao estado de um marco. **Nada é descartado**: \
                 se houver trabalho não salvo, ele vira um marco automático antes, e a \
                 restauração entra como um passo novo no histórico — o que veio depois do marco \
                 continua lá, recuperável. Não reescreve histórico.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::High)
            .requires_user_approval(true)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "nome": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Nome do marco a restaurar. Use `milestone.list` para ver os disponíveis."
                    }
                },
                "required": ["nome"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "commit_id": {"type": "string", "description": "Commit criado pela restauração."},
                    "marco_automatico": {
                        "type": ["string", "null"],
                        "description": "Nome do marco automático criado antes, se havia trabalho pendente. null se a árvore estava limpa."
                    }
                },
                "required": ["commit_id", "marco_automatico"]
            })))
            .requires_file_write(true)
            .capability("milestone.write")
            .timeout_ms(60_000)
            .build()
            .expect("manifesto de milestone.restore bem-formado")
    }
}

#[async_trait]
impl Tool for MilestoneRestoreTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let Some(nome) = arguments.get("nome").and_then(serde_json::Value::as_str) else {
            return ToolResult::err(tool_id, "`nome` é obrigatório");
        };

        let projeto_id = match super::projeto_da_conversa(&tool_id, &self.deps, ctx).await {
            Ok(id) => id,
            Err(e) => return e,
        };

        let engine = ProjectEngine::new(&self.deps.pool);
        match engine
            .restaurar_marco(projeto_id, nome, &super::autor_do_app())
            .await
        {
            Ok(r) => ToolResult::ok(
                tool_id,
                json!({
                    "commit_id": r.commit_id,
                    "marco_automatico": r.marco_automatico.map(|m| m.nome),
                }),
                Vec::new(),
            ),
            Err(e) => super::erro_para_resultado(tool_id, &e),
        }
    }
}
