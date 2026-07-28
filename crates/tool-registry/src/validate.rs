//! `validate_tool_call` — os 10 passos do spec §7.7.
//!
//! O spec `tool-registry-specification.md` §"Validação antes de
//! execução" lista 10 checagens, em ordem, e diz "Falha em qualquer
//! etapa produz erro estruturado `TOOL_NOT_FOUND` (ou código
//! equivalente) **sem fallback silencioso**". Esta função implementa
//! a Etapa 2 dos 10 passos:
//!
//! - ✅ Passo 1: ID existe no registro (`TOOL_NOT_FOUND`).
//! - ✅ Passo 2: versão bate (`TOOL_VERSION_MISMATCH`).
//! - ✅ Passo 3: `available` e `healthy` agora
//!   (`TOOL_UNAVAILABLE`).
//! - 🟡 Passo 4: ferramenta está no inventário desta execução
//!   (`TOOL_NOT_IN_INVENTORY`). A Etapa 2 não tem o conceito de
//!   "execução" além do `allowed_for_run`; a checagem
//!   `effective_tools` já cobre a versão mais simples. A
//!   defesa contra "manifest injection" (a ferramenta que o
//!   modelo viu vs. a que está sendo chamada) entra na Etapa 4
//!   via o `tool_call_id` do provedor.
//! - 🟡 Passo 5: permissões OK (`TOOL_PERMISSION_DENIED`). A Etapa 3
//!   implementa. Aqui devolvemos erro genérico se a flag
//!   `requires_user_approval` estiver ligada e não houver
//!   aprovação passada.
//! - ✅ Passo 6: argumentos validam contra `input_schema`
//!   (`TOOL_SCHEMA_INVALID`).
//! - ✅ Passo 7: caminhos normalizados e dentro do jail
//!   (`TOOL_JAIL_VIOLATION`).
//! - ✅ Passo 8: limites aplicados (`TOOL_LIMITS_EXCEEDED`).
//! - ✅ Passo 9: aprovação obtida se necessária
//!   (`TOOL_APPROVAL_REQUIRED`).
//! - 🟡 Passo 10: entrada de auditoria registrada
//!   (`TOOL_AUDIT_FAILED`). A Etapa 5 grava no journal. A Etapa 2
//!   emite um log estruturado; a falha de log é monitorada pelo
//!   `tracing`, não pelo validador.
//!
//! Os passos com 🟡 funcionam no esqueleto (passam ou falham de
//! forma defensiva), mas a implementação completa vem nas etapas
//! seguintes. Cada um tem um teste que documenta o comportamento
//! atual.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use frederico_core::ToolId;
use serde::Serialize;

use crate::approval::{ApprovalDecision, ApprovalRequest};
use crate::error::{ToolError, ToolErrorCode};
use crate::manifest::{RiskLevel, ToolManifest};
use crate::permission::{FileReadPermission, PermissionSet};
use crate::registry::ToolRegistry;
use crate::workspace::Jail;

/// Pedido de `tool_call` para validação. Vem do executor (Etapa 4);
/// na Etapa 2 é construído direto nos testes.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    /// ID da ferramenta chamada.
    pub tool_id: ToolId,
    /// Versão que o modelo "viu" (passada no `tool_call` do provedor).
    /// Tem que bater com a do manifesto no registry.
    pub version: String,
    /// Argumentos como `serde_json::Value` (raw, ainda não validados).
    pub arguments: serde_json::Value,
    /// Decisão de aprovação obtida do usuário, se houver.
    pub approval: Option<ApprovalDecision>,
}

