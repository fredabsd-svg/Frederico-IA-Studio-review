<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-28
Fase correspondente: 4 (Etapa 1)
-->

# Arquitetura de Memória e Continuidade

> Especificação aprofundada na Etapa 1 da Fase 4. Sai do estado de
> stub da Fase 0 e ganha detalhes suficientes para a Etapa 2
> (retrieval híbrido) e a Etapa 3 (classificador) escreverem código
> contra este documento sem ambiguidade.

## Decisões tomadas

- **Recuperação híbrida** combinando escopo + recência + FTS5 +
  similaridade semântica + importância + confirmação do usuário
  (`PROMPT MESTRE` §10.6).
- **Escopo é pré-filtro, não peso** ([ADR-0011](../decisions/0011-scoring-structure.md)).
  Memória de outro projeto **não é candidata**, independente de
  score. Isso mitiga a ameaça **I4** do
  [`security-threat-model.md`](./security-threat-model.md).
- **Embeddings por provedor configurado por padrão** ([ADR-0010](../decisions/0010-embedding-provider-default.md));
  modelo local ONNX apenas sob demanda, **nunca** na inicialização
  do app (`PROMPT MESTRE` §10.13).
- **Recuperação semântica com prazo máximo de 2 s**; estourado, a
  execução segue sem ela e o fato é registrado — memória nunca
  trava uma resposta (`PROMPT MESTRE` §10.13).
- **Busca lexical FTS5 funciona mesmo sem embeddings** disponíveis
  (`PROMPT MESTRE` §10.13).
- **Mensagens recentes da conversa têm prioridade** sobre memória
  semântica (`PROMPT MESTRE` §10.4) — regra de desempate
  estrutural no `Retriever`, não peso no scoring.
- **Zero memórias é resposta válida** — não se recupera conteúdo
  só para preencher espaço (`PROMPT MESTRE` §10.7).
- **Memória é dado, não instrução** — não pode alterar system
  prompt, permissões, identidade do agente (`PROMPT MESTRE` §10.10).
  Mitigação reforçada por **procedência obrigatória**
  ([ADR-0012](../decisions/0012-memory-classifier.md) §3): conteúdo
  externo não vira memória sem confirmação humana.
- **Classificador de candidatos é LLM-based, fora do caminho
  crítico, falsificável** ([ADR-0012](../decisions/0012-memory-classifier.md)).
- **Reindexação de embeddings em background, com progresso
  visível, app usável** ([ADR-0013](../decisions/0013-embedding-reindex.md)).
- **Expiração + `supersededBy` + coleta preguiçosa na leitura**
  ([ADR-0014](../decisions/0014-expiration-supersededby-gc.md)) —
  sem job tokio em background; GC é a `WHERE` do `Retriever`.

## Escopos e tipos

### Escopos (`PROMPT MESTRE` §10.1)

9 escopos:

| Escopo | Visibilidade | Exemplo |
|--------|--------------|---------|
| `profile` | global (vale em qualquer conversa) | "O usuário é brasileiro" |
| `preference` | global | "O usuário odeia figo" |
| `assistant` | global (vinculado a um `AssistantId`) | "O assistente X fala português do Brasil" |
| `project` | específico (1 `ProjectId`) | "Este projeto usa Postgres" |
| `client` | específico (1 `ClientId`) | "Cliente Acme prefere relatório curto" |
| `conversation` | específico (1 `ConversationId`) | "Nessa conversa o usuário perguntou sobre X" |
| `document` | específico (1 `DocumentId`) | "Documento Y cita que ..." |
| `task` | específico (1 `TaskId`) | "Tarefa atual: refatorar módulo de auth" |
| `session` | específico (1 `SessionId`) | "Sessão de hoje: configuração do OpenRouter" |

**Globais** (visíveis em qualquer escopo específico):
`profile`, `preference`, `assistant`. O `Retriever` aplica:

```sql
WHERE (scope_type IN ('profile', 'preference', 'assistant')
       OR (scope_type = ?scope_type AND scope_id = ?scope_id))
```

