//! `github.create_pr` — abre um pull request.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_github_engine::RepoRef;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

use super::GithubDeps;

/// A ferramenta `github.create_pr`.
pub struct GithubCreatePrTool {
    pub manifest: ToolManifest,
    deps: GithubDeps,
}

impl GithubCreatePrTool {
    #[must_use]
    pub fn new(deps: GithubDeps) -> Self {
        Self {
            manifest: Self::build_manifest(),
            deps,
        }
    }

    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("github.create_pr"), "github")
            .version("0.1.0")
            .display_name("Abrir pull request")
            .description(
                "Abre um pull request no GitHub, da branch de origem para a de destino. Só \
                 funciona para repositórios e branches autorizados na matriz. Um PR aberto \
                 notifica revisores e permanece no histórico mesmo depois de fechado — não há \
                 desfazer.",
            )
            .category(ToolCategory::GitHub)
            .risk_level(RiskLevel::Critical)
            .requires_user_approval(true)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "repositorio": {
                        "type": "string",
                        "pattern": "^[^/]+/[^/]+$",
                        "description": "Repositório no formato `owner/repo`."
                    },
                    "origem": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Branch de onde vem o trabalho (`head`). É esta que a matriz autoriza."
                    },
                    "destino": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Branch de destino (`base`), normalmente `main`. Abrir PR não escreve nela."
                    },
                    "titulo": {"type": "string", "minLength": 1, "maxLength": 256},
                    "corpo": {"type": "string", "maxLength": 60000}
                },
                "required": ["repositorio", "origem", "destino", "titulo"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "numero": {"type": "integer"},
                    "url": {"type": "string"},
                    "titulo": {"type": "string"}
                },
                "required": ["numero", "url", "titulo"]
            })))
            .requires_network(true)
            .capability("github.create_pr")
            .timeout_ms(60_000)
            .build()
            .expect("manifesto de github.create_pr bem-formado")
    }
}

#[async_trait]
impl Tool for GithubCreatePrTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, _ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let campos = ["repositorio", "origem", "destino", "titulo"];
        let mut valores = Vec::new();
        for campo in campos {
            match arguments.get(campo).and_then(serde_json::Value::as_str) {
                Some(v) if !v.trim().is_empty() => valores.push(v),
                _ => return ToolResult::err(tool_id, format!("`{campo}` é obrigatório")),
            }
        }
        let (repo_texto, origem, destino, titulo) =
            (valores[0], valores[1], valores[2], valores[3]);
        let corpo = arguments
            .get("corpo")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        let repo = match RepoRef::parse(repo_texto) {
            Ok(r) => r,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        match self
            .deps
            .engine
            .criar_pr(&repo, origem, destino, titulo, corpo)
            .await
        {
            Ok(pr) => ToolResult::ok(
                tool_id,
                json!({ "numero": pr.numero, "url": pr.url, "titulo": pr.titulo }),
                Vec::new(),
            ),
            Err(e) => ToolResult::err(tool_id, e.to_string()),
        }
    }
}
