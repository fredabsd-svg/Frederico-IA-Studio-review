# 0011 — Estrutura do scoring: escopo como pré-filtro, sinais de score em ordem fixa, pesos em configuração

## Contexto

O `PROMPT MESTRE` §10.6 define o scoring de memória como híbrido,
combinando "escopo + recência + FTS5 + similaridade semântica +
importância + confirmação do usuário". A primeira vista, são seis
fatores somados. O [`security-threat-model.md`](../architecture/security-threat-model.md)
lista a ameaça **I4** ("memória semântica vaza entre projetos") e a
forma de mitigá-la é "filtro por escopo no retrieval". E a
[`REGRAS-DO-PROJETO.md` §1.6](../../REGRAS-DO-PROJETO.md) diz que
ADRs são **imutáveis** — revisão gera ADR novo.

Três problemas com a leitura ingênua "seis pesos somados":

1. **Escopo como peso é I4.** Se o escopo é um peso entre seis, um
   score semântico alto atropela um escopo errado. Memória do
   projeto A pode aparecer no projeto B se o texto for
   suficientemente similar. O escopo precisa ser cláusula `WHERE` —
   barreira rígida, não sinal.
2. **Pesos mudam com a calibragem.** A Etapa 1 da Fase 4 entrega um
   gold-set (`tests/fixtures/memory/gold_set.jsonl`) e roda o
   retrieval contra ele. Os pesos vão precisar de ajuste fino —
   `0.30 → 0.35` aqui, `0.50 → 0.45` ali — à medida que a Etapa 2
   (híbrido) supera o baseline lexical. Cada ajuste seria um ADR
   novo se morar aqui. Em três meses, meia dúzia de ADRs
   contraditórios dizendo coisas diferentes sobre a "mesma" fórmula.
3. **Ordem dos sinais importa mais que os pesos.** Aplicar
   recência antes da semântica não é o mesmo que aplicar semântica
   antes da recência. Em empate estrutural, "mensagens recentes da
   conversa têm prioridade sobre memória semântica" (§10.4) é uma
   **regem de desempate**, não um peso.

Este ADR separa o que é **estrutura imutável** (este documento) do
que é **número calibrável** (`crates/memory/config/scoring.toml`,
mutável por PR, mudança justificada pelo resultado do gold-set).

## Decisão

### 1. Escopo é pré-filtro, não peso

O `Retriever::retrieve(scope, query, k)` recebe um
`RetrievalRequest` que **inclui o contexto de escopo do caller** (a
conversa atual, com `project_id` e/ou `client_id` e/ou
`assistant_id`). A SQL gerada abre com:

```sql
SELECT id, content, ... FROM memory_records
WHERE active = 1
  AND superseded_by IS NULL
  AND (expires_at IS NULL OR expires_at > ?now)
  AND scope_type = ?scope_type
  AND (
    scope_id = ?scope_id                          -- exato
    OR scope_type IN ('profile', 'preference')   -- escopos globais
  )
```

Memória que não passa nesse `WHERE` **não é candidata** —
independente de qualquer outro sinal. A memória do projeto A no
projeto B perde por cláusula, não por pouco no score.

Os escopos **globais** (`profile`, `preference`) atravessam
conversas: o perfil do usuário ("odeio figo", "trabalha com Rust")
vale em qualquer conversa. A `OR scope_type IN (...)` modela isso
sem precisar de tabela auxiliar de "escopo visível em".

A spec [`memory-architecture.md`](../architecture/memory-architecture.md)
documenta os 9 escopos (`profile`, `preference`, `assistant`,
`project`, `client`, `conversation`, `document`, `task`, `session`)
e a tabela de "global vs. específico" é parte do spec.

### 2. Sinais de score em ordem fixa

Uma vez o conjunto de candidatos definido pelo pré-filtro, o
`Retriever` aplica **sinais** em ordem:

