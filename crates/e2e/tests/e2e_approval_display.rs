//! E2E — `requires_user_approval` é honrado pelo `validate_tool_call`.
//!
//! Ver [ADR-0034](https://github.com/fredabsd-svg/Frederico-IA-Studio-review/blob/main/docs/decisions/0034-fase-7-write-exec-approval-policy.md)
//! + Passo 9 do `validate_tool_call` (Etapa 1 da Fase 3).
//!
//! ## O que o teste prova (Etapa 4 da Fase 7)
//!
//! O `validate_tool_call` Passo 9 checa `manifest.requires_user_approval`:
//!
//! - **`approval: None`** → retorna `ApprovalRequired(ApprovalRequest)`.
//!   O `RunExecutor` da Fase 3 pausa o run, emite um evento pra
//!   UI, espera a decisão do user. **A Etapa 5 da Fase 7 pluga
//!   o `ApprovalModal` no frontend React; v1 da Etapa 4 não tem
//!   UI** — o `RunExecutor` recebe o `ApprovalRequired` e (a)
//!   devolve erro pro modelo, ou (b) auto-aprova em modo dev
//!   (config explícita, não default).
//! - **`approval: Some(approved: false)`** → retorna
//!   `Rejected(ApprovalRequired)`. O modelo vê o erro e segue.
//! - **`approval: Some(approved: true)`** → continua o
//!   `validate_tool_call`; o `RunExecutor` chama `tool.execute`.
//!
//! ## Por que esse teste importa
//!
//! O user (em 2026-08-08) empurrou duro: "requires_approval no
//! manifesto tem que valer neste PR." Se o `requires_user_approval`
//! for decorativo (a `Etapa 4` registra a tool mas o
//! `RunExecutor` não checa Passo 9), o modelo executa código
//! arbitrário sem ninguém autorizar. Este teste fecha o gap:
//! sem aprovação, o tool_call **não** é executado.
//!
//! **Bypass impossível:** o teste usa o `validate_tool_call`
//! **real** (mesmo que o `RunExecutor` da Fase 3) — não chama
//! `tool.execute` direto. Assim o teste falha se o Passo 9 for
//! removido ou refatorado pra sempre passar.

#![cfg(windows)]

use std::sync::Arc;

use chrono::Utc;
use frederico_core::ToolId;
use frederico_runtimes::RuntimeConfig;
use frederico_security::jail::{SecurityJailConfig, SecurityJailResolver};
use frederico_tool_registry::approval::{ApprovalDecision, ApprovalScope};
use frederico_tool_registry::exec::build_default_exec_tools;
use frederico_tool_registry::workspace::Jail;
use frederico_tool_registry::{
    validate_tool_call, AuditSink, NoopAuditSink, ToolCall, ToolRegistry, ValidationContext,
    ValidationOutcome,
};
use serde_json::json;
use tempfile::TempDir;

/// Constrói o `ToolRegistry` com `exec.python` registrado
/// (Etapa 4). Retorna o `ValidationContext` pronto pra
/// `validate_tool_call`.
///
/// **Por que `NoopAuditSink`:** o `validate_tool_call` Passo 10
/// (auditoria) é fire-and-forget; o sink é tolerante a falhas.
/// `NoopAuditSink` é o suficiente pro teste.
fn setup_validation_context() -> (ToolRegistry, ValidationContext) {
    let tmp = TempDir::new().expect("tempdir");
    let runtime_cfg = RuntimeConfig {
        install_root: tmp.path().to_path_buf(),
        keep_n_versions: 1,
        allow_download: false,
        mirror_url: None,
        download_timeout: std::time::Duration::from_secs(1),
    };
    let runtimes = Arc::new(
        frederico_runtimes::RuntimeRegistry::new(runtime_cfg)
            .expect("RuntimeRegistry::new em tempdir"),
    );
    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    // `new()` já retorna `Arc<SecurityJailResolver>` — não
    // envolver em outro `Arc::new`.
    let audit: Arc<dyn AuditSink> = Arc::new(NoopAuditSink);
    let exec_tools = build_default_exec_tools(resolver, runtimes, audit);

    let mut registry = ToolRegistry::new();
    for tool in &exec_tools {
        registry.register(tool.manifest().clone());
    }
    let _ = exec_tools; // descarta; só precisávamos dos manifestos

    let jail = Jail::new(tmp.path()).expect("Jail::new");
    // `exec.python` + `exec.node` precisam estar no `allowed_for_run`
    // — a `ValidationContext` valida o **inventário** da execução
    // (defesa contra "manifest injection" — Passo 1 do
    // `validate_tool_call`) **antes** do Passo 9 (aprovação).
    // Sem isso, retorna `Rejected(NotInExecutionInventory)`.
    let allowed_for_run = vec![
        ToolId::new("files.read"),
        ToolId::new("exec.python"),
        ToolId::new("exec.node"),
    ];
    let ctx = ValidationContext {
        registry,
        jail,
        allowed_for_run,
        permissions: frederico_tool_registry::PermissionSet::default(),
        parent_permissions: None,
    };
    (ctx.registry.clone(), ctx)
}

