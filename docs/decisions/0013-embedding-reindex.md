# 0013 — Reindexação de embeddings quando o provedor/modelo muda: progresso visível, app continua usável

## Contexto

O [ADR-0010](./0010-embedding-provider-default.md) fixa que o
`EmbeddingProvider` é pluggable e o modelo default é
`openai/text-embedding-3-small` (1536 dim). Mas a Fase 4 inteira
assume que o usuário pode trocar:

- **Provedor** (OpenRouter → OpenAI direto → modelo local).
- **Modelo** (`text-embedding-3-small` → `text-embedding-3-large`
  → `mistral-embed` → outro).

Quando o provedor ou modelo muda, **todas as embeddings
existentes precisam ser recalculadas**. O `Retriever` não pode
misturar embeddings de modelos diferentes — cosine similarity
entre vetores de dimensionalidade ou normalização diferente é
matematicamente sem sentido.

Três problemas:

1. **Tamanho da fila.** Milhares de memórias × latência de embedding
  (~200ms por chamada, 50 memórias por batch) = minutos de
  reindexação. Em escala (Fase 9, anos de uso), pode ser
  horas.
2. **App não pode travar.** §10.13 é claro: "memória nunca trava
  uma resposta". Uma reindexação bloqueante na inicialização
  viola isso.
3. **Progresso visível.** Se o usuário mudou o provedor esperando
  melhoria na busca semântica, ele quer saber se está
  funcionando — quanto falta, quantas falharam, se vale a pena
  esperar.

A reindexação também é necessária quando o `provider-engine`
detecta que o **provedor foi reconfigurado** (credencial trocada,
endpoint mudou) ou quando o **usuário roda o "reindexar agora"**
manualmente (Fase 4 Etapa 5).

## Decisão

### 1. Reindexação em background, app continua usável

A reindexação é um job tokio em background, não bloqueante:

```text
1. Usuário troca `embedding_provider` ou `embedding_model` no painel
   (Etapa 5). O `MemoryConfigRepo` grava a nova config.
2. O `MemoryConfigService` detecta a mudança (compare-and-swap
   no campo) e enfileira um `ReindexJob { new_provider, new_model,
   new_dimensions, old_provider, old_model }`.
3. O `ReindexWorker` consome o job. Itera sobre
   `memory_records` paginadas (1000 por vez). Para cada batch:
   a. Chama o `EmbeddingProvider::embed(batch)`.
   b. Atualiza `memory_embeddings(memory_id, provider, model, dim, vec_blob)`.
   c. Marca `last_reindexed_at` na `memory_records`.
   d. Incrementa contador no `embedding_reindex_jobs`.
4. Enquanto o job roda, o `Retriever` continua funcionando:
   a. Memórias com embeddings do modelo novo: semântica ativa.
   b. Memórias sem embeddings do modelo novo: cai pra lexical-only
      para essas memórias (a SQL filtra por `provider = current AND
      model = current`).
   c. FTS5 cobre todas (não depende de modelo).
5. O `ReindexWorker` registra progresso em
   `embedding_reindex_jobs` (lido pelo painel).
```

**Por que não bloqueante:** §10.13 proíbe. Em 1000 memórias,
reindexação leva ~30s com batch 50; em 10k leva ~5min. Bloquear
o app por 5min é UX ruim.

**Por que o `Retriever` filtra por `provider`/`model`:** embeddings
de modelos diferentes não são comparáveis. Memória "ainda com
embedding antigo" tem que ser tratada como "sem semântica
disponível" até o worker terminar.

