<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-04
Fase correspondente: 4 (Etapas 1, 2, 3, 4, 5 e 6) + Fase de Ligação (Etapa 3)
-->

# `frederico-memory`

Memória e continuidade do Frederico IA Studio (Fase 4,
Etapas 1, 2, 3, 4, 5 e 6). Crate do núcleo (sem dependência de plataforma)
que entrega o **retrieval híbrido** (lexical FTS5 + cosine
semântica + recência + importância + confirmação) com
fallback pro caminho lexical puro quando não há embeddings,
a **classificação automática pós-resposta** (LLM-based,
falsificável, fora do caminho crítico), o **fluxo de
correção, expiração e retomada** (regra do `ADR-0014`),
a **UI do painel de memória** com `ScoreBreakdown` visível
(explicabilidade `PROMPT MESTRE` §10.11), e o **gate de CI**
com gold-set expandido (21 cenários cobrindo as 17
categorias do plano de avaliação).

## 1. O que este módulo faz

Gerencia o ciclo de vida de memórias de longo prazo: criação
(classificador ou captura manual), recuperação (lexical-only
na Etapa 1; híbrida na Etapa 2), correção (`supersededBy`),
expiração (`temporary` com `expires_at`), e persistência
(SQLite via `frederico-storage`).

A Etapa 1 entrega a **fundação** — a infra que as Etapas 2-6
constroem em cima:

- Schema (`memory_records` + `memory_fts` + `memory_embeddings`
  + `embedding_reindex_jobs`) na migration 0007.
- `MemoryRepo` com CRUD + busca lexical FTS5 + invariantes de
  procedência (`insert_auto_captured` rejeita `ExternalContent`).
- `EmbeddingProvider` trait + `NoopEmbeddingAdapter` (sempre
  devolve `Unavailable` — força o caminho lexical).
- `MemoryClassifier` trait + `NoopMemoryClassifier` (sempre
  devolve `None` — classificador real é Etapa 3).
- `Retriever` trait + `LexicalRetriever` (implementação da
  Etapa 1; híbrida na Etapa 2).
- Gold-set baseline (`tests/fixtures/gold_set.jsonl`) com 10
  cenários + runner de avaliação (`tests/evaluation.rs`).
- Configuração versionada (`config/scoring.toml` e
  `config/eval.toml`) — pesos numéricos e alvos do gate.

A Etapa 3 entrega a **classificação automática** — LLM com
prompt restrito e output estruturado, rodando **pós-resposta**
via worker `MemoryExtractor` (canal mpsc, `tokio::spawn` em
background, fora do caminho crítico), com sobrescrita de
`origin` baseada em proveniência real (mitiga E2):

- `CompletionProvider` trait + `NoopCompletionProvider`
  (porta fina pro `provider-engine` sem dependência cíclica).
- `LlmMemoryClassifier` (LLM-based, `openai/gpt-4o-mini` via
  OpenRouter, prompt restrito, parse de JSON, validação de
  scope/confidence/importance, threshold 0.6 default, cota
  5/min via `Mutex<Vec<Instant>>`).
- `MemoryExtractionJob` (run_id, conversation_id, messages,
  finished_at) + `MemoryExtractor` + `MemoryExtractorHandle`
  (mesma estratégia do `EmbeddingWorker` — sem
  `tokio::time::interval`, ADR-0014 §1).
- `determine_real_origin` (helper que sobrescreve o `origin`
  proposto pelo LLM com base na proveniência real das
  mensagens: `tool_output:` ou `document_attachment:` →
  `ExternalContent`; mensagem `assistant`/`tool` → `Assistant`;
  senão, o que o LLM propôs).

A Etapa 4 entrega o **fluxo de correção, expiração e retomada**
(`ADR-0014 §2-3`):

