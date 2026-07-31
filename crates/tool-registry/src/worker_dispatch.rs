//! `WorkerToolDispatcher` — a ponte entre `Tool::execute` e
//! `WorkerHandle::invoke`, com **path safety forte** (Etapa 3 da
//! Fase 5).
//!
//! ## Por que existe
//!
//! O `WorkerHandle::invoke` é genérico e opaco — recebe um
//! `serde_json::Value` e envia pro worker. A barreira de path
//! dentro do worker Python (`document-worker.py: validate_path`)
//! é **mínima** (rejeita `..`, exige path absoluto ou relativo
//! ao `cwd`, confere que o pai existe/é gravável). É a barreira
//! certa pro worker, mas é **fraca demais pra um kit do
//! Frederico**: o `docs.generate` da Etapa 3 da Fase 5 aceita
//! `output_path` do chamador (modelo), e o chamador pode
//! apontar pra qualquer lugar do filesystem.
//!
//! O `ToolManifest::allowed_paths` fecha essa lacuna: o
//! dispatcher valida o(s) campo(s) de path do `args` **antes**
//! de chamar `WorkerHandle::invoke`. Canonicaliza o path
//! pedido e compara por `starts_with` contra cada entrada
//! canonicalizada da allowlist. O worker nunca vê o call.
//!
//! Esta é a "path safety forte" registrada como pendência no
//! `docs/modules/process-architecture.md` §"Pendências para a
//! próxima sessão", item 1.
//!
//! ## API
//!
//! ```ignore
//! let dispatcher = WorkerToolDispatcher::new(
//!     worker_handle.clone(),
//!     vec![workspace_root.to_path_buf()],
//! );
//! let result = dispatcher
//!     .dispatch(json!({"output_path": "C:\\Users\\me\\out.docx"}), &["output_path"])
//!     .await?;
//! ```
//!
//! O retorno é o payload do `tool.result` (o que o worker
//! devolveu), ou um `DispatchError::PathNotAllowed` (com o
//! path que falhou e a allowlist) se a validação falhou.
//!
//! ## Runtime flavor
//!
//! Esta função é `async fn` e roda em qualquer runtime tokio
//! (current_thread ou multi_thread). **O `WorkerHandle` em si**
//! é o que precisa de `flavor = "multi_thread"` se o caller
//! quiser tirar proveito de invokes concorrentes
//! (`process-architecture.md` §"Decisões"). Para testes
//! simples (1 invoke por vez), `#[tokio::test]` default
//! (current_thread) é suficiente.

use std::path::{Path, PathBuf};

use frederico_process_architecture::{ProcessError, WorkerHandle};
use serde_json::Value;
use thiserror::Error;

/// Erro do dispatcher. `PathNotAllowed` é estruturado — o caller
/// (o `Tool::execute` do kit) traduz em `ToolResult::err`
/// preservando o path e a allowlist no payload (auditável).
#[derive(Debug, Error)]
pub enum DispatchError {
    /// O path no `args[path_field]` não casa com nenhum dos
    /// `allowed_paths` (canonicalizados).
    #[error("path '{path}' não está em nenhum dos diretórios permitidos")]
    PathNotAllowed {
        path: PathBuf,
        allowed: Vec<PathBuf>,
    },

    /// O `WorkerHandle::invoke` falhou (transporte, timeout,
    /// protocolo). Re-exportado pra que o caller não precise
    /// importar `process-architecture` direto.
    #[error(transparent)]
    Process(#[from] ProcessError),

    /// Path no `args` não é uma string (tipo errado).
    #[error("campo de path '{field}' não é uma string")]
    NotAString { field: String, value: Value },
}

/// Ponte `Tool::execute` → `WorkerHandle::invoke` com
/// allowlist. Clonável (o `WorkerHandle` interno é `Arc`-ed).
///
/// `Debug` **não** é derivado: o `WorkerHandle` interno
/// carrega um `Arc<WorkerState>` que não implementa `Debug`
/// (decisão do `process-architecture` — a saúde do worker é
/// observada via `health_snapshot()`, não via `Debug`).
#[derive(Clone)]
pub struct WorkerToolDispatcher {
    handle: WorkerHandle,
    allowed_paths: Vec<PathBuf>,
}

impl WorkerToolDispatcher {
    /// Cria o dispatcher. `allowed_paths` é a allowlist
    /// canonicalizada em runtime (não precisa ser absoluta
    /// na entrada — `validate_against_allowlist` cuida).
    #[must_use]
    pub fn new(handle: WorkerHandle, allowed_paths: Vec<PathBuf>) -> Self {
        Self {
            handle,
            allowed_paths,
        }
    }

