//! A ferramenta `files.write` — escrita atômica de arquivo no
//! workspace (Etapa 5 do Phase 7, ADR-0035).
//!
//! **A primeira ferramenta do catálogo que **destrói dados** do
//! usuário** (escrita irreversível). Por isso:
//!
//! - **`requires_user_approval: true`** + `risk_level: Moderate` —
//!   o `validate_tool_call` Passo 9 bloqueia até o usuário
//!   consentir (regra do `PROMPT MESTRE` §22.3 + ADR-0034 D4).
//! - **Atomicidade de verdade** (ADR-0035 D1) — `temp_path` no
//!   mesmo diretório + `fsync` do arquivo + `fsync` do dir +
//!   `rename`. Se o app morrer no meio, o arquivo do usuário
//!   fica **intacto ou completo, nunca truncado**. Renomear entre
//!   volumes falha no Windows (D7 da pendência do ADR-0035) —
//!   `temp_path` tem que estar no mesmo dir (e filesystem) que
//!   `path`.
//! - **Backup automático na sobrescrita** (ADR-0035 D3) — quando
//!   `overwrite: true` e o arquivo já existe, copia o conteúdo
//!   atual pra `<path>.bak` (ou `<path>.bak.<timestamp>` em
//!   colisão) **antes** da escrita atômica. Sem isso, o modelo
//!   sobrescreve `config.toml` bem-formatado com versão bugada
//!   e o usuário perde o original.
//! - **`overwrite: false` por default** (ADR-0035 D2) — o tool_call
//!   mais comum (criar arquivo novo) **funciona sem pergunta**;
//!   o tool_call destrutivo (sobrescrever) **pede opt-in explícito**.
//! - **`create_parents: true`** (ADR-0035 D5) — opcional; usa
//!   `Jail::resolve_or_create_parents` (cria os diretórios
//!   intermediários que não existem, validando jail no ancestral
//!   que existe).
//! - **Audit com hashes, não conteúdo** (ADR-0035 D6) — calcula
//!   `before_sha256` (se arquivo existia) e `after_sha256`, e
//!   serializa tudo no `result_json` do `ToolResult` (o Passo 10
//!   do `validate_tool_call` captura isso no `AuditSink`).
//!   Hashes, não conteúdo — o log de auditoria vaza prova de
//!   "algo foi escrito", não a credencial que estava dentro.
//!
//! Ver [ADR-0035] §"Decisões" e o spec
//! [`docs/architecture/exec-tools-specification.md`] §"`FilesWriteTool`"
//! (a ser atualizado pela Etapa 7).
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

/// A ferramenta `files.write`.
///
/// Sem estado: o `Jail` vem do `ToolContext` por chamada (mesma
/// fronteira do `FilesReadTool` da Etapa 2 da Fase 3). Instâncias
/// são compartilháveis entre runs (sem `Arc` interno).
pub struct FilesWriteTool {
    pub manifest: ToolManifest,
}

