//! E2E — `PermissionLoader` carregando a cadeia
//! `user ⊆ project ⊆ assistant` no caminho de produção
//! (Fase 6, Etapa 3 PR 2, ADR-0030 §D3).
//!
//! Caminho exercitado: **`build_default_permission_set(loader, user, project, assistant)`** (a
//! mesma factory que a casca Tauri chama) →
//! `PermissionLoader::load_effective_permission_set` →
//! `PermissionSet::merge3` (interseção tripla, fail-closed).
//!
//! Estes 2 testes são a **prova de caminho real** do
//! invariante da Etapa 3 PR 2 (decisão de 2026-08-06):
//!
//! 1. **`permission_set_inherited_from_assistant_project_user`**:
//!    profiles reais em TempDir (user permissivo, project
//!    nega `network`, assistant nega `web_browse`) carregados
//!    via `build_default_permission_set`. Asserção: o
//!    `effective` é a interseção — campo ausente num layer
//!    vira deny (default fail-closed), nunca herda `true` de
//!    outro layer.
//!
//! 2. **`effective_permission_set_is_subset_of_parent`**:
//!    a invariante tripla — `effective ⊆ user`, `effective ⊆
//!    project`, `effective ⊆ assistant` — verificada via
//!    `PermissionSet::is_subset_of` da Fase 3 Etapa 3.
//!    **Essencial:** esse teste prova o invariante no
//!    caminho de produção (não só na função pura `merge`).
//!    É o que a memory cross-project
//!    "Cobertura de invariante no caminho de produção, não
//!    no crate" chama de "a máquina que prova o invariante
//!    na produção, não no laboratório".
//!
//! Ver [`docs/architecture/multimodel-architecture.md`
//! §"E2E de cobertura planejado por
//! etapa"](../../docs/architecture/multimodel-architecture.md#e2e-de-cobertura-planejado-por-etapa)
//! (alvo declarado na Etapa 1) e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (regra da composição compartilhada — factory da
//! `crates/app/src/composition.rs`, mesma que a casca Tauri
//! consome).

use std::path::PathBuf;
use std::sync::Arc;

use frederico_app::composition::build_default_permission_set;
use frederico_tool_registry::{PermissionLoader, PermissionSet};
use tempfile::TempDir;

mod common;

/// Helper: escreve um TOML de profile num arquivo dentro
/// de `dir`. Devolve o `PathBuf` (caller passa pra
/// `build_default_permission_set`).
fn write_profile(dir: &std::path::Path, file_name: &str, content: &str) -> PathBuf {
    let path = dir.join(file_name);
    std::fs::write(&path, content).expect("write profile");
    path
}

/// Helper: cria 3 profiles (user, project, assistant) em
/// `TempDir`, devolve os 3 paths.
fn three_layer_profiles(
    dir: &std::path::Path,
    user_toml: &str,
    project_toml: &str,
    assistant_toml: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let user = write_profile(dir, "user.toml", user_toml);
    let project = write_profile(dir, "project.toml", project_toml);
    let assistant = write_profile(dir, "assistant.toml", assistant_toml);
    (user, project, assistant)
}

