//! `JailResolver` trait + `FileSystemJailResolver` (default da Etapa 1).
//!
//! Ver ADR-0022 §D2 para a motivação e o contexto.
//!
//! ## Posição do trait (revisão do ADR-0022 §D2)
//!
//! O **trait** `JailResolver` mora em `frederico-tool-registry`
//! (reexportado aqui como `pub use`). A `FileSystemJailResolver`
//! (a impl concreta default usada em produção) mora aqui. Esta
//! divisão evita ciclo: o toolkit depende do `frederico-core`
//! (não pode depender do `frederico-app`); o `frederico-app`
//! depende do toolkit. A Etapa 7 (modo desenvolvedor) introduz
//! `SecurityJailResolver` aqui mesmo, sem mudar a interface.
//!
//! ## Princípio de erro duro
//!
//! `FileSystemJailResolver::resolve` falha com `JailResolverError`
//! quando a criação do workspace não é possível. **Não há fallback**
//! para `temp_dir` ou outro local compartilhado — degradação
//! silenciosa num caminho de isolamento é o tipo de bug que a
//! Fase de Ligação existe para eliminar (decisão registrada na
//! conversa da Etapa 1: "Sem fallback para `temp_dir`. Falha ao
//! resolver o jail é erro duro, propagado como `ToolResult`
//! legível. O fallback existia no código antigo; não o carregue
//! para o novo."). A Etapa 7 (modo desenvolvedor) substitui esta
//! implementação por `SecurityJailResolver` via `frederico-security`,
//! sem mudar o trait.

use std::path::PathBuf;

use frederico_core::ConversationId;
use frederico_tool_registry::{Jail, JailResolver, ToolError};
use thiserror::Error;
use tracing::warn;

// Reexporta o trait do `frederico-tool-registry` sob o nome
// `JailResolver` neste módulo. O `impl JailResolver for
// FileSystemJailResolver` abaixo é o impl do trait do toolkit —
// não há trait paralelo no `frederico-app`. Isso garante
// coerência de tipos: `Arc<dyn frederico_tool_registry::JailResolver>`
// e `Arc<dyn frederico_app::jail::JailResolver>` são o mesmo trait.
pub use frederico_tool_registry::JailResolver as _;

/// Erro do `JailResolver` (específico do `FileSystemJailResolver`).
///
/// O trait `JailResolver` no toolkit define um `JailResolverError`
/// mais genérico (Box<dyn Error>); aqui temos as variantes
/// específicas do filesystem para mensagens PT-BR mais precisas.
/// `From<FileSystemJailResolverError> for frederico_tool_registry::JailResolverError`
/// faz a ponte (no impl do trait).
///
/// **Erro duro** (sem fallback): `FileSystemJailResolver::resolve`
/// propaga `io::Error` quando o `mkdir -p` falha, em vez de
/// degradar para `temp_dir` (que reintroduziria vazamento entre
/// conversas).
#[derive(Debug, Error)]
pub enum FileSystemJailResolverError {
    /// Falha ao criar o diretório do workspace da conversa.
    /// Inclui o `conversation_id` e o `io::Error` original.
    #[error("falha ao criar workspace para conversa {conversation_id}: {source}")]
    CreateWorkspace {
        conversation_id: ConversationId,
        #[source]
        source: std::io::Error,
    },

    /// Falha ao construir o `Jail` em torno do diretório criado.
    /// Na prática, só dispara se o `canonicalize` falhar (permissão
    /// foi revogada entre o `create_dir_all` e o `canonicalize`,
    /// race condition, etc.).
    #[error("falha ao construir jail para conversa {conversation_id}: {source}")]
    JailSetup {
        conversation_id: ConversationId,
        #[source]
        source: ToolError,
    },
}

/// Resolvedor de jail baseado em filesystem. **Default da Etapa 1**.
///
/// Cria `<workspaces_root>/<conversation_id>/` se não existir e
/// devolve um `Jail` apontando para esse diretório. Falha ao
/// criar o diretório é erro duro.
#[derive(Debug, Clone)]
pub struct FileSystemJailResolver {
    workspaces_root: PathBuf,
}

