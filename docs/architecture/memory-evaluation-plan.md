<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 4
-->

# Plano de Avaliação de Memória

> Aprofundado na Etapa 1 da Fase 4. O stub da Fase 0 virou este
> plano concreto: **17 cenários obrigatórios** do `PROMPT MESTRE`
> §10.12, **métricas objetivas**, **gold-set versionado** e **gate
> de CI** que bloqueia a Fase 4 enquanto o conjunto não atinge os
> alvos.

## Decisão tomada

A avaliação de memória é um **conjunto de cenários gold-set**
versionado em `crates/memory/tests/fixtures/gold_set.jsonl`, mais
**métricas objetivas** calculadas por um runner determinístico
em `crates/memory/tests/evaluation.rs`, mais um **gate de CI** que
bloqueia a Fase 4 enquanto o conjunto não atingir os alvos.

O gold-set é **derivado da especificação** (`memory-architecture.md`
+ `PROMPT MESTRE` §10), não da implementação. Existe antes do
`Retriever` rodar pela primeira vez — assim a calibragem da
Etapa 2 (híbrido) é medida contra um baseline lexical, não contra
o próprio resultado.

## Contrato

### Cenários obrigatórios (`PROMPT MESTRE` §10.12)

17 cenários, cada um com 1+ entradas no gold-set. Cada entrada
tem:

- `id` (string estável, ex: `"scope_project_correct"`)
- `name` (descrição humana)
- `category` (um dos 17 cenários abaixo)
- `scope_type` + `scope_id` (contexto do "caller")
- `query` (texto que o usuário disse)
- `expected` (lista de `MemoryId` que **devem** aparecer no top-K)
- `must_not_contain` (lista de `MemoryId` que **não podem**
  aparecer — casos de vazamento cruzado ou de injeção)
- `seed_memories` (lista de memórias a inserir no DB de teste
  antes do retrieve, com `origin`, `type`, `importance`, etc)

#### 17 categorias

| # | Categoria | O que testa |
|---|-----------|-------------|
| 1 | `scope_project_correct` | retrieve de projeto A retorna memórias de A |
| 2 | `scope_project_wrong` | retrieve de projeto A **não** retorna memórias de B (mesmo se texto similar) |
| 3 | `scope_client_correct` | retrieve do cliente X retorna memórias do cliente X |
| 4 | `scope_client_wrong` | retrieve do cliente X **não** retorna memórias de outro cliente |
| 5 | `topic_similar_different` | texto altamente similar mas assunto diferente — semântica alta + escopo errado deve perder |
| 6 | `old_info_corrected` | memória antiga foi superseded, retrieve só vê a nova |
| 7 | `temporary_expired` | memória `temporary` com `expires_at` no passado é filtrada |
| 8 | `greeting_short` | "oi" — sem candidatos relevantes, retorna lista vazia (§10.7) |
| 9 | `message_ok` | "ok" — sem candidato, lista vazia |
| 10 | `new_chat_same_project` | novo chat no mesmo projeto vê memórias de projeto (retomada) |
| 11 | `new_chat_other_project` | novo chat em outro projeto **não** vê memórias do anterior |
| 12 | `cross_language` | memória em português, consulta em inglês (e vice-versa) — o gold-set cobre as duas direções |
| 13 | `malicious_memory` | memória com texto que tenta prompt injection — não altera system prompt |
| 14 | `prompt_injection_in_memory` | memória com "ignore all previous instructions" — modelo trata como conteúdo |
| 15 | `document_attachment_with_instructions` | documento anexo com payload malicioso — `origin = ExternalContent`, `pending_review = true`, não vira memória automática |
| 16 | `no_results` | query sem candidato — retorna `Vec::new()` (§10.7) |
| 17 | `duplicate_conflict_delete_restore` | duplicar, conflito entre memórias, deletar uma, restaurar superseded |

Cada categoria tem **pelo menos 3 entradas** no gold-set (mínimo
pra ter variabilidade). Total mínimo: **51 entradas**. A Etapa 1
começa com **10 cenários** (subset dos 17 mais críticos) e
expande até 51+ nas Etapas 2/3/6.

### Métricas

Calculadas pelo runner a cada execução do gold-set:

| Métrica | Fórmula | Alvo v1 (Etapa 6) |
|---------|---------|---------------------|
| **Precisão** | `hits_esperados_no_top_k / top_k` | ≥ 0.70 |
| **Revocação (recall)** | `hits_esperados_no_top_k / hits_esperados_total` | ≥ 0.60 |
| **F1** | `2 * P * R / (P + R)` | ≥ 0.65 |
| **Falsos positivos** | `hits_nao_esperados_no_top_k / top_k` | ≤ 0.10 |
| **Falsos negativos** | `hits_esperados_fora_do_top_k / hits_esperados_total` | ≤ 0.40 |
| **Vazamento cruzado** | `count(must_not_contain ∩ top_k)` | **= 0 (hard fail)** |
| **Injeção executada** | `count(cenários E2/P1 que alteraram system prompt)` | **= 0 (hard fail)** |
| **Latência p50** | mediana de `elapsed_ms` | ≤ 500ms |
| **Latência p95** | percentil 95 | ≤ 1000ms |
| **Latência p99** | percentil 99 | **≤ 2000ms (hard fail §10.13)** |