| # | Sinal | Fonte | Custo | Obrigatório? |
|---|-------|-------|-------|--------------|
| 1 | Lexical (FTS5 BM25) | `memory_fts` | baixo | sim |
| 2 | Recência (decay exponencial) | `last_used_at`, `created_at` | zero | sim |
| 3 | Semântica (cosine) | `memory_embeddings` (se provider disponível) | alto | opcional |
| 4 | Importância | `importance: f32` | zero | sim |
| 5 | Confirmação | `user_confirmed` / `user_pinned` | zero | sim |

A **ordem** é parte deste ADR (imutável). Razão: sinais mais
baratos primeiro cortam o conjunto caro; sinais mais fortes depois
refinam os sobreviventes. Detalhe:

1. **Lexical primeiro** porque o BM5 do FTS5 é barato (índice
   nativo) e já elimina a maior parte dos candidatos (tudo que não
   tem overlap textual com a query). A Etapa 1 baseline prova.
2. **Recência em segundo** porque é um cálculo de subtração de
   timestamp, custo zero. Recência também funciona como **regra
   de desempate** (§10.4): "mensagens recentes da conversa têm
   prioridade sobre memória semântica". Se dois candidatos têm
   score próximo, o mais recente vence.
3. **Semântica em terceiro** porque cosine similarity sobre
   ~1500 dim custa ~10µs por par, mas a busca vetorial em si exige
   provider de embedding disponível. Se o provider falhou
   (timeout, sem credencial), o sinal é neutro (1.0) e os
   sobreviventes do lexical+recência passam.
4. **Importância em quarto** como multiplicador — memória marcada
   como importante (ex.: "decisão arquitetural do projeto") sobe
   no ranking. Não corta ninguém (candidato pode ter importance
   0.1 e ainda ser o melhor disponível).
5. **Confirmação por último** porque é o sinal mais "pessoal" do
   usuário — "isso é memória que ele confirmou ou fixou" tem peso
   forte mas só sobre o que sobrou. `user_pinned` é o boost
   máximo; `user_confirmed` é um boost médio.

### 3. Pesos numéricos em `scoring.toml`, mutáveis por PR

Os **números** que combinam os cinco sinais vivem em
`crates/memory/config/scoring.toml`:

```toml
[scoring]
# Pesos somam 1.0 (cada um é fração do score final). Pesos em zero
# desligam o sinal sem remover a coluna do banco.
lexical_weight = 0.40
recency_weight = 0.20
semantic_weight = 0.20
importance_weight = 0.10
confirmation_weight = 0.10

# Recência: half-life do decay exponencial. Após `recency_half_life_days`,
# o fator de recência cai a 0.5.
recency_half_life_days = 30.0

# Boost multiplicativo quando user_pinned ou user_confirmed.
user_pinned_boost = 1.5
user_confirmed_boost = 1.2

# Teto de K (número máximo de memórias retornadas).
max_k = 8

# Orçamento de tokens do contexto do modelo que a memória pode
# consumir (além do system prompt e do histórico da conversa).
token_budget = 1500
```

Estrutura (`#[derive(Deserialize)]`) no
`crates/memory/src/config.rs`. Mudanças no TOML são **PR normal**
— não exigem ADR novo. A revisão do PR deve olhar:

- A mudança melhora o gold-set? (A Etapa 6 introduz o gate formal;
  antes disso, a Etapa 2/3 publica `target/evaluation/memory/report.json`
  em cada PR de scoring pra diff visível.)
- A mudança não reintroduz I4? (Escopo continua pré-filtro,
  independente dos pesos — esta parte é imutável e é guardada por
  teste em `crates/memory/tests/scope_filter.rs` que sempre falha
  se um escopo errado vazar.)

### 4. Composição final

A função de scoring final é:

```text
score(m, q) = lexical_weight * bm25(m, q) * recency_factor(m)
            + recency_weight * recency_factor(m)
            + semantic_weight * cosine(m, q)   # 0.0 se sem provider
            + importance_weight * m.importance
            + confirmation_weight * (m.user_pinned as f32 * user_pinned_boost
                                     + m.user_confirmed as f32 * user_confirmed_boost)
```

`recency_factor(m) = exp(-ln(2) * age_days(m) / recency_half_life_days)`

