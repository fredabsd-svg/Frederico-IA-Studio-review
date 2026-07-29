//! Tipos de memória da Fase 4 (Memória e continuidade).
//!
//! Vivem no `core` porque são compartilhados entre o crate
//! `frederico-memory` (repositório + retriever), a casca Tauri
//! (UI da Etapa 5) e os workers (classificador da Etapa 3).
//!
//! A Etapa 1 da Fase 4 introduz estes tipos. Nenhum campo
//! representa estado em runtime — são tipos de **domínio** que
//! trafegam entre camadas. As invariantes de inserção e
//! recuperação ficam no `frederico-memory` (regra
//! `insert_auto_captured` rejeita `origin = ExternalContent`,
//! `Retriever` filtra escopo + active + !superseded + !expirado).
//!
//! Ver [`docs/architecture/memory-architecture.md`](../../docs/architecture/memory-architecture.md)
//! e os ADRs `0010-0014` para o desenho completo.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

crate::opaque_id!(
    MemoryId,
    "Identificador opaco de uma memória. UUID v4, mesmo padrão dos \
     outros IDs do núcleo."
);

/// Escopo de uma memória (`PROMPT MESTRE` §10.1).
///
/// Escopos **globais** (`Profile`, `Preference`, `Assistant`)
/// atravessam conversas: aparecem em qualquer `RetrievalRequest`
/// independente do `scope_id`. Escopos **específicos**
/// (`Project`, `Client`, `Conversation`, `Document`, `Task`,
/// `Session`) só aparecem quando o contexto do caller casa.
///
/// O `Retriever` materializa a SQL do pré-filtro
/// (`ADR-0011 §1`) com base nesta enum. Mitiga a ameaça **I4**
/// do `security-threat-model.md` ("memória semântica vaza entre
/// projetos") por cláusula `WHERE`, não por peso de score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScopeType {
    /// Perfil do usuário (vale em qualquer conversa). Ex: "O usuário
    /// é brasileiro".
    Profile,
    /// Preferência do usuário (vale em qualquer conversa). Ex:
    /// "O usuário odeia figo".
    Preference,
    /// Característica de um assistente específico (Fase 3 Etapa 6+).
    /// Vale para qualquer conversa que use esse `AssistantId`.
    Assistant,
    /// Específico de um projeto. Só aparece quando o caller
    /// tem o mesmo `ProjectId`.
    Project,
    /// Específico de um cliente. Na v1, derivado de `ProjectId`
    /// (1 cliente por projeto). A Fase 7.x refina.
    Client,
    /// Específico de uma conversa. Aparece só na conversa que
    /// gerou a memória.
    Conversation,
    /// Específico de um documento (referência).
    Document,
    /// Específico de uma tarefa.
    Task,
    /// Específico de uma sessão (período de uso).
    Session,
}

impl MemoryScopeType {
    /// Representação canônica em snake_case (mesma forma usada
    /// na `CHECK` constraint do SQLite). Usado pelo `Retriever`
    /// pra montar a SQL.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MemoryScopeType::Profile => "profile",
            MemoryScopeType::Preference => "preference",
            MemoryScopeType::Assistant => "assistant",
            MemoryScopeType::Project => "project",
            MemoryScopeType::Client => "client",
            MemoryScopeType::Conversation => "conversation",
            MemoryScopeType::Document => "document",
            MemoryScopeType::Task => "task",
            MemoryScopeType::Session => "session",
        }
    }

    /// Escopo é global? Se sim, atravessa conversas (aparece
    /// em qualquer `RetrievalRequest` independente do
    /// `scope_id`). Se não, o `Retriever` exige casamento
    /// exato de `scope_type` + `scope_id`.
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(
            self,
            MemoryScopeType::Profile | MemoryScopeType::Preference | MemoryScopeType::Assistant
        )
    }

    /// Lista canônica dos 9 escopos, na ordem de declaração
    /// (mesma ordem do spec). Útil para iteração em testes e
    /// em geração de fixtures.
    #[must_use]
    pub const fn all() -> &'static [MemoryScopeType] {
        &[
            MemoryScopeType::Profile,
            MemoryScopeType::Preference,
            MemoryScopeType::Assistant,
            MemoryScopeType::Project,
            MemoryScopeType::Client,
            MemoryScopeType::Conversation,
            MemoryScopeType::Document,
            MemoryScopeType::Task,
            MemoryScopeType::Session,
        ]
    }

    /// Quantidade canônica. 9. Mudou, atualizar este contador
    /// **e** a `CHECK` constraint do SQLite no mesmo commit
    /// (regra do `REGRAS §1.13`).
    pub const COUNT: usize = 9;
}

