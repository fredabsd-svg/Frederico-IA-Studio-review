//! A ferramenta `files.edit` — find/replace literal com atomicidade
//! (Etapa 5 do Phase 7, ADR-0035 D4).
//!
//! **O coração da regra "falha se o conteúdo mudou":** o tool_call
//! recebe `expected_sha256` (opcional) com o SHA-256 do arquivo no
//! momento em que o `files.read` (ou último `files.edit`) o viu.
//! Se o `actual_sha256` do arquivo no momento do edit **não** bate,
//! o tool_call **recusa** em vez de aplicar a substituição no lugar
//! errado. Sem isso, o modelo que leu `config.toml` minutos atrás e
//! agora faz edit pode estar sobrescrevendo mudanças que outra
//! invocação (ou o usuário) fez no meio — corrompendo o arquivo
//! silenciosamente.
//!
//! **Regras (ADR-0035 D4):**
//!
//! - `find` é **texto literal**, não regex. Regex silenciosamente
//!   casa mais do que o usuário espera (`.` `*` `+` `?` `(` `[` `\`),
//!   e 95% do uso de `files.edit` é "substituir definição de
//!   função" / "mudar string de config" / "ajustar import".
//!   Regex fica pra Fase 8 (com `files.regex_edit` + UI de
//!   "test pattern").
//! - `find` deve aparecer **exatamente uma vez**, a menos que
//!   `replace_all: true`. 0 matches → `PatternNotFound`. 2+
//!   matches sem `replace_all` → `AmbiguousMatch` (recusa; o caller
//!   passa `replace_all: true` OU refina o `find` pra ser único).
//! - `replace` preserva indentação: pega o whitespace do início da
//!   linha do primeiro match e prepend em cada linha do `replace`.
//!   O uso comum é "muda a definição desta função" e a substituição
//!   mantém a indentação do código existente.
//! - Operação atômica (D1): lê arquivo, calcula replace em memória,
//!   escreve via protocolo atômico do `files.write` (temp + rename).
//!   Se o conteúdo mudou entre o read e o write (race com outro
//!   `files.edit` paralelo), o `expected_sha256` recusa; sem ele,
//!   a última escrita vence.
//!
//! **Risco conhecido (Etapa 5+ do Phase 7):** conflito read-modify-write
//! entre `files.edit` paralelo no mesmo path — a última escrita vence.
//! A Etapa 8 (com UI de projeto) introduz lock por arquivo se virar
//! problema real. Ver ADR-0035 §"Pendências".
//!
//! Ver [ADR-0035] §"Decisões" e §"Alternativas consideradas".
//!
//! [ADR-0035]: ../docs/decisions/0035-fase-7-file-ops-overwrite-semantics.md

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use frederico_core::ToolId;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::manifest::{JsonSchema, RiskLevel, ToolCategory, ToolManifest, ToolManifestBuilder};
use crate::tools::{Tool, ToolContext, ToolResult};

/// A ferramenta `files.edit`.
///
/// Sem estado: `Jail` vem do `ToolContext` por chamada (mesma
/// fronteira do `FilesReadTool` / `FilesWriteTool`).
pub struct FilesEditTool {
    pub manifest: ToolManifest,
}

