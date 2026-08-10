//! Workspace, `NormalizedPath` e o **Jail** — o ponto único de
//! normalização de caminhos.
//!
//! O Jail é a peça de segurança central do `files.read` (e vai ser
//! de `files.write`, `files.list`, `files.edit` nas próximas etapas).
//! Tudo que entra na Etapa 2 passa por aqui: a regra
//! `software-architecture.md` §"Invariantes" diz
//! "Nenhum caminho de arquivo é construído concatenando strings em
//! mais de um lugar. Tudo passa por `AppPaths::resolve(LogicalPath)`,
//! e a normalização é responsabilidade única desse ponto". O
//! `Jail::resolve` é esse ponto.
//!
//! Ameaças cobertas (do `security-threat-model.md` §"Ameaças",
//! linha I3 — Path traversal via tool `files.read`):
//!
//! - **`..`** (path traversal textual — `../etc/passwd`).
//! - **Caminho absoluto** (`C:\...` no Windows, `/etc/...` no
//!   Unix).
//! - **UNC** (`\\server\share\file` no Windows).
//! - **Letra de unidade diferente** (`D:\...` quando o workspace é
//!   `C:\workspace`).
//! - **Symlink** (arquivo em `C:\workspace\link.txt` apontando pra
//!   `D:\external\file.txt` — o `canonicalize` resolve e o
//!   `starts_with` rejeita).

use std::path::{Component, Path, PathBuf};

use crate::error::{ToolError, ToolErrorCode};

/// Trait do workspace. A Etapa 2 modela a interface; a Etapa 7 (modo
/// desenvolvedor) define como `frederico-security` materializa isso em
/// Windows. Os testes usam `FakeWorkspace` (no escopo do crate, via
/// `tests/`).
pub trait Workspace {
    /// Raiz do workspace. Todo caminho resolvido pelo Jail é
    /// canonicalizado em relação a esse caminho.
    fn root(&self) -> &Path;
}

/// Jail: encapsula a raiz do workspace e a regra
/// "caminho resolvido tem que estar dentro da raiz canonicalizada".
///
/// Construir um `Jail` falha se a raiz não puder ser canonicalizada
/// (ex.: pasta não existe). Depois de construído, `resolve` é a
/// única forma de obter um caminho "dentro do jail".
#[derive(Debug, Clone)]
pub struct Jail {
    root: PathBuf,
    root_canonical: PathBuf,
}

impl Jail {
    /// Cria um jail em torno de `root`. Canonicaliza a raiz —
    /// necessário pra comparar caminhos depois (que também são
    /// canonicalizados).
    pub fn new(root: &Path) -> Result<Self, ToolError> {
        let root_canonical = root.canonicalize().map_err(|e| {
            ToolError::new(
                ToolErrorCode::JailViolation,
                format!("raiz do workspace inválida (não existe?): {e}"),
            )
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            root_canonical,
        })
    }

    /// Raiz original (não canonicalizada). Útil para mensagens de
    /// erro amigáveis.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Raiz **canonicalizada** (resolvida via `Path::canonicalize`
    /// no [`Jail::new`]). Diferente de [`Jail::root`], que devolve
    /// a raiz como foi passada (case original do caller). Use este
    /// getter pra **comparações** com outros paths canonicalizados
    /// (ex.: `validate_against_allowlist` recebe o canônico do
    /// output e a allowlist `&[root_canonical()]` — assim os dois
    /// lados da comparação vêm da **mesma** canonicalização e o
    /// `Path::starts_with` component-wise é confiável mesmo com
    /// case misto do Windows).
    ///
    /// **Não usar** este path como argumento pra criar arquivos
    /// — o `root` (não canônico) é o que o caller passou, e é
    /// semanticamente mais limpo pra logs e mensagens de erro.
    /// Pra I/O, use o resultado de [`Jail::resolve`] ou
    /// [`Jail::resolve_allowing_nonexistent`].
    #[must_use]
    pub fn root_canonical(&self) -> &Path {
        &self.root_canonical
    }