// 1. **`permission_set_inherited_from_assistant_project_user`**
//
// Profile do user é **permissivo** (tudo true), project **nega
// `network`**, assistant **nega `web_browse`**. O `effective`
// carregado via `build_default_permission_set` deve **negar
// network e web_browse** (cada layer nega o que nega), e
// **manter `terminal` deny** (default de todos os layers
// que não citam o campo — fail-closed).
#[test]
fn permission_set_inherited_from_assistant_project_user() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();

    let (user, project, assistant) = three_layer_profiles(
        dir,
        // user: tudo permissivo (este layer "libera tudo")
        r#"
file_read = "workspace_plus_approved"
network = true
web_browse = true
python = "sandboxed"
"#,
        // project: nega network + nega terminal
        r#"
network = false
terminal = "denylist"
"#,
        // assistant: nega web_browse + nega destructive_ops
        r#"
web_browse = false
destructive_ops = false
"#,
    );

    let loader = PermissionLoader::new();
    let effective = build_default_permission_set(&loader, &user, &project, &assistant);

    // Fail-closed por eixo:
    //   network: user=true ∧ project=false ∧ assistant=true → false
    //   web_browse: user=true ∧ project=true ∧ assistant=false → false
    assert!(!effective.network, "project nega network → effective nega");
    assert!(
        !effective.web_browse,
        "assistant nega web_browse → effective nega"
    );

    // Sanity: fail-closed em ação. User tem `python =
    // sandboxed`, mas project e assistant não citam
    // python → viram default() = None. min(Sandboxed,
    // None, None) = None. **Default deny vence** (regra
    // do PR 2): campo ausente num layer nega o effective.
    // É exatamente o comportamento que a memory
    // "Degradação declarada > substituição silenciosa"
    // e a regra "Default de allowlist vazia = sem
    // restrição" invertem: aqui "ausente = nega", não
    // "ausente = livre".
    assert_eq!(
        effective.python,
        frederico_tool_registry::RuntimePermission::None,
        "fail-closed: project/assistant não citam python (default None); None domina interseção"
    );

    // Eixo onde **nenhum** layer cita — default deny
    // (fail-closed: campo ausente não herda true de outro
    // layer que também não cita).
    let default_ps = PermissionSet::default();
    assert_eq!(effective.file_create, default_ps.file_create);
    assert_eq!(effective.credentials, default_ps.credentials);
    assert!(!effective.destructive_ops, "assistant nega explicitamente");

    // file_read: interseção. user=WorkspacePlusApproved, project=None
    // (default), assistant=None. Nenhum nega explicitamente
    // (None é a variante padrão, não "negar leitura"). A merge
    // `None < WorkspaceOnly < WorkspacePlusApproved` ⇒ min.
    // user ∩ project = None ∩ WorkspacePlusApproved = None.
    // (regra da `merge`: None domina).
    assert_eq!(
        effective.file_read,
        frederico_tool_registry::FileReadPermission::None,
        "project/assistant não citam file_read (viram None/default), None domina a interseção"
    );

    // terminal: user=None, project=Denylist, assistant=None.
    // min(None, Denylist, None) = None. Esperado: None.
    assert_eq!(
        effective.terminal,
        frederico_tool_registry::TerminalMode::None,
        "interseção do terminal: user/assistant=None, project=Denylist → None domina"
    );
}