/// Resultado da validação. `Approved` significa "pode executar"; as
/// outras variantes são "rejeitado antes" (com `ToolError`) ou
/// "precisa de aprovação" (com `ApprovalRequest` pra UI).
///
/// `ToolManifest` (variante `Approved`) e `ToolError` (variante
/// `Rejected`) são `Box`-ed porque ambos carregam `serde_json::Value`
/// (detalhes/payload) que infla o enum. A `ApprovalRequest` é pequena
/// (sem `Value`). `Box` mantém o enum raso, o que ajuda o
/// `ValidationContext` (que é `Clone` e vive em threads).
#[derive(Debug, Clone)]
pub enum ValidationOutcome {
    /// Passou todas as checagens. `tool_call` pode ser executado.
    Approved {
        /// Manifesto validado (a Etapa 4 usa para acessar `category`,
        /// `risk_level`, etc. no journal).
        manifest: Box<ToolManifest>,
        /// Caminhos normalizados que foram validados como dentro do jail.
        /// Vai pro `ToolResult.accessed_paths` quando a ferramenta
        /// executar.
        resolved_paths: Vec<PathBuf>,
    },
    /// Precisa de aprovação do usuário antes de executar. O
    /// `ApprovalRequest` é o que vai pra UI.
    ApprovalRequired(ApprovalRequest),
    /// Rejeitado antes de executar. O `ToolError` carrega o código
    /// canônico.
    Rejected(Box<ToolError>),
}

/// Contexto de validação. Encapsula o que é estável durante uma
/// execução: o registry, o jail, as ferramentas permitidas e as
/// permissões (Etapa 3) da execução.
#[derive(Debug, Clone)]
pub struct ValidationContext {
    /// Registry com os manifestos das ferramentas.
    pub registry: ToolRegistry,
    /// Jail do workspace atual. Mesmo objeto passado para a
    /// ferramenta concreta (`FilesReadTool::new`).
    pub jail: Jail,
    /// Ferramentas permitidas para este `Run` específico.
    /// Vem da Etapa 4 do `Run.allowed_tools` (ou do
    /// `RunExecutor`).
    pub allowed_for_run: Vec<ToolId>,
    /// Permissões da execução. O Passo 5 do `validate_tool_call`
    /// consome. **Default é deny** (todo `false` / `None`); a
    /// Etapa 4 carrega o `PermissionSet` real do
    /// `assistant`/`project`/`user` antes de validar.
    pub permissions: PermissionSet,
    /// Permissões do agente pai, se esta execução for um
    /// subagente. Usado para validar o invariante
    /// "subagente ⊆ pai" da Fase 6 — a Etapa 3 modela o campo e
    /// valida estaticamente no `is_subset_of`; a Etapa 4 (ou 6)
    /// aplica em runtime.
    pub parent_permissions: Option<Box<PermissionSet>>,
}

impl ValidationContext {
    /// Construtor básico. `permissions` default deny; sem pai
    /// (não é subagente).
    pub fn new(registry: ToolRegistry, jail: Jail, allowed_for_run: Vec<ToolId>) -> Self {
        Self {
            registry,
            jail,
            allowed_for_run,
            permissions: PermissionSet::default(),
            parent_permissions: None,
        }
    }

    /// Construtor com permissões explícitas.
    pub fn with_permissions(
        registry: ToolRegistry,
        jail: Jail,
        allowed_for_run: Vec<ToolId>,
        permissions: PermissionSet,
        parent_permissions: Option<Box<PermissionSet>>,
    ) -> Self {
        Self {
            registry,
            jail,
            allowed_for_run,
            permissions,
            parent_permissions,
        }
    }

    /// Valida o invariante "subagente ⊆ pai" se `parent_permissions`
    /// estiver presente. Retorna `Ok(())` se OK, ou `Err(ToolError)`
    /// com `TOOL_PERMISSION_DENIED` se o subagente tem mais
    /// permissões que o pai.
    pub fn check_subagent_invariant(&self) -> Result<(), ToolError> {
        if let Some(parent) = &self.parent_permissions {
            if !self.permissions.is_subset_of(parent) {
                return Err(ToolError::new(
                    ToolErrorCode::PermissionDenied,
                    "subagente tem permissão que o pai não tem (violação do invariante subagente ⊆ pai)",
                ));
            }
        }
        Ok(())
    }
}