**Específicos**: `project`, `client`, `conversation`, `document`,
`task`, `session`. Memórias desses escopos **só** aparecem quando
o contexto do `Retriever` casa.

`client` é um conceito do modo `developer` (Fase 7) — na v1, o
`ClientId` é derivado de `ProjectId` (1 cliente por projeto). A
Fase 7.x refina.

### Tipos (`PROMPT MESTRE` §10.2)

12 tipos:

| Tipo | Significado | Pode expirar? |
|------|-------------|----------------|
| `preference` | gosto do usuário | não |
| `fact` | facto consolidado | não |
| `decision` | decisão arquitetural/técnica | não |
| `correction` | substitui memória anterior (sempre com `superseded_by` na antiga) | não |
| `project_instruction` | instrução específica do projeto | não |
| `client_context` | contexto do cliente | não |
| `procedure` | como o agente fez algo (passos reutilizáveis) | não |
| `delivery_pattern` | padrão de entrega (formato, hora, etc) | não |
| `temporary` | memória com prazo — **exige `expires_at`** | sim |
| `conversation_summary` | resumo de uma conversa encerrada | não |
| `document_reference` | referência a um documento (com id, hash) | não |
| `user_pinned` | memória fixada manualmente pelo usuário (`user_pinned = true`) | não (pin bypassa `expires_at`) |

A regra "`temporary` exige `expires_at`" é enforçada no
`MemoryRepo::insert` (não apenas convenção).

## Contrato

### `MemoryRecord` (persistido em `memory_records`)

Campos:

```rust
pub struct MemoryRecord {
    pub id: MemoryId,                       // UUID v4
    pub scope_type: MemoryScopeType,        // enum: 9 valores
    pub scope_id: String,                   // string (UUID serializado ou string livre)
    pub type_: MemoryType,                  // enum: 12 valores
    pub content: String,                    // texto da memória (1+ parágrafos)
    pub origin: MemoryOrigin,               // enum: 3 valores — User | Assistant | ExternalContent
    pub source_type: String,                // ex: "user_message", "assistant_message",
                                           //     "tool_output:<tool_id>", "document_attachment:<id>"
    pub source_id: Option<String>,          // ex: MessageId, ToolId, DocumentId
    pub confidence: f32,                    // 0.0..=1.0 — saída do classificador
    pub importance: f32,                    // 0.0..=1.0 — boost no scoring
    pub embedding_status: EmbeddingStatus,  // enum: Pending | Ready | Failed
    pub embedding_provider: Option<String>, // ex: "openrouter"
    pub embedding_model: Option<String>,    // ex: "openai/text-embedding-3-small"
    pub embedding_dimensions: Option<u32>,  // 1536, 3072, etc
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>, // atualizado pelo Retriever em cada hit
    pub expires_at: Option<DateTime<Utc>>,  // obrigatório se type_ = Temporary
    pub superseded_by: Option<MemoryId>,
    pub superseded_at: Option<DateTime<Utc>>,
    pub user_confirmed: bool,               // false por padrão; true após confirmação explícita
    pub user_pinned: bool,                  // bypassa expires_at (não superseded_by)
    pub active: bool,                       // false = "deletada" (soft delete pra audit)
    pub pending_review: bool,               // true se origin = ExternalContent ainda não confirmado
}
```

A regra **I4** do `security-threat-model.md` é enforçada pela SQL
do `Retriever` (`WHERE scope_type IN (...) OR (scope_type = ? AND
scope_id = ?)`). É **impossível** que uma memória de outro escopo
seja recuperada, independente de qualquer combinação de sinais
de score.

### `MemoryOrigin` (procedência)

```rust
pub enum MemoryOrigin {
    User,             // texto digitado/colado pelo usuário
    Assistant,        // texto gerado pelo modelo
    ExternalContent,  // página web, saída de ferramenta, documento anexo
                      // NUNCA vira memória sem user_confirmed = true
}
```

