//! E2E — `files.edit` no caminho de produção.
//!
//! **Por que `validate_tool_call` + `Tool::execute` direto, sem
//! ChatOrchestrator:** mesma justificativa do
//! `e2e_files_write_under_jail.rs` — `files.edit` tem
//! `requires_user_approval: true` (ADR-0034 D4), e o caminho B
//! (pausar o run e continuar após resposta) ainda não está
//! implementado. Aqui o teste:
//!
//! 1. **`validate_tool_call`** direto (Passo 9 honrado: sem
//!    `ApprovalDecision` → `ApprovalRequired`; com → `Approved`).
//! 2. **`FilesEditTool::execute`** direto com o `Jail` real do
//!    workspace temporário.
//!
//! **A regra mais importante deste E2E** (regra do user:
//! "files.edit tem que falhar se o conteúdo mudou"): o
//! `expected_sha256` no `tool_call` é o SHA-256 do arquivo no
//! momento em que o `files.read` (ou último `files.edit`) o viu.
//! Se o `actual_sha256` do arquivo no momento do edit não bate,
//! o tool **recusa** em vez de aplicar a substituição no lugar
//! errado. Sem isso, o modelo que leu `config.toml` minutos
//! atrás e agora faz edit pode estar sobrescrevendo mudanças
//! que outra invocação (ou o usuário) fez no meio — corrompendo
//! o arquivo silenciosamente. **Este E2E prova o caminho da
//! race condition read-modify-write** com um cenário realista:
//! `read` (round 1) → outra edição altera o arquivo (race) →
//! `edit` com `expected_sha256` do round 1 (recusa).

use frederico_core::{ConversationId, MessageId, RunId, ToolId};
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{
    approval::ApprovalDecision, FilesEditTool, Tool, ToolContext, ToolRegistry, ValidationContext,
    ValidationOutcome,
};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// sha256 — helper pra D6 + expected_sha256
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// setup — Jail + ToolContext + ToolRegistry com 1 tool (files.edit)
// ---------------------------------------------------------------------------

struct TestSetup {
    workspace_dir: TempDir,
    jail: Jail,
    tool: FilesEditTool,
    registry: ToolRegistry,
}

impl TestSetup {
    fn new() -> Self {
        let workspace_dir = TempDir::new().expect("cria tempdir");
        let jail = Jail::new(workspace_dir.path()).expect("Jail::new");
        let tool = FilesEditTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(tool.manifest().clone());
        Self {
            workspace_dir,
            jail,
            tool,
            registry,
        }
    }

    fn ctx(&self) -> ToolContext {
        ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            self.jail.clone(),
        )
    }

    fn validation_ctx(&self) -> ValidationContext {
        ValidationContext::with_permissions(
            self.registry.clone(),
            self.jail.clone(),
            vec![ToolId::new("files.edit")],
            frederico_tool_registry::permission::PermissionSet::default(),
            None,
        )
    }

    /// Helper: escreve um arquivo inicial no workspace.
    fn write_initial(&self, rel_path: &str, content: &str) {
        let full = self.workspace_dir.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("cria parent dirs");
        }
        std::fs::write(&full, content).expect("escreve arquivo inicial");
    }

    /// Helper: lê o conteúdo de um arquivo do workspace.
    fn read(&self, rel_path: &str) -> String {
        std::fs::read_to_string(self.workspace_dir.path().join(rel_path))
            .expect("le arquivo do workspace")
    }
}