/// Função principal de validação. Recebe o `call` e o `ctx`,
/// devolve o `ValidationOutcome` apropriado.
#[must_use]
pub fn validate_tool_call(ctx: &ValidationContext, call: &ToolCall) -> ValidationOutcome {
    // Passo 1: ID existe.
    let Some(manifest) = ctx.registry.get(&call.tool_id) else {
        return ValidationOutcome::Rejected(Box::new(
            ToolError::new(
                ToolErrorCode::ToolNotFound,
                format!("ferramenta {} não está no registro", call.tool_id),
            )
            .with_tool_id(call.tool_id.clone()),
        ));
    };

    // Passo 2: versão.
    if manifest.version != call.version {
        return ValidationOutcome::Rejected(Box::new(
            ToolError::new(
                ToolErrorCode::VersionMismatch,
                format!(
                    "versão {} não bate com a do manifesto {}",
                    call.version, manifest.version
                ),
            )
            .with_tool_id(call.tool_id.clone()),
        ));
    }

    // Passo 3: disponível e saudável **neste momento**.
    if !manifest.is_callable_now(ctx.registry.health(&call.tool_id)) {
        return ValidationOutcome::Rejected(Box::new(
            ToolError::new(
                ToolErrorCode::Unavailable,
                format!(
                    "ferramenta {} não está disponível agora (availability={:?}, health={:?})",
                    call.tool_id,
                    manifest.availability,
                    ctx.registry.health(&call.tool_id)
                ),
            )
            .with_tool_id(call.tool_id.clone()),
        ));
    }

    // Passo 4: no inventário desta execução.
    if !ctx.allowed_for_run.contains(&call.tool_id) {
        return ValidationOutcome::Rejected(Box::new(
            ToolError::new(
                ToolErrorCode::NotInExecutionInventory,
                format!(
                    "ferramenta {} não está no inventário desta execução (defesa contra manifest injection)",
                    call.tool_id
                ),
            )
            .with_tool_id(call.tool_id.clone()),
        ));
    }

    // Passo 5: permissões. Etapa 3 implementa o `PermissionSet`.
    // - Se a execução é subagente e o invariante `subagente ⊆ pai`
    //   falha, rejeita com `PermissionDenied`.
    // - Se a ferramenta exige `file_read` e as permissões
    //   têm `file_read == None`, rejeita.
    if let Err(e) = ctx.check_subagent_invariant() {
        return ValidationOutcome::Rejected(Box::new(e));
    }
    if manifest.requires_file_read && ctx.permissions.file_read == FileReadPermission::None {
        return ValidationOutcome::Rejected(Box::new(
            ToolError::new(
                ToolErrorCode::PermissionDenied,
                "file_read desabilitado nas permissões da execução",
            )
            .with_tool_id(call.tool_id.clone()),
        ));
    }
    // Para `WorkspaceOnly` e `WorkspacePlusApproved`: o Jail já garante
    // que o path está dentro do workspace (Passo 7 abaixo). O
    // `WorkspacePlusApproved` permite leitura fora do workspace via
    // aprovação — a Etapa 6 (UI) consome essa flag no modal de
    // aprovação; a Etapa 2 não tem essa nuance (Jail é estrito).

    // Passo 6: schema dos argumentos.
    if let Err(e) = manifest.input_schema.validate(&call.arguments) {
        return ValidationOutcome::Rejected(Box::new(e.with_tool_id(call.tool_id.clone())));
    }

    // Passo 7: jail. Para ferramentas que envolvem path, normaliza o
    // caminho via Jail. A Etapa 2 só tem `files.read` que
    // conhece o campo `path` no `arguments`. A Etapa 4 generaliza
    // via `Tool::resolve_paths(&self, args) -> Vec<PathBuf>`.
    let mut resolved_paths = Vec::new();
    if manifest.requires_file_read {
        // Para a Etapa 2, esperamos `arguments.path` ser uma string.
        // A Etapa 4 introduz um trait `Tool` com
        // `validate_paths(&self, args, &Jail)`.
        if let Some(path_str) = call.arguments.get("path").and_then(|v| v.as_str()) {
            match ctx.jail.resolve(std::path::Path::new(path_str)) {
                Ok(p) => resolved_paths.push(p),
                Err(e) => {
                    return ValidationOutcome::Rejected(Box::new(
                        e.with_tool_id(call.tool_id.clone()),
                    ));
                }
            }
        }
    }

    // Passo 8: limites. Na Etapa 2, o schema do manifesto já
    // carrega os limites sintáticos (ex.: `maximum: 52428800` no
    // `files.read` cobre o teto de 50 MB). `ToolErrorCode::LimitsExceeded`
    // fica no enum para a Etapa 4 reintroduzir quando houver limites
    // que não cabem no schema (timeoutMs, contadores).
    // Veja o teste `limits_exceeded_via_schema_is_rejected`.

    // Passo 9: aprovação. A ferramenta exige aprovação e nenhuma
    // decisão foi passada, OU a decisão foi "Negar".
    if manifest.requires_user_approval {
        match &call.approval {
            None => {
                return ValidationOutcome::ApprovalRequired(
                    ApprovalRequest::new(
                        call.tool_id.clone(),
                        "esta ferramenta exige aprovação do usuário antes de executar",
                        call.arguments.clone(),
                    )
                    .with_mandatory_for_risk(manifest.risk_level)
                    .with_paths(resolved_paths.clone())
                    .with_requested_at_now(),
                );
            }
            Some(d) if !d.approved => {
                return ValidationOutcome::Rejected(Box::new(ToolError::new(
                    ToolErrorCode::ApprovalRequired,
                    "aprovação negada pelo usuário",
                )));
            }
            Some(_) => {
                // aprovado — segue
            }
        }
    }

    // Passo 10: auditoria. A Etapa 5 grava no journal. Aqui
    // apenas emitimos um log estruturado via `tracing` (se ativo).
    // A falha de log é tolerada — `tracing` não falha.
    tracing::info!(
        tool_id = %call.tool_id,
        version = %call.version,
        "tool_call validado"
    );

    ValidationOutcome::Approved {
        manifest: Box::new(manifest.clone()),
        resolved_paths,
    }
}

