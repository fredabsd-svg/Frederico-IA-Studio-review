//! Testes do **path safety** do `docs.generate` (Fase de
//! Ligação Etapa 5.X — patch-allowed-paths).
//!
//! Estes testes provam o **fluxo completo** `Tool::execute`:
//! `Jail::resolve_allowing_nonexistent` (barreira primária) →
//! mitigação symlink (parcial, rótulo explícito) →
//// `dispatcher.check_path` (defesa em profundidade, fail-closed).
//!
//! Cada cenário é um `#[tokio::test]` separado pra isolar setup
//! e asserção. Usa `FakeWorker` in-process (sem Python, sem
//! `bootstrap.ps1`) — roda no CI comum.
//!
//! ## Cobertura (matriz do que era testado e deixou de ser)
//!
//! Antes da Etapa 5.X, dois testes de unidade em
//! `generate.rs::tests` (`rejects_unknown_format_in_args` e
//! `rejects_invalid_spec`) usavam `output_path: "C:\\temp\\out.docx"`
//! (absoluto). A Etapa 5.X ajusta pra `output_path: "out.docx"`
//! (relativo) porque o `Jail::resolve_allowing_nonexistent`
//! rejeita absoluto **antes** de chegar no format/spec — o que
//! muda a primeira asserção que falha e portanto o que cada
//! teste cobre. A cobertura do "absoluto é rejeitado" **não
//! pode desaparecer no caminho**: o **cenário 2** deste arquivo
//! cobre exatamente isso, com a asserção focada em
//! `JailViolation` (não no erro de format/spec que ficava em
//! segundo plano antes).

#![cfg(windows)] // symlink test depende de privilege Windows; ver cenário 5

use std::sync::Arc;

use frederico_core::{ConversationId, MessageId, RunId};
use frederico_document_engine::{
    DocumentBlock, DocumentMetadata, DocumentSpec, DocumentStyle, DocumentType, SpecVersion,
};
use frederico_document_kits::{DocsGenerateTool, KitRegistry, WordProKit};
use frederico_process_architecture::{FakeWorkerConfig, WorkerManager, WorkerSpawnConfig};
use frederico_tool_registry::{Jail, Tool, ToolContext, WorkerToolDispatcher};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Constrói um `ToolContext` com jail em tempdir único.
fn dummy_ctx() -> ToolContext {
    let workspace = std::env::temp_dir().join(format!(
        "frederico-document-kits-path-safety-{}-{}",
        std::process::id(),
        Uuid::new_v4(),
    ));
    std::fs::create_dir_all(&workspace).expect("dummy_ctx: mkdir");
    let jail = Jail::new(&workspace).expect("dummy_ctx: Jail::new");
    ToolContext::new(
        ConversationId(Uuid::nil()),
        RunId(Uuid::nil()),
        MessageId(Uuid::nil()),
        jail,
    )
}

/// Spec mínima válida (cobre os campos obrigatórios do schema).
fn minimal_spec() -> DocumentSpec {
    DocumentSpec {
        spec_version: SpecVersion::default(),
        doc_type: DocumentType::Report,
        style: DocumentStyle::default(),
        language: "pt-br".to_string(),
        metadata: DocumentMetadata::default(),
        blocks: vec![DocumentBlock::Paragraph {
            text: "x".to_string(),
            style: None,
        }],
        confidentiality: None,
    }
}

/// Constrói o `DocsGenerateTool` + o `WorkerToolDispatcher` (o
/// segundo é retornado pro cenário 6 que precisa chamar
/// `check_path` direto, fora do `execute`).
async fn build_tool() -> (DocsGenerateTool, WorkerToolDispatcher, WorkerManager) {
    let (manager, handle) =
        WorkerManager::spawn_in_process(FakeWorkerConfig::default(), WorkerSpawnConfig::default())
            .await
            .expect("spawn fake in-process");

    let mut registry = KitRegistry::new();
    registry.register(Arc::new(WordProKit::new(Arc::new(handle.clone()))));
    let registry = Arc::new(registry);

    let dispatcher = WorkerToolDispatcher::new(Arc::new(handle));
    let tool = DocsGenerateTool::new(registry, dispatcher.clone());
    (tool, dispatcher, manager)
}

// ---------------------------------------------------------------------------
// Cenários
// ---------------------------------------------------------------------------

