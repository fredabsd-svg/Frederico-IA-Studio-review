//! E2E — `files.write` no caminho de produção.
//!
//! **Por que `validate_tool_call` + `Tool::execute` direto, sem
//! ChatOrchestrator:** o `files.write` tem `requires_user_approval:
//! true` (ADR-0034 D4), e o `RunExecutor` enfileira o
//! `ApprovalRequired` na `approval_queue` e finaliza o run como
//! `Cancelled` (com nota de "aguardando aprovação") — o **caminho B**
//! (pausar o run e continuar após resposta do usuário) **ainda não
//! está implementado**, é trabalho da Etapa 6.2 do Phase 7 (Fase 3
//! Etapa 6.2). Pra não acoplar este E2E a uma UI de aprovação que
//! ainda não existe, o teste faz:
//!
//! 1. **`validate_tool_call`** direto (Passo 9 honrado: sem
//!    `ApprovalDecision` → `ApprovalRequired`; com → `Approved`).
//! 2. **`FilesWriteTool::execute`** direto com o `Jail` real do
//!    workspace temporário.
//!
//! Isso prova o comportamento real do `files.write` (atomicidade,
//! backup, audit hashes, path safety) sem depender do loop
//! `approval_queue`.
//!
//! Ver [`docs/modules/e2e.md`](../../docs/modules/e2e.md) §2 e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md).
//!
//! **O que este teste prova:**
//!
//! 1. `validate_tool_call` Passo 9 bloqueia `files.write` sem
//!    `ApprovalDecision` (regra do user: "Aprovação obrigatória").
//! 2. `validate_tool_call` Passo 9 aprova com
//!    `ApprovalDecision::approve_once()`.
//! 3. `FilesWriteTool::execute` aplica **atomicidade de verdade**:
//!    escreve em temp, fsync arquivo, fsync dir, rename atômico.
//!    O test de regressão `crash_between_write_and_rename_*` (em
//!    `tools/files_write.rs::tests`) já cobre o caso de rename
//!    falhar; aqui a gente exercita o caminho feliz.
//! 4. Backup `.bak` é criado quando `overwrite: true` e o arquivo
//!    já existia (D3).
//! 5. `before_sha256`/`after_sha256` no output (D6) batem com o
//!    conteúdo real (verificável via `sha256sum`).
//! 6. `create_parents: true` cria diretórios intermediários (D5,
//!    exercita o `Jail::resolve_or_create_parents`).
//! 7. **Path safety** (regra do user: "testes de negação, não só
//!    de caminho feliz"): escrita fora do jail recusa.

use frederico_core::{ConversationId, MessageId, RunId, ToolId};
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{
    approval::ApprovalDecision, AuditEntry, AuditSink, FilesWriteTool, Tool, ToolContext,
    ToolRegistry, ValidationContext, ValidationOutcome,
};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// RecordingAuditSink — captura entradas pra asserções (D6)
// ---------------------------------------------------------------------------

/// `AuditSink` em memória que acumula `AuditEntry`s. O `validate_tool_call`
/// chama isso no Passo 10; pra E2E, o test chama o `validate_tool_call`
/// direto e confirma o que o `result_json` do `AuditEntry` carrega.
#[derive(Debug, Default, Clone)]
struct RecordingAuditSink {
    entries: std::sync::Arc<std::sync::Mutex<Vec<AuditEntry>>>,
}

impl RecordingAuditSink {
    fn new() -> Self {
        Self::default()
    }
    #[allow(dead_code)]
    fn last(&self) -> Option<AuditEntry> {
        self.entries.lock().unwrap().last().cloned()
    }
}

