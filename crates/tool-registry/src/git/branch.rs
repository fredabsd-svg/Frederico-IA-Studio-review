//! `git.branch` — criar e trocar de branch. **Nunca apagar.**

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `git.branch`.
///
/// **Apagar branch não está no schema**, e a ausência é a proteção:
/// o spec exclui a operação, e uma ferramenta que a aceitasse como
/// `acao: "apagar"` dependeria de a validação segurar. Sem o valor
/// no enum, não há entrada.
pub struct GitBranchTool {
    pub manifest: ToolManifest,
}

impl Default for GitBranchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitBranchTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("git.branch"), "git")
            .version("0.1.0")
            .display_name("Branch do Git")
            .description(
                "Lista, cria ou troca de branch no workspace da conversa. `acao` aceita \
                 `listar` (default), `criar` e `trocar`. Criar exige que já exista pelo menos um \
                 commit. Trocar de branch por cima de mudança pendente é recusado, para não \
                 descartar trabalho. **Não apaga branch** — a operação não existe nesta \
                 ferramenta.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Moderate)
            // Aprovação por invocação (ADR-0034): muda o estado da
            // árvore de trabalho que o usuário está vendo.
            .requires_user_approval(true)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "acao": {
                        "type": "string",
                        "enum": ["listar", "criar", "trocar"],
                        "description": "listar (default), criar ou trocar. Apagar não existe."
                    },
                    "nome": {
                        "type": "string",
                        "description": "Nome do branch. Obrigatório para criar e trocar."
                    },
                    "trocar_apos_criar": {
                        "type": "boolean",
                        "description": "Com acao=criar, passa para o branch novo (default true)."
                    }
                },
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "acao": {"type": "string"},
                    "branch_atual": {"type": ["string", "null"]},
                    "branches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "nome": {"type": "string"},
                                "atual": {"type": "boolean"}
                            },
                            "required": ["nome", "atual"]
                        }
                    }
                },
                "required": ["acao", "branch_atual", "branches"]
            })))
            .requires_file_write(true)
            .capability("git.write")
            .timeout_ms(15_000)
            .build()
            .expect("manifesto de git.branch bem-formado")
    }
}

// `tool_id` — mesmo helper do `FilesListTool`: evita repetir a
// string do id no `execute` e mantém manifesto como fonte única.
impl GitBranchTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[async_trait]
impl Tool for GitBranchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let acao = arguments
            .get("acao")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("listar");
        let nome = arguments.get("nome").and_then(serde_json::Value::as_str);

        let repo = match super::abrir_repo(&tool_id, ctx) {
            Ok(r) => r,
            Err(e) => return e,
        };

        let resultado = match acao {
            "listar" => Ok(()),
            "criar" => {
                let Some(nome) = nome else {
                    return ToolResult::err(tool_id, "`nome` é obrigatório para criar branch");
                };
                let trocar = arguments
                    .get("trocar_apos_criar")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                repo.criar_branch(nome, trocar).map(|_| ())
            }
            "trocar" => {
                let Some(nome) = nome else {
                    return ToolResult::err(tool_id, "`nome` é obrigatório para trocar de branch");
                };
                repo.trocar_branch(nome)
            }
            outra => {
                return ToolResult::err(
                    tool_id,
                    format!("ação `{outra}` não existe; use listar, criar ou trocar"),
                );
            }
        };
        if let Err(e) = resultado {
            return ToolResult::err(tool_id, e.to_string());
        }

        let branches = match repo.branches() {
            Ok(b) => b,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };
        let itens: Vec<serde_json::Value> = branches
            .iter()
            .map(|b| json!({ "nome": b.nome, "atual": b.atual }))
            .collect();
        ToolResult::ok(
            tool_id,
            json!({
                "acao": acao,
                "branch_atual": repo.branch_atual(),
                "branches": itens,
            }),
            Vec::new(),
        )
    }
}
