//! `resolve_document_worker_runtime` — onde está o Python + libs do
//! `document-worker` em runtime?
//!
//! ADR-0023 §D1. Função pura, sem I/O bloqueante, sem side effects.
//! Recebe um [`RuntimeContext`] com os paths candidatos e devolve o
//! primeiro que tiver o runtime completo, ou `None` se nenhum
//! servir.
//!
//! **Por que existe um resolvedor com precedência fixa:** o código
//! do resolvedor **não muda** quando a Fase 9 do PROMPT MESTRE
//! empacotar o `document-worker` como `bundle.resources` do Tauri.
//! A Fase 9 popula a opção 2 do `RuntimeContext` (recursos do app);
//! as opções 1 e 3 permanecem. A função de resolução é a mesma.
//!
//! ## Precedência (1ª que achar vence)
//!
//! 1. **Variável de ambiente `FREDERICO_DOCUMENT_WORKER_RUNTIME`** —
//!    `PathBuf` absoluta pro diretório que contém `python.exe` e
//!    `document-worker.py`. Usada em testes e em setups não-padrão
//!    (ex.: desenvolvedor com Python instalado em outro path).
//! 2. **Recursos do app** — `runtime_root` passado pela casca
//!    (em produção, vem de `tauri::AppHandle::path().resolve(...)`).
//!    Em dev, a casca pode passar o `CARGO_MANIFEST_DIR` ou
//!    `None`; em produção, esse é o caminho que o instalador NSIS
//!    usa pra extrair o `bundle.resources`. Quando a Fase 9
//!    empacotar, esse campo passa a ser `Some(...)` em produção
//!    e a opção 2 começa a retornar `Some(_)`.
//! 3. **Caminho de dev no repositório** — `dev_root` passado pela
//!    casca (em dev, `CARGO_MANIFEST_DIR` aponta pro repo). A casca
//!    resolve `<dev_root>/../workers/document-worker/runtime/` e
//!    passa o resultado como `dev_root` desta opção. Em produção
//!    (`.exe` instalado), `dev_root` é `None` porque o
//!    `CARGO_MANIFEST_DIR` do app instalado não é o repo.
//!
//! ## Detecção de runtime "completo"
//!
//! Para um candidato ser aceito, **3 artefatos** precisam estar
//! presentes no diretório:
//!
//! - `python.exe` (Windows; em outras plataformas seria
//!   `python3.exe`, mas o gate `#[cfg(windows)]` do
//!   `ExternalSpawnConfig` torna isso acadêmico por enquanto).
//! - `document-worker.py` — entry-point do worker.
//! - `Lib/site-packages/` — diretório onde as deps (pywin32,
//!   python-docx, openpyxl, reportlab, pdfplumber, pytesseract,
//!   etc.) são instaladas pelo `bootstrap.ps1`.
//!
//! Se faltar qualquer um, o candidato é rejeitado e a próxima
//! opção é tentada. **Ausência = indisponibilidade** (ADR-0023
//! §D2), não erro.

use std::path::{Path, PathBuf};

/// Localização resolvida do runtime do `document-worker`.
///
/// Carrega o `python.exe` (caminho absoluto pro executável) e o
/// `document-worker.py` (entry-point do worker, passado como
/// argumento pro `ExternalSpawnConfig`). O `args[0]` no spawn é
/// exatamente esse `script`; o `command` é `python.exe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLocation {
    /// Caminho do `python.exe` (passado como `command` no
    /// `ExternalSpawnConfig`).
    pub python_exe: PathBuf,
    /// Caminho do `document-worker.py` (passado como
    /// `args[0]`).
    pub script: PathBuf,
    /// Diretório raiz do runtime (mesmo valor das 3 opções
    /// do `RuntimeContext`, normalizado). Útil pra logs e pra
    /// a UI de diagnóstico mostrar o caminho resolvido.
    pub root: PathBuf,
    /// Qual opção do `RuntimeContext` foi a vencedora (1, 2
    /// ou 3). Útil pra logs e diagnóstico.
    pub source: RuntimeSource,
}

/// Qual opção do `RuntimeContext` foi escolhida. As numerações
/// seguem a precedência do ADR-0023 §D1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSource {
    /// Variável de ambiente `FREDERICO_DOCUMENT_WORKER_RUNTIME`
    /// (opção 1).
    EnvVar,
    /// Recursos do app, populados pela casca via
    /// `tauri::AppHandle::path().resolve(...)` (opção 2).
    AppResources,
    /// Caminho de dev no repositório (opção 3).
    DevRepo,
}