impl Default for FilesEditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesEditTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Self::build_manifest(),
        }
    }

    fn input_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Caminho do arquivo, RELATIVO à raiz do workspace. \
                                    Não pode conter '..', nem ser absoluto, nem UNC. \
                                    O arquivo DEVE existir (use `files.write` com \
                                    `overwrite: true` para criar)."
                },
                "find": {
                    "type": "string",
                    "description": "Texto LITERAL a encontrar. Não é regex. \
                                    Deve aparecer exatamente 1x (ou Nx com \
                                    `replace_all: true`). 0 matches -> `PatternNotFound`. \
                                    2+ matches sem `replace_all` -> `AmbiguousMatch`."
                },
                "replace": {
                    "type": "string",
                    "description": "Texto de substituição. A indentação do primeiro \
                                    char de `find` na linha é preservada (prepended em \
                                    cada linha do `replace`). String vazia é válida \
                                    (deleta o `find`)."
                },
                "replace_all": {
                    "type": "boolean",
                    "default": false,
                    "description": "Substituir TODAS as ocorrências de `find`. Sem esse \
                                    flag, mais de 1 match é `AmbiguousMatch` (recusa). \
                                    Use para renomear variável em todo o arquivo, etc."
                },
                "expected_sha256": {
                    "type": "string",
                    "description": "SHA-256 do arquivo no momento em que o caller \
                                    (files.read ou files.edit anterior) o leu. **Se \
                                    passado, o tool RECUSA se o `actual_sha256` não \
                                    bater** — defesa contra race read-modify-write. \
                                    Hex lowercase, 64 chars. Opcional: omitir = \
                                    'aceito risco de race', mas a Etapa 5+ UI \
                                    (ADR-0034 D5) vai exigir."
                },
                "create_parents": {
                    "type": "boolean",
                    "default": false,
                    "description": "Criar o arquivo se não existir (com `find` e \
                                    `replace` virando o conteúdo inicial). Default \
                                    `false` — se o arquivo não existe, retorna \
                                    `FileNotFound`. (Diferente de `files.write`, \
                                    aqui `create_parents: true` é sugar pra \
                                    'se não existe, criar com find+replace como \
                                    conteúdo'.)"
                }
            },
            "required": ["path", "find", "replace"],
            "additionalProperties": false
        }))
    }

    fn output_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "before_sha256": {"type": "string"},
                "after_sha256": {"type": "string"},
                "replacements": {"type": "integer", "description": "Número de substituições aplicadas (1 sem replace_all, N com)."},
                "backup_path": {"type": ["string", "null"]}
            },
            "required": ["path", "before_sha256", "after_sha256", "replacements", "backup_path"]
        }))
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("files.edit"), "files")
            .version("0.1.0")
            .display_name("Editar arquivo (find/replace)")
            .description(
                "Encontra `find` (texto literal, não regex) em um arquivo e substitui \
                 por `replace`, **atomicamente** (temp + rename, ADR-0035 D1). \
                 `find` deve aparecer exatamente 1x; ou Nx com `replace_all: true`. \
                 Preserva indentação do `find` na linha. Recusa se o conteúdo \
                 mudou desde a leitura (via `expected_sha256`) — sem isso, \
                 o modelo corrompe arquivo silenciosamente. Cria backup `.bak` \
                 automático. **Requer aprovação do usuário** (Passo 9 do \
                 validador — destrutivo).",
            )
            .category(ToolCategory::Files)
            .risk_level(RiskLevel::Moderate)
            .requires_file_write(true)
            .requires_user_approval(true)
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .capability("fs.write")
            .capability("fs.write.text")
            .timeout_ms(30_000)
            .build()
            .expect("manifesto de files.edit bem-formado")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// Computa o `replace` final com indentação preservada. Pega
    /// o whitespace do início da linha do primeiro match (medido
    /// a partir do `start` do `find` no conteúdo) e prepend em
    /// cada linha do `replace`. Se o `find` começa no meio da
    /// linha (não tem whitespace antes), `indent` é `""` e
    /// `replace` é usado como veio.
    fn indent_preserved(replace: &str, content: &str, start: usize) -> String {
        // line_start = posição do \n imediatamente antes de `start`,
        // ou 0 se `start` está no início do conteúdo.
        let line_start = content[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prefix = &content[line_start..start];
        // Só prepend se `prefix` é whitespace puro. Se o caller
        // passou `find` que começa no meio de uma palavra (sem
        // indent), `prefix` tem código → não prepend (deixa o
        // caller responsável pela indentação do `replace`).
        if !prefix.chars().all(|c| c.is_whitespace()) {
            return replace.to_string();
        }
        if prefix.is_empty() {
            return replace.to_string();
        }
        // Prepend `prefix` em cada linha do `replace`.
        let mut out = String::with_capacity(replace.len() + prefix.len() * 2);
        for (i, line) in replace.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if !line.is_empty() {
                out.push_str(prefix);
                out.push_str(line);
            } else {
                // Linha vazia — não prepend (mantém a quebra limpa)
                // mas isso só acontece se `replace` termina com \n.
            }
        }
        out
    }

    /// Conta quantas vezes `needle` aparece em `haystack` sem
    /// regex. `find` é texto literal (ADR-0035 D4).
    fn count_matches(haystack: &str, needle: &str) -> usize {
        if needle.is_empty() {
            return 0; // edge case: find vazio é PatternNotFound
        }
        haystack.match_indices(needle).count()
    }

    /// Backup path (mesma lógica do `FilesWriteTool`).
    fn backup_path_for(path: &std::path::Path) -> PathBuf {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let simple = parent.join(format!("{file_name}.bak"));
        if !simple.exists() {
            return simple;
        }
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        parent.join(format!("{file_name}.bak.{ts}"))
    }

    /// Temp path no mesmo dir (D1).
    fn temp_path_for(path: &std::path::Path) -> PathBuf {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let uuid = Uuid::new_v4();
        parent.join(format!("{file_name}.{uuid}.tmp"))
    }

    fn cleanup_temp(temp: &std::path::Path) {
        let _ = fs::remove_file(temp);
    }

    /// Escreve o `content` atomicamente (D1 do ADR-0035). Helper
    /// compartilhado entre `files.write` e `files.edit` — mas
    /// pra evitar ciclo de dependência entre tools, está duplicado
    /// aqui. A Etapa 5.X pode extrair pra `tools::atomic_write`.
    fn write_atomic(path: &std::path::Path, content: &str) -> Result<(), String> {
        let temp_path = Self::temp_path_for(path);
        let write_result: Result<(), String> = (|| {
            let mut file = match fs::File::create(&temp_path) {
                Ok(f) => f,
                Err(e) => return Err(format!("create temp falhou: {e}")),
            };
            if let Err(e) = file.write_all(content.as_bytes()) {
                return Err(format!("write temp falhou: {e}"));
            }
            if let Err(e) = file.sync_all() {
                return Err(format!("fsync temp falhou: {e}"));
            }
            drop(file);
            if let Some(parent) = temp_path.parent() {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
            if let Err(e) = fs::rename(&temp_path, path) {
                return Err(format!("rename atômico falhou: {e}"));
            }
            Ok(())
        })();
        if write_result.is_err() {
            Self::cleanup_temp(&temp_path);
        }
        write_result
    }
}