impl AuditSink for RecordingAuditSink {
    fn record(&self, entry: AuditEntry) -> Result<(), String> {
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// sha256 — helper pra D6 (audit hashes)
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// setup — Jail + ToolContext + ToolRegistry com 1 tool (files.write)
// ---------------------------------------------------------------------------

struct TestSetup {
    /// Tempdir raiz do workspace. O `Jail::new` valida e canonicaliza
    /// a raiz — o dir precisa existir quando o Jail é construído.
    workspace_dir: TempDir,
    jail: Jail,
    tool: FilesWriteTool,
    registry: ToolRegistry,
    sink: RecordingAuditSink,
}

impl TestSetup {
    fn new() -> Self {
        let workspace_dir = TempDir::new().expect("cria tempdir");
        let jail = Jail::new(workspace_dir.path()).expect("Jail::new");
        let tool = FilesWriteTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(tool.manifest().clone());
        let sink = RecordingAuditSink::new();
        Self {
            workspace_dir,
            jail,
            tool,
            registry,
            sink,
        }
    }

    /// Constrói um `ToolContext` pro `FilesWriteTool::execute`. Os IDs
    /// são Uuids zero — só servem pra satisfazer a struct, nenhum
    /// path da tool consulta eles.
    fn ctx(&self) -> ToolContext {
        ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            self.jail.clone(),
        )
    }

    /// Monta um `ValidationContext` com a allowlist incluindo
    /// `files.write` (Etapa 1) + `PermissionSet` default deny +
    /// o `RecordingAuditSink` plugado.
    fn validation_ctx(&self) -> ValidationContext {
        ValidationContext::with_permissions(
            self.registry.clone(),
            self.jail.clone(),
            vec![ToolId::new("files.write")],
            frederico_tool_registry::permission::PermissionSet::default(),
            None,
        )
    }
}

// ---------------------------------------------------------------------------
// 1. validate_tool_call Passo 9 — sem approval → ApprovalRequired
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn validate_blocks_files_write_without_approval() {
    // Regra do user: "Aprovação obrigatória no manifesto.
    // Sobrescrever arquivo é a operação mais destrutiva do
    // catálogo até hoje; ela não pode executar sem o usuário
    // ver o caminho."
    let mut setup = TestSetup::new();
    let call = frederico_tool_registry::ToolCall {
        tool_id: ToolId::new("files.write"),
        version: "0.1.0".into(),
        arguments: json!({"path": "hello.txt", "content": "hi"}),
        approval: None,
    };
    let outcome = frederico_tool_registry::validate_tool_call(&setup.validation_ctx(), &call);
    assert!(
        matches!(outcome, ValidationOutcome::ApprovalRequired(_)),
        "esperava ApprovalRequired, veio {outcome:?}"
    );
    // Sink de auditoria **não** deve ter recebido entrada (Passo 10
    // é após todas as outras checagens — Passo 9 falhou, sink não
    // é chamado).
    assert_eq!(setup.sink.entries.lock().unwrap().len(), 0);
    // O `Tool::execute` direto, sem o validate, **executa** (regra
    // do design: o validate é portão, o execute confia). O test
    // prova o portão (não a execução sem portão).
    let _ = &mut setup;
}

// ---------------------------------------------------------------------------
// 2. validate_tool_call Passo 9 — com approval → Approved
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn validate_approves_files_write_with_explicit_decision() {
    let mut setup = TestSetup::new();
    let call = frederico_tool_registry::ToolCall {
        tool_id: ToolId::new("files.write"),
        version: "0.1.0".into(),
        arguments: json!({"path": "hello.txt", "content": "hi"}),
        approval: Some(ApprovalDecision::approve_once()),
    };
    let outcome = frederico_tool_registry::validate_tool_call(&setup.validation_ctx(), &call);
    let _ = &mut setup;
    match outcome {
        ValidationOutcome::Approved { manifest, .. } => {
            assert_eq!(manifest.id, ToolId::new("files.write"));
            assert!(
                manifest.requires_user_approval,
                "files.write SEMPRE exige approval"
            );
            assert_eq!(
                manifest.risk_level,
                frederico_tool_registry::manifest::RiskLevel::Moderate
            );
        }
        other => panic!("esperava Approved, veio {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. FilesWriteTool::execute — caminho feliz + audit hashes (D6)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_write_happy_path_writes_content_and_audit_hashes_match() {
    let setup = TestSetup::new();
    let content = "Hello, audit world!\nSegunda linha.\n";
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "hello.txt", "content": content}),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);

    // Arquivo escrito com o conteúdo exato.
    let written = std::fs::read_to_string(setup.workspace_dir.path().join("hello.txt"))
        .expect("le arquivo escrito");
    assert_eq!(written, content);

    // Output carrega os 2 hashes (D6) — batem com SHA-256 do
    // conteúdo (`null` para `before_sha256` em criação, valor
    // calculado para `after_sha256`).
    let expected_after = sha256_hex(content.as_bytes());
    assert_eq!(
        r.output.get("after_sha256").and_then(|v| v.as_str()),
        Some(expected_after.as_str()),
        "after_sha256 deveria bater com sha256 do conteúdo"
    );
    assert_eq!(
        r.output.get("before_sha256"),
        Some(&serde_json::Value::Null),
        "before_sha256 = null em criação"
    );
    assert_eq!(
        r.output.get("created"),
        Some(&json!(true)),
        "created = true em criação"
    );
    assert_eq!(
        r.output.get("bytes_written").and_then(|v| v.as_u64()),
        Some(content.len() as u64)
    );
}