/// Candidatos de runtime. A casca Tauri monta este struct
/// (sem chamar `resolve_document_worker_runtime`) e o
/// `frederico-app` consome. **Pura** — sem I/O, sem leitura
/// de env, sem nada. Quem lê o env é a casca (que tem
/// permissão pra isso via `std::env::var`); quem resolve
/// `tauri::AppHandle::path()` é a casca (que importa `tauri`).
/// O `frederico-app` recebe os paths já materializados e só
/// checa a presença de artefatos.
#[derive(Debug, Clone, Default)]
pub struct RuntimeContext {
    /// Opção 1: env var `FREDERICO_DOCUMENT_WORKER_RUNTIME`
    /// (materializado pela casca).
    pub env_override: Option<PathBuf>,
    /// Opção 2: recursos do app (materializado pela casca via
    /// `tauri::AppHandle::path().resolve(...)` em produção;
    /// `None` em dev quando a casca não popula).
    pub app_resources: Option<PathBuf>,
    /// Opção 3: caminho de dev no repositório. Em produção
    /// (`.exe` instalado), a casca passa `None` porque o
    /// `CARGO_MANIFEST_DIR` do app instalado não é o repo.
    pub dev_repo: Option<PathBuf>,
}

impl RuntimeContext {
    /// Cria um `RuntimeContext` vazio (sem nenhum candidato).
    /// Útil em testes que exercitam o caminho de "todos
    /// ausentes" (`resolve_document_worker_runtime` retorna
    /// `None`).
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Cria um `RuntimeContext` só com a opção 3 (caminho de
    /// dev). Usado pela casca Tauri em dev quando o
    /// `bootstrap.ps1` rodou e o `runtime/` existe no repo.
    #[must_use]
    pub fn dev_only(root: PathBuf) -> Self {
        Self {
            dev_repo: Some(root),
            ..Self::default()
        }
    }

    /// Cria um `RuntimeContext` só com a opção 2 (recursos do
    /// app). Usado pela casca Tauri em produção quando a Fase 9
    /// empacotar o `document-worker` como `bundle.resources`.
    #[must_use]
    pub fn app_only(root: PathBuf) -> Self {
        Self {
            app_resources: Some(root),
            ..Self::default()
        }
    }
}

/// Resolvedor — itera sobre os 3 candidatos do `RuntimeContext`
/// em ordem de precedência fixa, e devolve o primeiro que tiver
/// o runtime completo. Se nenhum servir, devolve `None` (D2:
/// indisponibilidade, não erro).
///
/// **Função pura** — não lê env, não toca disco (exceto o
/// `try_exists`/`is_dir` síncrono, que é I/O de stat, não de
/// leitura). A I/O pesada (spawnar o Python, baixar runtime) é
/// trabalho da casca via `spawn_external`, não do resolvedor.
///
/// ## Erros reportados no log
///
/// A função loga via `tracing::debug!` cada candidato rejeitado
/// e o motivo. Erros inesperados (permissão, I/O) são logados
/// via `tracing::warn!` mas **não** interrompem a iteração — a
/// próxima opção é tentada. Isso é importante: um env var
/// apontando pra um diretório sem permissão não pode impedir o
/// runtime de dev de ser achado.
#[must_use]
pub fn resolve_document_worker_runtime(ctx: &RuntimeContext) -> Option<RuntimeLocation> {
    // Opção 1: env var
    if let Some(root) = ctx.env_override.as_ref() {
        if let Some(mut loc) = check_candidate(root) {
            loc.source = RuntimeSource::EnvVar;
            tracing::info!(
                runtime_root = %loc.root.display(),
                "document-worker runtime resolvido via env var"
            );
            return Some(loc);
        }
        tracing::debug!(
            candidate = %root.display(),
            "candidato env var rejeitado (runtime incompleto)"
        );
    }

    // Opção 2: recursos do app
    if let Some(root) = ctx.app_resources.as_ref() {
        if let Some(mut loc) = check_candidate(root) {
            loc.source = RuntimeSource::AppResources;
            tracing::info!(
                runtime_root = %loc.root.display(),
                "document-worker runtime resolvido via recursos do app"
            );
            return Some(loc);
        }
        tracing::debug!(
            candidate = %root.display(),
            "candidato recursos do app rejeitado (runtime incompleto)"
        );
    }

    // Opção 3: dev repo
    if let Some(root) = ctx.dev_repo.as_ref() {
        if let Some(mut loc) = check_candidate(root) {
            loc.source = RuntimeSource::DevRepo;
            tracing::info!(
                runtime_root = %loc.root.display(),
                "document-worker runtime resolvido via dev repo"
            );
            return Some(loc);
        }
        tracing::debug!(
            candidate = %root.display(),
            "candidato dev repo rejeitado (runtime incompleto)"
        );
    }

    tracing::info!("document-worker runtime indisponível: nenhum candidato válido");
    None
}

/// Checa se um diretório candidato tem o runtime completo. Se
/// sim, monta o `RuntimeLocation` com `RuntimeSource::DevRepo`
/// como **placeholder** — o
/// [`resolve_document_worker_runtime`] sobrescreve o `source`
/// com o valor correto baseado em qual campo do
/// `RuntimeContext` foi o vencedor. Se não, devolve `None` (sem
/// erro — ausência é o caminho normal, não exceção).
fn check_candidate(root: &Path) -> Option<RuntimeLocation> {
    // O resolvedor é symlink-friendly: `try_exists` segue
    // symlinks. Em Windows, symlinks precisam de privilege
    // elevated ou developer mode; o `bootstrap.ps1` cria
    // diretórios físicos, então isso não é problema.
    let python_exe = root.join(if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    });
    let script = root.join("document-worker.py");
    let site_packages = root.join("Lib").join("site-packages");