#[async_trait]
impl Tool for FilesEditTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();

        // 1. Parse args.
        let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err(tool_id, "argumento 'path' ausente ou não-string"),
        };
        let find = match arguments.get("find").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err(tool_id, "argumento 'find' ausente ou não-string"),
        };
        let replace = match arguments.get("replace").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::err(tool_id, "argumento 'replace' ausente ou não-string"),
        };
        let replace_all = arguments
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let expected_sha256 = arguments
            .get("expected_sha256")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let create_parents = arguments
            .get("create_parents")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 2. Resolve o path.
        let requested = std::path::Path::new(path_str);
        let resolved: PathBuf = if create_parents {
            match ctx.jail.resolve_or_create_parents(requested) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(tool_id, e.to_string()),
            }
        } else {
            // `files.edit` exige arquivo existente (default).
            // Usa `resolve` (não `resolve_allowing_nonexistent`)
            // — se não existe, erro.
            match ctx.jail.resolve(requested) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(tool_id, e.to_string()),
            }
        };

        // 3. Lê o arquivo.
        let original = match fs::read_to_string(&resolved) {
            Ok(s) => s,
            Err(e) => {
                if create_parents && e.kind() == std::io::ErrorKind::NotFound {
                    // create_parents:true + arquivo não existe →
                    // cria o arquivo com `find + replace` como
                    // conteúdo inicial (não há indentação a
                    // preservar, arquivo é zero-linhas).
                    if find.is_empty() {
                        return ToolResult::err(
                            tool_id,
                            "`find` vazio + `create_parents: true` é ambíguo \
                             (não tem onde procurar); use `files.write` direto",
                        );
                    }
                    // Verifica que `find` e `replace` casam (caso
                    // `replace_all`, seriam múltiplas — mas arquivo
                    // é zero-linhas, então 0 matches → PatternNotFound).
                    return self.create_file_from_scratch(tool_id, &resolved, find, replace, ctx);
                }
                return ToolResult::err(tool_id, format!("não consegui ler o arquivo: {e}"));
            }
        };

        // 4. before_sha256.
        let before_sha256 = Self::sha256_hex(original.as_bytes());

        // 5. **Regra do user: falha se o conteúdo mudou.** Se
        //    `expected_sha256` foi passado, confere. Recusa em
        //    vez de aplicar no lugar errado.
        if let Some(expected) = &expected_sha256 {
            if expected != &before_sha256 {
                return ToolResult::err(
                    tool_id,
                    format!(
                        "conteúdo mudou desde a leitura: caller disse `{expected}`, \
                         arquivo é `{before_sha256}`. Releia o arquivo (files.read) \
                         e refaça o edit com o novo `expected_sha256`. \
                         Sem isso, o edit seria aplicado silenciosamente no \
                         lugar errado."
                    ),
                );
            }
        }

        // 6. Conta matches.
        let match_count = Self::count_matches(&original, find);
        if match_count == 0 {
            return ToolResult::err(
                tool_id,
                format!(
                    "`find` não encontrado em '{path_str}' (0 matches). \
                     Verifique que o trecho bate exatamente — espaços, \
                     tabs e quebras de linha contam."
                ),
            );
        }
        if !replace_all && match_count > 1 {
            return ToolResult::err(
                tool_id,
                format!(
                    "`find` aparece {match_count}x em '{path_str}' sem `replace_all: true`. \
                     Refine o `find` pra ser único, OU passe `replace_all: true` \
                     pra substituir todas as ocorrências."
                ),
            );
        }

        // 7. Backup: cria `.bak` antes da escrita (mesma regra
        //    do files.write — D3). Edit também é destrutivo.
        let backup_path = Self::backup_path_for(&resolved);
        if let Err(e) = fs::copy(&resolved, &backup_path) {
            return ToolResult::err(
                tool_id,
                format!(
                    "backup falhou: {e}. Sem backup, o edit foi ABORTADO — \
                     o arquivo original está intacto."
                ),
            );
        }
        let backup_path_str = backup_path
            .strip_prefix(ctx.jail.root_canonical())
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/").to_string())
            .unwrap_or_else(|| backup_path.to_string_lossy().replace('\\', "/").to_string());

        // 8. Aplica o replace. Preserva indentação usando o
        //    primeiro match como referência.
        let first_start = original
            .find(find)
            .expect("first match exists (checked above)");
        let indented_replace = Self::indent_preserved(replace, &original, first_start);
        let new_content = if replace_all {
            original.replace(find, &indented_replace)
        } else {
            // match_count == 1 nesse caminho (validado acima)
            original.replacen(find, &indented_replace, 1)
        };
        let replacements_applied = if replace_all { match_count } else { 1 };

        // 9. Atomic write (D1).
        if let Err(msg) = Self::write_atomic(&resolved, &new_content) {
            return ToolResult::err(
                tool_id,
                format!(
                    "escrita atômica falhou: {msg}. O backup `.bak` tem o \
                     conteúdo original — pode restaurar manualmente."
                ),
            );
        }

        // 10. after_sha256.
        let after_sha256 = Self::sha256_hex(new_content.as_bytes());

        // 11. Output.
        let path_display = resolved
            .strip_prefix(ctx.jail.root_canonical())
            .map(|p| p.to_string_lossy().replace('\\', "/").to_string())
            .unwrap_or_else(|_| path_str.to_string());
        let path_display = if path_display.is_empty() {
            path_str.to_string()
        } else {
            path_display
        };

        ToolResult::ok(
            tool_id,
            json!({
                "path": path_display,
                "before_sha256": before_sha256,
                "after_sha256": after_sha256,
                "replacements": replacements_applied,
                "backup_path": backup_path_str,
            }),
            vec![resolved],
        )
    }
}