    /// Resolve um caminho pedido, devolvendo o caminho **canonicalizado
    /// e dentro do jail**. Falha com `ToolError::JailViolation`
    /// (código `TOOL_JAIL_VIOLATION`) se:
    ///
    /// - o caminho contém `..`;
    /// - o caminho é absoluto;
    /// - o caminho é UNC (`\\server\share`);
    /// - a canonicalização (que resolve symlinks) cai fora da raiz
    ///   canonicalizada.
    pub fn resolve(&self, requested: &Path) -> Result<PathBuf, ToolError> {
        // 1. Componentes perigosos no caminho textual (antes de
        // tocar o disco — economiza syscall em tentativa óbvia de
        // traversal).
        for comp in requested.components() {
            match comp {
                Component::ParentDir => {
                    return Err(self.violation(
                        "path traversal não permitido: '..' no caminho",
                        Some(serde_json::json!({"componente": ".."})),
                    ));
                }
                Component::Prefix(_) => {
                    return Err(self.violation(
                        "caminho absoluto ou UNC não permitido",
                        Some(serde_json::json!({"componente": "prefix"})),
                    ));
                }
                Component::RootDir => {
                    return Err(self.violation(
                        "caminho absoluto não permitido (raiz do filesystem)",
                        Some(serde_json::json!({"componente": "/"})),
                    ));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }

        // 2. UNC explícito (`\\server\share`) — `Component::Prefix` já
        // pega isso, mas afinamos a mensagem para UNC e variantes
        // verbatim.
        if let Some(Component::Prefix(p)) = requested.components().next() {
            let kind = p.kind();
            // O `Prefix` (sem o "Component") tem os variants. UNC,
            // VerbatimDisk, DeviceNS e VerbatimUNC todos apontam pra
            // recursos fora do workspace.
            let is_unc_like = matches!(
                kind,
                std::path::Prefix::UNC(_, _)
                    | std::path::Prefix::VerbatimUNC(_, _)
                    | std::path::Prefix::VerbatimDisk(_)
                    | std::path::Prefix::DeviceNS(_)
                    | std::path::Prefix::Verbatim(_)
            );
            if is_unc_like {
                return Err(self.violation(
                    "UNC path não permitido",
                    Some(serde_json::json!({"componente": "\\\\server\\share ou \\\\?\\"})),
                ));
            }
        }

        // 3. Junta com a raiz e canonicaliza. Como `requested` não é
        // absoluto (caso 1+2), `root.join(requested)` é relativo à
        // raiz.
        let joined = self.root.join(requested);
        let canonical = joined.canonicalize().map_err(|e| {
            self.violation(
                &format!("caminho não pôde ser resolvido (arquivo não existe?): {e}"),
                Some(serde_json::json!({"joined": joined.display().to_string()})),
            )
        })?;

        // 4. Jail final: o canonicalizado tem que estar dentro da
        // raiz canonicalizada. Isso é o que pega symlinks (que
        // `canonicalize` resolve).
        if !canonical.starts_with(&self.root_canonical) {
            return Err(self.violation(
                "caminho fora do jail (symlink aponta pra fora? UNC via junction?)",
                Some(serde_json::json!({
                    "canonical": canonical.display().to_string(),
                    "root_canonical": self.root_canonical.display().to_string(),
                })),
            ));
        }

        Ok(canonical)
    }

    /// Versão mais permissiva: não exige que o arquivo exista, mas
    /// ainda valida que **se** existisse, estaria dentro do jail.
    /// Útil para a Etapa 4 quando quiser validar antes de criar
    /// (`files.write`). A Etapa 2 não usa — `files.read` precisa do
    /// arquivo existir.
    ///
    /// Implementação: canonicaliza o **pai** (que deve existir) e
    /// checa jail. Se o pai não existe, rejeita.
    pub fn resolve_allowing_nonexistent(&self, requested: &Path) -> Result<PathBuf, ToolError> {
        for comp in requested.components() {
            match comp {
                Component::ParentDir => {
                    return Err(self.violation("'..' não permitido", None));
                }
                Component::Prefix(_) => {
                    return Err(self.violation("caminho absoluto ou UNC não permitido", None));
                }
                Component::RootDir => {
                    return Err(self.violation("caminho absoluto não permitido", None));
                }
                _ => {}
            }
        }
        let parent = requested.parent().unwrap_or(Path::new("."));
        let parent_abs = self.root.join(parent);
        let parent_canonical = parent_abs.canonicalize().map_err(|e| {
            self.violation(
                &format!("diretório pai não existe: {e}"),
                Some(serde_json::json!({"parent": parent.display().to_string()})),
            )
        })?;
        if !parent_canonical.starts_with(&self.root_canonical) {
            return Err(self.violation(
                "diretório pai fora do jail",
                Some(serde_json::json!({
                    "parent_canonical": parent_canonical.display().to_string(),
                })),
            ));
        }
        let final_path = parent_canonical.join(requested.file_name().unwrap_or_default());
        Ok(final_path)
    }

    /// **`files.write` com `create_parents: true`** (Etapa 5 do
    /// Phase 7, ADR-0035 D5). Valida que o caminho está dentro do
    /// jail **e** cria os diretórios intermediários que não existem.
    ///
    /// **Algoritmo:**
    ///
    /// 1. Mesma validação textual de [`Jail::resolve_allowing_nonexistent`]
    ///    (rejeita `..`, absoluto, UNC) — barreira de jail não muda.
    /// 2. Acha o ancestral mais próximo que **existe** (sobe na
    ///    hierarquia até `parent.exists()`).
    ///    - Se nenhum ancestral existe (até a raiz do jail), o
    ///      `create_parents` não tem pra onde criar — erro.
    /// 3. Canonicaliza esse ancestral, valida que está dentro do
    ///    jail (`starts_with(root_canonical)`).
    /// 4. Cria os diretórios intermediários do ancestral até o
    ///    **pai** do `path` (não cria o `path` em si — isso é
    ///    trabalho do `files.write`).
    ///    - `std::fs::create_dir_all` é idempotente (não falha
    ///      se já existe).
    /// 5. Retorna o `path` final = `ancestral + componentes_faltantes`.
    ///
    /// **Por que subir a hierarquia:** o `Jail` só conhece a sua
    /// `root_canonical`. Se o `path` é `src/utils/helper.py` e
    /// `src/utils/` não existe, precisamos achar `src/` (que
    /// existe) e validar ele como ancestral; depois criar `utils/`
    /// abaixo dele. Sem essa subida, o método seria
    /// "canonicalize o pai" e falharia quando o pai não existe.
    ///
    /// **Teste de regressão** (coberto em
    /// `e2e_files_write_under_jail::create_parents_makes_intermediate_dirs`):
    /// `path = "src/utils/helper.py"` em workspace onde só `src/`
    /// existe — o método cria `src/utils/` e devolve o path
    /// final; o `files.write` então cria `helper.py` lá.
    pub fn resolve_or_create_parents(&self, requested: &Path) -> Result<PathBuf, ToolError> {
        // 1. Validação textual (mesma do `resolve_allowing_nonexistent`).
        for comp in requested.components() {
            match comp {
                Component::ParentDir => {
                    return Err(self.violation("'..' não permitido", None));
                }
                Component::Prefix(_) => {
                    return Err(self.violation("caminho absoluto ou UNC não permitido", None));
                }
                Component::RootDir => {
                    return Err(self.violation("caminho absoluto não permitido", None));
                }
                _ => {}
            }
        }
        // UNC verbatim — mesma proteção do `resolve`.
        if let Some(Component::Prefix(p)) = requested.components().next() {
            if matches!(
                p.kind(),
                std::path::Prefix::UNC(_, _)
                    | std::path::Prefix::VerbatimUNC(_, _)
                    | std::path::Prefix::VerbatimDisk(_)
                    | std::path::Prefix::DeviceNS(_)
                    | std::path::Prefix::Verbatim(_)
            ) {
                return Err(self.violation("UNC path não permitido", None));
            }
        }

        // 2. Se o `path` completo (caminho absoluto via
        //    `self.root.join(requested)`) **já existe**, delega
        //    pro `resolve_allowing_nonexistent` (que valida jail).
        //    Caso comum de "create_parents: true" em path que
        //    existe (substituição de arquivo).
        let full_path = self.root.join(requested);
        if full_path.exists() {
            return self.resolve_allowing_nonexistent(requested);
        }

        // 3. `path` não existe. Acha o ancestral mais próximo
        //    existente subindo na hierarquia. Para `src/utils/
        //    helper.py` em workspace com só `src/`, itera:
        //    `src/utils/helper.py` (não) → `src/utils` (não) →
        //    `src` (sim!). O root em si sempre existe.
        //
        //    Para `current`, começamos com o `full_path` e
        //    subimos `parent()` até achar um que existe ou
        //    chegar no root_canonical (que sempre existe).
        let mut current = full_path.clone();
        let mut found_existing = false;
        loop {
            if current.exists() {
                found_existing = true;
                break;
            }
            // Sobe um nível.
            let parent = match current.parent() {
                Some(p) => p.to_path_buf(),
                None => break, // sem pai (raiz do filesystem)
            };
            // Se chegamos no root canonical, paramos (root sempre
            // existe por construção no `Jail::new`).
            if parent == self.root_canonical {
                current = parent;
                found_existing = true;
                break;
            }
            current = parent;
        }
        if !found_existing {
            return Err(self.violation(
                "nenhum ancestral existente dentro do jail; \
                 `create_parents` não tem pra onde criar",
                Some(serde_json::json!({
                    "requested": requested.display().to_string(),
                })),
            ));
        }

        // 3. Canonicaliza o ancestral encontrado. Esse é o ponto
        //    onde a barreira de jail é **real** — symlinks que
        //    apontam pra fora são detectados aqui.
        let ancestor_canonical = current.canonicalize().map_err(|e| {
            self.violation(
                &format!("ancestral não pôde ser canonicalizado: {e}"),
                Some(serde_json::json!({"ancestor": current.display().to_string()})),
            )
        })?;
        if !ancestor_canonical.starts_with(&self.root_canonical) {
            return Err(self.violation(
                "ancestral fora do jail (symlink aponta pra fora?)",
                Some(serde_json::json!({
                    "ancestor_canonical": ancestor_canonical.display().to_string(),
                    "root_canonical": self.root_canonical.display().to_string(),
                })),
            ));
        }

        // 4. Constrói o `path` final a partir do ancestral + os
        //    componentes do `requested` que ainda não foram
        //    cobertos. Na prática, é `self.root.join(requested)`
        //    — porque `requested` é relativo e a junção é
        //    determinística. O caminho final **ainda não existe**
        //    (é o que o `files.write` vai criar), mas o pai
        //    imediato existe ou foi criado abaixo.
        let final_path = self.root.join(requested);

        // 5. Cria os diretórios intermediários. Começa do
        //    ancestral (que existe) e vai descendo até o **pai**
        //    do `path` (não cria o `path` em si).
        //    `create_dir_all` é idempotente: se um dir já existe,
        //    não falha.
        let parent_of_final = final_path
            .parent()
            .ok_or_else(|| self.violation("path sem pai (caminho raiz?)", None))?;
        std::fs::create_dir_all(parent_of_final).map_err(|e| {
            self.violation(
                &format!("`create_dir_all` falhou: {e}"),
                Some(serde_json::json!({
                    "parent": parent_of_final.display().to_string(),
                })),
            )
        })?;

        Ok(final_path)
    }

    fn violation(&self, msg: &str, details: Option<serde_json::Value>) -> ToolError {
        let mut err = ToolError::new(ToolErrorCode::JailViolation, msg);
        if let Some(d) = details {
            err = err.with_details(d);
        }
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_workspace() -> (Tempdir, Jail) {
        let dir = Tempdir::new();
        // Cria alguns arquivos e subdiretórios
        fs::write(dir.join("hello.txt"), "Hello, world!").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/inner.txt"), "Inner").unwrap();
        let jail = Jail::new(&dir).unwrap();
        (dir, jail)
    }

    struct Tempdir(PathBuf);

    /// Contador atômico: o relógio sozinho não garante unicidade (no Windows
    /// a granularidade de `timestamp_nanos` é grosseira e testes paralelos
    /// podem colidir no mesmo valor, compartilhando o mesmo diretório).
    static TEMPDIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let unique = format!(
                "frederico-tool-registry-jail-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                n,
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

    #[test]
    fn resolve_relative_path_inside_workspace() {
        let (dir, jail) = setup_workspace();
        let resolved = jail.resolve(Path::new("hello.txt")).unwrap();
        // resolved é canônico; tem que apontar pro arquivo de verdade
        let content = fs::read_to_string(&resolved).unwrap();
        assert_eq!(content, "Hello, world!");
        // E estar dentro da raiz
        assert!(resolved.starts_with(jail.root_canonical_or_self()));
        // sanity: tempdir foi usado
        let _ = dir.join("hello.txt");
    }

    // Helper para acessar o root_canonical no teste (Jail não o
    // expõe na API pública). Usado só em testes internos.
    impl Jail {
        #[cfg(test)]
        fn root_canonical_or_self(&self) -> &Path {
            &self.root_canonical
        }
    }

    #[test]
    fn resolve_subdirectory_file() {
        let (_dir, jail) = setup_workspace();
        let resolved = jail.resolve(Path::new("sub/inner.txt")).unwrap();
        let content = fs::read_to_string(&resolved).unwrap();
        assert_eq!(content, "Inner");
    }

    #[test]
    fn reject_parent_dir_component() {
        let (_dir, jail) = setup_workspace();
        let err = jail.resolve(Path::new("../etc/passwd")).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
        assert!(err.message.contains(".."));
    }

    #[test]
    fn reject_nested_parent_dir() {
        let (_dir, jail) = setup_workspace();
        // "sub/../escape.txt" tem `..` no meio
        let err = jail.resolve(Path::new("sub/../escape.txt")).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
    }

    #[test]
    fn reject_absolute_windows_path() {
        let (_dir, jail) = setup_workspace();
        // C:\Windows\System32\drivers\etc\hosts
        let abs = Path::new(r"C:\Windows\System32\drivers\etc\hosts");
        let err = jail.resolve(abs).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
    }

    #[test]
    fn reject_unc_path() {
        let (_dir, jail) = setup_workspace();
        // \\server\share\file
        let unc = Path::new(r"\\server\share\file.txt");
        let err = jail.resolve(unc).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
    }

    #[test]
    fn reject_nonexistent_relative_path() {
        // canonicalize falha quando o arquivo não existe — vira
        // JailViolation com mensagem sobre "não pôde ser resolvido".
        let (_dir, jail) = setup_workspace();
        let err = jail.resolve(Path::new("does_not_exist.txt")).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
    }

    #[test]
    fn reject_symlink_pointing_outside() {
        // Cria symlink em workspace apontando pra /etc/passwd (Unix)
        // ou pra C:\Windows\System32\drivers\etc\hosts (Windows).
        // Pula se não conseguir criar (Windows exige privilege).
        let (dir, jail) = setup_workspace();
        let target = if cfg!(windows) {
            r"C:\Windows\System32\drivers\etc\hosts"
        } else {
            "/etc/hosts"
        };
        let link = dir.join("link.txt");
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(target, &link);
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_file(target, &link);
        // Se o symlink não foi criado (privilege), pula o teste
        if !link.exists() {
            return;
        }
        let err = jail.resolve(Path::new("link.txt")).unwrap_err();
        assert_eq!(err.code, ToolErrorCode::JailViolation);
    }
}