// ---------------------------------------------------------------------------
// 1. validate_tool_call Passo 9 — sem approval → ApprovalRequired
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn validate_blocks_files_edit_without_approval() {
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello, world!");
    let call = frederico_tool_registry::ToolCall {
        tool_id: ToolId::new("files.edit"),
        version: "0.1.0".into(),
        arguments: json!({"path": "hello.txt", "find": "Hello", "replace": "Goodbye"}),
        approval: None,
    };
    let outcome = frederico_tool_registry::validate_tool_call(&setup.validation_ctx(), &call);
    assert!(
        matches!(outcome, ValidationOutcome::ApprovalRequired(_)),
        "esperava ApprovalRequired, veio {outcome:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn validate_approves_files_edit_with_explicit_decision() {
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello");
    let call = frederico_tool_registry::ToolCall {
        tool_id: ToolId::new("files.edit"),
        version: "0.1.0".into(),
        arguments: json!({"path": "hello.txt", "find": "Hello", "replace": "Hi"}),
        approval: Some(ApprovalDecision::approve_once()),
    };
    let outcome = frederico_tool_registry::validate_tool_call(&setup.validation_ctx(), &call);
    match outcome {
        ValidationOutcome::Approved { manifest, .. } => {
            assert_eq!(manifest.id, ToolId::new("files.edit"));
            assert!(manifest.requires_user_approval);
            assert_eq!(
                manifest.risk_level,
                frederico_tool_registry::manifest::RiskLevel::Moderate
            );
        }
        other => panic!("esperava Approved, veio {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 2. FilesEditTool::execute — find único, replace_all, preserves indent
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_edit_unique_match_replaces_once() {
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello, world!");
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "hello.txt", "find": "world", "replace": "Rust"}),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);
    assert_eq!(r.output.get("replacements"), Some(&json!(1)));
    assert_eq!(setup.read("hello.txt"), "Hello, Rust!");
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_replace_all_substitutes_every_occurrence() {
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello, world!");
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
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
    assert_eq!(setup.read("hello.txt"), "HeLLo, worLd!");
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_preserves_indentation_of_first_match() {
    let setup = TestSetup::new();
    let code = "def hello():\n    print(\"Hello\")\n    return 42\n";
    setup.write_initial("code.py", code);
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "code.py",
                "find": "print(\"Hello\")",
                "replace": "print(\"Goodbye\")"
            }),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);
    let new_content = setup.read("code.py");
    assert!(
        new_content.contains("    print(\"Goodbye\")"),
        "indentação (4 espaços) deveria ser preservada: {new_content}"
    );
}

// ---------------------------------------------------------------------------
// 3. FilesEditTool::execute — REGRA DO USER: falha se o conteúdo mudou
// ---------------------------------------------------------------------------

/// **Regra do user: "files.edit tem que falhar se o conteúdo mudou."**
///
/// Cenário real (read-modify-write race): o modelo fez `files.read` no
/// round 1 (viu `before_sha256 = X`), outra invocação (ou o usuário)
/// alterou o arquivo no meio, e o round 2 do `files.edit` ainda passa
/// o `expected_sha256 = X`. O tool **recusa** em vez de aplicar a
/// substituição no lugar errado.
///
/// Sem isso, o modelo corrompe arquivo silenciosamente.
#[tokio::test(flavor = "current_thread")]
async fn files_edit_expected_sha256_mismatch_refuses_edit_and_leaves_file_intact() {
    let setup = TestSetup::new();
    let initial_content = "config_value = 1\nother = 2\n";
    setup.write_initial("config.py", initial_content);

    // Round 1: o `caller` viu o SHA-256 do conteúdo inicial.
    let sha_round_1 = sha256_hex(initial_content.as_bytes());

    // Race: outra invocação (ou o usuário) alterou o arquivo
    // DEPOIS do `files.read` que produziu `sha_round_1`. O
    // arquivo agora tem outro conteúdo, outro SHA.
    let intermediate_content = "config_value = 99\nother = 2\n";
    setup.write_initial("config.py", intermediate_content);

    // Round 2: o `caller` faz `files.edit` com o `expected_sha256`
    // do round 1 — confia que o arquivo ainda é o mesmo.
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "config.py",
                "find": "config_value = 1",
                "replace": "config_value = 42",
                "expected_sha256": sha_round_1
            }),
        )
        .await;

    // O tool **recusa**: `find` ("config_value = 1") não está no
    // arquivo atual (que tem "config_value = 99"). E o
    // `expected_sha256` mismatch reforça a recusa.
    assert!(!r.ok, "esperava recusa, veio ok");
    let err = r.error_message.unwrap();
    // A mensagem deve mencionar "conteúdo mudou" (a defesa forte).
    assert!(
        err.contains("conteúdo mudou"),
        "msg deveria mencionar race read-modify-write: {err}"
    );

    // O arquivo no disco está **intacto** (com o conteúdo
    // `intermediate_content` da race). A edição não tocou em nada.
    let final_content = setup.read("config.py");
    assert_eq!(
        final_content, intermediate_content,
        "arquivo NÃO pode ter sido modificado"
    );
}

