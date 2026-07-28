# 0014 — Expiração de memórias temporárias, `supersededBy` e coleta preguiçosa na leitura

## Contexto

O `PROMPT MESTRE` §10.8 ("Política de `supersededBy` quando o
usuário corrige algo") e a spec
[`memory-architecture.md`](../architecture/memory-architecture.md)
lista o tipo `temporary` (memórias com TTL) e `correction`
(marca uma memória anterior como substituída).

Três problemas concretos:

1. **Quando o usuário diz "na verdade, eu uso Postgres, não MySQL",
   a memória "usa MySQL" tem que ser marcada como substituída.**
   Sem isso, a memória antiga e a nova convivem, e o `Retriever`
   retorna as duas — a antiga com score alto (texto similar) e
   a nova idem. O usuário vê lixo.
2. **Memórias temporárias (ex.: "vou viajar dia 15-20, lembra
   disso") têm que expirar.** Sem TTL, "lembra que viajei em
   janeiro" vira memória permanente e aparece em toda conversa
   sobre o ano que vem.
3. **Coleta das expiradas** pode ser (a) job tokio periódico
   (estilo cron), (b) trigger SQLite, ou (c) on-read. Cada um
   tem trade-off de testabilidade e timing.

A [`REGRAS-DO-PROJETO.md` §1.10](../../REGRAS-DO-PROJETO.md) e
[`testing-strategy.md`](../architecture/testing-strategy.md) §"Relógio
virtual, sempre" (do [ADR-0008](../decisions/0008-fake-provider-strategy.md))
fixam que **tempo é virtual, sempre** — testes determinísticos
com `FakeClock`. Um job tokio com `tokio::time::interval` quebra
isso: ou roda tempo real (testes lentos e flaky), ou precisa de
fake-time wiring em todo lugar.

## Decisão

### 1. Expiração: campo `expires_at`, tipo `temporary` requer

`memory_records` ganha `expires_at TEXT NULL` na migration 0007.
A regra de modelagem:

- **`type = 'temporary'`** exige `expires_at` não-nulo (validado
  no `MemoryRepo::insert`).
- **Outros tipos** (`preference`, `fact`, `decision`, etc) têm
  `expires_at = NULL` por padrão. Não expiram.
- **`user_pinned = true`** **bypassa** o filtro de `expires_at`
  (memória fixada pelo usuário não expira automaticamente, mesmo
  se for `temporary`). Isso evita o caso "o usuário fixou
  'lembra disso até dia 20' e o sistema apagou no dia 20 sem
  aviso".
- O `Retriever::retrieve` filtra `(expires_at IS NULL OR
  expires_at > ?now)`. O `?now` vem de um `Clock` trait
  injetado (mesmo padrão do `provider-engine` da Fase 2,
  ADR-0008 §3.2 — `FakeClock` em testes, `SystemClock` em
  produção).

### 2. `supersededBy`: FK nullable, com `superseded_at`

`memory_records` ganha:

- `superseded_by TEXT NULL REFERENCES memory_records(id) ON DELETE SET NULL`
- `superseded_at TEXT NULL`

A regra:

- **Quando o usuário corrige**, o `MemoryRepo` chama
  `mark_superseded(old_id, new_id)`:
  - `UPDATE memory_records SET superseded_by = ?new_id, superseded_at = ?now
    WHERE id = ?old_id AND superseded_by IS NULL`
  - Se a `old_id` já foi superseded, é no-op (idempotente).
- **O `Retriever`** filtra `superseded_by IS NULL` no pré-filtro
  (junto com escopo, `active`, `expires_at`). Memória superseded
  **não é recuperada** mesmo se o score for alto.
- **`user_pinned = true` não bypassa `superseded_by`.** A correção
  do usuário é mais forte que o pin — se ele corrigiu, a
  antiga some, mesmo fixada.
- **A nova memória (que corrige)** é inserida normalmente, com
  `type = 'correction'` ou `type = 'preference'` (depende da
  intenção). A spec
  [`memory-architecture.md`](../architecture/memory-architecture.md)
  documenta os 12 tipos e quando cada um é usado.

A Etapa 4 da Fase 4 introduz o fluxo de UI ("o usuário diz
'corrija para X' e o sistema marca a antiga superseded + grava
a nova"). A Etapa 1 da Fase 4 entrega só o schema e a
`MemoryRepo::mark_superseded` com teste.

### 3. Coleta preguiçosa na leitura (não job background)

Toda query de leitura (`list_by_scope`, `retrieve`, `get_by_id`
para exibir no painel) já filtra
`expires_at IS NULL OR expires_at > ?now`. Isso é o **GC**.

Não há job tokio, não há `tokio::time::interval`, não há trigger
SQLite. Razão:

- **Testabilidade.** `FakeClock` da Fase 2 + `tokio::time::pause()`
  + `Clock::advance(...)` em teste cobre o caminho. Avança o
  relógio virtual em 100 dias, roda `list_by_scope`, valida que
  só as não-expiradas voltam. Sem tempo real.
- **Determinismo.** Job tokio periódico depende de quando o
  scheduler acordou, e suites em paralelo podem ver ordens
  diferentes. On-read é determinístico por query.
- **Custo zero em repouso.** Se o usuário tem 10k memórias e
  nenhuma expirou, o filtro é uma comparação de timestamp por
  linha — barato. Job tokio periódico roda mesmo sem necessidade
  e desperdiça ciclos.
- **§10.13 "memória nunca trava".** O filtro de `expires_at` é
  uma `WHERE` adicional no índice — não adiciona latência
  mensurável ao retrieval.

A coleta "ativa" (deletar de fato a linha) acontece via
`MemoryRepo::purge_expired(now)` que é chamada **manualmente** em
dois pontos:

1. Na inicialização do app (`apps/desktop/src-tauri/src/main.rs`),
  depois das migrações. Custo: 1 `DELETE` com `RETURNING` que
  lista os IDs deletados pra log.
2. Quando o usuário pede "limpar memórias expiradas" no painel
  (Etapa 5). Útil pra quem tem 10k memórias expiradas e quer
  ver o disco encolher.

`purge_expired` **deleta linhas com `superseded_by IS NOT NULL`**
também — superseded semântica é "essa memória não é mais
verdade", e mantê-la no banco só ocupa espaço.

### 4. Testabilidade com `FakeClock`

A `MemoryRepo` aceita `Arc<dyn Clock>` (reuso do trait da Fase 2):

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
```

- Produção: `SystemClock`.
- Teste: `FakeClock::new(2026, 7, 28)` → `advance(Duration::days(100))`
  → `now() == 2026-11-05`.

A suíte da Etapa 1 (e Etapa 4) usa `FakeClock` pra validar:

- `mark_superseded` é idempotente (segunda chamada não muda nada).
- `list_by_scope` após `advance(40 dias)` exclui memórias com
  `expires_at = now + 30 dias`.
- `list_by_scope` **inclui** memórias com `user_pinned = true`
  mesmo se expiradas.
- `list_by_scope` **exclui** memórias com `superseded_by != NULL`
  independente de `user_pinned`.

### 5. Memórias `temporary` requerem `expires_at` no insert

A `MemoryRepo::insert` valida:

```rust
if record.type_ == MemoryType::Temporary && record.expires_at.is_none() {
    return Err(StorageError::InvalidMemory(
        "memória 'temporary' requer expires_at não-nulo".into(),
    ));
}
```

E `expires_at` no passado é rejeitado (não faz sentido inserir
memória já expirada). Erro estruturado, não panic.

## Alternativas descartadas

- **Job tokio periódico (`tokio::time::interval`).** Descartada:
  quebra §1.10 ("relógio virtual, sempre"). Testes teriam que
  ou rodar tempo real (lentos e flaky) ou injetar time provider
  em todos os lugares, o que é exatamente o que `Clock` trait já
  faz — só que o filtro on-read já cobre, sem o overhead do job.
- **Trigger SQLite `AFTER INSERT ON conversation_summary ...`.**
  Descartada: triggers são poderosas mas **invisíveis** ao
  testador. O `FakeClock` não tem como pular tempo entre o INSERT
  e o trigger. Validação no código Rust é explícita.
- **GC em background após cada `insert`/`update`.** Descartada:
  se o usuário insere 100 memórias em batch, são 100 scans
  desnecessários. O `purge_expired` explícito é mais barato.
- **`user_pinned` bypassar `superseded_by`.** Descartada:
  correção do usuário é mais forte que pin. Se ele disse
  "corrija para X", a "X" antiga some — mesmo se ele tinha
  fixado. Caso contrário, o usuário fica preso à memória errada
  que ele mesmo corrigiu.
- **Deletar superseded imediatamente (sem `superseded_at`).**
  Descartada: `superseded_at` permite o painel de memória
  (Etapa 5) mostrar "essa memória foi substituída por X há
  3 dias" com link pra nova. UX melhor que sumiço silencioso.
- **Coletar expiradas via `VACUUM` automático.** Descartada:
  `VACUUM` é caro (reconstrói o banco). Pra v1, basta o
  `purge_expired` explícito na inicialização + sob demanda do
  usuário.
- **TTL configurável por tipo (não só `temporary`).**
  Descartada pra v1: complica a spec. O `temporary` é o tipo
  canônico de "memória com prazo"; outros tipos são
  permanentes por design. Se o usuário quiser "fact com TTL",
  ele pode criar como `temporary` e o painel da Etapa 5
  oferece.

## Consequências

**Mais fácil:**

- §10.8 atendido: correção → `mark_superseded` → `Retriever`
  ignora. Testável deterministicamente com `FakeClock` +
  fixture de `gold_set.jsonl` (caso "informação antiga corrigida"
  do [memory-evaluation-plan.md](../architecture/memory-evaluation-plan.md)).
- TTL de `temporary` é trivial: campo + filtro. Sem job, sem
  trigger, sem VACUUM.
- **Determinismo** da suíte: a Etapa 1 da Fase 4 cobre
  expiração e supersedência com `FakeClock`, e a Etapa 6 expande
  o gold-set com cenários de tempo.
- `purge_expired` é opt-in (inicialização + UI da Etapa 5).
  Usuário que quer auditoria de "todas as memórias que já
  existiram" pode desabilitar a coleta e o `superseded_at`
  preserva o histórico.
- O `Clock` trait é reuso da Fase 2 — sem nova abstração.

**Mais difícil:**

- A Etapa 1 da Fase 4 introduz dependência do `frederico-security`
  (onde mora o `Clock` trait). **Decisão de empacotamento da
  Etapa 1:** o `frederico-memory` importa o `Clock` do
  `frederico-security` direto, sem mover o trait para o
  `frederico-core`. Mover o `Clock` para o `core` continua sendo
  trabalho válido (vários crates passariam a depender do `core`
  em vez do `security`), mas tem custo de refatoração que não
  cabe na Etapa 1 (atualiza 4 call sites da Fase 2/3: `execution-engine`,
  `provider-engine/tests/recovery.rs`, `apps/desktop/src-tauri`,
  e o próprio `security`). O ganho é arquitetural, não
  funcional; **fica como pendência de empacotamento** registrada
  em `docs/status.md` da Fase 4, sem bloquear a Etapa 1.
- A `purge_expired` na inicialização tem que ser **best-effort**:
  se o banco está em uso (outro processo do app aberto), o
  `DELETE` pode falhar com `SQLITE_BUSY`. Mitigação: log do
  erro, segue adiante. A próxima inicialização tenta de novo.
- A regra "`user_pinned` bypassa `expires_at` mas não
  `superseded_by`" pode ser contra-intuitiva pro revisor de PR.
  Mitigação: comentário no `MemoryRepo::list_by_scope` +
  teste explícito em `crates/memory/tests/expiration.rs`.
- A combinação `superseded_by` + `user_pinned` + `expires_at`
  + `active` é 4 dimensões de filtro. A spec
  [`memory-architecture.md`](../architecture/memory-architecture.md)
  tem uma tabela "O que o `Retriever` filtra (e em que ordem)"
  pra evitar divergência entre código e spec.
- A Etapa 4 da Fase 4 (correção via comando do usuário) precisa
  de um sinal claro na UI ("essa memória foi substituída — ver
  nova"). A Etapa 1 não toca UI; a Etapa 5 cobre, mas a Etapa 4
  do Fase 4 já precisa decidir o JSON da `mark_superseded`
  (devolve `old_id`, `new_id`, `superseded_at` pro caller).