impl FileSystemJailResolver {
    /// Constrói o resolvedor. `workspaces_root` é tipicamente
    /// `<data_local_dir>/workspaces/` resolvido pela casca
    /// (`apps/desktop/src-tauri/src/main.rs`) ou pelo modo
    /// servidor (§5.5).
    #[must_use]
    pub fn new(workspaces_root: PathBuf) -> Self {
        Self { workspaces_root }
    }

    /// Diretório raiz dos workspaces. Útil para logs e diagnóstico.
    #[must_use]
    pub fn workspaces_root(&self) -> &std::path::Path {
        &self.workspaces_root
    }
}

impl JailResolver for FileSystemJailResolver {
    fn resolve(
        &self,
        conversation_id: &ConversationId,
    ) -> frederico_tool_registry::JailResolverResult<Jail> {
        // Path do workspace da conversa: workspaces_root / UUID.
        // UUID em formato hyphenated (mesmo formato de
        // `ConversationId::as_uuid().to_string()`), igual ao que
        // `frederico_core::ConversationId` carrega internamente.
        let workspace = self
            .workspaces_root
            .join(conversation_id.as_uuid().to_string());

        // `mkdir -p` idempotente: não falha se o dir já existe.
        // Erro de permissão, disco cheio, caminho inválido etc.
        // é erro duro (sem fallback). Logamos warn com o motivo
        // antes de propagar.
        if let Err(source) = std::fs::create_dir_all(&workspace) {
            warn!(
                conversation_id = %conversation_id,
                workspace = %workspace.display(),
                error = %source,
                "falha ao criar workspace da conversa; jail NAO resolvido"
            );
            return Err(Self::wrap_error(
                *conversation_id,
                FileSystemJailResolverError::CreateWorkspace {
                    conversation_id: *conversation_id,
                    source,
                },
            ));
        }

        // `Jail::new` faz o canonicalize da raiz. Em Windows, o
        // `\\?\` prefix pode ser introduzido; o `starts_with` usado
        // depois pelo `Jail::resolve` lida com isso.
        let jail = Jail::new(&workspace).map_err(|source| {
            Self::wrap_error(
                *conversation_id,
                FileSystemJailResolverError::JailSetup {
                    conversation_id: *conversation_id,
                    source,
                },
            )
        })?;

        Ok(jail)
    }
}

