//! `JailResolver` trait + `JailResolverError` + `StaticJailResolver`.
//!
//! Ver ADR-0022 §D2 para a motivação e o contexto.
//!
//! ## Posição no grafo de dependências
//!
//! O trait mora em `frederico-tool-registry` (não em `frederico-app`,
//! como a versão original do ADR-0022 §D2 dizia) por uma razão
//! prática: a `FilesReadTool` precisa de uma referência ao trait,
//! e o `frederico-tool-registry` não pode depender do `frederico-app`
//! (seria ciclo — `frederico-app` já depende de `tool-registry`).
//! A `FileSystemJailResolver` (a implementação concreta default
//! usada em produção) continua em `frederico-app` — apenas a
//! abstração foi promovida para o toolkit. ADR-0022 §D2 foi
//! revisado com esta nota para refletir o ajuste.
//!
//! ## `StaticJailResolver`
//!
//! Implementação trivial que devolve sempre o mesmo `Jail`.
//! Usado por:
//!
//! - **Testes do `frederico-tool-registry`** que não querem se
//!   preocupar com o ciclo de vida do diretório.
//! - **Testes do `frederico-execution-engine`** que constroem
//!   o `RunExecutor` com um jail fixo por execução de teste.
//! - **Fase de transição** (se houver) entre a Etapa 1 da Fase
//!   de Ligação e a Etapa 7 (modo desenvolvedor, que substitui
//!   pelo `SecurityJailResolver`).
//!
//! Não é fallback de produção. A casca Tauri usa
//! `FileSystemJailResolver` (do `frederico-app`) em runtime —
//! ver ADR-0022 §D2.

use std::sync::Arc;

use frederico_core::ConversationId;
use thiserror::Error;

use crate::workspace::Jail;

/// Erro do `JailResolver`.
///
/// **Erro duro** (sem fallback): quando a resolução falha, o
/// `Jail` **não** é construído e a falha propaga. Degradação
/// silenciosa num caminho de isolamento é o tipo de bug que a
/// Fase de Ligação existe para eliminar (ver ADR-0022 §D2).
#[derive(Debug, Error)]
pub enum JailResolverError {
    /// Falha ao preparar o workspace da conversa (mkdir falhou,
    /// jail não pôde ser construído, etc.). Inclui o
    /// `conversation_id` para o caller montar a mensagem PT-BR.
    #[error("falha ao resolver jail para conversa {conversation_id}: {source}")]
    Resolve {
        conversation_id: ConversationId,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// `Result` padrão do `JailResolver`.
pub type JailResolverResult<T> = Result<T, JailResolverError>;

/// `JailResolver` é o ponto de entrada para o workspace per-conversa.
///
/// Cada `ConversationId` tem o seu próprio jail. O contrato é
/// estável: a Etapa 7 (modo desenvolvedor) substitui as
/// implementações concretas por `SecurityJailResolver` via
/// `frederico-security` (Job Objects + AllowVolumeAccess) sem
/// mudar o `ChatOrchestrator`, o `RunExecutor` nem a
/// `FilesReadTool` — a troca é drop-in.
pub trait JailResolver: Send + Sync {
    /// Resolve o jail para a conversa dada. Falha com erro duro
    /// (sem fallback) se a preparação do workspace não for possível.
    fn resolve(&self, conversation_id: &ConversationId) -> JailResolverResult<Jail>;
}

/// `JailResolver` que devolve sempre o mesmo `Jail`.
///
/// Usado em testes e em fase de transição. **Não** é o resolvedor
/// de produção — a casca usa `FileSystemJailResolver` (do
/// `frederico-app`) que cria um diretório por conversa sob
/// `<data_local_dir>/workspaces/`.
#[derive(Debug, Clone)]
pub struct StaticJailResolver {
    jail: Jail,
}

impl StaticJailResolver {
    /// Constrói o resolvedor estático em torno de um `Jail` já
    /// criado.
    #[must_use]
    pub fn new(jail: Jail) -> Self {
        Self { jail }
    }

    /// `Jail` subjacente. Útil para asserts em testes.
    #[must_use]
    pub fn jail(&self) -> &Jail {
        &self.jail
    }
}

impl JailResolver for StaticJailResolver {
    fn resolve(&self, _conversation_id: &ConversationId) -> JailResolverResult<Jail> {
        Ok(self.jail.clone())
    }
}

/// Helper para type-erased boxing (uso em `Arc<dyn JailResolver>`).
///
/// `Arc::new(StaticJailResolver::new(jail))` funciona, mas em
/// testes essa forma é mais ergonômica:
/// `static_jail_resolver(jail)` onde o caller não precisa
/// lembrar do `Arc::new`.
#[must_use]
pub fn static_jail_resolver(jail: Jail) -> Arc<dyn JailResolver> {
    Arc::new(StaticJailResolver::new(jail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Tempdir(PathBuf);

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    impl Tempdir {
        fn new() -> Self {
            let base = std::env::temp_dir();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let unique = format!(
                "frederico-tool-registry-jr-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                n,
            );
            let dir = base.join(unique);
            fs::create_dir_all(&dir).unwrap();
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
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_conversation_id() -> ConversationId {
        ConversationId(uuid::Uuid::from_bytes([1u8; 16]))
    }

    #[test]
    fn static_resolver_always_returns_same_jail() {
        let dir = Tempdir::new();
        let jail = Jail::new(&dir).unwrap();
        let resolver = StaticJailResolver::new(jail.clone());

        let cid_a = sample_conversation_id();
        let cid_b = ConversationId(uuid::Uuid::from_bytes([2u8; 16]));

        let j1 = resolver.resolve(&cid_a).expect("resolve cid_a");
        let j2 = resolver.resolve(&cid_b).expect("resolve cid_b");

        // Mesmo jail independente do conversation_id (é "estático").
        assert_eq!(
            j1.root().canonicalize().unwrap(),
            j2.root().canonicalize().unwrap()
        );
        assert_eq!(
            j1.root().canonicalize().unwrap(),
            jail.root().canonicalize().unwrap()
        );
    }

    #[test]
    fn static_jail_resolver_helper_returns_arc() {
        let dir = Tempdir::new();
        let jail = Jail::new(&dir).unwrap();
        let resolver: Arc<dyn JailResolver> = static_jail_resolver(jail.clone());

        let cid = sample_conversation_id();
        let j = resolver.resolve(&cid).expect("resolve via Arc");
        assert_eq!(
            j.root().canonicalize().unwrap(),
            jail.root().canonicalize().unwrap()
        );
    }
}