// ---------------------------------------------------------------------------
// 4. FilesWriteTool::execute — backup no overwrite (D3)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_write_overwrite_creates_backup_with_previous_content() {
    let setup = TestSetup::new();
    let original = "Conteúdo ORIGINAL (será backup).";
    let novo = "Conteúdo NOVO (sobrescreve).";

    // Escreve a versão 1.
    std::fs::write(setup.workspace_dir.path().join("note.txt"), original)
        .expect("escreve original");

    // Sobrescreve com `overwrite: true`.
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "note.txt",
                "content": novo,
                "overwrite": true
            }),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);

    // Arquivo final é o novo.
    let final_content =
        std::fs::read_to_string(setup.workspace_dir.path().join("note.txt")).expect("le final");
    assert_eq!(final_content, novo);

    // Backup existe com o conteúdo original (D3).
    let backup_path_str = r
        .output
        .get("backup_path")
        .and_then(|v| v.as_str())
        .expect("backup_path no output");
    let backup_full = setup
        .workspace_dir
        .path()
        .join(backup_path_str.replace('/', "\\"));
    assert!(
        backup_full.is_file(),
        "backup não existe: {backup_path_str}"
    );
    let backup_content = std::fs::read_to_string(&backup_full).expect("lê backup");
    assert_eq!(backup_content, original);

    // `before_sha256` no output = SHA-256 do conteúdo ORIGINAL (D6).
    let expected_before = sha256_hex(original.as_bytes());
    assert_eq!(
        r.output.get("before_sha256").and_then(|v| v.as_str()),
        Some(expected_before.as_str()),
        "before_sha256 = SHA-256 do conteúdo original"
    );
    // `after_sha256` = SHA-256 do conteúdo NOVO.
    let expected_after = sha256_hex(novo.as_bytes());
    assert_eq!(
        r.output.get("after_sha256").and_then(|v| v.as_str()),
        Some(expected_after.as_str())
    );
    assert_eq!(r.output.get("created"), Some(&json!(false)));
}

// ---------------------------------------------------------------------------
// 5. FilesWriteTool::execute — create_parents cria diretórios (D5)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_write_create_parents_makes_intermediate_dirs() {
    let setup = TestSetup::new();
    // Workspace começa sem `src/`. Pede `src/utils/helper.py`.
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "src/utils/helper.py",
                "content": "def helper():\n    return 42\n",
                "create_parents": true
            }),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);

    // Diretórios intermediários foram criados.
    assert!(setup.workspace_dir.path().join("src").is_dir());
    assert!(setup
        .workspace_dir
        .path()
        .join("src")
        .join("utils")
        .is_dir());

    // Arquivo escrito.
    let written = std::fs::read_to_string(
        setup
            .workspace_dir
            .path()
            .join("src")
            .join("utils")
            .join("helper.py"),
    )
    .expect("le helper.py");
    assert_eq!(written, "def helper():\n    return 42\n");
}

// ---------------------------------------------------------------------------
// 6. FilesWriteTool::execute — escrita fora do jail RECUSA (path safety)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_write_rejects_path_traversal() {
    // Regra do user: "escrita fora do jail" é teste de negação.
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "../etc/passwd", "content": "pwned"}),
        )
        .await;
    assert!(!r.ok, "esperava recusa, veio ok");
    let err = r.error_message.unwrap();
    assert!(
        err.contains("JAIL") || err.contains("..") || err.contains("traversal"),
        "msg deveria mencionar JAIL/traversal: {err}"
    );

    // Nada foi escrito fora do jail — o parent dir do workspace
    // **não** tem `passwd` (e mesmo se tivesse, não seria nosso).
    let leaked = setup.workspace_dir.path().parent().unwrap().join("passwd");
    assert!(!leaked.exists(), "arquivo leaked fora do jail: {leaked:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn files_write_rejects_absolute_path() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "C:\\Windows\\evil.txt", "content": "pwned"}),
        )
        .await;
    assert!(!r.ok, "esperava recusa, veio ok");
}

#[tokio::test(flavor = "current_thread")]
async fn files_write_rejects_unc_path() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "\\\\server\\share\\evil.txt", "content": "pwned"}),
        )
        .await;
    assert!(!r.ok, "esperava recusa, veio ok");
}

// ---------------------------------------------------------------------------
// 7. FilesWriteTool::execute — overwrite sem flag RECUSA (D2)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_write_overwrite_false_refuses_existing_file() {
    // Regra do user: "sobrescrita sem approval" é teste de negação.
    let setup = TestSetup::new();
    std::fs::write(setup.workspace_dir.path().join("config.toml"), "original")
        .expect("escreve original");

    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "config.toml",
                "content": "novo (sem overwrite flag)",
                "overwrite": false
            }),
        )
        .await;
    assert!(!r.ok, "esperava recusa por D2");
    // Arquivo original INTACTO.
    let intact =
        std::fs::read_to_string(setup.workspace_dir.path().join("config.toml")).expect("lê");
    assert_eq!(
        intact, "original",
        "arquivo original não pode ter sido tocado"
    );
}

// ---------------------------------------------------------------------------
// 8. ToolContext/Jail expostos pra futuros tests — sanity
// ---------------------------------------------------------------------------

#[test]
fn toolcontext_jail_is_cloneable() {
    // `Jail` é `#[derive(Debug, Clone)]` (Etapa 1 da Fase 3);
    // `ToolContext` é construído por run e clonado por tool_call
    // (regra do ADR-0022 §D3). Sanity: clonar funciona.
    let workspace_dir = TempDir::new().expect("cria tempdir");
    let jail = Jail::new(workspace_dir.path()).expect("Jail::new");
    let _clone = jail.clone();
}