impl FromStr for MemoryScopeType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "profile" => Ok(MemoryScopeType::Profile),
            "preference" => Ok(MemoryScopeType::Preference),
            "assistant" => Ok(MemoryScopeType::Assistant),
            "project" => Ok(MemoryScopeType::Project),
            "client" => Ok(MemoryScopeType::Client),
            "conversation" => Ok(MemoryScopeType::Conversation),
            "document" => Ok(MemoryScopeType::Document),
            "task" => Ok(MemoryScopeType::Task),
            "session" => Ok(MemoryScopeType::Session),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MemoryScopeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tipo da memória (`PROMPT MESTRE` §10.2).
///
/// A regra "tipo `temporary` exige `expires_at`" é enforçada no
/// `MemoryRepo::insert` (não apenas convenção).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Gosto do usuário. Permanente.
    Preference,
    /// Facto consolidado. Permanente.
    Fact,
    /// Decisão arquitetural/técnica. Permanente.
    Decision,
    /// Substitui memória anterior. O `MemoryRepo::insert` com
    /// `type = Correction` chama `mark_superseded` na antiga.
    /// Permanente.
    Correction,
    /// Instrução específica do projeto. Permanente.
    ProjectInstruction,
    /// Contexto do cliente. Permanente.
    ClientContext,
    /// Como o agente fez algo (passos reutilizáveis). Permanente.
    Procedure,
    /// Padrão de entrega (formato, hora, etc). Permanente.
    DeliveryPattern,
    /// Memória com prazo. **Exige `expires_at` não-nulo** no
    /// insert. É o único tipo que pode expirar.
    Temporary,
    /// Resumo de uma conversa encerrada. Permanente.
    ConversationSummary,
    /// Referência a um documento (id, hash). Permanente.
    DocumentReference,
    /// Memória fixada manualmente pelo usuário. Marcada com
    /// `user_pinned = true` no insert.
    UserPinned,
}

impl MemoryType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MemoryType::Preference => "preference",
            MemoryType::Fact => "fact",
            MemoryType::Decision => "decision",
            MemoryType::Correction => "correction",
            MemoryType::ProjectInstruction => "project_instruction",
            MemoryType::ClientContext => "client_context",
            MemoryType::Procedure => "procedure",
            MemoryType::DeliveryPattern => "delivery_pattern",
            MemoryType::Temporary => "temporary",
            MemoryType::ConversationSummary => "conversation_summary",
            MemoryType::DocumentReference => "document_reference",
            MemoryType::UserPinned => "user_pinned",
        }
    }

    /// Esse tipo exige `expires_at` não-nulo?
    #[must_use]
    pub const fn requires_expires_at(self) -> bool {
        matches!(self, MemoryType::Temporary)
    }

    #[must_use]
    pub const fn all() -> &'static [MemoryType] {
        &[
            MemoryType::Preference,
            MemoryType::Fact,
            MemoryType::Decision,
            MemoryType::Correction,
            MemoryType::ProjectInstruction,
            MemoryType::ClientContext,
            MemoryType::Procedure,
            MemoryType::DeliveryPattern,
            MemoryType::Temporary,
            MemoryType::ConversationSummary,
            MemoryType::DocumentReference,
            MemoryType::UserPinned,
        ]
    }

    pub const COUNT: usize = 12;
}