/// **Caminho feliz do `expected_sha256`:** o caller passou o SHA-256
/// correto (que ele viu no `files.read` anterior) — o edit procede.
#[tokio::test(flavor = "current_thread")]
async fn files_edit_expected_sha256_match_proceeds() {
    let setup = TestSetup::new();
    let content = "name = 'old'\n";
    setup.write_initial("name.py", content);
    let correct_sha = sha256_hex(content.as_bytes());
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({
                "path": "name.py",
                "find": "'old'",
                "replace": "'new'",
                "expected_sha256": correct_sha
            }),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);
    assert_eq!(setup.read("name.py"), "name = 'new'\n");
    // Hashes no output batem (D6).
    assert_eq!(
        r.output.get("before_sha256").and_then(|v| v.as_str()),
        Some(correct_sha.as_str())
    );
}

// ---------------------------------------------------------------------------
// 4. FilesEditTool::execute — testes de negação
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_edit_pattern_not_found_is_error() {
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello, world!");
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "hello.txt", "find": "GOODBYE", "replace": "hi"}),
        )
        .await;
    assert!(!r.ok);
    assert!(r.error_message.unwrap().contains("não encontrado"));
    // Arquivo intacto.
    assert_eq!(setup.read("hello.txt"), "Hello, world!");
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_ambiguous_match_without_replace_all_is_error() {
    // "l" aparece 3x em "Hello, world!" — sem `replace_all` é
    // `AmbiguousMatch` (regra do user: "edição ambígua").
    let setup = TestSetup::new();
    setup.write_initial("hello.txt", "Hello, world!");
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "hello.txt", "find": "l", "replace": "L"}),
        )
        .await;
    assert!(!r.ok);
    assert!(r.error_message.unwrap().contains("3x"));
    // Arquivo intacto.
    assert_eq!(setup.read("hello.txt"), "Hello, world!");
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_rejects_path_traversal() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "../etc/passwd", "find": "x", "replace": "y"}),
        )
        .await;
    assert!(!r.ok);
    assert!(r.error_message.unwrap().contains("JAIL"));
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_rejects_absolute_path() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "C:\\Windows\\evil.txt", "find": "x", "replace": "y"}),
        )
        .await;
    assert!(!r.ok);
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_rejects_unc_path() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "\\\\server\\share\\evil.txt", "find": "x", "replace": "y"}),
        )
        .await;
    assert!(!r.ok);
}

#[tokio::test(flavor = "current_thread")]
async fn files_edit_rejects_nonexistent_file() {
    let setup = TestSetup::new();
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "nope.txt", "find": "x", "replace": "y"}),
        )
        .await;
    assert!(!r.ok);
}

// ---------------------------------------------------------------------------
// 5. FilesEditTool::execute — backup .bak automático (D3)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn files_edit_creates_backup_with_previous_content() {
    let setup = TestSetup::new();
    let original = "Hello, world!";
    setup.write_initial("hello.txt", original);
    let r = setup
        .tool
        .execute(
            &setup.ctx(),
            &json!({"path": "hello.txt", "find": "Hello", "replace": "Goodbye"}),
        )
        .await;
    assert!(r.ok, "erro: {:?}", r.error_message);
    // Backup existe com o conteúdo original.
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
}
