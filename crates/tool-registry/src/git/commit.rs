//! `git.commit` — registra as mudanças do workspace num commit.

use async_trait::async_trait;
use frederico_core::ToolId;
use frederico_git_engine::Autor;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// Identidade que assina os commits feitos pelo app.
///
/// **Fixa, e não vem do modelo.** O `git2` aceita assinatura
/// explícita, e é isso que o `git-engine` usa — mas *quem* assina
/// não pode ser argumento de ferramenta: um `autor` no schema
/// deixaria o modelo atribuir a mudança a qualquer pessoa, e o
/// histórico do Git é exatamente o registro que se consulta para
/// saber quem fez o quê.
///
/// Também não vem do `user.name`/`user.email` da máquina — seria a
/// dependência de ambiente que o [ADR-0040] §D1 ponto 1 rejeita, com
/// o agravante de o app assinar com o nome do usuário mudanças que
/// não foram ele que escreveu.
///
/// A identidade real do usuário chega com o `github-engine`
/// ([ADR-0041]), que é onde ela passa a existir de fato.
///
/// [ADR-0040]: ../../docs/decisions/0040-git-engine-biblioteca-e-fronteira.md
/// [ADR-0041]: ../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
const AUTOR_NOME: &str = "Frederico IA Studio";
const AUTOR_EMAIL: &str = "frederico-ia-studio@localhost";

/// Tamanho máximo da mensagem de commit, em bytes.
const LIMITE_MENSAGEM: usize = 4_000;

/// A ferramenta `git.commit`.
pub struct GitCommitTool {
    pub manifest: ToolManifest,
}

impl Default for GitCommitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitCommitTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("git.commit"), "git")
            .version("0.1.0")
            .display_name("Commit do Git")
            .description(
                "Registra no índice tudo que mudou no workspace da conversa e cria um commit com \
                 a mensagem informada. Árvore limpa é recusada em vez de gerar commit vazio. \
                 O autor é fixo (`Frederico IA Studio`) — o commit não é atribuído ao usuário nem \
                 a quem o modelo escolher. Não reescreve histórico: sem amend, sem rebase, sem \
                 reset.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::High)
            // Aprovação por invocação (ADR-0034). A fila de aprovação
            // mostra a mensagem e o que mudou — é o que o spec chama
            // de "pedido mostra arquivos e mensagem".
            .requires_user_approval(true)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "mensagem": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": LIMITE_MENSAGEM,
                        "description": "Mensagem do commit. Primeira linha curta e imperativa."
                    }
                },
                "required": ["mensagem"],
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "SHA-1 do commit criado."},
                    "resumo": {"type": "string"},
                    "autor": {"type": "string"},
                    "pais": {"type": "integer", "description": "0 no primeiro commit do repositório."},
                    "arquivos": {"type": "integer", "description": "Quantos arquivos entraram no commit."}
                },
                "required": ["id", "resumo", "autor", "pais", "arquivos"]
            })))
            .requires_file_write(true)
            .capability("git.write")
            .timeout_ms(30_000)
            .build()
            .expect("manifesto de git.commit bem-formado")
    }
}

// `tool_id` — mesmo helper do `FilesListTool`: evita repetir a
// string do id no `execute` e mantém manifesto como fonte única.
impl GitCommitTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let Some(mensagem) = arguments
            .get("mensagem")
            .and_then(serde_json::Value::as_str)
        else {
            return ToolResult::err(tool_id, "`mensagem` é obrigatória");
        };
        if mensagem.trim().is_empty() {
            return ToolResult::err(tool_id, "`mensagem` não pode ser vazia");
        }
        if mensagem.len() > LIMITE_MENSAGEM {
            return ToolResult::err(
                tool_id,
                format!(
                    "mensagem tem {} bytes; o máximo é {LIMITE_MENSAGEM}",
                    mensagem.len()
                ),
            );
        }

        let repo = match super::abrir_repo(&tool_id, ctx) {
            Ok(r) => r,
            Err(e) => return e,
        };

        // Conta o que vai entrar **antes** de commitar: depois do
        // commit o status volta limpo, e o número serve à trilha de
        // auditoria e ao texto do pedido de aprovação.
        let arquivos = repo.status().map(|s| s.len()).unwrap_or(0);

        let autor = Autor {
            nome: AUTOR_NOME.to_string(),
            email: AUTOR_EMAIL.to_string(),
        };
        match repo.commitar(mensagem, &autor) {
            Ok(c) => ToolResult::ok(
                tool_id,
                json!({
                    "id": c.id,
                    "resumo": c.resumo,
                    "autor": c.autor,
                    "pais": c.pais,
                    "arquivos": arquivos,
                }),
                Vec::new(),
            ),
            Err(e) => ToolResult::err(tool_id, e.to_string()),
        }
    }
}
