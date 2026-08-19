//! `github.push` — empurra uma branch do workspace para o GitHub.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_github_engine::RepoRef;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

use super::GithubDeps;

/// A ferramenta `github.push`.
pub struct GithubPushTool {
    pub manifest: ToolManifest,
    deps: GithubDeps,
}

impl GithubPushTool {
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
        ToolManifestBuilder::new(ToolId::new("github.push"), "github")
            .version("0.1.0")
            .display_name("Enviar para o GitHub")
            .description(
                "Envia uma branch do workspace desta conversa para o GitHub. Só funciona para \
                 repositórios, branches e operações autorizados na matriz — sem entrada, é \
                 recusado. **Não faz envio forçado**: a operação não existe nesta ferramenta, em \
                 nenhuma forma. O remoto configurado precisa apontar para o repositório \
                 autorizado, senão é recusado.",
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
                        "description": "Repositório no formato `owner/repo`. Tem que estar na matriz de autorização."
                    },
                    "branch": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Branch local a enviar. Tem que estar autorizada na matriz — branch protegida exige menção nominal."
                    },
                    "remoto": {
                        "type": "string",
                        "description": "Nome do remoto (default `origin`)."
                    }
                },
                "required": ["repositorio", "branch"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "repositorio": {"type": "string"},
                    "branch": {"type": "string"},
                    "commits": {"type": "integer", "description": "Commits que estavam à frente do remoto."}
                },
                "required": ["repositorio", "branch", "commits"]
            })))
            .requires_network(true)
            .capability("github.push")
            .timeout_ms(120_000)
            .build()
            .expect("manifesto de github.push bem-formado")
    }
}

#[async_trait]
impl Tool for GithubPushTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let Some(repo_texto) = arguments
            .get("repositorio")
            .and_then(serde_json::Value::as_str)
        else {
            return ToolResult::err(tool_id, "`repositorio` é obrigatório");
        };
        let Some(branch) = arguments.get("branch").and_then(serde_json::Value::as_str) else {
            return ToolResult::err(tool_id, "`branch` é obrigatória");
        };
        let remoto = arguments
            .get("remoto")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("origin");

        let repo = match RepoRef::parse(repo_texto) {
            Ok(r) => r,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        // O workspace é sempre o da conversa. `repositorio` nomeia o
        // alvo **da matriz**, não um caminho — não há como apontar
        // esta ferramenta para outro diretório.
        match self
            .deps
            .engine
            .push(ctx.jail.root(), &repo, branch, remoto)
            .await
        {
            Ok(feito) => ToolResult::ok(
                tool_id,
                json!({
                    "repositorio": feito.repo,
                    "branch": feito.branch,
                    "commits": feito.commits,
                }),
                Vec::new(),
            ),
            Err(e) => ToolResult::err(tool_id, e.to_string()),
        }
    }
}