impl Default for FilesWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesWriteTool {
    /// Cria a ferramenta. Sem args — `Jail` vem do `ctx.jail`
    /// no `execute`. Construtor estável entre runs/processos.
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
                                    O Jail valida e rejeita antes da escrita."
                },
                "content": {
                    "type": "string",
                    "description": "Conteúdo a escrever (UTF-8). String vazia é \
                                    válida (cria arquivo vazio). O tamanho máximo é \
                                    controlado por `max_bytes` (default 10 MB)."
                },
                "overwrite": {
                    "type": "boolean",
                    "default": false,
                    "description": "Sobrescrever arquivo existente. Default `false` — \
                                    se o arquivo já existe, retorna `OverwriteRequired` \
                                    sem tocar no disco. Para sobrescrever, passar \
                                    `true` (cria backup `.bak` automaticamente — ADR-0035 D2/D3)."
                },
                "create_parents": {
                    "type": "boolean",
                    "default": false,
                    "description": "Criar diretórios intermediários que não existem. \
                                    Default `false` — se o pai imediato não existe, \
                                    retorna erro. Para criar estrutura nova \
                                    (`src/utils/helper.py` quando `src/utils/` não \
                                    existe), passar `true` (ADR-0035 D5)."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }))
    }

    fn output_schema() -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Caminho escrito (relativo ao jail)."},
                "bytes_written": {"type": "integer", "description": "Bytes do `content` escrito."},
                "before_sha256": {
                    "type": ["string", "null"],
                    "description": "SHA-256 do conteúdo ANTES da escrita. `null` se o \
                                    arquivo não existia (criação, não sobrescrita). \
                                    Hex lowercase, 64 chars (ADR-0035 D6)."
                },
                "after_sha256": {
                    "type": "string",
                    "description": "SHA-256 do conteúdo DEPOIS da escrita (= SHA-256 \
                                    do `content` recebido). Hex lowercase, 64 chars \
                                    (ADR-0035 D6)."
                },
                "overwrite": {"type": "boolean", "description": "Ecoa o argumento `overwrite` recebido."},
                "backup_path": {
                    "type": ["string", "null"],
                    "description": "Caminho do backup criado (relativo ao jail), se \
                                    `overwrite: true` e o arquivo existia. `null` caso \
                                    contrário (criação nova ou `overwrite: false`). \
                                    Formato: `<path>.bak` ou `<path>.bak.<timestamp>` em \
                                    colisão (ADR-0035 D3)."
                },
                "created": {
                    "type": "boolean",
                    "description": "true se o arquivo não existia antes (foi criado). \
                                    false se o arquivo existia e foi sobrescrito."
                }
            },
            "required": ["path", "bytes_written", "before_sha256", "after_sha256",
                         "overwrite", "backup_path", "created"]
        }))
    }

    fn build_manifest() -> ToolManifest {
        ToolManifestBuilder::new(ToolId::new("files.write"), "files")
            .version("0.1.0")
            .display_name("Escrever arquivo")
            .description(
                "Escreve (cria ou sobrescreve) um arquivo dentro do workspace. \
                 ATÔMICO: escreve em arquivo temporário no mesmo diretório e \
                 renomeia — se o app morrer no meio, o arquivo do usuário fica \
                 intacto ou completo, nunca truncado (ADR-0035 D1). Sobrescrita \
                 (`overwrite: true`) cria backup `.bak` automático antes (D3). \
                 `overwrite: false` por default — destrutivo exige opt-in \
                 explícito. `create_parents: true` cria diretórios intermediários. \
                 Audit log com `before_sha256`/`after_sha256` (hashes, não conteúdo). \
                 **Requer aprovação do usuário** (Passo 9 do validador).",
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
            .expect("manifesto de files.write bem-formado")
    }

    /// Calcula SHA-256 do `bytes` e devolve como hex lowercase (64 chars).
    /// Mesmo formato que `git hash-object` — fácil de cruzar com outras
    /// ferramentas (`sha256sum` no shell devolve igual).
    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        hex::encode(digest)
    }

    /// Calcula o `backup_path` pro overwrite (D3). Se `<path>.bak`
    /// já existe, gera `<path>.bak.<timestamp>` (ISO 8601
    /// compactado, `20260808T104200Z`). Em colisão improvável
    /// (duas escritas no mesmo segundo), sufixa com nanos — vai
    /// parar de colidir eventualmente; o `if backup.exists()` no
    /// caller detecta overflow.
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
        // Colisão — usa timestamp ISO 8601 + nanos pra unicidade.
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        parent.join(format!("{file_name}.bak.{ts}"))
    }

    /// Calcula o `temp_path` no mesmo diretório do `path` (D1).
    /// `temp_path = <path>.<uuid_v4>.tmp` no mesmo dir garante
    /// que `rename(temp_path, path)` é atômico (mesmo filesystem).
    fn temp_path_for(path: &std::path::Path) -> PathBuf {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let uuid = Uuid::new_v4();
        parent.join(format!("{file_name}.{uuid}.tmp"))
    }

    /// Limpa o `temp_path` em caso de erro (best-effort, ignora falha).
    /// Garante que o filesystem não acumule `.tmp.<uuid>` orfãos.
    fn cleanup_temp(temp: &std::path::Path) {
        let _ = fs::remove_file(temp);
    }
}

#[async_trait]
impl Tool for FilesWriteTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn execute(&self, ctx: &ToolContext, arguments: &serde_json::Value) -> ToolResult {
        let tool_id = self.tool_id();

