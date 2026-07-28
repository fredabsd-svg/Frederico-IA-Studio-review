//! `frederico-memory` — Memória e Continuidade (Fase 4).
//!
//! A Etapa 1 entrega a **fundação**: o crate, o `MemoryRepo` com
//! busca lexical FTS5 (funciona sem embeddings — `PROMPT MESTRE`
//! §10.13), as traits `EmbeddingProvider` (com `NoopEmbeddingAdapter`
//! para o caminho lexical-only) e `MemoryClassifier` (com
//! `NoopMemoryClassifier` para a Etapa 1), o `Retriever` lexical
//! e o runner de avaliação que mede o baseline contra o gold-set.
//!
//! Etapas seguintes (no mesmo crate, sem novo `Cargo.toml`):
//!
//! - **Etapa 2 — Retrieval híbrido:** adapter `OpenRouterEmbeddingAdapter`
//!   (OpenAI-compat, default OpenRouter — `ADR-0010`); o `Retriever`
//!   ganha os sinais 2-5 do `ADR-0011` (recência, semântica,
//!   importância, confirmação) e o timeout 2s. Tem que **superar
//!   o baseline da Etapa 1** — sem isso, é bug.
//! - **Etapa 3 — Classificação automática:** `LlmMemoryClassifier`
//!   com prompt restrito e output estruturado (`ADR-0012`),
//!   falsificável pelo fake provider da Fase 2 (`ADR-0008`).
//!   Worker pós-resposta (fora do caminho crítico).
//! - **Etapa 4 — Correções, expiração, retomada:** UI
//!   "corrija para X" → `mark_superseded`; `purge_expired`
//!   na inicialização; `user_pinned` bypassa `expires_at`
//!   mas não `superseded_by` (`ADR-0014`).
//! - **Etapa 5 — UI do painel de memória:** `services/memory.ts`
//!   + `MemoryPanel.tsx` com `ScoreBreakdown` visível
//!     (explicabilidade `PROMPT MESTRE` §10.11).
//! - **Etapa 6 — Gold-set expandido + gate de CI:** 17 cenários
//!   do [`memory-evaluation-plan.md`](../../docs/architecture/memory-evaluation-plan.md),
//!   gate formal com alvos (precisão ≥ 0.70, p99 ≤ 2s, vazamento
//!   cruzado = 0).
//!
//! O crate é **puro**: `unsafe_code = "forbid"`, sem `tauri` /
//! `windows` / `winapi`, sem dependência de plataforma
//! (verificado por `scripts/check-core-purity.ps1` — `ADR-0003`).
//! Depende de `frederico-core` (tipos), `frederico-storage`
//! (SQLite + `Database`) e `frederico-security` (trait `Clock` —
//! `ADR-0014 §1`; mover para o `core` é trabalho de empacotamento
//! futuro, não bloqueia a Etapa 1).
//!
//! O escopo do filtro é cláusula `WHERE` (pré-filtro), não peso
//! de score — ver `ADR-0011 §1`. Memória de outro projeto
//! **não é candidata**, independente de qualquer combinação
//! de sinais. Mitiga a ameaça **I4** do
//! [`security-threat-model.md`](../../docs/architecture/security-threat-model.md).
//!
//! Memória é **dado, não instrução** (`PROMPT MESTRE` §10.10 +
//! `ADR-0012 §3`). Conteúdo externo nunca vira memória
//! automática — só com `user_confirmed = true`. Mitiga a
//! ameaça **E2**.

#![deny(missing_docs)]

pub mod classifier;
pub mod config;
pub mod embedding;
pub mod error;
pub mod memory_repo;
pub mod retriever;
pub mod sanitize;

pub use classifier::{MemoryClassifier, NoopMemoryClassifier};
pub use config::{EvalGate, ScoringWeights};
pub use embedding::{EmbeddingError, EmbeddingProvider, NoopEmbeddingAdapter};
pub use error::MemoryError;
pub use memory_repo::{MemoryRepo, NewMemoryInput};
pub use retriever::{LexicalRetriever, Retriever};