    /// `WorkerHandle` interno. Útil pra ping/health antes
    /// do invoke.
    #[must_use]
    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }

    /// Allowlist atual (paths **não** canonicalizados — eles
    /// são canonicalizados em cada validação; manter o
    /// original permite mover o diretório sem re-cadastrar).
    #[must_use]
    pub fn allowed_paths(&self) -> &[PathBuf] {
        &self.allowed_paths
    }

    /// Dispatcha `args` ao worker. `path_fields` lista os
    /// campos do JSON que carregam um path a ser validado
    /// contra a allowlist. Os campos são validados na ordem;
    /// o primeiro que falhar aborta o dispatch.
    ///
    /// Se `allowed_paths` está vazio, **não valida** (o
    /// trabalho é do worker — defesa em profundidade). A
    /// `Tool` que constrói o dispatcher é responsável por
    /// popular a allowlist (vazio é OK pra tools que não
    /// recebem path do chamador, mas a recomendação é sempre
    /// popular).
    ///
    /// # Erros
    /// - `DispatchError::PathNotAllowed` se algum `path_field`
    ///   aponta pra fora da allowlist.
    /// - `DispatchError::NotAString` se o campo não é string.
    /// - `DispatchError::Process` se o `WorkerHandle::invoke`
    ///   falha (transporte/timeout/protocolo).
    pub async fn dispatch(
        &self,
        args: Value,
        path_fields: &[&str],
    ) -> Result<Value, DispatchError> {
        // 1. Valida os paths ANTES de tocar no handle.
        if !self.allowed_paths.is_empty() {
            for field in path_fields {
                if let Some(v) = args.get(*field) {
                    if let Some(s) = v.as_str() {
                        validate_against_allowlist(s, &self.allowed_paths)?;
                    } else {
                        return Err(DispatchError::NotAString {
                            field: (*field).to_string(),
                            value: v.clone(),
                        });
                    }
                }
                // Se o campo não está presente, sem problema —
                // o schema do manifesto (validado antes) já
                // cobre a obrigatoriedade.
            }
        }

        // 2. Invoke opaco. O handle cuida do envelope IPC.
        let result = self.handle.invoke(args).await?;
        Ok(result)
    }

    /// Valida um path string contra a allowlist do dispatcher
    /// (atalho pra `validate_against_allowlist`).
    pub fn check_path(&self, path_str: &str) -> Result<(), DispatchError> {
        validate_against_allowlist(path_str, &self.allowed_paths)?;
        Ok(())
    }
}