// Helpers privados para `ApprovalRequest::with_*` que estão sendo
// usados dentro deste módulo. Mantidos em `validate.rs` para não
// inflar a API pública de `approval.rs`.
impl ApprovalRequest {
    pub(crate) fn with_mandatory_for_risk(mut self, risk: RiskLevel) -> Self {
        // Spec `tool-permission-model.md` §"Invariantes":
        // "`risk_level = critical` exigem aprovação explícita a cada
        // invocação, sem 'lembrar para esta execução'." Para outros
        // riscos, o caller decide o escopo (Once/Run/Project).
        if matches!(risk, RiskLevel::Critical) {
            self.mandatory = true;
        }
        self
    }

    pub(crate) fn with_paths(mut self, paths: Vec<PathBuf>) -> Self {
        if let Some(p) = paths.into_iter().next() {
            self.path = Some(p);
        }
        self
    }

    pub(crate) fn with_requested_at_now(mut self) -> Self {
        self.requested_at = DateTime::<Utc>::from(std::time::SystemTime::now());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalScope;
    use crate::manifest::{RiskLevel, ToolCategory, ToolManifestBuilder};
    use std::fs;

    fn setup() -> (Tempdir, ValidationContext) {
        let dir = Tempdir::new();
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        let jail = Jail::new(&dir).unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolManifestBuilder::new(ToolId::new("files.read"), "files")
                .display_name("Ler arquivo")
                .description("Lê arquivo do workspace.")
                .risk_level(RiskLevel::Safe)
                .category(ToolCategory::Files)
                .requires_file_read(true)
                .input_schema(crate::manifest::JsonSchema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "max_bytes": {"type": "integer", "minimum": 1, "maximum": 52428800}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                })))
                .build()
                .unwrap(),
        );
        let perms = crate::permission::PermissionSet {
            file_read: crate::permission::FileReadPermission::WorkspaceOnly,
            ..Default::default()
        };
        let ctx = ValidationContext::with_permissions(
            registry,
            jail,
            vec![ToolId::new("files.read")],
            perms,
            None,
        );
        (dir, ctx)
    }

    struct Tempdir(PathBuf);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let unique = format!(
                "frederico-tool-registry-validate-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            let dir = base.join(unique);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn join(&self, p: &str) -> PathBuf {
            self.0.join(p)
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
    fn unknown_tool_is_rejected() {
        let (_d, ctx) = setup();
        let call = ToolCall {
            tool_id: ToolId::new("unknown.tool"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::ToolNotFound);
            }
            _ => panic!("esperava Rejected, veio {out:?}"),
        }
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let (_d, ctx) = setup();
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "9.9.9".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::VersionMismatch);
            }
            _ => panic!("esperava Rejected, veio {out:?}"),
        }
    }

    #[test]
    fn not_in_run_allowlist_is_rejected() {
        let (_d, mut ctx) = setup();
        // Tira files.read da allowlist da execução
        ctx.allowed_for_run.clear();
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::NotInExecutionInventory);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn schema_invalid_is_rejected() {
        let (_d, ctx) = setup();
        // arguments.path é u64, não string — schema do files.read
        // exige string
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": 42}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::SchemaInvalid);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn jail_violation_is_rejected() {
        let (_d, ctx) = setup();
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "../etc/passwd"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::JailViolation);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn limits_exceeded_via_schema_is_rejected() {
        // Cobertura do "limits" do spec §7.7 item 8: na Etapa 2, o
        // schema do manifesto é o gate primário de limites
        // (ex.: `max_bytes <= 50 MB`). O `ToolErrorCode::LimitsExceeded`
        // existe no enum (cobrindo o caso de Etapa 4 onde limits
        // dinâmicos como timeoutMs ou contadores precisam ser
        // checados em runtime); a Etapa 4 reintroduz o `if
        // max > LIMITE` se necessário.
        let (_d, ctx) = setup();
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt", "max_bytes": 100_000_000}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                // O schema tem `maximum: 52428800` (50 MB); 100 MB
                // cai no `SchemaInvalid`. Etapa 4 reintroduz a
                // distinção se houver limites que não cabem no
                // schema (timeoutMs, contadores).
                assert_eq!(e.code, ToolErrorCode::SchemaInvalid);
            }
            _ => panic!("esperava Rejected, veio {out:?}"),
        }
    }

    #[test]
    fn approval_required_when_manifest_says_so() {
        let (_d, mut ctx) = setup();
        // Adiciona uma tool com risco Critical (que automaticamente
        // vira `mandatory: true` na `ApprovalRequest`).
        ctx.registry.register(
            ToolManifestBuilder::new(ToolId::new("exec.dangerous"), "exec")
                .display_name("Comando perigoso")
                .description("Executa comando arbitrário.")
                .risk_level(RiskLevel::Critical)
                .category(ToolCategory::Exec)
                .requires_user_approval(true)
                .build()
                .unwrap(),
        );
        ctx.allowed_for_run.push(ToolId::new("exec.dangerous"));
        let call = ToolCall {
            tool_id: ToolId::new("exec.dangerous"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::ApprovalRequired(req) => {
                assert!(req.mandatory, "risco Critical deve ser mandatory");
                assert_eq!(req.tool_id, ToolId::new("exec.dangerous"));
            }
            _ => panic!("esperava ApprovalRequired, veio {out:?}"),
        }
    }

    #[test]
    fn approval_denied_is_rejected() {
        let (_d, mut ctx) = setup();
        ctx.registry.register(
            ToolManifestBuilder::new(ToolId::new("files.write"), "files")
                .display_name("Escrever arquivo")
                .description("Escreve arquivo no workspace.")
                .risk_level(RiskLevel::Moderate)
                .category(ToolCategory::Files)
                .requires_user_approval(true)
                .build()
                .unwrap(),
        );
        ctx.allowed_for_run.push(ToolId::new("files.write"));
        let call = ToolCall {
            tool_id: ToolId::new("files.write"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({}),
            approval: Some(ApprovalDecision {
                scope: ApprovalScope::Once,
                approved: false,
                decided_at: Utc::now(),
            }),
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::ApprovalRequired);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn happy_path_is_approved() {
        let (_d, ctx) = setup();
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Approved {
                manifest,
                resolved_paths,
            } => {
                assert_eq!(manifest.id, ToolId::new("files.read"));
                assert_eq!(resolved_paths.len(), 1);
            }
            _ => panic!("esperava Approved, veio {out:?}"),
        }
    }

    #[test]
    fn file_read_none_permission_rejects() {
        // Permissões default (file_read: None) rejeitam files.read
        // com TOOL_PERMISSION_DENIED — Passo 5 do spec §7.7.
        let (_d, mut ctx) = setup();
        // Zerar file_read (já é None por default, mas explicito).
        ctx.permissions.file_read = FileReadPermission::None;
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::PermissionDenied);
            }
            _ => panic!("esperava Rejected(PermissionDenied), veio {out:?}"),
        }
    }

    #[test]
    fn file_read_workspace_plus_approved_accepts() {
        let (_d, mut ctx) = setup();
        ctx.permissions.file_read = FileReadPermission::WorkspacePlusApproved;
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Approved { .. } => {}
            _ => panic!("esperava Approved, veio {out:?}"),
        }
    }

    #[test]
    fn subagent_with_more_permissions_than_parent_is_rejected() {
        // Invariante "subagente ⊆ pai" — Fase 6, modelado na Etapa 3.
        // Subagente tem `file_read = WorkspacePlusApproved` mas o pai
        // tem `WorkspaceOnly` → o `is_subset_of` retorna false, e o
        // `validate_tool_call` rejeita no Passo 5.
        let (_d, mut ctx) = setup();
        let parent = PermissionSet {
            file_read: FileReadPermission::WorkspaceOnly,
            ..Default::default()
        };
        ctx.permissions.file_read = FileReadPermission::WorkspacePlusApproved;
        ctx.parent_permissions = Some(Box::new(parent));
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Rejected(e) => {
                assert_eq!(e.code, ToolErrorCode::PermissionDenied);
                assert!(e.message.contains("subagente"));
            }
            _ => panic!("esperava Rejected(PermissionDenied), veio {out:?}"),
        }
    }

    #[test]
    fn subagent_with_subset_of_parent_permissions_is_accepted() {
        let (_d, mut ctx) = setup();
        let parent = PermissionSet {
            file_read: FileReadPermission::WorkspacePlusApproved,
            ..Default::default()
        };
        ctx.permissions.file_read = FileReadPermission::WorkspaceOnly;
        ctx.parent_permissions = Some(Box::new(parent));
        let call = ToolCall {
            tool_id: ToolId::new("files.read"),
            version: "0.1.0".to_string(),
            arguments: serde_json::json!({"path": "hello.txt"}),
            approval: None,
        };
        let out = validate_tool_call(&ctx, &call);
        match out {
            ValidationOutcome::Approved { .. } => {}
            _ => panic!("esperava Approved, veio {out:?}"),
        }
    }
}