impl FileSystemJailResolver {
    /// Converte o erro específico do filesystem no erro genérico
    /// do trait (`JailResolverError` no toolkit). Box<dyn Error>
    /// carrega o erro concreto para diagnóstico.
    fn wrap_error(
        conversation_id: ConversationId,
        specific: FileSystemJailResolverError,
    ) -> frederico_tool_registry::JailResolverError {
        frederico_tool_registry::JailResolverError::Resolve {
            conversation_id,
            source: Box::new(specific),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Contador atômico para nomes únicos de tempdir. Mesmo padrão
    /// usado em `crates/tool-registry/src/workspace.rs::tests`.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Tempdir(PathBuf);

    impl Tempdir {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            // PID + counter atômico dão unicidade sem depender
            // do relógio do Windows (granularidade grosseira
            // em nanosegundos + paralelismo de testes podem
            // colidir no mesmo valor).
            let unique = format!("frederico-app-jail-{}-{}-{}", label, std::process::id(), n,);
            let dir = base.join(unique);
            std::fs::create_dir_all(&dir).expect("cria tempdir raiz do teste");
            Self(dir)
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
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample_conversation_id_a() -> ConversationId {
        // UUID fixo para ter path determinístico nos logs.
        // `from_bytes` (em `frederico_core::ConversationId` via
        // `uuid::Uuid::from_bytes`) é estável e não exige dep
        // direta do crate `uuid` no `Cargo.toml` deste crate.
        ConversationId(uuid::Uuid::from_bytes([
            0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55,
            0x55, 0x55,
        ]))
    }

    fn sample_conversation_id_b() -> ConversationId {
        ConversationId(uuid::Uuid::from_bytes([
            0x99, 0x99, 0x99, 0x99, 0x88, 0x88, 0x77, 0x77, 0x66, 0x66, 0x55, 0x55, 0x55, 0x55,
            0x55, 0x55,
        ]))
    }

    #[test]
    fn resolve_creates_workspace_dir() {
        let root = Tempdir::new("resolve-creates");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid = sample_conversation_id_a();

        let jail = resolver.resolve(&cid).expect("resolve deve succeed");
        let expected = root.0.join("11111111-2222-3333-4444-555555555555");
        assert!(
            expected.exists(),
            "workspace dir não foi criado em {expected:?}"
        );
        // `Jail::new` canonicaliza o root; em Windows pode introduzir
        // `\\?\` prefix, então comparamos via `canonicalize` dos dois
        // lados (mesmo método usado pelo próprio `Jail`).
        let expected_canonical = expected.canonicalize().unwrap();
        assert_eq!(jail.root().canonicalize().unwrap(), expected_canonical);
    }

    #[test]
    fn resolve_is_idempotent() {
        // Resolver duas vezes para a mesma conversa deve succeed
        // sem erro (`mkdir -p` idempotente).
        let root = Tempdir::new("resolve-idempotent");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid = sample_conversation_id_a();

        let jail1 = resolver.resolve(&cid).expect("primeiro resolve");
        let jail2 = resolver.resolve(&cid).expect("segundo resolve");
        assert_eq!(
            jail1.root().canonicalize().unwrap(),
            jail2.root().canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_creates_independent_dirs_per_conversation() {
        // Cada conversa tem seu próprio dir; vazamento entre
        // conversas é o que o `JailResolver` foi criado para
        // impedir (mesma classe de I4 da memória).
        let root = Tempdir::new("resolve-independent");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid_a = sample_conversation_id_a();
        let cid_b = sample_conversation_id_b();

        let _ = resolver.resolve(&cid_a).unwrap();
        let _ = resolver.resolve(&cid_b).unwrap();

        let dir_a = root.0.join("11111111-2222-3333-4444-555555555555");
        let dir_b = root.0.join("99999999-8888-7777-6666-555555555555");
        assert!(dir_a.exists(), "dir da conversa A não foi criado");
        assert!(dir_b.exists(), "dir da conversa B não foi criado");
        assert_ne!(dir_a, dir_b, "A e B não podem compartilhar dir");
    }

    #[test]
    fn jail_returned_accepts_path_inside_workspace() {
        // Smoke: cria um arquivo dentro do workspace, `Jail::resolve`
        // tem que aceitar.
        let root = Tempdir::new("jail-accepts");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid = sample_conversation_id_a();
        let jail = resolver.resolve(&cid).unwrap();

        let workspace = root.0.join("11111111-2222-3333-4444-555555555555");
        std::fs::write(workspace.join("hello.txt"), "oi").unwrap();

        let resolved = jail
            .resolve(Path::new("hello.txt"))
            .expect("resolve hello.txt dentro do jail");
        let content = std::fs::read_to_string(&resolved).unwrap();
        assert_eq!(content, "oi");
    }

    #[test]
    fn jail_returned_rejects_parent_dir() {
        // Regressão §I3: o jail por conversa ainda bloqueia
        // path traversal via `..` (não é "barreira fraca").
        let root = Tempdir::new("jail-rejects-parent");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid = sample_conversation_id_a();
        let jail = resolver.resolve(&cid).unwrap();

        let err = jail
            .resolve(Path::new("../etc/passwd"))
            .expect_err(".. deve ser bloqueado");
        assert_eq!(
            err.code,
            frederico_tool_registry::ToolErrorCode::JailViolation
        );
    }

    #[test]
    fn jail_returned_rejects_absolute_windows_path() {
        // Regressão §I3: caminhos absolutos bloqueados mesmo
        // depois de resolver o jail por conversa.
        let root = Tempdir::new("jail-rejects-abs");
        let resolver = FileSystemJailResolver::new(root.0.clone());
        let cid = sample_conversation_id_a();
        let jail = resolver.resolve(&cid).unwrap();

        let abs = Path::new(r"C:\Windows\System32\drivers\etc\hosts");
        let err = jail.resolve(abs).expect_err("absoluto deve ser bloqueado");
        assert_eq!(
            err.code,
            frederico_tool_registry::ToolErrorCode::JailViolation
        );
    }
}