/// **Função pura** (testável sem `WorkerHandle`): valida um
/// path string contra uma allowlist. Canonicaliza o path
/// pedido e cada entrada da allowlist, depois compara por
/// `starts_with`.
///
/// Regras:
/// - Se `allowed` está vazio, **passa** (sem validação —
///   caller decide).
/// - Se o path pedido não puder ser canonicalizado (e.g.
///   arquivo não existe), usa o path como está e tenta
///   `starts_with` mesmo assim. A defesa em profundidade
///   (worker) confere a existência depois — não somos
///   gate de existência, somos gate de **diretório
///   permitido**.
/// - **Divergência 8.3 vs long no Windows (Etapa 3
///   hotfix):** quando o path **não pode** ser canonicalizado,
///   o `allowed` **também não é canonicalizado**. Senão o
///   `canonicalize()` do `allowed` resolve nomes no formato
///   curto 8.3 (`RUNNER~1`) pro formato longo (`runneradmin`)
///   enquanto o path fica em formato curto, e o `starts_with`
///   falha mesmo o path estando dentro do dir. Bug apareceu
///   no CI do PR #13 (TEMP = `C:\Users\RUNNER~1\...`); local
///   passava porque o username `conta` não tem short name 8.3.
/// - Path traversal via `..` é coberto pelo
///   `canonicalize` (resolve `..` no nível do FS) ou pelo
///   `normalize_lexically` (colapsa `..` textual).
///
/// ## Verbatim prefix no Windows
///
/// `Path::canonicalize` no Windows retorna paths com o
/// prefixo verbatim `\\?\` (ex.: `\\?\C:\Users\...`).
/// `Path::starts_with` é component-wise E considera o
/// prefixo — `\\?\C:\...` não bate com `C:\...`. Por isso
/// strippamos o verbatim prefix antes de comparar.
pub fn validate_against_allowlist(
    path_str: &str,
    allowed: &[PathBuf],
) -> Result<(), DispatchError> {
    if allowed.is_empty() {
        return Ok(());
    }

    // Decide ANTES se vamos canonicalizar o `allowed`: só
    // canonicalizamos se o path_str TAMBÉM puder ser
    // canonicalizado. Isso garante que os dois lados da
    // comparação fiquem na mesma forma (long ou "como
    // está") e evita a divergência 8.3 vs long no Windows
    // (CI do PR #13 — `RUNNER~1` vs `runneradmin`).
    let path_canonical = Path::new(path_str).canonicalize().ok();
    let use_canonical = path_canonical.is_some();
    let canonical = path_canonical.map_or_else(
        || normalize_lexically(Path::new(path_str)),
        |c| strip_windows_verbatim(&c),
    );

    for allowed_path in allowed {
        let allowed_canonical = if use_canonical {
            // Path existe: canonicalizar o allowed (formato
            // long). Se o allowed não puder canonicalizar
            // (caso degenerado), cai pro "como está" —
            // `strip_windows_verbatim` apenas.
            allowed_path
                .canonicalize()
                .map(|c| strip_windows_verbatim(&c))
                .unwrap_or_else(|_| strip_windows_verbatim(allowed_path))
        } else {
            // Path NÃO existe: NÃO canonicalizar o allowed
            // — senão divergência de formato (Windows
            // 8.3 vs long). Compara lexicalmente.
            strip_windows_verbatim(allowed_path)
        };
        if canonical.starts_with(&allowed_canonical) {
            return Ok(());
        }
    }

    Err(DispatchError::PathNotAllowed {
        path: canonical,
        allowed: allowed.to_vec(),
    })
}

/// Strippa o prefixo verbatim `\\?\` (Windows). Em outras
/// plataformas, é identity.
fn strip_windows_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.as_os_str();
        let s_lossy = s.to_string_lossy();
        if let Some(rest) = s_lossy.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
    }
    p.to_path_buf()
}