- `MemoryRepo::apply_correction(old_id, replacement, now) ->
  CorrectionResult` — fluxo atômico de "corrija para X" (a UI
  da Etapa 5 consome): valida replacement → BEGIN IMMEDIATE →
  insere a nova (com `user_confirmed = true`,
  `pending_review = false`) → marca a antiga como superseded
  (idempotente) → COMMIT. Devolve `CorrectionResult { old_id,
  new_record, superseded_at }` pro caller exibir "essa memória
  foi substituída por X há 3 dias".
- `MemoryRepo::mark_superseded` (idempotente, base do
  apply_correction).
- `MemoryRepo::purge_expired(now) -> usize` — DELETE físico
  de memórias soft-deletadas, superseded, ou expiradas-não-
  pinned. Chamado na inicialização da casca Tauri
  (`apps/desktop/src-tauri/src/main.rs::setup`) — best-effort,
  log + segue em caso de erro.
- `MemoryRepo::list_pending_review(now)` — lista memórias com
  `pending_review = true` (fila de revisão de ExternalContent
  aguardando humano, `ADR-0012 §3`). A Etapa 5 consome pra
  mostrar a fila no painel.
- Regra "`user_pinned` bypassa `expires_at` mas não
  `superseded_by`" enforçada nas queries do `list_by_scope`,
  `list_pending_review` e `purge_expired` (correção do usuário
  é mais forte que pin).

A Etapa 5 entrega o **painel de memória na UI** (rota
`/memories` em `apps/desktop/src/routes/Memories.tsx`):

- IPC backend (`shared-contracts` + `main.rs`): 6 novos
  `AppOp` (`MemoryList`, `MemoryRetrieve`,
  `MemoryApplyCorrection`, `MemoryConfirmPending`,
  `MemoryRejectPending`, `MemoryPurgeExpired`) + 4 views
  (`MemoryView`, `ScoreBreakdownView`, `MemoryHitView`,
  `CorrectionResultView`) + 1 input (`NewMemoryInputView`).
- `services/memory.ts` (camada IPC do frontend): 6 funções
  que consomem os `AppOp` (regra do `ADR-0003` — única camada
  que faz `invoke` no Tauri).
- `components/MemoryPanel.tsx`: lista de memórias com
  `ScoreBreakdown` visível (lexical, recency, semantic,
  importance, confirmation, scope_match), badges
  (pendente de revisão, pinned, expirada, superseded),
  form inline de "corrija para X" (modal), e banner
  "Memória substituída" com link pra nova.
- Wire do `MemoryExtractor` no `ChatOrchestrator`: a casca
  Tauri cria o `LlmMemoryClassifier` (com
  `NoopCompletionProvider` por enquanto — a Etapa 5.x
  injeta o OpenRouter real) + `MemoryExtractor` em
  background, e passa o `MemoryExtractorHandle` pro
  `ChatOrchestrator` que enfileira um
  `MemoryExtractionJob` após cada Run finalizar.

A Etapa 6 fecha a Fase 4 com o **gate de CI** baseado em
gold-set versionado:

- `crates/memory/tests/fixtures/gold_set.jsonl` com **21
  cenários** cobrindo as 17 categorias do
  [`memory-evaluation-plan.md`](../architecture/memory-evaluation-plan.md)
  (escopo project/client/conversation, correção de info
  antiga, expiração com pinned bypass, vazamento cruzado
  entre projetos, sinônimos, cross-language, prompt
  injection como dado, malicious memory, documento
  anexo com payload → `pending_review`, duplicate
  conflict, no_results, greeting curto, etc).
- `crates/memory/config/eval.toml` com os **alvos de
  produção** (Etapa 6, hard fail em CI): precisão ≥ 0.70,
  F1 ≥ 0.65, vazamento cruzado = 0 (I4), p99 ≤ 2000ms
  (§10.13). p95 ≤ 1000ms (warning).
- `crates/memory/tests/evaluation.rs` runner
  determinístico (SQLite in-memory + `FakeClock` +
  `FakeHashEmbed` pro híbrido) que carrega o `eval.toml`,
  roda os 21 cenários, e aplica o gate — falha o teste
  se qualquer hard fail estourar (CI-friendly).