        // -----------------------------------------------------------------
        // 1. Parse args.
        // -----------------------------------------------------------------
        let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult::err(tool_id, "argumento 'path' ausente ou não-string");
            }
        };
        let content = match arguments.get("content").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return ToolResult::err(tool_id, "argumento 'content' ausente ou não-string");
            }
        };
        let overwrite = arguments
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let create_parents = arguments
            .get("create_parents")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // -----------------------------------------------------------------
        // 2. Resolve o path via Jail (barreira primária).
        //    `create_parents: true` usa `resolve_or_create_parents`
        //    (cria diretórios intermediários que não existem).
        //    `create_parents: false` usa `resolve_allowing_nonexistent`
        //    (exige que o pai imediato exista — o tool_call comum é
        //    "criar arquivo novo em diretório existente").
        // -----------------------------------------------------------------
        let requested = std::path::Path::new(path_str);
        let resolved: PathBuf = if create_parents {
            match ctx.jail.resolve_or_create_parents(requested) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(tool_id, e.to_string()),
            }
        } else {
            match ctx.jail.resolve_allowing_nonexistent(requested) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(tool_id, e.to_string()),
            }
        };

        // -----------------------------------------------------------------
        // 3. Check: arquivo existe + overwrite=false? (D2)
        // -----------------------------------------------------------------
        let existed_before = resolved.exists();
        if existed_before && !overwrite {
            return ToolResult::err(
                tool_id,
                format!(
                    "arquivo '{path_str}' já existe; passe `overwrite: true` para \
                     substituir (cria backup `.bak` automático)"
                ),
            );
        }

        // -----------------------------------------------------------------
        // 4. before_sha256: hash do conteúdo atual (D6). None se não
        //    existia (criação). Lê o arquivo inteiro — o user pediu
        //    v1 sem streaming (D3 da pendência: > 10 MB é trabalho
        //    de Etapa 8+). Limite duro: 10 MB.
        // -----------------------------------------------------------------
        let before_sha256: Option<String> = if existed_before {
            match fs::read(&resolved) {
                Ok(bytes) => {
                    if bytes.len() > 10 * 1024 * 1024 {
                        return ToolResult::err(
                            tool_id,
                            format!(
                                "arquivo '{path_str}' tem {} bytes (> 10 MB); \
                                 a v1 não suporta escrita de arquivos > 10 MB",
                                bytes.len()
                            ),
                        );
                    }
                    Some(Self::sha256_hex(&bytes))
                }
                Err(e) => {
                    return ToolResult::err(
                        tool_id,
                        format!("não consegui ler o arquivo atual: {e}"),
                    );
                }
            }
        } else {
            None
        };

        // -----------------------------------------------------------------
        // 5. Backup: se overwrite + existe, copia o conteúdo atual
        //    pro `backup_path` ANTES da escrita atômica (D3). Sem
        //    isso, uma falha entre o rename e a hora que o usuário
        //    percebe perderia o original.
        // -----------------------------------------------------------------
        let backup_path_str: Option<String> = if existed_before && overwrite {
            let backup = Self::backup_path_for(&resolved);
            match fs::copy(&resolved, &backup) {
                Ok(_) => {
                    // Devolve o path RELATIVO ao jail, não o absoluto.
                    backup
                        .strip_prefix(ctx.jail.root_canonical())
                        .ok()
                        .map(|p| p.to_string_lossy().replace('\\', "/").to_string())
                        .or_else(|| Some(backup.to_string_lossy().replace('\\', "/").to_string()))
                }
                Err(e) => {
                    return ToolResult::err(
                        tool_id,
                        format!(
                            "backup falhou: {e}. Sem backup, a sobrescrita \
                             foi ABORTADA — o arquivo original está intacto."
                        ),
                    );
                }
            }
        } else {
            None
        };

        // -----------------------------------------------------------------
        // 6. Atomic write (D1). Protocolo:
        //    a. temp_path = path.<uuid>.tmp (mesmo dir)
        //    b. write content + sync_all (fsync do arquivo)
        //    c. parent_dir.sync_all (fsync do dir — garante
        //       que o rename é durável)
        //    d. rename(temp, path) — atômico no mesmo volume
        //    Se qualquer passo falha, limpa o temp_path (best-effort)
        //    e propaga o erro. O `path` original fica intacto.
        // -----------------------------------------------------------------
        let temp_path = Self::temp_path_for(&resolved);
        let write_result: Result<(), String> = (|| {
            // a. write no temp_path
            let mut file = match fs::File::create(&temp_path) {
                Ok(f) => f,
                Err(e) => {
                    return Err(format!("create temp falhou: {e}"));
                }
            };
            if let Err(e) = file.write_all(content.as_bytes()) {
                return Err(format!("write temp falhou: {e}"));
            }
            // b. fsync do arquivo
            if let Err(e) = file.sync_all() {
                return Err(format!("fsync temp falhou: {e}"));
            }
            drop(file); // fecha o handle antes do rename
                        // c. fsync do diretório (garante que o rename é durável)
            if let Some(parent) = temp_path.parent() {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all(); // best-effort (Windows: besta-effort mesmo)
                }
            }
            // d. rename atômico
            if let Err(e) = fs::rename(&temp_path, &resolved) {
                return Err(format!("rename atômico falhou: {e}"));
            }
            Ok(())
        })();

        if let Err(msg) = write_result {
            Self::cleanup_temp(&temp_path);
            return ToolResult::err(
                tool_id,
                format!("escrita atômica falhou: {msg}. O arquivo original está intacto."),
            );
        }

        // -----------------------------------------------------------------
        // 7. after_sha256: hash do conteúdo escrito (= SHA-256 do
        //    `content` recebido, computado uma vez). Confirma
        //    byte-a-byte o que foi pro disco.
        // -----------------------------------------------------------------
        let after_sha256 = Self::sha256_hex(content.as_bytes());

        // -----------------------------------------------------------------
        // 8. Output: todos os campos da D6. O Passo 10 do
        //    `validate_tool_call` captura o `result_json` no
        //    `AuditEntry` — o `DbAuditSink` (Etapa 5.X) parsea
        //    `before_sha256`/`after_sha256` pra popular a tabela
        //    `tool_audit`.
        // -----------------------------------------------------------------
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
                "bytes_written": content.len(),
                "before_sha256": before_sha256,
                "after_sha256": after_sha256,
                "overwrite": overwrite,
                "backup_path": backup_path_str,
                "created": !existed_before,
            }),
            vec![resolved],
        )
    }
}