impl FilesEditTool {
    fn tool_id(&self) -> ToolId {
        self.manifest.id.clone()
    }

    /// Cria o arquivo do zero com `find + replace` como conteúdo
    /// inicial (caso `create_parents: true` + arquivo não existe).
    /// Não há indentação a preservar (arquivo é zero-linhas), e
    /// o `find` não pode estar no conteúdo (zero matches).
    /// Reusa `write_atomic` (D1).
    fn create_file_from_scratch(
        &self,
        tool_id: ToolId,
        path: &std::path::Path,
        find: &str,
        replace: &str,
        ctx: &ToolContext,
    ) -> ToolResult {
        // Validação: `find` deve ser exatamente igual a `replace`
        // pra criar do zero (não há substring a substituir).
        // Caso contrário, o caller deveria usar `files.write`.
        if find != replace {
            return ToolResult::err(
                tool_id,
                "`create_parents: true` + arquivo não existe: o `find` \
                 não pode ser diferente do `replace` (não há onde procurar). \
                 Para criar arquivo novo, use `files.write` com `content: <find+replace>`.",
            );
        }
        let content = replace; // == find nesse caminho
        if let Err(msg) = Self::write_atomic(path, content) {
            return ToolResult::err(tool_id, format!("escrita atômica falhou: {msg}"));
        }
        let after_sha256 = Self::sha256_hex(content.as_bytes());
        let backup_path = Self::backup_path_for(path);
        let backup_path_str = backup_path
            .strip_prefix(ctx.jail.root_canonical())
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/").to_string());
        let path_display = path
            .strip_prefix(ctx.jail.root_canonical())
            .map(|p| p.to_string_lossy().replace('\\', "/").to_string())
            .unwrap_or_else(|_| path.display().to_string());
        ToolResult::ok(
            tool_id,
            json!({
                "path": path_display,
                "before_sha256": Value::Null,
                "after_sha256": after_sha256,
                "replacements": 0,
                "backup_path": backup_path_str,
            }),
            vec![path.to_path_buf()],
        )
    }
}