Estado atual medido (Etapa 6): precisão **0.905**, F1
**0.937**, vazamento **0**, p99 **1-3ms** (gate 2000ms
— folga de 3 ordens de grandeza).

## 2. O que ele expõe

- Tipos no `frederico-core::memory`:
  `MemoryId`, `MemoryScopeType`, `MemoryType`, `MemoryOrigin`,
  `MemorySourceType`, `EmbeddingStatus`, `MemoryRecord`,
  `MemoryHit`, `ScoreBreakdown`, `RetrievalRequest`,
  `RetrievalResult`, `NewMemory`, `MemoryClassifierOutput`,
  `ClassificationContext`, `ConversationMessage`. 9 escopos,
  12 tipos, 3 origens (ver [`memory-architecture.md`](../architecture/memory-architecture.md)).
- `MemoryRepo<'a>` — `new`, `insert_auto_captured`,
  `insert_user_confirmed`, `insert_pending_review`, `get`,
  `list_by_scope`, `search_lexical`, `mark_superseded`,
  `apply_correction`, `purge_expired`, `confirm_pending`,
  `reject_pending`, `list_pending_review`, `set_embedding`,
  `get_embedding`, `mark_embedding_failed`,
  `list_pending_embeddings`.
- `CorrectionResult` (devolvido por `apply_correction`: `old_id`,
  `new_record`, `superseded_at`).
- `EmbeddingProvider` trait + `NoopEmbeddingAdapter` +
  `OpenRouterEmbeddingAdapter` (Etapa 2).
- `MemoryClassifier` trait + `NoopMemoryClassifier` +
  `LlmMemoryClassifier` + `CompletionProvider` trait +
  `NoopCompletionProvider` (Etapa 3).
- `LlmClassifierConfig` (modelo, max_messages, confidence_threshold, quota_per_minute).
- `Retriever` trait + `LexicalRetriever` + `HybridRetriever` (Etapa 2).
- `EmbeddingWorker` + `EmbeddingWorkerHandle` (worker de embeddings, Etapa 2).
- `MemoryExtractor` + `MemoryExtractorHandle` +
  `MemoryExtractionJob` (worker de classificação, Etapa 3).
- `ScoringWeights` (config) + `EvalGate` (gate de avaliação).
- `NewMemoryInput` (input dos métodos de insert do `MemoryRepo`).
- `MemoryError` (erro unificado do subsistema) + `MemoryResult`.

## 3. De quem depende e quem depende dele

**Depende de:**

- `frederico-core` — tipos compartilhados (`MemoryId`, `MemoryRecord`, etc).
- `frederico-storage` — `Database` (SQLite) + `Database::open_in_memory`
  (pro runner de avaliação).
- `frederico-security` — `Clock` trait (reuso da Fase 2; mover
  pro `core` é trabalho de empacotamento futuro, ver
  [ADR-0014 §"Consequências"](../decisions/0014-expiration-supersededby-gc.md)).
- `sqlx` (SQLite), `serde`/`serde_json`, `chrono`, `thiserror`,
  `async-trait`, `tokio`, `tracing`.

**Quem depende dele (hoje):**

- `apps/desktop/src-tauri` (Etapa 5 da Fase 4): consome
  `MemoryRepo` (list, retrieve, apply_correction,
  confirm/reject_pending, purge_expired) via IPC. A casca
  também inicia o `MemoryExtractor` em background e passa
  o `MemoryExtractorHandle` pro `ChatOrchestrator` da Fase
  3, que enfileira um `MemoryExtractionJob` após cada Run.
- `crates/execution-engine` (Etapa 5 da Fase 4): o
  `ChatOrchestrator` agora tem campo
  `memory_extractor: Option<Arc<MemoryExtractorHandle>>` e
  chama `extractor.enqueue(...)` no `send_message` (best-
  effort).

**Quem vai depender dele (próximas etapas):**

- `frederico-provider-engine` — classificador pode usar o
  mesmo `OpenAiCompatAdapter` da Fase 2.
