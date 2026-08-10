//! A ferramenta `files.list` — listagem de diretório dentro do
//! workspace (Etapa 5 do Phase 7, [ADR-0035] §"O que esta entrega").
//!
//! Equivalente in-process do `ls`: dado um `path` (relativo à raiz
//! do workspace), devolve os entries imediatamente dentro daquele
//! diretório — sem recursão, sem ler conteúdo, sem seguir symlinks
//! pra fora do jail.
//!
//! **Barreira primária:** `Jail::resolve(path)` (igual `files.read`).
//! O `validate_tool_call` Passo 7 já rodou contra o mesmo jail; o
//! `Tool::execute` revalida por defesa em profundidade.
//!
//! **Sem `requires_user_approval`:** listar diretório é read-only
//! (não muda estado do workspace, não vaza dados pra fora do jail).
//! `RiskLevel::Safe` é coerente com `files.read`.
//!
//! **Diferente de `files.read`:** `files.list` não devolve conteúdo
//! (só metadados — `name`, `is_dir`, `size` em bytes). Conteúdo é
//! `files.read`. Isso evita o caso "agente pede listagem esperando
//! conteúdo e o tool despeja 50 MB de log no message_event".
//!
//! **Sort:** alfabético (case-insensitive) por nome. Determinístico
//! entre runs — importante pro `before/after_sha256` do `files.edit`
//! em diretórios versionados (se o `files.list` fosse nondeterminístico,
//! o `files.edit` que recebe a listagem e opera em um entry teria
//! reprodutibilidade dependente de FS).
//!
//! Ver ADR-0035 (`docs/decisions/0035-fase-7-file-ops-overwrite-semantics.md`)
//! §"Consequências" — `FilesListTool` é uma das 3 ferramentas
//! prometidas no ADR (junto com `FilesWriteTool` e `FilesEditTool`).
//!
//! [ADR-0035]: ../docs/decisions/0035-fase-7-file-ops-overwrite-semantics.md
//!
//! ## Por que `Safe` (não `Moderate` como `files.write`)
//!
//! A regra do `RiskLevel` no `ToolManifest` (spec §7.1) é
//! "operação que muda estado do usuário" para `Moderate`.
//! `files.list` **não muda estado** — é read-only equivalente do
//! `ls` (mesma família de `files.read` que é `Safe`).

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use frederico_core::ToolId;
use serde_json::json;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `files.list`.
///
/// Sem estado: o `Jail` vem do `ToolContext` por chamada (mesma
/// fronteira do `FilesReadTool` da Etapa 2 da Fase 3 — Jail
/// resolvido pelo `RunExecutor` por `ConversationId`).
pub struct FilesListTool {
    pub manifest: ToolManifest,
}