impl FromStr for MemoryType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "preference" => Ok(MemoryType::Preference),
            "fact" => Ok(MemoryType::Fact),
            "decision" => Ok(MemoryType::Decision),
            "correction" => Ok(MemoryType::Correction),
            "project_instruction" => Ok(MemoryType::ProjectInstruction),
            "client_context" => Ok(MemoryType::ClientContext),
            "procedure" => Ok(MemoryType::Procedure),
            "delivery_pattern" => Ok(MemoryType::DeliveryPattern),
            "temporary" => Ok(MemoryType::Temporary),
            "conversation_summary" => Ok(MemoryType::ConversationSummary),
            "document_reference" => Ok(MemoryType::DocumentReference),
            "user_pinned" => Ok(MemoryType::UserPinned),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Procedência da memória (`PROMPT MESTRE` §10.10 + `ADR-0012 §3`).
///
/// A regra "`ExternalContent` não vira memória automática" é
/// enforçada no **tipo** (enum exaustivo) e no `MemoryRepo`
/// (método `insert_auto_captured` rejeita; `insert_user_confirmed`
/// aceita). Mitiga a ameaça **E2** do `security-threat-model.md`
/// (memória como instrução): conteúdo externo só vira memória
/// após confirmação humana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    /// Texto digitado ou colado pelo usuário.
    User,
    /// Texto gerado pelo modelo (consolidação, factos extraídos).
    Assistant,
    /// Conteúdo externo: página web, saída de ferramenta,
    /// documento anexo, etc. **Nunca vira memória automática** —
    /// o `MemoryRepo::insert_auto_captured` rejeita; o
    /// `insert_user_confirmed` aceita (inserção manual via UI).
    ExternalContent,
}

impl MemoryOrigin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            MemoryOrigin::User => "user",
            MemoryOrigin::Assistant => "assistant",
            MemoryOrigin::ExternalContent => "external_content",
        }
    }

    /// É conteúdo externo (exige confirmação humana)?
    #[must_use]
    pub const fn is_external(self) -> bool {
        matches!(self, MemoryOrigin::ExternalContent)
    }
}

impl FromStr for MemoryOrigin {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(MemoryOrigin::User),
            "assistant" => Ok(MemoryOrigin::Assistant),
            "external_content" => Ok(MemoryOrigin::ExternalContent),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MemoryOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tipo da fonte original (string livre, mas com valores
/// canônicos).
///
/// Valores típicos:
/// - `"user_message"` — mensagem do usuário em uma conversa
/// - `"assistant_message"` — mensagem do assistente
/// - `"tool_output:<tool_id>"` — saída de ferramenta
/// - `"document_attachment:<id>"` — anexo de documento
/// - `"manual"` — inserção manual via painel (Etapa 5)
///
/// O classificador ([ADR-0012](../docs/decisions/0012-memory-classifier.md))
/// usa `source_type` pra detectar `ExternalContent` mesmo quando
/// o LLM propõe outra origem: se a proveniência real é externa,
/// o worker **sobrescreve** a origem.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemorySourceType(pub String);

impl MemorySourceType {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Começa com o prefixo? (ex: `starts_with("tool_output:")`).
    #[must_use]
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }
}

impl Default for MemorySourceType {
    fn default() -> Self {
        Self::new("manual")
    }
}

impl fmt::Display for MemorySourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for MemorySourceType {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for MemorySourceType {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Status do embedding de uma memória.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingStatus {
    /// Aguardando cálculo de embedding. Memória existe, mas o
    /// `Retriever` não tem vetor pra ela (cai pra lexical-only).
    Pending,
    /// Embedding calculada e persistida em `memory_embeddings`.
    /// `Retriever` usa semântica se `provider` e `model`
    /// casarem com a config atual.
    Ready,
    /// Tentativa de embedding falhou (timeout, credencial,
    /// parsing). `Retriever` cai pra lexical-only. Worker da
    /// Etapa 2 pode retentar.
    Failed,
}

impl EmbeddingStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EmbeddingStatus::Pending => "pending",
            EmbeddingStatus::Ready => "ready",
            EmbeddingStatus::Failed => "failed",
        }
    }
}

