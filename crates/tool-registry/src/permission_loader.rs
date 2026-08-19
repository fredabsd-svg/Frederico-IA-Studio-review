//! `permission_loader` — carrega o `PermissionSet` real do
//! `assistant` / `project` / `user` no caminho de produção
//! (Etapa 3 da Fase 6, PR 2, ADR-0030 §D3).
//!
//! Fecha a pendência deixada pela Etapa 3 da Fase 3 Etapa 4:
//! "a Etapa 4 carrega o `PermissionSet` real do `assistant` /
//! `project` / `user` antes de validar — pendente"
//! (`docs/modules/tool-registry.md §3`).
//!
//! ## Hierarquia
//!
//! 1. **Profile do usuário** (`~/.config/frederico/profiles/default.toml`
//!    em Linux/macOS ou `%APPDATA%\Frederico\profiles\default.toml`
//!    em Windows). Default é deny-all (`PermissionSet::default()`).
//! 2. **Profile do projeto** (`./.frederico/project.toml`,
//!    relativo ao CWD). Default é "herdar do usuário" (não cita
//!    campo = `PermissionSet::default()` aplicado a esse layer).
//! 3. **Profile do assistant** (`./.frederico/assistants/<id>.toml`,
//!    relativo ao CWD). Default idem.
//!
//! `effective = user.merge3(&project, &assistant)` — `merge`
//! fail-closed, "mais restritivo vence" em cada eixo. **Default
//! deny**: campo ausente num layer não herda `true` de outro
//! layer (regra do `tool-permission-model.md §"Invariantes"` +
//! decisão de 2026-08-06 no PR 2).
//!
//! ## Caching — **parse, não decisão**
//!
//! Princípio do PR 2 (decisão de 2026-08-06): cache de
//! permissão é fonte clássica de **privilégio obsoleto**. Se
//! o cache guardasse a decisão final (o `PermissionSet`
//! merged), revogar uma permissão num layer não propagaria
//! até o cache expirar.
//!
//! Solução: o cache guarda **o parse** (a `PermissionSet`
//! derivada de **um** TOML, por layer), chaveado por
//! `(path, content_hash)`. Conteúdo do TOML muda → hash
//! muda → cache invalida. A **decisão** é computada a cada
//! chamada via `merge3` — sem janela de privilégio obsoleto.
//!
//! Persistência cross-restart: tabela `permission_profiles` no
//! SQLite (migração `0028_profiles.sql`) guarda o TOML bruto
//! + `content_hash` + `schema_version`. O cache em memória
//!
//! é best-effort (evita re-parse em chamadas sucessivas no
//! mesmo run). Se o TOML do disco mudar mid-run, o hash
//! recauculado diverge do cache, o que invalida e força
//! re-parse do disco.
//!
//! ## Etapa 4 (subagente)
//!
//! O `SubagentRunner` da Etapa 4 consome o `effective` desta
//! função e ainda intersecta com `parent_permissions` (pai do
//! subagente) e com `SpecialistDefinition.allowed_tools −
//! denied_tools`. **Quando a Etapa 4 entrar, o `permission_loader`
//! já está em produção** (decisão de 2026-08-06 na conversa
//! do PR 2 — nenhuma janela de privilégio entre os 2 PRs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use directories::ProjectDirs;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, warn};

use crate::permission::PermissionSet;