A regra "ExternalContent exige confirmação" é enforçada **no
tipo** (enum exaustivo) e **na lógica do `MemoryRepo`** (ver
`insert_auto_captured` vs. `insert_user_confirmed` no
[`docs/modules/memory.md`](../modules/memory.md)).

O classificador ([ADR-0012](../decisions/0012-memory-classifier.md))
pode propor `origin` na saída dele, mas o **worker** sobrescreve
com base na proveniência real: se a mensagem veio com
`role = "tool"` (output de ferramenta) ou é um anexo de
documento, o `origin` final é `ExternalContent` mesmo que o LLM
tenha proposto `User`.

### `MemoryHit` (saída do `Retriever`)

```rust
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub score: f32,                          // score final composto
    pub score_breakdown: ScoreBreakdown,     // explicabilidade (§10.11)
    pub explanation: String,                 // frase curta ("match lexical + recente + confirmado")
}

pub struct ScoreBreakdown {
    pub lexical: f32,        // BM25 normalizado
    pub recency: f32,        // decay exponencial
    pub semantic: f32,       // cosine (0.0 se provider ausente)
    pub importance: f32,     // importance * weight
    pub confirmation: f32,   // boost por user_pinned/user_confirmed
    pub scope_match: bool,   // true se passou no pré-filtro (sempre true em MemoryHit)
}
```

`scope_match` é sempre `true` em `MemoryHit` (a `WHERE` do
`Retriever` já exclui quem não casou). Existe no tipo pra
explicabilidade do painel da Etapa 5.

### `RetrievalRequest` / `RetrievalResult`

```rust
pub struct RetrievalRequest {
    pub scope_type: MemoryScopeType,         // escopo do caller (ex: project)
    pub scope_id: String,                    // id do escopo (ex: ProjectId)
    pub query: String,                       // texto da query (já sanitizado)
    pub k: usize,                            // teto de memórias (default 8)
    pub token_budget: usize,                 // teto de tokens consumidos
    pub recency_epsilon: f32,                // desempate por recência (default 0.01)
}

pub struct RetrievalResult {
    pub hits: Vec<MemoryHit>,
    pub semantic_used: bool,                 // false se provider ausente/timeout
    pub elapsed_ms: u64,                     // tempo total do retrieval
}
```

## Pipeline de classificação (Etapa 3)

```text
1. Run termina (Completed/Failed/Cancelled/...).
2. ChatOrchestrator enfileira MemoryExtractionJob { run_id,
   conversation_id, last_messages } num canal mpsc.
3. Worker pega o job, monta ClassificationContext com as últimas
   N=6 mensagens + escopo candidato.
4. MemoryClassifier::classify retorna MemoryClassifierOutput.
5. Se output.record.is_some():
   a. Worker valida origin com base na proveniência real.
   b. Se origin = ExternalContent: insert com
      user_confirmed=false, pending_review=true. UI da Etapa 5
      mostra como pendente.
   c. Se origin ∈ {User, Assistant}: insert normal.
6. Memória entra em memory_records com embedding_status = Pending.
7. Embedding worker (Etapa 2) embarca a memória e atualiza pra Ready.
```

A janela "memória disponível no próximo Run" é < 1s em prática.
Aceitável e determinístico.

## Pipeline de reindexação (Etapa 2/3)

```text
1. Usuário troca embedding_provider ou embedding_model no painel
   (Etapa 5) OU inicia o app pela primeira vez com embeddings
   habilitados.
2. MemoryConfigService detecta a mudança, enfileira ReindexJob.
3. ReindexWorker (tokio task em background) itera em batches
   de 1000, chama EmbeddingProvider::embed, atualiza
   memory_embeddings.
4. Retriever continua funcionando: filtra por
   (provider = current AND model = current) — memórias com
   embedding antigo caem pra lexical-only.
5. Progresso fica em embedding_reindex_jobs (lido pelo painel).
```

Detalhe completo em [ADR-0013](../decisions/0013-embedding-reindex.md).

## Não-objetivos