impl FromStr for EmbeddingStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(EmbeddingStatus::Pending),
            "ready" => Ok(EmbeddingStatus::Ready),
            "failed" => Ok(EmbeddingStatus::Failed),
            _ => Err(()),
        }
    }
}

impl fmt::Display for EmbeddingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decomposição do score final de uma memória recuperada
/// (`PROMPT MESTRE` §10.11 — explicabilidade).
///
/// Cada campo é a contribuição **antes do peso** (o `Retriever`
/// soma `lexical_weight * lexical + ...`). O painel de memória
/// da Etapa 5 mostra essa decomposição ao usuário ("essa memória
/// foi recuperada por match lexical + recente + confirmada").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// BM25 normalizado do FTS5. 0.0 se sem overlap textual.
    pub lexical: f32,
    /// Fator de recência (decay exponencial, 0.0..=1.0).
    pub recency: f32,
    /// Cosine similarity (0.0 se embeddings ausentes). Na Etapa
    /// 1, sempre 0.0 (baseline lexical-only).
    pub semantic: f32,
    /// Importância da memória (0.0..=1.0).
    pub importance: f32,
    /// Boost por confirmação do usuário (0.0 se nem pinned
    /// nem confirmed; 1.0+ se pinned).
    pub confirmation: f32,
    /// Passou no pré-filtro de escopo? Em `MemoryHit`, sempre
    /// `true` (a `WHERE` do `Retriever` já exclui quem não
    /// casou). Existe no tipo para explicabilidade do painel
    /// da Etapa 5.
    pub scope_match: bool,
}

impl ScoreBreakdown {
    /// Breakdown zerado — usado quando o `Retriever` ainda não
    /// calculou (ex: testes que só verificam o filtro, não o
    /// score).
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            lexical: 0.0,
            recency: 0.0,
            semantic: 0.0,
            importance: 0.0,
            confirmation: 0.0,
            scope_match: false,
        }
    }
}

/// Memória retornada pelo `Retriever` com score e explicabilidade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    pub record: MemoryRecord,
    /// Score final composto (0.0..=1.0+ — sem teto rígido, mas
    /// valores típicos ficam em 0.0–2.0 com boosts aplicados).
    pub score: f32,
    pub score_breakdown: ScoreBreakdown,
    /// Frase curta explicando por que essa memória foi
    /// recuperada. Mostrada no painel da Etapa 5.
    pub explanation: String,
}

/// Registro persistido de uma memória. Espelha a tabela
/// `memory_records` (migration 0007).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub scope_type: MemoryScopeType,
    pub scope_id: String,
    pub type_: MemoryType,
    pub content: String,
    pub origin: MemoryOrigin,
    pub source_type: MemorySourceType,
    pub source_id: Option<String>,
    /// Saída do classificador, 0.0..=1.0.
    pub confidence: f32,
    /// 0.0..=1.0. Boost no scoring (sinal 4 do `ADR-0011 §2`).
    pub importance: f32,
    pub embedding_status: EmbeddingStatus,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Obrigatório se `type_ == MemoryType::Temporary`. Ignorado
    /// se `user_pinned = true` (pin bypassa expiração).
    pub expires_at: Option<DateTime<Utc>>,
    pub superseded_by: Option<MemoryId>,
    pub superseded_at: Option<DateTime<Utc>>,
    /// Conteúdo externo (`origin = ExternalContent`) entra como
    /// `false` na auto-captura. Só vira `true` após confirmação
    /// humana via painel da Etapa 5.
    pub user_confirmed: bool,
    /// Bypassa `expires_at` (mas não `superseded_by`).
    pub user_pinned: bool,
    /// `false` = soft-deletada (audit trail preservado). A Etapa
    /// 4 introduz `purge_expired` que faz `DELETE` físico.
    pub active: bool,
    /// `true` = aguardando revisão humana (apenas memórias com
    /// `origin = ExternalContent` antes da confirmação).
    pub pending_review: bool,
}