- `apps/desktop/src-tauri` — IPC `AppOp::MemoryRetrieve`,
  `AppOp::MemoryList`, `AppOp::MemoryConfirm`, etc. (Etapa 5).

## 4. Decisões não óbvias e armadilhas conhecidas

- **Escopo é pré-filtro, não peso** ([ADR-0011 §1](../decisions/0011-scoring-structure.md)).
  Memória de outro projeto **não é candidata**, independente
  de score. A SQL do `search_lexical` e do `list_by_scope`
  começa com `WHERE scope_type = ?1` (específico) ou
  `WHERE scope_type = ?1` (global). Mitiga a ameaça **I4**
  do [`security-threat-model.md`](../architecture/security-threat-model.md).
- **ExternalContent exige confirmação humana** ([ADR-0012 §3](../decisions/0012-memory-classifier.md)).
  `insert_auto_captured` rejeita `origin = ExternalContent` com
  `MemoryError::ExternalContentAutoCaptured`. `insert_user_confirmed`
  aceita. `insert_pending_review` insere com `pending_review = true`
  (memória fica oculta até a Etapa 5 confirmar/rejeitar via
  painel). Mitiga **E2** (memória como instrução).
- **`type = Temporary` exige `expires_at` no futuro.**
  Validado no `NewMemoryInput::validate`. O `MemoryRepo::insert_*`
  chama `validate` antes de ir ao banco.
- **`user_pinned` bypassa `expires_at`, não `superseded_by`.**
  Fixada pelo usuário não expira automaticamente; correção
  do usuário é mais forte que pin (regra do
  [ADR-0014 §1](../decisions/0014-expiration-supersededby-gc.md)).
- **BM25 do FTS5 é normalizado pra [0.0, 1.0]** pelo
  `LexicalRetriever` (1.0 / (1.0 + |bm25|)). A Etapa 6
  pode refinar com BM25F ou outra fórmula.
- **Regra de desempate por recência** ([ADR-0011 §5](../decisions/0011-scoring-structure.md)):
  se dois hits têm score dentro de `recency_epsilon`
  (default 0.01), o mais recente vence. Implementado
  como tie-breaker no `sort_by` do `LexicalRetriever`.
- **`Clock` mora no `frederico-security`, não no `core`.**
  Decisão de empacotamento da Etapa 1 (atualizada no
  ADR-0014). Mover pro `core` é trabalho futuro, sem
  impacto funcional.
- **Embeddings são partição `(provider, model)`.** A
  tabela `memory_embeddings` tem PK composta —
  embeddings de modelos diferentes não são comparáveis
  (regra do [ADR-0010 §3](../decisions/0010-embedding-provider-default.md)).
  O `Retriever` da Etapa 2 filtra por `provider = current
  AND model = current` antes de calcular cosine.
- **Gold-set baseline fica versionado.** Cada cenário
  tem `id` estável (sem reuso). A Etapa 6 expande
  (atualmente 10 cenários; alvo: 51+ cobrindo os 17
  cenários do [memory-evaluation-plan.md](../architecture/memory-evaluation-plan.md)).

## 5. Como testá-lo isoladamente

```pwsh
# Suíte do crate (unit + runner de avaliação E2E)
cargo test -p frederico-memory

# Apenas o runner (imprime tabela de métricas no stdout)
cargo test -p frederico-memory --test evaluation -- --nocapture

# Verificação completa do projeto
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify.ps1
```

Cobertura atual (Etapas 1 + 2 + 3 + 4 + 5 + 6):

- **`src/error.rs`**: 0 testes (tipos puros).
- **`src/sanitize.rs`**: 10 testes (escape de aspas,
  truncation, neutralização de injection, OR explícito,
  preservação de unicode).
- **`src/config.rs`**: 6 testes (defaults, validação de
  pesos, soma ≈ 1.0, gate de p99).
- **`src/embedding.rs`**: 4 testes unit (Noop devolve
  Unavailable, ids, error display, `Debug` redata key).
- **`src/embedding_codec.rs`**: 9 testes (encode/decode
  roundtrip, dim errada, cosine idêntico/ortogonal/oposto/
  zero_norm/similar_direction).