impl Default for FilesListTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesListTool {
    /// Cria a ferramenta. Sem args — `Jail` vem do `ctx.jail`
    /// no `execute`. Construtor é estável entre runs e processos.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    /// Schema de input: `path` (opcional, default = raiz do jail)
    /// + `max_entries` (opcional, default 1000, max 10000).
    ///
    /// `recursive` é deliberadamente **não** exposto na v1 —
    /// `files.list` é não-recursivo. Recursão vira com a Etapa 7
    /// (UI de "procurar em subdiretórios") e vira tool separada
    /// (`files.tree` ou similar), porque recursão muda a forma
    /// da barreira de jail (precisa de canonicalização por entry).
    fn input_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Caminho do diretório, RELATIVO à raiz do workspace. \
                                    Default: raiz do workspace. Não pode conter '..', \
                                    nem ser absoluto, nem UNC."
                },
                "max_entries": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10000,
                    "description": "Limite de entries retornados (default 1000, máximo 10000). \
                                    Se o diretório tem mais, o output marca `truncated: true`."
                }
            },
            "additionalProperties": false
        }))
    }

    fn output_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Caminho do diretório listado (relativo ao jail)."},
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "Nome do entry (não o path completo)."},
                            "is_dir": {"type": "boolean", "description": "true se diretório, false se arquivo ou symlink quebrado."},
                            "size": {"type": "integer", "description": "Tamanho em bytes. 0 para diretórios (ou symlinks cuja meta não conseguimos ler)."}
                        },
                        "required": ["name", "is_dir", "size"]
                    },
                    "description": "Entries do diretório, ordenados alfabeticamente (case-insensitive) por nome."
                },
                "truncated": {"type": "boolean", "description": "true se o diretório tem mais entries que `max_entries`."},
                "entry_count": {"type": "integer", "description": "Quantos entries foram devolvidos (≤ max_entries)."}
            },
            "required": ["path", "entries", "truncated", "entry_count"]
        }))
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("files.list"), "files")
            .version("0.1.0")
            .display_name("Listar diretório")
            .description(
                "Lista os entries (arquivos + subdiretórios) de um diretório dentro do workspace. \
                 Não recursivo. Cada entry devolve `name`, `is_dir`, `size`. Ordenado alfabeticamente \
                 (case-insensitive). O caminho tem que ser relativo à raiz do workspace; caminhos \
                 com '..', absolutos ou UNC são rejeitados pelo jail. Não lê conteúdo dos arquivos \
                 (use `files.read` para isso).",
            )
            .category(ToolCategory::Files)
            .risk_level(RiskLevel::Safe)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .requires_file_read(true)
            .capability("fs.list")
            .timeout_ms(5_000)
            .build()
            .expect("manifesto de files.list bem-formado")
    }
}

#[async_trait]
impl Tool for FilesListTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();

        // `path` é opcional — default é a raiz do jail.
        let path_str = arguments.get("path").and_then(|v| v.as_str());
        let request_path = std::path::Path::new(path_str.unwrap_or("."));

        // Re-valida contra o `ctx.jail` (defesa em profundidade: o
        // validador já rodou contra o mesmo jail no Passo 7 do
        // `validate_tool_call`; o `execute` revalida porque pode
        // ser chamado direto em testes com jail diferente).
        // `resolve` (não `resolve_allowing_nonexistent`) — listagem
        // exige que o diretório exista.
        let resolved: PathBuf = match ctx.jail.resolve(request_path) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(tool_id, e.to_string()),
        };

        let max_entries: usize = arguments
            .get("max_entries")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1000);

        // Lê os entries. Erro de I/O (permissão, dir sumiu
        // entre `resolve` e `read_dir`) vira `ToolResult::err`.
        let read_dir = match fs::read_dir(&resolved) {
            Ok(rd) => rd,
            Err(e) => {
                return ToolResult::err(tool_id, format!("não consegui ler o diretório: {e}"));
            }
        };

        // Coleta em Vec pra poder ordenar e truncar. Não lê
        // conteúdo dos arquivos (metadados via `entry.metadata()`,
        // que falha gracefully se o arquivo sumiu — vira size=0).
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for entry_res in read_dir {
            let entry = match entry_res {
                Ok(e) => e,
                Err(_) => continue, // pula entries que não conseguiu ler
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.map(|m| m.len()).unwrap_or(0);
            entries.push(json!({
                "name": name,
                "is_dir": is_dir,
                "size": size,
            }));
        }

        // Sort alfabético case-insensitive por nome. `to_lowercase`
        // em PT-BR com acentos funciona pros casos comuns (a, á, b, c, ç, d, ...);
        // para casos exóticos (turco, etc.) o sort é aproximado
        // mas determinístico entre runs do mesmo locale — suficiente
        // para o contrato "resultado reprodutível".
        entries.sort_by(|a, b| {
            let na = a
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let nb = b
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            na.cmp(&nb)
        });

        let total = entries.len();
        let truncated = total > max_entries;
        if truncated {
            entries.truncate(max_entries);
        }

        // O `path` retornado é o path **relativo** ao jail — não o
        // canonical absoluto (que vaza a estrutura de diretórios
        // do user, ex.: `/home/alice/projects/foo` em vez de `.`).
        // Calcula via `strip_prefix` do `root_canonical` do jail.
        let display_path = resolved
            .strip_prefix(ctx.jail.root_canonical())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let display_path = if display_path.is_empty() {
            ".".to_string()
        } else {
            display_path.replace('\\', "/")
        };

        ToolResult::ok(
            tool_id,
            json!({
                "path": display_path,
                "entries": entries,
                "truncated": truncated,
                "entry_count": entries.len(),
            }),
            vec![resolved],
        )
    }
}

