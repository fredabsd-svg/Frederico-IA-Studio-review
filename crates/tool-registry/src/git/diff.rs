//! `git.diff` — patch unificado das mudanças pendentes.

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// Teto do patch devolvido ao modelo, em bytes.
///
/// Um diff de refatoração grande passa fácil de 1 MB, e despejar
/// isso no contexto custa caro e ainda estoura a janela. O corte é
/// **declarado no output** (`truncado: true`) — patch cortado em
/// silêncio faria o modelo raciocinar sobre metade da mudança
/// achando que viu tudo.
const LIMITE_PATCH_BYTES: usize = 100_000;

/// A ferramenta `git.diff`.
pub struct GitDiffTool {
    pub manifest: ToolManifest,
}

impl Default for GitDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GitDiffTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("git.diff"), "git")
            .version("0.1.0")
            .display_name("Diff do Git")
            .description(
                "Devolve o patch unificado das mudanças pendentes no workspace da conversa. \
                 Com `staged: true`, compara o índice com o último commit — o que entraria no \
                 próximo commit. Com `staged: false` (default), compara a árvore de trabalho com \
                 o índice — o que ficaria de fora. Patches acima de 100 KB são cortados, e o \
                 output marca `truncado: true`.",
            )
            .category(ToolCategory::Git)
            .risk_level(RiskLevel::Safe)
            .input_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "staged": {
                        "type": "boolean",
                        "description": "true = o que entraria no commit (índice vs. HEAD). \
                                        false (default) = o que ficaria de fora (árvore vs. índice)."
                    }
                },
                "additionalProperties": false
            })))
            .output_schema(JsonSchema(json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string", "description": "Patch unificado. Vazio se não há diferença."},
                    "staged": {"type": "boolean", "description": "Qual das duas perguntas foi respondida."},
                    "vazio": {"type": "boolean", "description": "true se não há diferença nenhuma."},
                    "truncado": {"type": "boolean", "description": "true se o patch foi cortado em 100 KB."}
                },
                "required": ["patch", "staged", "vazio", "truncado"]
            })))
            .requires_file_read(true)
            .capability("git.read")
            .timeout_ms(15_000)
            .build()
            .expect("manifesto de git.diff bem-formado")
    }
}

// `tool_id` — mesmo helper do `FilesListTool`: evita repetir a
// string do id no `execute` e mantém manifesto como fonte única.
impl GitDiffTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();
        let staged = arguments
            .get("staged")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let repo = match super::abrir_repo(&tool_id, ctx) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let patch = match repo.diff(staged) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let vazio = patch.is_empty();
        let truncado = patch.len() > LIMITE_PATCH_BYTES;
        let patch = if truncado {
            // Corta em fronteira de caractere: `String::truncate`
            // entra em pânico no meio de um UTF-8 multibyte, e um
            // patch com acento é o caso comum aqui.
            let mut corte = LIMITE_PATCH_BYTES;
            while corte > 0 && !patch.is_char_boundary(corte) {
                corte -= 1;
            }
            patch[..corte].to_string()
        } else {
            patch
        };

        ToolResult::ok(
            tool_id,
            json!({
                "patch": patch,
                "staged": staged,
                "vazio": vazio,
                "truncado": truncado,
            }),
            Vec::new(),
        )
    }
}