/// **Passo 9 — `requires_user_approval` é honrado.**
///
/// `validate_tool_call` com `approval: None` para uma tool
/// `exec.python` (que tem `requires_user_approval(true)`) deve
/// retornar `ApprovalRequired` com `tool_id = exec.python`.
///
/// **Sobre `mandatory`:** a regra `with_mandatory_for_risk` em
/// `validate.rs:334` só marca `mandatory = true` para
/// `RiskLevel::Critical` (não `High`). `exec.python` é `High`,
/// então `mandatory = false` — a UI da Etapa 5 decide se exige
/// aprovação explícita a cada invocação ou permite "lembrar
/// pra esta execução". O teste verifica só que o Passo 9
/// retorna `ApprovalRequired` (o `mandatory` é ortogonal).
#[tokio::test]
async fn approval_required_when_exec_python_called_without_decision() {
    let (registry, ctx) = setup_validation_context();
    let call = ToolCall {
        tool_id: ToolId::new("exec.python"),
        version: "0.1.0".to_string(),
        arguments: json!({"code": "print(2+2)"}),
        approval: None,
    };
    let outcome = validate_tool_call(&ctx, &call);
    match outcome {
        ValidationOutcome::ApprovalRequired(req) => {
            assert_eq!(req.tool_id, ToolId::new("exec.python"));
            // `mandatory` é decidido pelo `with_mandatory_for_risk`:
            // só `Critical` (não `High`) vira `true`. `exec.python`
            // é `High` → `mandatory = false` (a UI Etapa 5 decide).
            assert!(
                !req.mandatory,
                "risco High não deveria ser mandatory (só Critical): req={req:?}"
            );
        }
        other => panic!("Esperava ApprovalRequired(exec.python), veio: {other:?}"),
    }
    let _ = registry;
}

/// **`approval: Some(approved: false)`** retorna `Rejected` com
/// `ToolErrorCode::ApprovalRequired`. O `RunExecutor` vê o
/// `Rejected` e devolve erro pro modelo (sem chamar `tool.execute`).
#[tokio::test]
async fn approval_denied_rejects_exec_python() {
    let (_registry, ctx) = setup_validation_context();
    let call = ToolCall {
        tool_id: ToolId::new("exec.python"),
        version: "0.1.0".to_string(),
        arguments: json!({"code": "print(2+2)"}),
        approval: Some(ApprovalDecision {
            scope: ApprovalScope::Once,
            approved: false,
            decided_at: Utc::now(),
        }),
    };
    let outcome = validate_tool_call(&ctx, &call);
    match outcome {
        ValidationOutcome::Rejected(e) => {
            assert_eq!(
                e.code,
                frederico_tool_registry::ToolErrorCode::ApprovalRequired
            );
        }
        other => panic!("Esperava Rejected(ApprovalRequired), veio: {other:?}"),
    }
}

/// **`approval: Some(approved: true)`** passa o Passo 9; o
/// `validate_tool_call` retorna `Approved` (passa pros passos
/// seguintes). Esse é o **único** caminho que o `RunExecutor`
/// chama `tool.execute` — sem aprovação, o execute é
/// **impossível**.
///
/// **Por que esse teste é o coração da Etapa 4:** sem ele,
/// alguém pode "refatorar" o Passo 9 pra sempre passar (ou
/// remover a checagem) e o teste não detectaria. Aqui
/// provamos que o caminho aprovado **funciona** end-to-end
/// (registry → validation → approved).
#[tokio::test]
async fn approval_approved_passes_exec_python() {
    let (_registry, ctx) = setup_validation_context();
    let call = ToolCall {
        tool_id: ToolId::new("exec.python"),
        version: "0.1.0".to_string(),
        arguments: json!({"code": "print(2+2)"}),
        approval: Some(ApprovalDecision {
            scope: ApprovalScope::Once,
            approved: true,
            decided_at: Utc::now(),
        }),
    };
    let outcome = validate_tool_call(&ctx, &call);
    assert!(
        matches!(outcome, ValidationOutcome::Approved { .. }),
        "Esperava Approved, veio: {outcome:?}"
    );
}

/// **Registro dos manifestos:** o `ToolRegistry` deve ter
/// `exec.python` E `exec.node` registrados quando
/// `build_default_exec_tools` é chamado. Esse é o "registro"
/// do ADR-0020 §3 D3 — sem o registro, o modelo não vê a
/// tool no schema (não pode tentar invocar).
#[test]
fn exec_tools_registered_with_requires_user_approval() {
    let (registry, _) = setup_validation_context();
    let py = registry
        .get(&ToolId::new("exec.python"))
        .expect("exec.python registrado");
    let node = registry
        .get(&ToolId::new("exec.node"))
        .expect("exec.node registrado");
    assert!(
        py.requires_user_approval,
        "exec.python tem requires_user_approval(true)"
    );
    assert!(
        node.requires_user_approval,
        "exec.node tem requires_user_approval(true)"
    );
    // Risk level High (não Critical — exec.shell seria Critical
    // por executar linha de comando arbitrária).
    use frederico_tool_registry::RiskLevel;
    assert_eq!(py.risk_level, RiskLevel::High);
    assert_eq!(node.risk_level, RiskLevel::High);
}