/// Versão do schema do TOML de `permission_profiles`. Bump
/// quando o `PermissionProfileToml` ganha campo novo
/// incompatível (renomear, mudar tipo, mudar semântica de
/// merge). Cache de perfis cached usa `schema_version` pra
/// invalidar todos os profiles quando o schema muda (mesma
/// estratégia do `model-catalog` que bumpou a versão do
/// `catalog.json` na Etapa 2 da Fase 0).
///
/// **Por que versionar, não só confiar no `content_hash`:** o
/// `content_hash` detecta mudança no TOML, mas não detecta
/// mudança na **interpretação** do TOML (renomeei um campo
/// mantendo o mesmo conteúdo). Bump de schema → invalida
/// todos os profiles cached. Migração `0028_profiles.sql`
/// documenta isso em SQL.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum PermissionLoadError {
    /// I/O falhou ao ler o arquivo (permissão, path inválido,
    /// etc). **Não é erro fatal** — o loader cai pro default
    /// deny (mesma família do `OpenRouter API key ausente` da
    /// Fase de Ligação Etapa 3). O usuário fica com
    /// `PermissionSet::default()` (tudo deny), o que é o
    /// mais seguro.
    #[error("falha ao ler perfil de permissão em {path}: {cause}")]
    Io { path: PathBuf, cause: String },

    /// Parse do TOML falhou. **Também não é fatal** — cai
    /// pro default deny. Parse ruim é "permissão negada até
    /// o usuário corrigir o arquivo".
    #[error("TOML inválido em {path}: {cause}")]
    Parse { path: PathBuf, cause: String },

    /// Schema mudou (campo renomeado, novo enum). Loga
    /// warning e re-parse com o novo schema (o cache é
    /// invalidado automaticamente pelo `schema_version`).
    #[error("schema_version do cache ({cached}) desatualizado (atual: {current}) — perfil {path} precisa re-parse")]
    SchemaMismatch {
        path: PathBuf,
        cached: u32,
        current: u32,
    },
}

/// Forma crua do TOML de profile. Por design, **todos os campos
/// são `Option<...>`** (exceto os sub-enums que já são
/// `Option<enum>` por construção). Motivo: a merge
/// `PermissionSet::merge` é fail-closed — campo ausente é
/// tratado como "default deny" no layer, **nunca** como
/// "herda o valor permissivo de outro layer". Se o campo
/// fosse `T` direto (sem `Option`), o `Deserialize` falharia
/// quando o arquivo não cita o campo — `Option` permite
/// ausência sem erro de parse.
///
/// **Cuidado (decisão de 2026-08-06):** `None` num campo
/// **não significa "ausente"** na semântica de merge — significa
/// "esse layer explicitamente nega esse eixo". A
/// `PermissionSet::default()` aplicada ao layer ausente
/// é a interpretação: `default()` tem `network: false`, que
/// na `merge` **nega** o `network` mesmo se outro layer tem
/// `true`. **Default deny é a invariante**.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct PermissionProfileToml {
    #[serde(default)]
    file_read: Option<crate::permission::FileReadPermission>,
    #[serde(default)]
    file_create: Option<bool>,
    #[serde(default)]
    file_modify: Option<bool>,
    #[serde(default)]
    file_delete: Option<bool>,
    #[serde(default)]
    terminal: Option<crate::permission::TerminalMode>,
    #[serde(default)]
    python: Option<crate::permission::RuntimePermission>,
    #[serde(default)]
    node: Option<crate::permission::RuntimePermission>,
    #[serde(default)]
    git: Option<crate::permission::GitPermission>,
    /// Matriz de GitHub ([ADR-0049] §D1). Campo aditivo, como o
    /// `network_allowlist` foi: perfil antigo continua parseando e
    /// cai em vazio — que nega tudo, exatamente o que já acontecia
    /// implicitamente antes do campo existir.
    ///
    /// O `github` escalar de antes é ignorado se ainda estiver no
    /// arquivo. `deny_unknown_fields` **não** está ligado nesta
    /// struct e não passa a estar: recusar o perfil inteiro por uma
    /// chave obsoleta trocaria uma permissão a menos por um app que
    /// não abre.
    ///
    /// [ADR-0049]: ../../docs/decisions/0049-matriz-de-github-no-permission-set.md
    #[serde(default)]
    github_repos: Option<Vec<crate::permission::RegraGithubPerfil>>,
    #[serde(default)]
    web_browse: Option<bool>,
    #[serde(default)]
    web_download: Option<bool>,
    #[serde(default)]
    network: Option<bool>,
    /// Hosts liberados no proxy de rede do sandbox (Etapa 7 da
    /// Fase 7). Campo aditivo — TOMLs antigos (sem esta chave)
    /// continuam parseando normalmente e caem no
    /// `unwrap_or_default()` (`Vec::new()`, deny-by-default),
    /// mesmo comportamento que tinham implicitamente antes deste
    /// campo existir. Não exige bump de `SCHEMA_VERSION`: o cache
    /// persistido (`permission_profiles`) guarda o TOML bruto, não
    /// o `PermissionSet` parseado — re-parse com o schema novo é
    /// automático.
    #[serde(default)]
    network_allowlist: Option<Vec<String>>,
    #[serde(default)]
    screen_capture: Option<bool>,
    #[serde(default)]
    input_control: Option<bool>,
    #[serde(default)]
    memory: Option<crate::permission::MemoryPermission>,
    #[serde(default)]
    credentials: Option<bool>,
    #[serde(default)]
    documents: Option<crate::permission::DocumentPermission>,
    #[serde(default)]
    destructive_ops: Option<bool>,
}