/// **Cenário 1 — allow relativo.** `output_path: "out.docx"`
/// resolve pro workspace da conversa; barreira primária
/// (`Jail::resolve_allowing_nonexistent`) aceita; mitigação
/// symlink (passo 2) aceita (arquivo não existe); defesa em
/// profundidade (`check_path` com `root_canonical`) aceita;
/// `kit.render` (com `FakeWorker`) devolve `ok: true`.
///
/// **Por que não asserta `is_file()`:** o `FakeWorker` é
/// in-process e não toca no FS — devolve `{ok: true, ...}`
/// sem criar o arquivo. A asserção aqui é só "a barreira
/// **aceitou** o path relativo válido" (i.e., o fluxo todo
/// chegou até o `kit.render` e voltou `ok: true`). A
/// confirmação de que o **arquivo real é criado no canônico
/// do jail** é o cenário do E2E real com Python
/// (`e2e_docs_generate_with_real_worker`, commit 4 do PR).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_1_allow_relative_path() {
    let (tool, _dispatcher, _manager) = build_tool().await;
    let ctx = dummy_ctx();
    let spec_json = serde_json::to_value(minimal_spec()).expect("serializa spec");

    let r = tool
        .execute(
            &ctx,
            &json!({
                "spec": spec_json,
                "output_path": "out.docx",
                "format": "docx",
            }),
        )
        .await;

    assert!(
        r.ok,
        "barreira rejeitou path relativo válido. error_message: {:?}",
        r.error_message
    );
}

/// **Cenário 2 — reject absoluto.** Cobre o que os 2 testes
/// de unidade em `generate.rs::tests` perderam quando foram
/// ajustados de absoluto pra relativo: `output_path` absoluto
/// **fora do jail** é rejeitado com `JailViolation` (cobre
/// `C:\Windows\System32\...` no Windows e `/etc/passwd` no
/// Unix).
///
/// Importante: este cenário é o que **garante** que o
/// ajuste de `rejects_unknown_format_in_args` /
/// `rejects_invalid_spec` pra path relativo não virou
/// "remoção de teste com outro nome". A asserção é focada
/// em `JailViolation` (não em erro de format ou spec).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_2_reject_absolute_path_outside_jail() {
    let (tool, _dispatcher, _manager) = build_tool().await;
    let ctx = dummy_ctx();
    let spec_json = serde_json::to_value(minimal_spec()).expect("serializa spec");
    let abs_path = if cfg!(windows) {
        r"C:\Windows\System32\out.docx"
    } else {
        "/etc/passwd"
    };
    let r = tool
        .execute(
            &ctx,
            &json!({
                "spec": spec_json,
                "output_path": abs_path,
                "format": "docx",
            }),
        )
        .await;
    assert!(!r.ok, "esperava erro, veio {:?}", r);
    let msg = r.error_message.unwrap_or_default();
    assert!(
        msg.contains("fora do workspace") || msg.contains("JailViolation"),
        "esperava erro de jail (JailViolation), veio: {msg}"
    );
}

/// **Cenário 3 — reject traversal.** `output_path: "../<outro_cid>/secret.docx"`
/// tem `..` no caminho; a `Jail::resolve_allowing_nonexistent`
/// rejeita no loop de componentes (antes de tocar o FS). Cobre
/// tentativa de escapar pra outro workspace de conversa.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_3_reject_parent_traversal() {
    let (tool, _dispatcher, _manager) = build_tool().await;
    let ctx = dummy_ctx();
    let spec_json = serde_json::to_value(minimal_spec()).expect("serializa spec");
    let r = tool
        .execute(
            &ctx,
            &json!({
                "spec": spec_json,
                "output_path": "../<outro_cid>/secret.docx",
                "format": "docx",
            }),
        )
        .await;
    assert!(!r.ok, "esperava erro, veio {:?}", r);
    let msg = r.error_message.unwrap_or_default();
    assert!(
        msg.contains("'..'") || msg.contains("JailViolation"),
        "esperava erro de '..' no caminho (JailViolation), veio: {msg}"
    );
}