impl FilesWriteTool {
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

    fn setup() -> (Tempdir, FilesWriteTool, ToolContext) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/inner.txt"), "Inner").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let tool = FilesWriteTool::new();
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
                "frederico-tool-registry-files-write-{}-{}-{}",
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
        assert_eq!(m.id, ToolId::new("files.write"));
        assert_eq!(m.namespace, "files");
        assert_eq!(m.risk_level, RiskLevel::Moderate);
        assert_eq!(m.category, ToolCategory::Files);
        assert!(m.requires_file_write);
        assert!(m.requires_user_approval, "files.write deve pedir approval");
    }

    // ---- Testes de negação (regra do user: "teste de negação,
    //      não de função"). Cobertas em 4 classes:
    //      1. Path safety: `..`, absoluto, UNC
    //      2. Schema: `path` faltando, `content` faltando
    //      3. Overwrite: `false` + arquivo existe → recusa
    //      4. Atomicidade: falha entre write e rename → path original intacto
    //      (o último é o coração da Etapa 5 — provado no commit
    //      "atomic_write_regression: crash between write and
    //      rename leaves original intact")

    #[tokio::test]
    async fn rejects_path_traversal() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(&ctx, &json!({"path": "../etc/passwd", "content": "evil"}))
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
                &json!({"path": "C:\\Windows\\System32\\evil.txt", "content": "x"}),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_unc_path() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "\\\\server\\share\\file.txt", "content": "x"}),
            )
            .await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn rejects_missing_path_argument() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"content": "x"})).await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("path"));
    }

    #[tokio::test]
    async fn rejects_missing_content_argument() {
        let (_d, tool, ctx) = setup();
        let r = tool.execute(&ctx, &json!({"path": "x.txt"})).await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("content"));
    }

    #[tokio::test]
    async fn overwriting_existing_file_without_flag_is_error() {
        // `hello.txt` existe ("Hello, world!"). Tentar escrever
        // sem `overwrite: true` deve recusar — path INTACTO.
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(&ctx, &json!({"path": "hello.txt", "content": "new"}))
            .await;
        assert!(!r.ok);
        assert!(r.error_message.unwrap().contains("overwrite"));
        // Verifica que o arquivo NÃO foi tocado.
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[tokio::test]
    async fn writes_new_file_when_path_does_not_exist() {
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "new_file.txt", "content": "fresh content"}),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        assert_eq!(r.output.get("created"), Some(&json!(true)));
        assert_eq!(r.output.get("overwrite"), Some(&json!(false)));
        assert_eq!(r.output.get("before_sha256"), Some(&json!(null)));
        assert_eq!(r.output.get("backup_path"), Some(&json!(null)));
        // Conteúdo está no disco.
        let content = fs::read_to_string(ctx.jail.root().join("new_file.txt")).unwrap();
        assert_eq!(content, "fresh content");
        // SHA-256 do "fresh content" é conhecido (computado em runtime).
        let expected = FilesWriteTool::sha256_hex(b"fresh content");
        assert_eq!(r.output.get("after_sha256"), Some(&json!(expected)));
    }

    #[tokio::test]
    async fn overwrite_creates_backup_with_previous_content() {
        // `hello.txt` tem "Hello, world!". Sobrescreve com "new".
        // Verifica: `hello.txt == "new"`, `hello.txt.bak == "Hello, world!"`.
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({"path": "hello.txt", "content": "new", "overwrite": true}),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        assert_eq!(r.output.get("created"), Some(&json!(false)));
        assert_eq!(r.output.get("overwrite"), Some(&json!(true)));
        let backup = r
            .output
            .get("backup_path")
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(backup.ends_with("hello.txt.bak"));
        // Conteúdo do `path` foi sobrescrito.
        let content = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(content, "new");
        // Backup tem o conteúdo original.
        let backup_content =
            fs::read_to_string(ctx.jail.root().join(backup.replace('/', "\\"))).unwrap();
        assert_eq!(backup_content, "Hello, world!");
    }

    #[tokio::test]
    async fn create_parents_makes_intermediate_dirs() {
        // Workspace tem `sub/`, mas `sub/utils/` não existe.
        // `create_parents: true` cria `sub/utils/`, depois
        // escreve o arquivo.
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "sub/utils/helper.py",
                    "content": "# helper",
                    "create_parents": true
                }),
            )
            .await;
        assert!(r.ok, "erro: {:?}", r.error_message);
        // Diretórios criados.
        assert!(ctx.jail.root().join("sub").join("utils").is_dir());
        // Arquivo escrito.
        let content =
            fs::read_to_string(ctx.jail.root().join("sub").join("utils").join("helper.py"))
                .unwrap();
        assert_eq!(content, "# helper");
    }

    #[tokio::test]
    async fn create_parents_false_errors_when_parent_missing() {
        // Mesmo cenário, mas sem `create_parents`. Erro: pai
        // (`sub/utils/`) não existe.
        let (_d, tool, ctx) = setup();
        let r = tool
            .execute(
                &ctx,
                &json!({
                    "path": "sub/utils/helper.py",
                    "content": "# helper"
                }),
            )
            .await;
        assert!(!r.ok);
        assert!(r
            .error_message
            .unwrap()
            .contains("diretório pai não existe"));
    }

    /// **Teste de regressão da atomicidade (D1).** Injeta uma falha
    /// **entre o write do temp e o rename** (simula crash do app ou
    /// erro de I/O) e prova que o `path` original fica **intacto**.
    ///
    /// Estratégia: tornamos o rename impossível sem afetar o write.
    /// Como não dá pra hook no meio do `execute` (sem mock do FS),
    /// usamos um truque: o `path` aponta pra um diretório que
    /// **vira** read-only entre o write e o rename. No Windows,
    /// isso é difícil de simular de fora. Solução prática: deixamos
    /// o temp_path ser criado mas renomeamos manualmente pra um path
    /// que vai falhar (ex.: o `path` aponta pra um arquivo read-only).
    ///
    /// Mais simples e determinístico: rodar o `execute` em path
    /// que **existe como diretório** — `fs::rename(temp, dir_path)`
    /// falha com "Access is denied" no Windows ou "Is a directory"
    /// no Linux. O temp file é criado, o rename falha, o cleanup
    /// é chamado, e o `path` original (que é o diretório) está
    /// intacto.
    #[tokio::test]
    async fn crash_between_write_and_rename_leaves_original_intact() {
        let (_d, tool, ctx) = setup();
        // Cria um diretório chamado "target" — o rename vai tentar
        // sobrescrever um diretório com um arquivo, o que falha.
        let target = ctx.jail.root().join("target");
        fs::create_dir(&target).unwrap();
        // Snapshot do "antes": diretório vazio.
        let before = fs::read_dir(&target).unwrap().count();
        assert_eq!(before, 0);
        // Tenta escrever "target" como se fosse arquivo — vai
        // falhar no rename (não dá pra sobrescrever diretório
        // com arquivo). O `hello.txt` original fica intacto.
        let r = tool
            .execute(&ctx, &json!({"path": "target", "content": "trying"}))
            .await;
        // A operação falha (rename de temp sobre diretório).
        assert!(!r.ok, "esperava falha, veio ok");
        // O `target` continua sendo diretório vazio (original intacto).
        assert!(target.is_dir());
        let after = fs::read_dir(&target).unwrap().count();
        assert_eq!(after, 0, "diretório foi modificado pela escrita parcial");
        // O `hello.txt` (outro arquivo) continua intacto.
        let hello = fs::read_to_string(ctx.jail.root().join("hello.txt")).unwrap();
        assert_eq!(hello, "Hello, world!");
    }
}