impl PermissionProfileToml {
    /// Converte pro `PermissionSet` de domínio. Campos
    /// `None` viram o default do `PermissionSet` (que é
    /// deny). Isso é o que faz o fail-closed funcionar:
    /// mesmo se o usuário não cita `network`, o `network:
    /// false` no `PermissionSet::default()` **nega** o
    /// `network` na merge.
    fn into_permission_set(self) -> PermissionSet {
        PermissionSet {
            file_read: self.file_read.unwrap_or_default(),
            file_create: self.file_create.unwrap_or(false),
            file_modify: self.file_modify.unwrap_or(false),
            file_delete: self.file_delete.unwrap_or(false),
            terminal: self.terminal.unwrap_or_default(),
            python: self.python.unwrap_or_default(),
            node: self.node.unwrap_or_default(),
            git: self.git.unwrap_or_default(),
            github_repos: self.github_repos.unwrap_or_default(),
            web_browse: self.web_browse.unwrap_or(false),
            web_download: self.web_download.unwrap_or(false),
            network: self.network.unwrap_or(false),
            network_allowlist: self.network_allowlist.unwrap_or_default(),
            screen_capture: self.screen_capture.unwrap_or(false),
            input_control: self.input_control.unwrap_or(false),
            memory: self.memory.unwrap_or_default(),
            credentials: self.credentials.unwrap_or(false),
            documents: self.documents.unwrap_or_default(),
            destructive_ops: self.destructive_ops.unwrap_or(false),
        }
    }
}

/// Cache em memória de `PermissionSet` parseada por
/// `(path, content_hash)`. Single-thread (`Mutex`) — o loader
/// não é hot path, e o contention é desprezível (cada load =
/// uma chamada `read_to_string` do TOML, ordem de microssegundos).
type CacheKey = (PathBuf, String);
type CacheValue = PermissionSet;

/// Loader de `PermissionSet` com cache. O `Arc<Mutex<...>>`
/// permite compartilhar entre threads (o `ChatOrchestrator`
/// e a UI podem chamar concorrentemente).
#[derive(Debug, Clone)]
pub struct PermissionLoader {
    cache: Arc<Mutex<HashMap<CacheKey, CacheValue>>>,
}

