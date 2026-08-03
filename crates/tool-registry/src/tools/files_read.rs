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
//!
//! ## `execute` é `async fn` (Etapa 3 da Fase 5)
//!
//! A mudança para `async fn` no trait `Tool` não muda nada no
//! `files.read` em si (file I/O continua síncrono dentro do
//! `async fn`). Serve para que ferramentas worker-backed como
//! `docs.generate` possam chamar `WorkerHandle::invoke` direto,
//! sem ponte sync→async.
//!
//! ## `execute` recebe `ToolContext` (Etapa 1 da Fase de Ligação)
//!
//! A partir do commit `fase-ligacao/conectar-motor-a-casca` Etapa 1
//! commit 4a, o `Jail` vem do `ctx.jail` (resolvido pelo
//! `RunExecutor` por `ConversationId`). O `FilesReadTool` em si
//! não carrega mais o `Jail` — é construído uma vez por
//! processo (`FilesReadTool::new()` sem args) e usado em todos
//! os runs. **Breaking change** na assinatura do construtor
//! (de `FilesReadTool::new(jail)` para `FilesReadTool::new()`).
//! Testes do crate e do `execution-engine` que usavam o
//! construtor antigo foram migrados — ver
//! `docs/decisions/0022-jail-resolver-v1.md` §D3.

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `files.read`.
///
/// Sem estado: o `Jail` vem do `ToolContext` por chamada (resolvido
/// pelo `RunExecutor` para a conversa corrente). Uma única
/// instância pode ser compartilhada por todos os runs do
/// processo.
pub struct FilesReadTool {
    pub manifest: ToolManifest,
}

impl Default for FilesReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesReadTool {
    /// Cria a ferramenta. Sem args: o `Jail` é entregue por chamada
    /// via `ctx.jail` no `execute`. O construtor é estável entre
    /// runs e processos.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
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

#[async_trait]
impl Tool for FilesReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        // Re-valida o path contra o `ctx.jail` (defesa em
        // profundidade: o validador já rodou contra este mesmo
        // jail no Passo 7 do `validate_tool_call`, mas o
        // `execute` pode ser chamado direto em testes com um
        // jail diferente).
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

        let resolved: PathBuf = match ctx.jail.resolve(std::path::Path::new(path_str)) {
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

    use frederico_core::{ConversationId, MessageId, RunId};
    use uuid::Uuid;

    use crate::workspace::Jail;

    fn setup() -> (Tempdir, FilesReadTool, ToolContext) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        fs::write(dir.join("big.bin"), vec![0xABu8; 100]).unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/inner.txt"), "Inner file").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesReadTool::new();
        // Contexto de teste com IDs dummy. Os testes não
        // exercitam o significado dos IDs — só precisam estar
        // populados porque `ToolContext` exige todos os campos.
        let ctx = ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            jail,
        );
        (dir, tool, ctx)
    }

    struct Tempdir(PathBuf);

    /// Contador atômico: o relógio sozinho não garante unicidade (no Windows
    /// a granularidade de `timestamp_nanos` é grosseira e testes paralelos
    /// podem colidir no mesmo valor, compartilhando o mesmo diretório).
    static TEMPDIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let unique = format!(
                "frederico-tool-registry-files-read-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                n,
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
        let (_d, tool, _ctx) = setup();
        let m = tool.manifest();
        assert_eq!(m.id, ToolId::new("files.read"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Safe);
        assert_eq!(m.category, ToolCategory::Files);
        assert!(m.requires_file_read);
        assert!(!m.requires_file_write);
        assert!(!m.requires_user_approval);
    }

    // Os 8 testes abaixo chamam `tool.execute(...)` (agora `async fn`).
    // `current_thread` runtime é suficiente — o `files.read` não toca
    // em I/O assíncrono (file system é bloqueante, e está bem dentro
    // de um `async fn` curto). Ferramentas worker-backed (Etapa 3 da
    // Fase 5) usam `flavor = "multi_thread"` (ver
    // `crates/tool-registry/src/worker_dispatch.rs`).

    #[tokio::test]
    async fn reads_relative_file() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "hello.txt"})).await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let content = r.output.get("content").and_then(|v| v.as_str()).unwrap();
        assert_eq!(content, "Hello, world!");
        assert_eq!(r.output.get("truncated"), Some(&json!(false)));
        assert_eq!(r.accessed_paths.len(), 1);
    }

    #[tokio::test]
    async fn reads_subdir_file() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "sub/inner.txt"})).await;
        assert!(r.ok);
        assert_eq!(
            r.output.get("content").and_then(|v| v.as_str()).unwrap(),
            "Inner file"
        );
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "../etc/passwd"})).await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("JAIL"));
    }

    #[tokio::test]
    async fn rejects_absolute_path() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "C:\\Windows\\System32\\drivers\\etc\\hosts"}),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_unc_path() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(&ctx, &json!({"path": "\\\\server\\share\\file.txt"}))
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn max_bytes_truncates() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(&ctx, &json!({"path": "big.bin", "max_bytes": 10}))
            .await;
        assert!(r.ok);
        assert_eq!(r.output.get("truncated"), Some(&json!(true)));
        assert_eq!(r.output.get("bytes_read"), Some(&json!(10)));
    }

    #[tokio::test]
    async fn missing_file_is_error() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "nope.txt"})).await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn missing_path_argument_is_error() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({})).await;
        assert!(!r.ok);
    }
}