    if !is_file(&python_exe) {
        return None;
    }
    if !is_file(&script) {
        return None;
    }
    if !is_dir(&site_packages) {
        return None;
    }

    Some(RuntimeLocation {
        python_exe,
        script,
        root: root.to_path_buf(),
        source: RuntimeSource::DevRepo, // placeholder — sobrescrito pelo caller
    })
}

/// Wrapper de `Path::try_exists` que tolera `NotFound` e loga
/// o motivo em `warn` se for outro erro.
fn is_file(path: &Path) -> bool {
    match path.try_exists() {
        Ok(true) => path.is_file(),
        Ok(false) => false,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "is_file: erro de I/O");
            false
        }
    }
}

fn is_dir(path: &Path) -> bool {
    match path.try_exists() {
        Ok(true) => path.is_dir(),
        Ok(false) => false,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "is_dir: erro de I/O");
            false
        }
    }
}

/// Erro de runtime. Usado pela casca pra construir a resposta
/// do `tauri::command DocumentWorkerStatus` quando
/// `resolve_document_worker_runtime` retorna `None`. A mensagem
/// é PT-BR (regra do projeto) e cita os 3 candidatos pra
/// diagnóstico.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeUnavailableError {
    #[error(
        "document-worker indisponível: nenhum dos 3 candidatos tem runtime completo. \
         Verifique: (1) env FREDERICO_DOCUMENT_WORKER_RUNTIME, \
         (2) recursos do app, (3) workers/document-worker/runtime/ no repositório. \
         Execute o bootstrap.ps1 se necessário."
    )]
    NoCandidate,
}

impl RuntimeUnavailableError {
    /// Constrói a partir do `RuntimeContext` (pra incluir os
    /// paths tentados na mensagem). Útil pra UI de diagnóstico.
    #[must_use]
    pub fn from_context(ctx: &RuntimeContext) -> Self {
        // A mensagem é fixa (a enum) mas o `Display` cita os 3
        // caminhos. Quem quiser mais detalhe usa
        // `format!("{:?}", ctx)`.
        let _ = ctx;
        Self::NoCandidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cria um diretório temporário com a estrutura mínima do
    /// runtime: `python.exe` (arquivo vazio) + `document-worker.py`
    /// (arquivo vazio) + `Lib/site-packages/` (diretório vazio).
    fn make_complete_runtime() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // python.exe — arquivo vazio
        std::fs::write(
            root.join(if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }),
            b"",
        )
        .expect("write python");
        // document-worker.py
        std::fs::write(root.join("document-worker.py"), b"#!/usr/bin/env python3\n")
            .expect("write script");
        // Lib/site-packages
        std::fs::create_dir_all(root.join("Lib").join("site-packages"))
            .expect("mkdir site-packages");