// `tool_id` helper — evita repetir `ToolId::new("files.list")` no
// `execute`. Mesmo padrão do `FilesReadTool` (que usa
// `self.manifest.id` inline). `FilesListTool` segue o mesmo.
impl FilesListTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use frederico_core::{ConversationId, MessageId, RunId};
    use uuid::Uuid;

    use crate::workspace::Jail;

    fn setup() -> (Tempdir, FilesListTool, ToolContext) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/inner.txt"), "Inner").unwrap();
        fs::write(dir.join("zzz.txt"), "Last").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesListTool::new();
        let ctx = ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            jail,
        );
        (dir, tool, ctx)
    }

    struct Tempdir(PathBuf);

    static TEMPDIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let unique = format!(
                "frederico-tool-registry-files-list-{}-{}-{}",
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
        assert_eq!(m.id, ToolId::new("files.list"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Safe);
        assert_eq!(m.category, ToolCategory::Files);
        assert!(m.requires_file_read);
        assert!(!m.requires_user_approval);
    }

    #[tokio::test]
    async fn lists_root_with_default_path() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({})).await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let entries = r.output.get("entries").and_then(|v| v.as_array()).unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e.get("name").and_then(|n| n.as_str()).unwrap())
            .collect();
        // Sort case-insensitive: hello.txt, sub, zzz.txt
        assert_eq!(names, vec!["hello.txt", "sub", "zzz.txt"]);
        // `is_dir` discrimina
        let sub = entries
            .iter()
            .find(|e| e.get("name").and_then(|n| n.as_str()) == Some("sub"))
            .unwrap();
        assert_eq!(sub.get("is_dir"), Some(&json!(true)));
        let hello = entries
            .iter()
            .find(|e| e.get("name").and_then(|n| n.as_str()) == Some("hello.txt"))
            .unwrap();
        assert_eq!(hello.get("is_dir"), Some(&json!(false)));
        // path retornado é relativo (`"."` quando lista o root)
        assert_eq!(r.output.get("path"), Some(&json!(".")));
        // truncated false (3 entries, max default 1000)
        assert_eq!(r.output.get("truncated"), Some(&json!(false)));
        assert_eq!(r.output.get("entry_count"), Some(&json!(3)));
    }

    #[tokio::test]
    async fn lists_subdirectory() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "sub"})).await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let entries = r.output.get("entries").and_then(|v| v.as_array()).unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e.get("name").and_then(|n| n.as_str()).unwrap())
            .collect();
        assert_eq!(names, vec!["inner.txt"]);
        assert_eq!(r.output.get("path"), Some(&json!("sub")));
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "../etc"})).await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("JAIL"));
    }

    #[tokio::test]
    async fn rejects_absolute_path() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "C:\\Windows"})).await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_unc_path() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(&ctx, &json!({"path": "\\\\server\\share"}))
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn nonexistent_directory_is_error() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "nope"})).await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn max_entries_truncates() {
        // Cria 5 entries; pede max=2; verifica truncated=true.
        let dir = Tempdir::new();
        for i in 0..5 {
            fs::write(dir.join(format!("file{i}.txt")), "x").unwrap();
        }
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesListTool::new();
        let ctx = ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            jail,
        );
        let r = tool.execute(&ctx, &json!({"max_entries": 2})).await;
        assert!(r.ok);
        assert_eq!(r.output.get("truncated"), Some(&json!(true)));
        assert_eq!(r.output.get("entry_count"), Some(&json!(2)));
        let entries = r.output.get("entries").and_then(|v| v.as_array()).unwrap();
        assert_eq!(entries.len(), 2);
    }
}