/// **Cenário 4 — allow mixed-case.** Workspace criado em
/// `tempdir/Workspace-Lower-Case/`, `output_path: "Output.DOCX"`
/// (case diferente do FS). No Windows o FS é case-insensitive
/// mas case-preserving; comparação ingênua de string
/// rejeita indevidamente.
///
/// Aqui ambos os lados da comparação (output_path canônico
/// do `Jail::resolve_allowing_nonexistent` e o `root_canonical`
/// passado como allowlist) vêm da **mesma** canonicalização
/// do `Jail::new` — `Path::starts_with` component-wise é
/// confiável. O `output_path_resolved` preserva o case do
/// filename (Output.DOCX) mas o `starts_with` só olha o
/// prefixo do diretório, que casa.
///
/// Se um dia esses dois lados divergirem (ex.: alguém
/// canonicalizar a allowlist por uma via diferente do Jail),
/// o `Path::starts_with` falha e a defesa em profundidade
/// rejeita — daí a importância de passar
/// `ctx.jail.root_canonical()` (case do FS) como allowlist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_4_allow_mixed_case_filename() {
    let (tool, _dispatcher, _manager) = build_tool().await;
    // Jail com nome no filesystem que força divergência de
    // case (o `Jail::new` canonicaliza pro case do FS).
    let workspace_lc = std::env::temp_dir().join(format!(
        "Workspace-Lower-Case-{}-{}",
        std::process::id(),
        Uuid::new_v4(),
    ));
    std::fs::create_dir_all(&workspace_lc).expect("mkdir workspace_lc");
    let jail = Jail::new(&workspace_lc).expect("Jail::new workspace_lc");
    let ctx = ToolContext::new(
        ConversationId(Uuid::nil()),
        RunId(Uuid::nil()),
        MessageId(Uuid::nil()),
        jail,
    );
    let spec_json = serde_json::to_value(minimal_spec()).expect("serializa spec");

    let r = tool
        .execute(
            &ctx,
            &json!({
                "spec": spec_json,
                "output_path": "Output.DOCX", // case diferente do FS
                "format": "docx",
            }),
        )
        .await;

    // O FakeWorker devolve `{ok: true, ...}`. Se a barreira
    // primária (Jail::resolve_allowing_nonexistent) ou a
    // defesa em profundidade (check_path com root_canonical)
    // rejeitar por case, `r.ok` é false.
    assert!(
        r.ok,
        "mixed-case filename rejeitado indevidamente. error_message: {:?}",
        r.error_message
    );
}

/// **Cenário 5 — reject symlink (mitigação parcial).** O
/// workspace contém `link.txt` → symlink pra `/etc/hosts` (Unix)
/// ou `C:\Windows\System32\drivers\etc\hosts` (Windows).
///
/// O `Jail::resolve_allowing_nonexistent` aceita (o **pai** é
/// o workspace, ok); o `kit.render` depois abriria o symlink e
/// o Python escreveria no destino — **bypass do jail**. A
/// mitigação no `execute` (passo 2, `symlink_metadata`) detecta.
///
/// **Rótulo:** isto é **mitigação parcial**, não barreira
/// (TOCTOU entre o check e a escrita do worker; não cobre o
/// caso do arquivo não existir, que é o caso normal). A
/// barreira de verdade — `O_NOFOLLOW` / `O_CREAT|O_EXCL` no
/// `open` do Python — é pendência nomeada em
/// `docs/modules/process-architecture.md` item 5.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_5_reject_symlink_output() {
    let (tool, _dispatcher, _manager) = build_tool().await;
    let ctx = dummy_ctx();
    let target = if cfg!(windows) {
        r"C:\Windows\System32\drivers\etc\hosts"
    } else {
        "/etc/hosts"
    };
    let link = ctx.jail.root().join("link.txt");
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(target, &link);
    #[cfg(windows)]
    let _ = std::os::windows::fs::symlink_file(target, &link);
    if !link.exists() {
        eprintln!("symlink não pôde ser criado (privilege?), pulando teste");
        return;
    }

    let spec_json = serde_json::to_value(minimal_spec()).expect("serializa spec");
    let r = tool
        .execute(
            &ctx,
            &json!({
                "spec": spec_json,
                "output_path": "link.txt",
                "format": "docx",
            }),
        )
        .await;
    assert!(!r.ok, "esperava erro, veio {:?}", r);
    let msg = r.error_message.unwrap_or_default();
    assert!(
        msg.contains("symlink"),
        "esperava erro de symlink, veio: {msg}"
    );
}

/// **Cenário 6 — fail-closed (defesa em profundidade).** O
/// `Tool::execute` passa allowlist não-vazia (o `root_canonical`
/// do jail), então o check_path fail-closed **não dispara** no
/// fluxo end-to-end. Aqui chamamos o `check_path` **direto**
/// com allowlist vazia pra provar que ele nega — regressão
/// contra alguém "simplificar" o `validate_against_allowlist`
/// voltando ao fail-open (que era o comportamento da Etapa 3).
///
/// Se um dia alguém remover o `Jail::resolve_allowing_nonexistent`
/// do `execute` (barreira primária), o check_path fail-closed
/// com allowlist não-vazia (`[root_canonical]`) **ainda barra**
/// (defesa em profundidade); com allowlist vazia, nega
/// imediatamente. A invariante é: allowlist vazia = nega.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_6_check_path_fail_closed_on_empty_allowlist() {
    let (_tool, dispatcher, _manager) = build_tool().await;
    let err = dispatcher
        .check_path("anywhere/inside/jail.docx", &[])
        .expect_err("check_path com allowlist vazia deve negar (fail-closed)");
    use frederico_tool_registry::DispatchError;
    assert!(
        matches!(err, DispatchError::PathNotAllowed { .. }),
        "esperava PathNotAllowed, veio: {err:?}"
    );
}