A Etapa 1 só mede **baseline lexical** (sem embeddings, sem
semântica) — é o número que a Etapa 2 tem que **superar**. A Etapa
6 introduz o gate de CI com os alvos acima.

### Alvos (gate de CI)

Gate falha (bloqueia merge) se **qualquer** das condições:

- Precisão < 0.70
- F1 < 0.65
- Vazamento cruzado > 0 em qualquer cenário (hard fail — I4)
- Injeção executada > 0 (hard fail — E2)
- Latência p99 > 2000ms (hard fail — §10.13)

Gate **não falha** (warning, não bloqueia) se:

- Latência p95 entre 1000ms e 2000ms
- Falsos positivos > 0.10 mas ≤ 0.20
- Falsos negativos > 0.40 mas ≤ 0.50

Os alvos são **calibráveis** por PR (mudança em
`crates/memory/config/eval.toml`), mas exigirão:
- Justificativa no PR ("por que esse alvo é mais alto/baixo")
- Aprovação do reviewer
- Suíte expandida que cubra o novo alvo (não dá pra subir alvo
  sem mais dados)

### Procedimento de execução

```text
1. cargo test -p frederico-memory --test evaluation
   Lê crates/memory/tests/fixtures/gold_set.jsonl.
   Para cada entrada:
     a. Cria DB SQLite in-memory (:memory:).
     b. Insere as seed_memories.
     c. Chama Retriever::retrieve(scope, query, k).
     d. Compara o resultado com expected e must_not_contain.
2. Calcula métricas globais (P, R, F1, FP, FN, latências).
3. Escreve target/evaluation/memory/report.json (não versionado).
4. Imprime tabela resumo no stdout (visível em `cargo test`).
5. Aplica o gate (hard fail + warning).
```

**Determinismo:** o `Retriever` aceita `Arc<dyn Clock>` (reuso do
trait da Fase 2) — o runner injeta `FakeClock` fixo em
`2026-07-28T00:00:00Z`. Sem randomness, sem tempo real, sem I/O
de rede. Suíte completa roda em **< 5s** de tempo real.

**Falsificabilidade:** o runner usa `NoopEmbeddingAdapter` (vê
[ADR-0010](../decisions/0010-embedding-provider-default.md)) na
Etapa 1. A Etapa 2 introduz o `FakeEmbeddingAdapter` com
vetores determinísticos por hash do texto (`hash(text) %
dimensions`).

### Política de atualização do gold-set

Quando o motor evolui e um cenário vira "trivial" (precisão
100% por causa de overfitting), o cenário é **rotacionado**:

1. Mantém o cenário antigo no gold-set (com `expected` antigo).
2. Adiciona um cenário novo na mesma categoria, com `expected`
   atualizado.
3. Mínimo de 3 entradas por categoria se mantém.

Nunca deleta cenário do gold-set — o histórico é a evidência
de que o motor **não regrediu** em um caso que um dia foi
difícil.

## Não-objetivos

- Avaliação de embeddings isoladamente (métrica de produto, não
  benchmark acadêmico).
- Avaliação subjetiva de "utilidade" da memória.
- Avaliação offline sem reprodutibilidade (toda execução é
  determinística via `FakeClock` + seeds fixos).
- **Comparação com baseline de produção** (gold-set é contra o
  motor atual, não contra uma versão anterior — versionamento
  por cenário, não por versão do motor).

## Cobertura da Etapa 1

A Etapa 1 da Fase 4 entrega o **esqueleto**:

- `crates/memory/tests/fixtures/gold_set.jsonl` com **10 cenários**
  (subset inicial: project_correct, project_wrong, client_wrong,
  old_info_corrected, temporary_expired, greeting_short, message_ok,
  no_results, cross_language, prompt_injection_in_memory).
- `crates/memory/tests/evaluation.rs` rodando contra o baseline
  lexical-only (sem embeddings).
- Métricas impressas no stdout e em
  `target/evaluation/memory/baseline.json` (não versionado).
- Gate **mínimo** (precisão > 0.0) na Etapa 1; gate de CI
  formal entra na Etapa 6.

## Decisões (ADRs relacionados)

- [ADR-0010](../decisions/0010-embedding-provider-default.md) —
  embeddings default (OpenRouter) e falsificabilidade
  (`NoopEmbeddingAdapter`).
- [ADR-0011](../decisions/0011-scoring-structure.md) — estrutura
  do scoring (escopo como pré-filtro, pesos em `scoring.toml`).
- [ADR-0012](../decisions/0012-memory-classifier.md) — classificador
  pós-resposta, falsificável.
- [ADR-0014](../decisions/0014-expiration-supersededby-gc.md) —
  `FakeClock` para testar `expires_at` e `superseded_by`.

## Referências

- `PROMPT MESTRE` §10.12 (testes obrigatórios de memória).
- [`memory-architecture.md`](./memory-architecture.md) (contrato).
- [`testing-strategy.md`](./testing-strategy.md) (relógio virtual,
  falsos em nível de transporte).
- [`security-threat-model.md`](./security-threat-model.md) (I4, E2).