impl MemoryRecord {
    /// Esta memória está visível para o `Retriever`? Passa em
    /// todos os filtros (`active`, `superseded_by`, `expires_at`,
    /// `pending_review`)?
    ///
    /// `now` vem do `Clock` injetado (testável com `FakeClock`).
    #[must_use]
    pub fn is_visible(&self, now: DateTime<Utc>) -> bool {
        if !self.active {
            return false;
        }
        if self.superseded_by.is_some() {
            return false;
        }
        if self.pending_review {
            return false;
        }
        // user_pinned bypassa expires_at, mas não superseded_by.
        if !self.user_pinned {
            if let Some(expires) = self.expires_at {
                if expires <= now {
                    return false;
                }
            }
        }
        true
    }
}

/// Entrada nova de memória — o que o classificador
/// ([`MemoryClassifier`]) propõe ao worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemory {
    pub scope_type: MemoryScopeType,
    pub scope_id: String,
    pub type_: MemoryType,
    pub content: String,
    pub origin: MemoryOrigin,
    pub source_type: MemorySourceType,
    pub source_id: Option<String>,
    /// Saída do classificador, 0.0..=1.0.
    pub confidence: f32,
    /// 0.0..=1.0. Boost no scoring.
    pub importance: f32,
    /// Obrigatório se `type_ == MemoryType::Temporary`.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Saída do classificador (`ADR-0012 §2`). Se `record = None`,
/// nada vira memória — pode acontecer na maioria das conversas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryClassifierOutput {
    pub record: Option<NewMemory>,
    /// Escopo candidato. Se `None`, o worker descarta
    /// (não há onde gravar a memória).
    pub scope: Option<MemoryScopeType>,
    /// Importância 0.0..=1.0. Usada se `record` for `Some`.
    pub importance: f32,
    /// Razão da decisão (audit trail). Não exibida pro
    /// usuário final, mas vai pro log e pro painel de debug
    /// da Etapa 5.
    pub reason: String,
}

/// Mensagem de uma conversa que entra no classificador.
#[derive(Debug, Clone)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    /// Proveniência real. O worker usa isso pra sobrescrever
    /// o `origin` proposto pelo classificador
    /// ([`ADR-0012 §3`](../../docs/decisions/0012-memory-classifier.md)).
    pub source: MemorySourceType,
}

/// Contexto passado ao classificador.
#[derive(Debug, Clone)]
pub struct ClassificationContext {
    pub run_id: String,
    pub conversation_id: String,
    /// Últimas N mensagens (default 6, configurável na Etapa 3).
    pub messages: Vec<ConversationMessage>,
}

/// Request do `Retriever`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalRequest {
    pub scope_type: MemoryScopeType,
    pub scope_id: String,
    /// Texto da query (já sanitizado pelo caller).
    pub query: String,
    /// Teto de memórias. Default 8.
    pub k: usize,
    /// Teto de tokens consumidos pelas memórias no contexto do
    /// modelo. Default 1500.
    pub token_budget: usize,
    /// Desempate por recência (0.0..=1.0). Se dois candidatos
    /// têm score dentro de `recency_epsilon`, o mais recente
    /// vence. Default 0.01.
    pub recency_epsilon: f32,
}

impl Default for RetrievalRequest {
    fn default() -> Self {
        Self {
            scope_type: MemoryScopeType::Project,
            scope_id: String::new(),
            query: String::new(),
            k: 8,
            token_budget: 1500,
            recency_epsilon: 0.01,
        }
    }
}