- Memória como busca puramente vetorial.
- Auto-salvamento de toda mensagem como memória.
- Memória alterar system prompt, permissões ou identidade do
  agente.
- Sincronização de memória entre dispositivos na v1.
- Memória como feature exposta ao usuário final como "base de
  conhecimento editável" — o usuário gerencia via UI, mas não é
  o caso de uso principal.
- **Job tokio periódico para GC** ([ADR-0014](../decisions/0014-expiration-supersededby-gc.md)):
  coleta é on-read + `purge_expired` explícito na inicialização.
- **Modelo local ONNX na inicialização do app** ([ADR-0010](../decisions/0010-embedding-provider-default.md)):
  embeddings sempre remotos na v1; modelo local só em Fase 4.x.y
  com opt-in explícito.
- **Reindexação bloqueante**: reindexação é sempre em background
  ([ADR-0013](../decisions/0013-embedding-reindex.md)).

## Estrutura de crates

A Fase 4 Etapa 1 introduz:

- `frederico-memory` (novo, sem dependência de plataforma,
  `unsafe_code = "deny"`). Mora a trait `EmbeddingProvider`
  ([ADR-0010](../decisions/0010-embedding-provider-default.md)),
  o `MemoryRepo` (CRUD + lexical FTS5), a `MemoryClassifier`
  trait ([ADR-0012](../decisions/0012-memory-classifier.md)) e o
  `Retriever` (a partir da Etapa 2).
- `frederico-core` ganha tipos: `MemoryRecord`, `MemoryScopeType`,
  `MemoryScopeId`, `MemoryType`, `MemoryOrigin`, `MemorySourceType`,
  `MemoryId`, `MemoryHit`, `ScoreBreakdown`, `RetrievalRequest`,
  `RetrievalResult`, `MemoryClassifierOutput`. Tudo no
  `crates/core/src/memory.rs` (módulo novo).

A Etapa 2 adiciona `OpenRouterEmbeddingAdapter`,
`OpenAiDirectEmbeddingAdapter` e `Retriever::retrieve` no
`frederico-memory`. A Etapa 3 adiciona `LlmMemoryClassifier` e o
worker de classificação. A Etapa 5 adiciona o painel React.

## Decisões (ADRs da Fase 4 Etapa 1)

- [ADR-0010](../decisions/0010-embedding-provider-default.md) —
  provider de embeddings default (OpenRouter, pluggable, sem
  modelo local na inicialização).
- [ADR-0011](../decisions/0011-scoring-structure.md) — estrutura
  do scoring (escopo como pré-filtro, sinais em ordem fixa,
  pesos em `scoring.toml`).
- [ADR-0012](../decisions/0012-memory-classifier.md) — classificador
  de memórias (LLM-based, fora do caminho crítico, falsificável,
  com procedência obrigatória).
- [ADR-0013](../decisions/0013-embedding-reindex.md) —
  reindexação de embeddings em background, com progresso visível.
- [ADR-0014](../decisions/0014-expiration-supersededby-gc.md) —
  expiração, `supersededBy` e GC preguiçoso.

## Referências

- `PROMPT MESTRE` §10 (memória), §10.4 (recência > semântica),
  §10.6 (scoring), §10.7 (zero memórias é válido), §10.8
  (`supersededBy`), §10.9 (classificação), §10.10 (memória é
  dado), §10.11 (explicabilidade), §10.12 (avaliação),
  §10.13 (embeddings e indexação).
- [`memory-evaluation-plan.md`](./memory-evaluation-plan.md) —
  conjunto de avaliação, métricas, alvos, gate de CI.
- [`security-threat-model.md`](./security-threat-model.md) — I4
  (vazamento entre projetos), E2 (memória como instrução), E3
  (documento anexo com payload).
- [`testing-strategy.md`](./testing-strategy.md) — relógio virtual,
  falsos em nível de transporte, golden files.
- [`docs/development-roadmap.md`](./development-roadmap.md) — Fase 4
  no roadmap global.