/// Normaliza um path **lexicalmente** (sem tocar no FS):
/// colapsa `.` e `..` nos componentes. Usado quando
/// `canonicalize` falha (path não existe) e ainda assim
/// precisamos comparar com a allowlist.
///
/// O algoritmo é o clássico de `path_clean`:
/// 1. Itera os `components` do path.
/// 2. `.` → ignora.
/// 3. `..` → se o topo é um componente "normal", pop;
///    senão (rootdir, prefix, ou vazio), mantém `..`.
/// 4. Normal → push.
///
/// **Não** resolve symlinks (isso é trabalho do FS); apenas
/// colapsa `..` textuais.
fn normalize_lexically(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component<'_>> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {} // skip
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Em rootdir/prefix/vazio, mantém o `..` —
                // não podemos "subir" do root, e manter o
                // `..` faz a comparação falhar
                // legitimamente (path não está sob a
                // allowlist).
                _ => out.push(comp),
            },
            _ => out.push(comp),
        }
    }
    if out.is_empty() {
        // Path era "." ou equivalente — devolve "."
        PathBuf::from(".")
    } else {
        out.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Cria um tempdir único. Counter atômico evita colisão
    /// em testes paralelos no Windows (granularidade do
    /// timestamp é grosseira).
    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let unique = format!(
            "frederico-tool-registry-worker-dispatch-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            n,
        );
        let dir = base.join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    // ---- validate_against_allowlist (função pura) --------------

    #[test]
    fn path_inside_allowed_root_passes() {
        let dir = tempdir();
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        let inside = sub.join("file.txt");
        fs::write(&inside, "x").unwrap();
        let allowed = vec![dir.clone()];
        validate_against_allowlist(inside.to_str().unwrap(), &allowed).unwrap();
    }

    #[test]
    fn path_outside_allowed_root_fails() {
        let dir = tempdir();
        let outside = std::env::temp_dir().join("definitely_not_in_workspace.txt");
        let allowed = vec![dir.clone()];
        let err = validate_against_allowlist(outside.to_str().unwrap(), &allowed)
            .expect_err("deveria falhar");
        match err {
            DispatchError::PathNotAllowed { path, allowed: a } => {
                // temp_dir() chamado 2x (PathBuf não é Copy) —
                // barato, e evita mover e re-borrow.
                let td_prefix = std::env::temp_dir();
                let td_msg = std::env::temp_dir();
                assert!(
                    path.starts_with(&td_prefix),
                    "path {path:?} nao comeca com {td_msg:?}"
                );
                assert_eq!(a, vec![dir]);
            }
            other => panic!("esperava PathNotAllowed, veio {other:?}"),
        }
    }

    #[test]
    fn path_with_empty_allowlist_passes() {
        // Allowlist vazia = sem validação.
        let outside = std::env::temp_dir().join("anywhere.txt");
        validate_against_allowlist(outside.to_str().unwrap(), &[]).unwrap();
    }

    #[test]
    fn path_with_parent_traversal_rejected() {
        let dir = tempdir();
        // Tenta escapar via "..".
        let traversal = dir.join("..").join("..").join("etc").join("passwd");
        let allowed = vec![dir.clone()];
        let err = validate_against_allowlist(traversal.to_str().unwrap(), &allowed)
            .expect_err("deveria falhar");
        assert!(matches!(err, DispatchError::PathNotAllowed { .. }));
    }

    #[test]
    fn path_with_multiple_allowed_roots() {
        let dir_a = tempdir();
        let dir_b = tempdir();
        let inside_a = dir_a.join("file.txt");
        let inside_b = dir_b.join("file.txt");
        fs::write(&inside_a, "a").unwrap();
        fs::write(&inside_b, "b").unwrap();

        let allowed = vec![dir_a.clone(), dir_b.clone()];
        validate_against_allowlist(inside_a.to_str().unwrap(), &allowed).unwrap();
        validate_against_allowlist(inside_b.to_str().unwrap(), &allowed).unwrap();
    }

    #[test]
    fn path_nonexistent_canonicalize_fallback_works() {
        // Path que não existe: canonicalize falha, caímos no
        // fallback `path como está`. Se o path literal está
        // dentro da allowlist (string-level starts_with), passa.
        //
        // **Caso 8.3 vs long (Windows):** este teste pegou o
        // bug do PR #13 — no CI, `TEMP = C:\Users\RUNNER~1\...`
        // e o `canonicalize(dir)` resolve `RUNNER~1` para
        // `runneradmin` (formato long) enquanto o `path` não
        // existe e cai no `normalize_lexically` (formato
        // curto). `starts_with` falhava. Fix: quando path não
        // pode canonicalizar, allowed também não canonicaliza.
        // Local passava antes do fix porque o username
        // `conta` não tem short name 8.3.
        let dir = tempdir();
        let not_yet_created = dir.join("will_be_created.docx");
        let allowed = vec![dir.clone()];
        validate_against_allowlist(not_yet_created.to_str().unwrap(), &allowed).unwrap();
    }

    #[test]
    fn path_nonexistent_inside_deep_subdir_passes() {
        // Variante do teste anterior: path com 2 níveis de
        // subdir inexistente. Confirma que `normalize_lexically`
        // preserva os componentes e o `starts_with` continua
        // batendo.
        let dir = tempdir();
        let deep = dir.join("a").join("b").join("file.docx");
        let allowed = vec![dir.clone()];
        validate_against_allowlist(deep.to_str().unwrap(), &allowed).unwrap();
    }

    #[test]
    fn path_nonexistent_outside_with_short_form_allowed_fails() {
        // Path não existente FORA do allowed (que está em
        // formato "como está" porque path não pode
        // canonicalizar). Deve falhar consistentemente.
        let dir = tempdir();
        let outside = std::env::temp_dir().join("frederico_outside_workspace.docx");
        let allowed = vec![dir.clone()];
        let err = validate_against_allowlist(outside.to_str().unwrap(), &allowed)
            .expect_err("deveria falhar");
        assert!(matches!(err, DispatchError::PathNotAllowed { .. }));
    }

    #[test]
    fn path_nonexistent_outside_fails() {
        let dir = tempdir();
        let not_yet_created_outside = std::env::temp_dir().join("frederico_outside_workspace.docx");
        let allowed = vec![dir.clone()];
        let err = validate_against_allowlist(not_yet_created_outside.to_str().unwrap(), &allowed)
            .expect_err("deveria falhar");
        assert!(matches!(err, DispatchError::PathNotAllowed { .. }));
    }
}