        dir
    }

    #[test]
    fn resolve_with_empty_context_returns_none() {
        let ctx = RuntimeContext::empty();
        assert!(resolve_document_worker_runtime(&ctx).is_none());
    }

    #[test]
    fn resolve_with_env_var_complete_runtime_returns_some_with_env_source() {
        let dir = make_complete_runtime();
        let ctx = RuntimeContext {
            env_override: Some(dir.path().to_path_buf()),
            ..RuntimeContext::empty()
        };
        let loc = resolve_document_worker_runtime(&ctx).expect("runtime deve resolver");
        assert_eq!(loc.source, RuntimeSource::EnvVar);
        assert!(loc.python_exe.ends_with(if cfg!(windows) {
            "python.exe"
        } else {
            "python3"
        }));
        assert!(loc.script.ends_with("document-worker.py"));
    }

    #[test]
    fn resolve_with_app_resources_complete_runtime_returns_some_with_app_source() {
        let dir = make_complete_runtime();
        let ctx = RuntimeContext {
            app_resources: Some(dir.path().to_path_buf()),
            ..RuntimeContext::empty()
        };
        let loc = resolve_document_worker_runtime(&ctx).expect("runtime deve resolver");
        assert_eq!(loc.source, RuntimeSource::AppResources);
    }

    #[test]
    fn resolve_with_dev_repo_complete_runtime_returns_some_with_dev_source() {
        let dir = make_complete_runtime();
        let ctx = RuntimeContext {
            dev_repo: Some(dir.path().to_path_buf()),
            ..RuntimeContext::empty()
        };
        let loc = resolve_document_worker_runtime(&ctx).expect("runtime deve resolver");
        assert_eq!(loc.source, RuntimeSource::DevRepo);
    }

    #[test]
    fn env_var_takes_precedence_over_app_resources_and_dev_repo() {
        let env_dir = make_complete_runtime();
        let app_dir = make_complete_runtime();
        let dev_dir = make_complete_runtime();

        let ctx = RuntimeContext {
            env_override: Some(env_dir.path().to_path_buf()),
            app_resources: Some(app_dir.path().to_path_buf()),
            dev_repo: Some(dev_dir.path().to_path_buf()),
        };

        let loc = resolve_document_worker_runtime(&ctx).expect("deve resolver");
        assert_eq!(loc.source, RuntimeSource::EnvVar);
        // O resolvedor devolve o `root` que recebeu — sem
        // canonicalizar. A semântica é "o que a casca passou",
        // não "o caminho real do filesystem".
        assert_eq!(loc.root, env_dir.path());
    }

    #[test]
    fn app_resources_takes_precedence_over_dev_repo() {
        let app_dir = make_complete_runtime();
        let dev_dir = make_complete_runtime();

        let ctx = RuntimeContext {
            app_resources: Some(app_dir.path().to_path_buf()),
            dev_repo: Some(dev_dir.path().to_path_buf()),
            ..RuntimeContext::empty()
        };

        let loc = resolve_document_worker_runtime(&ctx).expect("deve resolver");
        assert_eq!(loc.source, RuntimeSource::AppResources);
    }

    #[test]
    fn incomplete_runtime_missing_python_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // document-worker.py OK
        std::fs::write(root.join("document-worker.py"), b"#!/usr/bin/env python3\n").unwrap();
        // Lib/site-packages OK
        std::fs::create_dir_all(root.join("Lib").join("site-packages")).unwrap();
        // python.exe FALTA

        let ctx = RuntimeContext {
            dev_repo: Some(root.to_path_buf()),
            ..RuntimeContext::empty()
        };
        assert!(resolve_document_worker_runtime(&ctx).is_none());
    }

    #[test]
    fn incomplete_runtime_missing_script_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // python.exe OK
        std::fs::write(
            root.join(if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }),
            b"",
        )
        .unwrap();
        // Lib/site-packages OK
        std::fs::create_dir_all(root.join("Lib").join("site-packages")).unwrap();
        // document-worker.py FALTA

        let ctx = RuntimeContext {
            dev_repo: Some(root.to_path_buf()),
            ..RuntimeContext::empty()
        };
        assert!(resolve_document_worker_runtime(&ctx).is_none());
    }

    #[test]
    fn incomplete_runtime_missing_site_packages_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // python.exe OK
        std::fs::write(
            root.join(if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }),
            b"",
        )
        .unwrap();
        // document-worker.py OK
        std::fs::write(root.join("document-worker.py"), b"#!/usr/bin/env python3\n").unwrap();
        // Lib/site-packages FALTA

        let ctx = RuntimeContext {
            dev_repo: Some(root.to_path_buf()),
            ..RuntimeContext::empty()
        };
        assert!(resolve_document_worker_runtime(&ctx).is_none());
    }

    #[test]
    fn nonexistent_candidate_is_rejected_silently_and_falls_through() {
        // Opção 1 (env) aponta pra path que não existe.
        // Opção 3 (dev) tem runtime válido.
        let dev_dir = make_complete_runtime();
        let ctx = RuntimeContext {
            env_override: Some(PathBuf::from("/caminho/que/nao/existe/garantido")),
            dev_repo: Some(dev_dir.path().to_path_buf()),
            ..RuntimeContext::empty()
        };
        let loc = resolve_document_worker_runtime(&ctx).expect("deve cair pra opção 3");
        assert_eq!(loc.source, RuntimeSource::DevRepo);
    }

    #[test]
    fn runtime_unavailable_error_message_cites_three_candidates() {
        let err = RuntimeUnavailableError::NoCandidate;
        let msg = err.to_string();
        assert!(msg.contains("FREDERICO_DOCUMENT_WORKER_RUNTIME"));
        assert!(msg.contains("recursos do app"));
        assert!(msg.contains("bootstrap.ps1"));
    }
}