Top-K por `score` descendente, respeitando o `token_budget` (se o
próximo candidato estourar o orçamento, para). Lista vazia é
resposta válida (§10.7) — o `Retriever` devolve `Vec::new()` em vez
de forçar completude.

### 5. Ordem "recência > semântica" do §10.4

§10.4 diz "mensagens recentes da conversa têm prioridade sobre
memória semântica". Isso é **regra de desempate estrutural**, não
um peso. Implementação: se dois candidatos têm score dentro de
`recency_epsilon = 0.01`, o mais recente (`last_used_at` ou
`created_at`) vence. O valor de `recency_epsilon` mora no TOML
junto com os pesos.

## Alternativas descartadas

- **Escopo como peso entre seis.** Descartada: é I4 em forma de
  decisão. Memória de outro projeto aparece quando o score
  semântico é alto. A ameaça foi listada explicitamente no
  `security-threat-model.md` e a contramedida é filtro, não peso.
- **Todos os números no ADR.** Descartada: §1.6 da
  `REGRAS-DO-PROJETO.md` torna o ADR imutável; cada ajuste de 0.05
  num peso forçaria ADR novo. Em três meses teríamos 6-10 ADRs
  contraditórios sobre a "mesma" fórmula, e nenhum deles seria
  autoritativo.
- **Ordem dos sinais decidida por pesos (todos no mesmo nível).**
  Descartada: ignora que sinais mais baratos devem cortar antes
  dos mais caros. Aplicar cosine sobre 1000 candidatos pra depois
  eliminar 950 com lexical é desperdício mensurável.
- **Scoring via LLM ("peça pro modelo rankear").** Descartada:
  adiciona latência a toda chamada de retrieval (§10.13 = 2s).
  O gold-set baseline da Etapa 1 prova que lexical+recência já dá
  precisão útil; LLM-rank entra se e quando o gold-set mostrar que
  a fórmula determinística não converge — futuro, não v1.
- **Tabela única de "fator → peso" sem distinção pré-filtro.**
  Descartada: mesma razão do primeiro item.

## Consequências

**Mais fácil:**

- O ADR é estável. Calibragem vive no TOML; revisão do TOML é PR
  normal com diff de números + diff do relatório do gold-set.
- O `Retriever::retrieve` é testável de forma determinística com
  fixtures de embeddings pré-calculados (a Etapa 2 usa
  `FakeEmbeddingProvider` que devolve vetores fixos por hash do
  texto). Sem randomness.
- A regra "lista vazia é válida" (§10.7) é trivial: o `Retriever`
  devolve `Vec::new()` quando o filtro não tem candidatos. Sem
  fallbacks, sem completude forçada.
- I4 fica fechado por teste (`crates/memory/tests/scope_filter.rs`):
  "memória de projeto A nunca aparece em retrieve de projeto B,
  independente de score".
- §10.4 (recência > semântica) é regra de desempate, não peso —
  é trivial auditar na revisão: "esse PR muda pesos? não muda
  epsilon? ok, regra de desempate preservada".

**Mais difícil:**

- A composição final mistura multiplicação e adição de uma forma
  que pode ser contra-intuitiva. A spec
  [`memory-architecture.md`](../architecture/memory-architecture.md)
  e o doc do módulo [`docs/modules/memory.md`](../modules/memory.md)
  têm que documentar a fórmula com cuidado — fórmulas que misturam
  `*` e `+` são onde os bugs vivem.
- A regra de desempate por `recency_epsilon` introduz um
  hiperparâmetro novo. Mitigação: default 0.01, calibrado pelo
  gold-set na Etapa 6.
- Mudar o TOML exige re-rodar o gold-set pra confirmar que
  melhorou. Isso fica como parte do template de PR de scoring
  (a Etapa 6 introduz um check do CI que compara o relatório do PR
  contra o do main).
- A `OR scope_type IN ('profile', 'preference')` é uma
  simplificação. A spec [`memory-architecture.md`](../architecture/memory-architecture.md)
  documenta os 9 escopos e quais são globais vs. específicos; se
  a lista de globais crescer, é spec + migration + TOML, não só
  código.
