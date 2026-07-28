//! Erros do subsistema de memória.

use std::path::PathBuf;
use thiserror::Error;

/// Erro genérico do `frederico-memory`. Cobre validação de
/// invariantes (regra do `ADR-0012 §3`: `ExternalContent` exige
/// confirmação), erros de SQL, e erros de parser do gold-set.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Falha ao abrir o banco SQLite.
    #[error("falha ao abrir o banco SQLite em {path}: {source}")]
    Open {
        /// Caminho do banco SQLite que falhou ao abrir.
        path: PathBuf,
        /// Erro original do `sqlx`.
        #[source]
        source: sqlx::Error,
    },

    /// Query falhou.
    #[error("query falhou: {0}")]
    Query(#[from] sqlx::Error),

    /// Migração falhou.
    #[error("falha ao rodar migração: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Memória com `type = Temporary` foi inserida sem
    /// `expires_at`. Regra do `memory-architecture.md` §"Tipos".
    #[error("memória do tipo 'temporary' exige expires_at não-nulo")]
    TemporaryRequiresExpiresAt,

    /// Memória com `type = Temporary` foi inserida com
    /// `expires_at` no passado. Sem sentido — já nasceu
    /// expirada.
    #[error("memória 'temporary' com expires_at no passado (não faz sentido inserir já expirada)")]
    TemporaryExpiresAtInPast,

    /// `origin = ExternalContent` foi passada para
    /// `insert_auto_captured`. Regra do `ADR-0012 §3`:
    /// conteúdo externo nunca vira memória automática. Use
    /// `insert_user_confirmed` após confirmação humana.
    #[error("conteúdo externo (origin = ExternalContent) não pode ser auto-capturado; use insert_user_confirmed após confirmação")]
    ExternalContentAutoCaptured,

    /// `expires_at` no passado. `is_visible` devolve `false` mesmo
    /// sem filtro explícito — mas rejeitar no insert evita lixo.
    #[error("expires_at deve ser no futuro")]
    ExpiresAtInPast,

    /// `confidence` ou `importance` fora de [0.0, 1.0].
    #[error("campo {field} fora da faixa [0.0, 1.0]: {value}")]
    OutOfRange {
        /// Nome do campo fora de faixa (`"confidence"` ou
        /// `"importance"`).
        field: &'static str,
        /// Valor recebido.
        value: f32,
    },

    /// `scope_id` vazio para escopo específico. Escopos
    /// globais (`Profile`, `Preference`, `Assistant`) aceitam
    /// `scope_id` vazio (a spec define o comportamento).
    #[error("scope_id vazio para escopo {0} (que não é global)")]
    ScopeIdRequiredForSpecificScope(frederico_core::MemoryScopeType),

    /// Parser de `gold_set.jsonl` falhou.
    #[error("falha ao parsear gold-set: {0}")]
    GoldSetParse(String),

    /// Provider de embedding devolveu `Unavailable` quando o
    /// `Retriever` exigia semântica. Erro de configuração —
    /// o caller deveria ter passado `NoopEmbeddingAdapter`
    /// explicitamente.
    #[error("embedding provider indisponível e semântica era exigida: {0}")]
    EmbeddingRequired(String),
}

/// Atalho de resultado.
pub type MemoryResult<T> = Result<T, MemoryError>;