/// Resultado do `Retriever`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub hits: Vec<MemoryHit>,
    /// `false` se embeddings estavam ausentes/timeout — o
    /// `Retriever` caiu pra lexical-only.
    pub semantic_used: bool,
    pub elapsed_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 28, 12, 0, 0).unwrap()
    }

    fn base_record() -> MemoryRecord {
        MemoryRecord {
            id: MemoryId::new(),
            scope_type: MemoryScopeType::Project,
            scope_id: "proj-1".into(),
            type_: MemoryType::Fact,
            content: "test".into(),
            origin: MemoryOrigin::User,
            source_type: MemorySourceType::new("user_message"),
            source_id: None,
            confidence: 0.8,
            importance: 0.5,
            embedding_status: EmbeddingStatus::Pending,
            embedding_provider: None,
            embedding_model: None,
            embedding_dimensions: None,
            created_at: fixed_now(),
            updated_at: fixed_now(),
            last_used_at: None,
            expires_at: None,
            superseded_by: None,
            superseded_at: None,
            user_confirmed: false,
            user_pinned: false,
            active: true,
            pending_review: false,
        }
    }

    #[test]
    fn scope_type_count_matches_all() {
        assert_eq!(MemoryScopeType::all().len(), MemoryScopeType::COUNT);
        assert_eq!(MemoryScopeType::COUNT, 9);
    }

    #[test]
    fn scope_type_roundtrip() {
        for s in MemoryScopeType::all() {
            assert_eq!(s.as_str().parse::<MemoryScopeType>().unwrap(), *s);
        }
    }

    #[test]
    fn scope_type_globals() {
        assert!(MemoryScopeType::Profile.is_global());
        assert!(MemoryScopeType::Preference.is_global());
        assert!(MemoryScopeType::Assistant.is_global());
        assert!(!MemoryScopeType::Project.is_global());
        assert!(!MemoryScopeType::Client.is_global());
        assert!(!MemoryScopeType::Conversation.is_global());
    }

    #[test]
    fn memory_type_count_matches_all() {
        assert_eq!(MemoryType::all().len(), MemoryType::COUNT);
        assert_eq!(MemoryType::COUNT, 12);
    }

    #[test]
    fn memory_type_roundtrip() {
        for t in MemoryType::all() {
            assert_eq!(t.as_str().parse::<MemoryType>().unwrap(), *t);
        }
    }

    #[test]
    fn only_temporary_requires_expires_at() {
        for t in MemoryType::all() {
            assert_eq!(t.requires_expires_at(), *t == MemoryType::Temporary);
        }
    }

    #[test]
    fn origin_roundtrip() {
        for s in ["user", "assistant", "external_content"] {
            let o = s.parse::<MemoryOrigin>().unwrap();
            assert_eq!(o.as_str(), s);
        }
    }

    #[test]
    fn only_external_content_is_external() {
        assert!(MemoryOrigin::ExternalContent.is_external());
        assert!(!MemoryOrigin::User.is_external());
        assert!(!MemoryOrigin::Assistant.is_external());
    }

    #[test]
    fn embedding_status_roundtrip() {
        for s in ["pending", "ready", "failed"] {
            let e = s.parse::<EmbeddingStatus>().unwrap();
            assert_eq!(e.as_str(), s);
        }
    }

    #[test]
    fn visibility_active_required() {
        let mut r = base_record();
        r.active = false;
        assert!(!r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_superseded_hidden() {
        let mut r = base_record();
        r.superseded_by = Some(MemoryId::new());
        assert!(!r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_pending_review_hidden() {
        let mut r = base_record();
        r.pending_review = true;
        assert!(!r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_expired_hidden() {
        let mut r = base_record();
        r.expires_at = Some(Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap());
        assert!(!r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_future_expiry_visible() {
        let mut r = base_record();
        r.expires_at = Some(Utc.with_ymd_and_hms(2026, 8, 28, 0, 0, 0).unwrap());
        assert!(r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_pinned_bypasses_expires_at() {
        let mut r = base_record();
        r.expires_at = Some(Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap());
        r.user_pinned = true;
        assert!(r.is_visible(fixed_now()));
    }

    #[test]
    fn visibility_pinned_does_not_bypass_superseded() {
        let mut r = base_record();
        r.user_pinned = true;
        r.superseded_by = Some(MemoryId::new());
        assert!(!r.is_visible(fixed_now()));
    }

    #[test]
    fn score_breakdown_zero() {
        let z = ScoreBreakdown::zero();
        assert_eq!(z.lexical, 0.0);
        assert_eq!(z.recency, 0.0);
        assert_eq!(z.semantic, 0.0);
        assert_eq!(z.importance, 0.0);
        assert_eq!(z.confirmation, 0.0);
        assert!(!z.scope_match);
    }

    #[test]
    fn memory_id_is_unique() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        assert_ne!(a, b);
    }
}