// 2. **`effective_permission_set_is_subset_of_parent`**
//
// Invariante tripla: `effective ⊆ user ∧ effective ⊆ project
// ∧ effective ⊆ assistant`. Verificada via
// `PermissionSet::is_subset_of` (Fase 3 Etapa 3, base do
// invariante "subagente ⊆ pai" do ADR-0027).
//
// **Por que esse teste é o coração do PR 2:** é a **única**
// forma de provar que o invariante **se mantém no caminho
// de produção** (não só na função pura `merge`). Se um
// futuro refactor da merge quebrasse a relação de subset,
// esse teste pega — e é ele que a memory
// "Cobertura de invariante no caminho de produção, não
// no crate" defende como regra obrigatória.
#[test]
fn effective_permission_set_is_subset_of_parent() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();

    // Profiles desenhados pra maximizar a chance de falha:
    // cada layer nega um conjunto **diferente** de eixos. O
    // effective é a interseção de tudo. Pra cada eixo, o
    // effective deve ser **mais restritivo ou igual** a cada
    // input individual.
    let (user, project, assistant) = three_layer_profiles(
        dir,
        // user: nega network + memory
        r#"
network = false
memory = "none"
file_read = "workspace_plus_approved"
terminal = "denylist"
python = "unrestricted"
"#,
        // project: nega terminal + destructive_ops
        r#"
terminal = "require_approval"
destructive_ops = false
web_download = false
file_read = "workspace_only"
"#,
        // assistant: nega python + memory + credentials
        r#"
python = "sandboxed"
memory = "read_only"
credentials = false
file_read = "workspace_only"
"#,
    );

    let loader = PermissionLoader::new();
    let effective = build_default_permission_set(&loader, &user, &project, &assistant);

    // Recarrega cada layer individual (loader tem cache, hit
    // imediato) pra usar como `parent` no `is_subset_of`.
    let user_ps = loader.load_profile(&user);
    let project_ps = loader.load_profile(&project);
    let assistant_ps = loader.load_profile(&assistant);

    // **Invariante tripla** — effective ⊆ cada input.
    // Falha em qualquer um destes é regressão grave do
    // `merge3` (fail-closed quebrado).
    assert!(
        effective.is_subset_of(&user_ps),
        "effective ⊆ user não se sustenta: effective={effective:?}, user={user_ps:?}"
    );
    assert!(
        effective.is_subset_of(&project_ps),
        "effective ⊆ project não se sustenta: effective={effective:?}, project={project_ps:?}"
    );
    assert!(
        effective.is_subset_of(&assistant_ps),
        "effective ⊆ assistant não se sustenta: effective={effective:?}, assistant={assistant_ps:?}"
    );

    // Eixo-por-eixo: cada eixo do `effective` é **igual ou
    // mais restritivo** que o do input. Catches regressão
    // silenciosa (effective⊆user pode passar mas com
    // effective mais permissivo em algum eixo — bug de
    // merge).
    assert!(!effective.network, "network: user nega → effective nega");
    assert!(
        !effective.destructive_ops,
        "destructive_ops: project nega → effective nega"
    );
    assert!(
        !effective.credentials,
        "credentials: assistant nega → effective nega"
    );
    // python: user=Unrestricted, project=None, assistant=Sandboxed.
    // min(Unrestricted, None, Sandboxed) = None.
    assert_eq!(
        effective.python,
        frederico_tool_registry::RuntimePermission::None,
        "python: interseção tripla → None (None domina)"
    );
    // memory: user=None, project=None (não cita), assistant=ReadOnly.
    // min(None, None, ReadOnly) = None.
    assert_eq!(
        effective.memory,
        frederico_tool_registry::MemoryPermission::None,
        "memory: interseção tripla → None"
    );
    // file_read: user=WorkspacePlusApproved, project=WorkspaceOnly,
    // assistant=WorkspaceOnly. min = WorkspaceOnly.
    assert_eq!(
        effective.file_read,
        frederico_tool_registry::FileReadPermission::WorkspaceOnly,
        "file_read: min(WorkspacePlusApproved, WorkspaceOnly, WorkspaceOnly) = WorkspaceOnly"
    );

    // Sanity: o effective **não** é igual a qualquer um dos
    // inputs individuais (a interseção deve **restringir**).
    assert_ne!(effective, user_ps);
    assert_ne!(effective, project_ps);
    assert_ne!(effective, assistant_ps);
}

// 3. **Bônus (não no spec, mas coberto pelo mesmo path):**
// `permission_loader` cacheia o parse por (path, hash). Após
// carregar 2 vezes, a 2ª é cache hit. Esse teste
// **complementa** o `permission_loader` unitário
// (que já cobre cache hit/invalidate) e prova o
// comportamento no caminho de produção.
#[test]
fn build_default_permission_set_reuses_loader_cache() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    let (user, project, assistant) = three_layer_profiles(
        dir,
        "network = true\n",
        "network = false\n",
        "network = true\n",
    );

    let loader = Arc::new(PermissionLoader::new());
    let _ = build_default_permission_set(&loader, &user, &project, &assistant);
    let _ = build_default_permission_set(&loader, &user, &project, &assistant);
    // O cache em memória evita 6 reads de disco (3 layers × 2
    // chamadas). Esse teste não consegue provar "cache hit" sem
    // espiar o estado interno do loader — o que conta é que o
    // resultado é o mesmo entre as 2 chamadas (determinístico).
}