**Por que `last_reindexed_at` na `memory_records`:** permite
retomar após crash (o worker lê `WHERE last_reindexed_at < job.started_at`
no resume) e permite diff ("essa memória foi embarcada com o
modelo novo").

### 2. Tabela `embedding_reindex_jobs`

Nova tabela na migration 0007:

```sql
CREATE TABLE embedding_reindex_jobs (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    old_provider TEXT NOT NULL,
    old_model TEXT NOT NULL,
    new_provider TEXT NOT NULL,
    new_model TEXT NOT NULL,
    new_dimensions INTEGER NOT NULL,
    total INTEGER NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK(status IN ('running', 'completed',
                                          'failed', 'cancelled'))
);
```

- `total`: snapshot do `COUNT(*)` no início (não muda durante o
  job — o `Retriever` pode criar memórias novas enquanto o job
  roda, e elas ficam pra um job futuro).
- `done`: incrementa a cada batch bem-sucedido.
- `failed`: incrementa a cada falha de embedding (provider
  retornou erro). Falha não aborta o job — segue adiante e
  registra no final.
- `status = 'cancelled'`: usuário cancelou manualmente (Etapa 5).
  Não deleta linhas — mantém o histórico pro painel.

### 3. Painel de progresso (Etapa 5)

A Etapa 5 do Fase 4 consome essa tabela pra mostrar:

- "Reindexação em andamento: 4321/10000 (43%)"
- "Última falha: openai/text-embedding-3-small — timeout (12 memórias)"
- "Reindexação concluída em 2min 14s"
- "Reindexação cancelada — 5.432 memórias continuam com embedding antigo"

A leitura do progresso é via IPC `AppOp::ReindexStatus` (Etapa 5
decide nome exato). O frontend poleia a cada 2s enquanto há
job em `running`.

### 4. Embeddings novas: gravadas com o modelo corrente

A `MemoryRepo::insert` (Etapa 1) **não** calcula embedding. Quem
calcula é o `MemoryRepo::insert_with_embedding` (Etapa 2) ou um
hook em background (Etapa 3). A regra:

- Memória entra em `memory_records` com `embedding_status = 'pending'`.
- Worker de embeddings pega memórias com `embedding_status = 'pending'`
  e calcula. Atualiza pra `'ready'` com `provider`, `model`,
  `dimensions`, `vec_blob`.
- Se a `MemoryRepo::insert` for chamada durante um job de
  reindexação, a memória nova fica `pending` e é embarcada no
  job **atual** (o worker pega ela no próximo batch — paginação
  com `last_reindexed_at < now` ou `embedding_status = 'pending'`).
  Isso evita race condition entre reindexação e criação.

### 5. Migração de embeddings existentes na v1

A Etapa 1 da Fase 4 entrega o schema com `embedding_status` mas
**sem** dados (tabela vazia). A Etapa 2 introduz o adapter real
(`OpenRouterEmbeddingAdapter`); na primeira vez que o usuário
abrir o app com embeddings habilitados, o `MemoryConfigService`
dispara o primeiro job de reindexação (de "nada" pra o modelo
default). Job termina em segundos (0 memórias a embarcar), e
memórias criadas a partir daí já nascem com embedding.

A reindexação **só roda quando o `provider` ou `model` muda**
(compare-and-swap no `MemoryConfigRepo`). Trocar a `api_key` não
dispara reindexação — a chave nova lê o provider/modelo antigos.

## Alternativas descartadas

- **Reindexação bloqueante na inicialização.** Descartada: §10.13
  proíbe. Em escala, minutos de app parado.
- **Reindexação lazy on-read.** Descartada: imprevisível
  (primeira leitura paga o custo), sem progresso visível, e
  derruba a garantia "lexical cobre tudo" porque a primeira
  leitura de cada memória é lenta.
- **Reindexação em thread tokio sem tracking.** Descartada: o
  usuário não tem como saber se está rodando, se travou, ou
  quantas falharam. Sem painel, é caixa-preta.
- **Embeddings por memória em uma única transação
  (`BEGIN IMMEDIATE; INSERT; UPDATE; COMMIT;`).** Descartada: o
  embedding é I/O de rede, e a Fase 5 da Fase 3 (recover) já
  documenta que transações SQLite longas são armadilha. Embedding
  é um job separado, com `BEGIN IMMEDIATE; UPDATE; COMMIT;`
  curto por batch.
- **Reindexar tudo quando o `provider-engine` detecta nova
  credencial.** Descartada: credencial nova não significa modelo
  novo. A chave OpenAI pode trocar sem mudar de `text-embedding-3-small`.
  Reindexação é por provider/model, não por credencial.
- **Reindexar tudo quando a versão do app muda.** Descartada:
  versão do app é bump de feature, não de embedding. A v1.0.0 e
  v1.1.0 podem usar o mesmo modelo de embedding. Reindexação
  é por provider/model, não por versão.
- **Reindexação sem `last_reindexed_at` (sem resume).** Descartada:
  crash no meio do job perde o progresso. Com `last_reindexed_at`,
  o resume pega de onde parou. Custo de uma coluna é trivial.

## Consequências

**Mais fácil:**

- §10.13 atendido: app continua usável, FTS5 cobre, semântica
  vai aparecendo aos poucos.
- Crash-safe: `last_reindexed_at` permite resume. O worker
  também tem `idempotency_key = memory_id + new_model` pra não
  embarcar duas vezes.
- Painel da Etapa 5 tem fonte de verdade única (a tabela
  `embedding_reindex_jobs`). Sem polling a múltiplos lugares.
- Migração de modelo é trivial: usuário troca no painel, job
  roda, pronto. Sem migração manual, sem script.
- `embedding_status = 'pending'` cobre o caso "memória
  acabada de criar, ainda sem embedding" — o `Retriever` cai
  pra lexical-only pra ela, e a semântica aparece quando o
  worker termina.

**Mais difícil:**

- A Etapa 1 da Fase 4 entrega o schema mas sem worker. Worker
  entra na Etapa 2 (junto com o adapter real). Sem Etapa 2,
  a Etapa 1 tem o painel mostrando "nenhum job em andamento"
  permanentemente.
- O snapshot do `total` no início do job é uma estimativa
  (memórias novas podem entrar). O painel mostra "X de Y" mas o
  denominador Y pode crescer. Mitigação: mostrar
  "X processadas (de Y estimadas)" com Y sendo o snapshot.
- O filtro `provider = current AND model = current` no
  `Retriever` é uma **decisão de escopo** (quais embeddings
  contar) que tem que ser enforçada no nível da SQL — sem isso,
  cosine entre embeddings de modelos diferentes vaza
  silenciosamente. A spec
  [`memory-architecture.md`](../architecture/memory-architecture.md)
  documenta o filtro.
- A combinação `embedding_status = 'pending'` + job de
  reindexação tem uma race ("o job pega a memória nova antes do
  insert terminar?"). Mitigação: o insert grava com
  `embedding_status = 'pending'`, e o job pega por paginação
  com `WHERE (embedding_status = 'pending' OR last_reindexed_at
  < job.started_at)`. Race existe mas é benigna — a próxima
  passada pega.
- A Etapa 5 do Fase 4 tem trabalho de UI não-trivial (painel de
  progresso + botão "reindexar agora" + área de "memórias
  pendentes de revisão" da Etapa 3). A Etapa 1 não toca UI; a
  Etapa 5 cobre.