- **`src/classifier.rs`**: 8 testes (Noop devolve None,
  error display, `LlmMemoryClassifier` com NoopCompletion
  devolve Unavailable, parse válido de JSON, threshold de
  confidence descarta, JSON inválido, quota estourada).
- **`src/memory_repo.rs`**: 7 testes unit (validações de
  NewMemoryInput). E2E via `tests/evaluation.rs` e
  `tests/expiration.rs`.
- **`src/retriever.rs`**: 5 testes unit (recency_factor,
  confirmation_factor).
- **`src/worker.rs`**: 9 testes (2 do `EmbeddingWorker` +
  7 do `MemoryExtractor`: classifica User→auto_captured,
  external_content→pending_review, assistant→Assistant,
  classifier devolve None não insere, classifier Err não
  panica, NoopMemoryClassifier compat, enqueue não-bloqueante).
- **`tests/evaluation.rs`**: 2 testes E2E — `run_gold_set_evaluation`
  (baseline lexical, 10 cenários) + `run_gold_set_evaluation_hybrid`
  (híbrido com `FakeHashEmbed`, 10 cenários). Gate da Etapa 2
  exige `F1 híbrido ≥ 0.9` (não regride o baseline).
- **`tests/embedding_adapter.rs`**: 5 testes E2E do
  `OpenRouterEmbeddingAdapter` com `TcpListener` local
  (request/parse correto, count errado, dim errada,
  HTTP 500, `Debug` redata key).
- **`tests/expiration.rs`** (Etapa 4): 9 testes E2E do
  `apply_correction` (atômico, idempotente, valida antes de
  abrir transação) + filtros de expiração e supersedência
  (pinned bypassa `expires_at`, correção é mais forte que
  pin) + `purge_expired` (deleta expired+superseded, preserva
  pinned) + `superseded_at` consistente com `now` do caller
  (audit trail da Etapa 5).

Total estimado: ~57 testes unit + 2 E2E do runner + 5 E2E
do adapter + 9 E2E de expiration.

## 6. O que ele **não** faz

- **Não classifica memórias automaticamente** com
  `CompletionProvider` real como **default**. A Etapa 3
  da Fase de Ligação (PR #28) introduziu o
  `OpenRouterCompletionProvider` (gpt-4o-mini via
  OpenRouter), e a casca consome via
  `frederico_app::composition::build_completion_provider`
  com degradação declarada: se a key do OpenRouter está
  disponível (DPAPI + env var), o real é usado; senão, cai
  pra `NoopCompletionProvider` com warning logado. O
  `frederico-app` **não** busca a key (puro, sem
  `windows`/`tauri`); a casca Tauri busca do DPAPI e
  passa. O controle pela UI (Settings → Memória →
  Classificador) chega na Fase 6 do plano mestre.
- **Não roda o `ReindexWorker` em background.** O schema
  de `embedding_reindex_jobs` está pronto (regra do
  [ADR-0013](../decisions/0013-embedding-reindex.md)) mas
  o worker entra na Etapa 2.
- **Não tem UI.** Sem `services/memory.ts`, sem
  `MemoryPanel.tsx`. A Etapa 5 introduz.
- **Não tem `mark_used` automático** (atualizar
  `last_used_at` quando o `Retriever` retorna um hit). A
  Etapa 2 adiciona — o "used" faz parte do sinal de
  recência.
- **Não tem `set_importance`/`set_pinned`/`set_confirmed`
  como métodos isolados.** A Etapa 4 introduz (UI de
  "corrija para X", "esqueça isso", "fixe esta memória").
- **Não tem UI de "memórias pendentes de revisão".** A
  Etapa 5 consome `pending_review = 1` (fila de
  ExternalContent aguardando humano).
- **Não tem export/import de memórias** (LGPD, Fase 9).
- **Não sincroniza entre dispositivos** (v1 single-device,
  [memory-architecture.md §"Não-objetivos"](../architecture/memory-architecture.md)).