impl Default for PermissionLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionLoader {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Carrega o `PermissionSet` de **um** profile (path
    /// arbitrário). Cache hit → imediato. Cache miss → read
    /// do disco + parse + cache. Falha de I/O ou parse →
    /// `PermissionSet::default()` (deny all) com warning
    /// logado (degradação declarada).
    pub fn load_profile(&self, path: &Path) -> PermissionSet {
        // Lê o disco (fonte primária — pode ter mudado
        // desde o último load).
        let (content_hash, content) = match read_and_hash(path) {
            Ok(pair) => pair,
            Err(e) => {
                warn!(
                    profile.path = %path.display(),
                    "perfil de permissão indisponível; caindo pro default deny. Causa: {e}"
                );
                return PermissionSet::default();
            }
        };

        // Cache hit (mesmo path, mesmo hash) → retorno
        // imediato.
        let key = (path.to_path_buf(), content_hash.clone());
        {
            let cache = self.cache.lock().expect("cache poisoned");
            if let Some(ps) = cache.get(&key) {
                debug!(
                    profile.path = %path.display(),
                    profile.hash = %content_hash,
                    "permission_profile cache hit"
                );
                return ps.clone();
            }
        }

        // Cache miss → parse + cache.
        let parsed: PermissionSet = match toml::from_str::<PermissionProfileToml>(&content) {
            Ok(toml_profile) => toml_profile.into_permission_set(),
            Err(e) => {
                warn!(
                    profile.path = %path.display(),
                    "TOML de perfil inválido; caindo pro default deny. Causa: {e}"
                );
                return PermissionSet::default();
            }
        };

        let mut cache = self.cache.lock().expect("cache poisoned");
        cache.insert(key, parsed.clone());
        debug!(
            profile.path = %path.display(),
            profile.hash = %content_hash,
            "permission_profile cached (parse + insert)"
        );
        parsed
    }

    /// Carrega o `PermissionSet` efetivo da cadeia
    /// `user ∩ project ∩ assistant` via `merge3`
    /// fail-closed.
    ///
    /// **Convenção de paths** (resolvido pelo caller — o
    /// `composition::build_default_permission_set` resolve
    /// cada path via `directories::ProjectDirs` ou CWD):
    /// - `user`: `~/.config/frederico/profiles/default.toml`
    /// - `project`: `./.frederico/project.toml` (relativo ao CWD)
    /// - `assistant`: `./.frederico/assistants/<id>.toml`
    ///
    /// **Path ausente** (arquivo não existe) é o caso
    /// comum no primeiro launch — tratado como
    /// `PermissionSet::default()` por `load_profile` (que
    /// loga warning mas não falha). O `effective` resultante
    /// ainda é **deny all**, que é o mais seguro.
    pub fn load_effective_permission_set(
        &self,
        user: &Path,
        project: &Path,
        assistant: &Path,
    ) -> PermissionSet {
        let user_ps = self.load_profile(user);
        let project_ps = self.load_profile(project);
        let assistant_ps = self.load_profile(assistant);
        // `merge3` é fail-closed: campo ausente num layer
        // (virou `default()` no `into_permission_set`) é
        // deny em `merge`, nunca herda `true` de outro
        // layer. Ver `PermissionSet::merge` + testes
        // unitários no `permission.rs`.
        user_ps.merge3(&project_ps, &assistant_ps)
    }

    /// Resolve o path default do **profile do usuário**
    /// (`~/.config/frederico/profiles/default.toml` em
    /// Linux/macOS, `%APPDATA%\Frederico\profiles\default.toml`
    /// em Windows). `None` se o OS não tem diretório
    /// de config home (raro).
    #[must_use]
    pub fn default_user_profile_path() -> Option<PathBuf> {
        let dirs = ProjectDirs::from("studio", "frederico", "ia")?;
        Some(dirs.config_dir().join("profiles").join("default.toml"))
    }

    /// Resolve o path default do **profile do projeto**
    /// (`./.frederico/project.toml` relativo ao CWD). Não
    /// tenta criar o diretório — o caller decide se quer
    /// criar o arquivo de template ou só tratar o "arquivo
    /// ausente" como `PermissionSet::default()`.
    #[must_use]
    pub fn default_project_profile_path() -> PathBuf {
        PathBuf::from(".frederico").join("project.toml")
    }

    /// Resolve o path default do **profile do assistant**
    /// (`./.frederico/assistants/<id>.toml` relativo ao CWD).
    #[must_use]
    pub fn default_assistant_profile_path(assistant_id: &str) -> PathBuf {
        PathBuf::from(".frederico")
            .join("assistants")
            .join(format!("{assistant_id}.toml"))
    }

    /// Invalida o cache em memória. Útil pra testes
    /// (criar arquivo, invalidar, recarregar) ou pra
    /// implementação futura de hot-reload via filesystem
    /// watch (Etapa 6+).
    pub fn invalidate(&self) {
        let mut cache = self.cache.lock().expect("cache poisoned");
        cache.clear();
    }
}

/// Lê o arquivo do disco e calcula o `content_hash` (BLAKE3
/// ou FNV64 se BLAKE3 não disponível — `model-catalog` já
/// faz a mesma coisa no `build.rs`, padrão do projeto).
///
/// **Por que `read_to_string` + hash manual, em vez de
/// `tokio::fs`:** o loader é chamado pelo
/// `ChatOrchestrator` no setup (síncrono, antes do
/// `tokio::spawn`). Async aqui só complicaria sem
/// benefício — o TOML é pequeno (< 1 KB típico).
fn read_and_hash(path: &Path) -> Result<(String, String), PermissionLoadError> {
    let content = std::fs::read_to_string(path).map_err(|e| PermissionLoadError::Io {
        path: path.to_path_buf(),
        cause: e.to_string(),
    })?;
    let hash = blake3_or_fnv64(&content);
    Ok((hash, content))
}

/// Hash do conteúdo. `blake3` crate não está como dep
/// direta do `tool-registry` (foi do `model-catalog` no
/// `build.rs`), então uso o mesmo FNV-64 do
/// `model-catalog/build.rs` (padrão do projeto, mesma
/// estratégia do `CATALOG_HASH`). Quando `blake3` virar
/// dep direta, bump daqui.
fn blake3_or_fnv64(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    format!("fnv64:{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_profile(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("create temp");
        f.write_all(content.as_bytes()).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn load_profile_missing_file_returns_default() {
        let loader = PermissionLoader::new();
        let p = Path::new("/nonexistent/that/does/not/exist.toml");
        // Degradação declarada: arquivo ausente → default
        // deny (não falha). O `effective` ainda é deny all,
        // que é o mais seguro.
        let ps = loader.load_profile(p);
        assert_eq!(ps, PermissionSet::default());
    }

    #[test]
    fn load_profile_parses_network_allowlist() {
        let f = write_profile(
            r#"
network = true
network_allowlist = ["pypi.org", "registry.npmjs.org"]
"#,
        );
        let loader = PermissionLoader::new();
        let ps = loader.load_profile(f.path());
        assert_eq!(
            ps.network_allowlist,
            vec!["pypi.org".to_string(), "registry.npmjs.org".to_string()]
        );
    }

    #[test]
    fn load_profile_missing_network_allowlist_defaults_empty() {
        // TOML antigo, sem a chave nova — campo aditivo cai no
        // default (deny-by-default), sem erro de parse.
        let f = write_profile(
            r#"
network = true
"#,
        );
        let loader = PermissionLoader::new();
        let ps = loader.load_profile(f.path());
        assert!(ps.network_allowlist.is_empty());
    }

    #[test]
    fn load_profile_parses_valid_toml() {
        let f = write_profile(
            r#"
file_read = "workspace_only"
network = true
python = "sandboxed"
"#,
        );
        let loader = PermissionLoader::new();
        let ps = loader.load_profile(f.path());
        assert_eq!(
            ps.file_read,
            crate::permission::FileReadPermission::WorkspaceOnly
        );
        assert!(ps.network);
        assert_eq!(ps.python, crate::permission::RuntimePermission::Sandboxed);
        // Campos não citados → default deny.
        assert!(!ps.credentials);
        assert!(!ps.destructive_ops);
    }

    #[test]
    fn load_profile_invalid_toml_returns_default_with_warning() {
        let f = write_profile(
            r#"
file_read = "valor_invalido"
"#,
        );
        let loader = PermissionLoader::new();
        let ps = loader.load_profile(f.path());
        // Fail-closed: parse ruim → default deny.
        assert_eq!(ps, PermissionSet::default());
    }

    #[test]
    fn load_profile_cache_hit_on_second_call() {
        let f = write_profile(
            r#"
file_read = "workspace_plus_approved"
"#,
        );
        let loader = PermissionLoader::new();
        let ps1 = loader.load_profile(f.path());
        let ps2 = loader.load_profile(f.path());
        assert_eq!(ps1, ps2);
        // Mesma instância lógica (mesmo parse, mesma cache).
        // O cache hit não muda o valor; só evita re-parse.
    }

    #[test]
    fn load_profile_cache_invalidates_on_content_change() {
        let f = write_profile(
            r#"
network = true
"#,
        );
        let loader = PermissionLoader::new();
        let ps1 = loader.load_profile(f.path());
        assert!(ps1.network);

        // Re-escreve o arquivo com conteúdo diferente (novo hash).
        std::fs::write(f.path(), "network = false\n").expect("rewrite");
        let ps2 = loader.load_profile(f.path());
        assert!(!ps2.network, "cache deve invalidar por hash mismatch");
    }

    #[test]
    fn load_effective_permission_set_intersects_three_layers_fail_closed() {
        // user = permissivo (tudo true), project = nega network,
        // assistant = nega web_browse. Effective = network=false
        // ∧ web_browse=false (deny vence em cada eixo).
        let user = write_profile(
            r#"
network = true
web_browse = true
file_read = "workspace_plus_approved"
"#,
        );
        let project = write_profile(
            r#"
network = false
file_read = "workspace_only"
"#,
        );
        let assistant = write_profile(
            r#"
web_browse = false
file_read = "workspace_only"
"#,
        );
        let loader = PermissionLoader::new();
        let effective =
            loader.load_effective_permission_set(user.path(), project.path(), assistant.path());
        assert!(!effective.network, "project nega network → effective nega");
        assert!(
            !effective.web_browse,
            "assistant nega web_browse → effective nega"
        );
        assert_eq!(
            effective.file_read,
            crate::permission::FileReadPermission::WorkspaceOnly,
            "WorkspaceOnly ∩ WorkspaceOnly = WorkspaceOnly (mais restritivo)"
        );
    }

    #[test]
    fn load_effective_permission_set_missing_layer_falls_to_default_deny() {
        // Layer ausente (path inexistente) → default deny.
        // Merge: user=true, project=default(false), assistant=true
        // → effective nega o que project nega (tudo).
        let user = write_profile(
            r#"
network = true
"#,
        );
        let assistant = write_profile(
            r#"
network = true
"#,
        );
        let loader = PermissionLoader::new();
        let effective = loader.load_effective_permission_set(
            user.path(),
            Path::new("/nonexistent/project.toml"),
            assistant.path(),
        );
        // project não cita nada → default deny → nega tudo.
        assert!(!effective.network);
        assert!(!effective.credentials);
    }

    #[test]
    fn default_user_profile_path_uses_project_dirs() {
        // Sanity check do `ProjectDirs::from("studio",
        // "frederico", "ia")` — não bate o caminho
        // exato (depende do OS), mas verifica que
        // termina em `default.toml`.
        if let Some(p) = PermissionLoader::default_user_profile_path() {
            assert!(p.ends_with("default.toml"), "got: {p:?}");
            assert!(p.to_string_lossy().contains("profiles"), "got: {p:?}");
        }
        // Em Windows sem HOME o ProjectDirs retorna None —
        // aceitável. Não falha o teste.
    }

    #[test]
    fn default_project_profile_path_is_dot_frederico() {
        let p = PermissionLoader::default_project_profile_path();
        assert_eq!(p, PathBuf::from(".frederico").join("project.toml"));
    }

    #[test]
    fn default_assistant_profile_path_includes_id() {
        let p = PermissionLoader::default_assistant_profile_path("arquiteto");
        assert_eq!(
            p,
            PathBuf::from(".frederico")
                .join("assistants")
                .join("arquiteto.toml")
        );
    }
}