use serde_json::Value;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use frederico_core::{ConversationId, MessageId, RunId};
    use uuid::Uuid;

    use crate::workspace::Jail;

    fn setup() -> (Tempdir, FilesEditTool, ToolContext) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesEditTool::new();
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
                "frederico-tool-registry-files-edit-{}-{}-{}",
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
        assert_eq!(m.id, ToolId::new("files.edit"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Moderate);
        assert_eq!(m.category, ToolCategory::Files);
        assert!(m.requires_file_write);
        assert!(m.requires_user_approval, "files.edit deve pedir approval");
    }

    // --- Testes de negação (regra do user) ---

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "../etc/passwd", "find": "x", "replace": "y"}),
            )
            .await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("JAIL"));
    }

    #[tokio::test]
    async fn rejects_absolute_path() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "C:\\Windows\\evil.txt", "find": "x", "replace": "y"}),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_missing_arguments() {
        let (_d, tool, ctx) = setup();
        let r1 = tool
            .execute(&ctx, &json!({"find": "x", "replace": "y"}))
            .await;
        assert!(!r1.ok);
        let r2 = tool
            .execute(&ctx, &json!({"path": "hello.txt", "replace": "y"}))
            .await;
        assert!(!r2.ok);
        let r3 = tool
            .execute(&ctx, &json!({"path": "hello.txt", "find": "x"}))
            .await;
        assert!(!r3.ok);
    }

    #[tokio::test]
    async fn rejects_nonexistent_file_without_create_parents() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "nope.txt", "find": "x", "replace": "y"}),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn pattern_not_found_is_error() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "hello.txt", "find": "GOODBYE", "replace": "hi"}),
            )
            .await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("não encontrado"));
    }

    #[tokio::test]
    async fn ambiguous_match_without_replace_all_is_error() {
        // hello.txt tem "Hello, world!" — `l` aparece 3x.
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "hello.txt", "find": "l", "replace": "L"}),
            )
            .await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("3x"));
    }

    /// **Regra do user: falha se o conteúdo mudou.** Caller diz
    /// "espero o SHA-256 X", mas o arquivo no disco é o SHA-256 Y
    /// (porque outra invocação editou). O tool **recusa**.
    #[tokio::test]
    async fn expected_sha256_mismatch_refuses_edit() {
        let (_d, tool, ctx) = setup();
        let wrong_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "hello.txt",
                    "find": "Hello",
                    "replace": "Goodbye",
                    "expected_sha256": wrong_sha
                }),
            )
            .await;
        assert!(!r.ok, "esperava recusa, veio ok");
        let err = r.error_message.unwrap();
        assert!(err.contains("conteúdo mudou"), "msg: {err}");
        // Arquivo INTACTO.
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[tokio::test]
    async fn expected_sha256_match_proceeds_with_edit() {
        let (_d, tool, ctx) = setup();
        let correct_sha = FilesEditTool::sha256_hex(b"Hello, world!");
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "hello.txt",
                    "find": "Hello",
                    "replace": "Goodbye",
                    "expected_sha256": correct_sha
                }),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "Goodbye, world!");
    }

    #[tokio::test]
    async fn unique_match_replaces_once() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "hello.txt", "find": "world", "replace": "Rust"}),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        assert_eq!(r.output.get("replacements"), Some(&json!(1)));
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "Hello, Rust!");
    }

    #[tokio::test]
    async fn replace_all_substitutes_every_occurrence() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "hello.txt",
                    "find": "l",
                    "replace": "L",
                    "replace_all": true
                }),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        assert_eq!(r.output.get("replacements"), Some(&json!(3)));
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "HeLLo, worLd!");
    }

    #[tokio::test]
    async fn preserves_indentation_of_first_match() {
        // Cria arquivo com indentação e testa que `replace`
        // herda o prefix.
        let (_d, tool, ctx) = setup();
        let code = "def hello():\n    print(\"Hello\")\n    return 42\n";
        fs::write(ctx.jail.root().join("code.py"), code).unwrap();
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "code.py",
                    "find": "print(\"Hello\")",
                    "replace": "print(\"Goodbye\")"
                }),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let new_content = fs::read_to_string(ctx.jail.root().join("code.py")).unwrap();
        // O replace pegou o prefix "    " (4 espaços) e prependeu
        // na linha do replace. Como replace é single-line, o
        // resultado é "    print("Goodbye")".
        assert!(
            new_content.contains("    print(\"Goodbye\")"),
            "got: {new_content}"
        );
    }

    #[tokio::test]
    async fn backup_contains_original_content() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "hello.txt", "find": "Hello", "replace": "Goodbye"}),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        let backup = r
            .output
            .get("backup_path")
            .and_then(|v| v.as_str())
            .unwrap();
        let backup_content =
            fs::read_to_string(ctx.jail.root().join(backup.replace('/', "\\"))).unwrap();
        assert_eq!(backup_content, "Hello, world!");
    }
}
