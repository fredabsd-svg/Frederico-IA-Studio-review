//! A ferramenta `files.read` — a única do catálogo inicial da
//! Fase 3 (Etapa 2).
//!
//! Lê um arquivo do workspace. O jail garante que o caminho está
//! dentro do workspace; a aprovação garante que o usuário consentiu
//! (configurada no manifesto via `requires_user_approval`).
//!
//! O jail cobre a defesa contra path traversal do
//! `security-threat-model.md` (ameaça I3). Esta implementação é a
//! camada in-process; workers sidecar (Fase 5) vão trazer a versão
//! "sandboxed" sem trocar o manifesto.

use std::fs;
use std::path::PathBuf;

use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolResult};
use crate::workspace::Jail;

/// A ferramenta `files.read`.
///
/// Construída uma vez por execução (a `Jail` é injetada para
/// isolar a regra de path entre execuções — se necessário, o
/// executor pode criar um `Jail` por `Run`).
pub struct FilesReadTool {
    pub manifest: ToolManifest,
    jail: Jail,
}

impl FilesReadTool {
    /// Cria a ferramenta. A `Jail` deve ser o `ValidationContext.jail`
    /// — o mesmo que validou o `tool_call`.
    pub fn new(jail: Jail) -> Self {
        Self {
            manifest: Self::build_manifest(),
            jail,
        }
    }

    /// Schema de input: `path` (obrigatório, string relativa ao
    /// workspace) e `max_bytes` (opcional, u32 — limita o output).
    fn input_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Caminho do arquivo, RELATIVO à raiz do workspace. \
                                    Não pode conter '..', nem ser absoluto, nem UNC."
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 52428800,
                    "description": "Limite de bytes lidos (default 1 MB, máximo 50 MB)."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }))
    }

    fn output_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Conteúdo do arquivo (UTF-8)."},
                "truncated": {"type": "boolean", "description": "true se `max_bytes` cortou."},
                "bytes_read": {"type": "integer", "description": "Bytes efetivamente lidos."}
            },
            "required": ["content", "truncated", "bytes_read"]
        }))
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("files.read"), "files")
            .version("0.1.0")
            .display_name("Ler arquivo")
            .description(
                "Lê o conteúdo de um arquivo dentro do workspace. O caminho \
                 tem que ser relativo à raiz do workspace; caminhos com '..', \
                 absolutos ou UNC são rejeitados pelo jail. A leitura é \
                 paginada com `max_bytes` (default 1 MB, máximo 50 MB) e \
                 o resultado é retornado em UTF-8.",
            )
            .category(ToolCategory::Files)
            .risk_level(RiskLevel::Safe)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .requires_file_read(true)
            .capability("fs.read")
            .capability("fs.read.text")
            .timeout_ms(5_000)
            .build()
            .expect("manifesto de files.read bem-formado")
    }
}

impl Tool for FilesReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn execute(&self, arguments: &serde_json::Value) -> ToolResult {
        // Re-valida o path (defesa em profundidade: o validador já
        // rodou, mas o `execute` pode ser chamado direto em testes).
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolResult::err(
                    ToolId::new("files.read"),
                    "argumento 'path' ausente ou não-string",
                )
            });

        let path_str = match path_str {
            Ok(s) => s,
            Err(r) => return r,
        };

        let resolved: PathBuf = match self.jail.resolve(std::path::Path::new(path_str)) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(ToolId::new("files.read"), e.to_string()),
        };

        let max_bytes: usize = arguments
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1024 * 1024); // 1 MB

        // Lê o arquivo. Se for maior que `max_bytes`, lê só os
        // primeiros `max_bytes` e marca `truncated: true`.
        match fs::read(&resolved) {
            Ok(bytes) => {
                let total = bytes.len();
                let truncated = total > max_bytes;
                let slice = if truncated {
                    &bytes[..max_bytes]
                } else {
                    &bytes[..]
                };
                // Tenta UTF-8; se falhar, devolve como latin-1
                // escapado (raro, mas possível em workspaces com
                // arquivos antigos).
                let content = match std::str::from_utf8(slice) {
                    Ok(s) => s.to_string(),
                    Err(_) => slice.iter().map(|b| *b as char).collect::<String>(),
                };
                ToolResult::ok(
                    ToolId::new("files.read"),
                    json!({
                        "content": content,
                        "truncated": truncated,
                        "bytes_read": if truncated { max_bytes } else { total },
                    }),
                    vec![resolved],
                )
            }
            Err(e) => ToolResult::err(
                ToolId::new("files.read"),
                format!("não consegui ler o arquivo: {e}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (Tempdir, FilesReadTool) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        fs::write(dir.join("big.bin"), vec![0xABu8; 100]).unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/inner.txt"), "Inner file").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesReadTool::new(jail);
        (dir, tool)
    }

    struct Tempdir(PathBuf);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let unique = format!(
                "frederico-tool-registry-files-read-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            let dir = base.join(unique);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl std::ops::Deref for Tempdir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tempdir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    use std::path::Path;

    #[test]
    fn manifest_is_well_formed() {
        let (_d, tool) = setup();
        let m = tool.manifest();
        assert_eq!(m.id, ToolId::new("files.read"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Safe);
        assert_eq!(m.category, ToolCategory::Files);
        assert!(m.requires_file_read);
        assert!(!m.requires_file_write);
        assert!(!m.requires_user_approval);
    }

    #[test]
    fn reads_relative_file() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "hello.txt"}));
        assert!(r.ok, "erro: {:?}", r.error_message);
        let content = r.output.get("content").and_then(|v| v.as_str()).unwrap();
        assert_eq!(content, "Hello, world!");
        assert_eq!(r.output.get("truncated"), Some(&json!(false)));
        assert_eq!(r.accessed_paths.len(), 1);
    }

    #[test]
    fn reads_subdir_file() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "sub/inner.txt"}));
        assert!(r.ok);
        assert_eq!(
            r.output.get("content").and_then(|v| v.as_str()).unwrap(),
            "Inner file"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "../etc/passwd"}));
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("JAIL"));
    }

    #[test]
    fn rejects_absolute_path() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "C:\\Windows\\System32\\drivers\\etc\\hosts"}));
        assert!(!r.ok);
    }

    #[test]
    fn rejects_unc_path() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "\\\\server\\share\\file.txt"}));
        assert!(!r.ok);
    }

    #[test]
    fn max_bytes_truncates() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "big.bin", "max_bytes": 10}));
        assert!(r.ok);
        assert_eq!(r.output.get("truncated"), Some(&json!(true)));
        assert_eq!(r.output.get("bytes_read"), Some(&json!(10)));
    }

    #[test]
    fn missing_file_is_error() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({"path": "nope.txt"}));
        assert!(!r.ok);
    }

    #[test]
    fn missing_path_argument_is_error() {
        let (_d, tool) = setup();
        let r = tool.execute(&json!({}));
        assert!(!r.ok);
    }
}
