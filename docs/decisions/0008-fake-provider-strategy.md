# 0008 — Provedor simulado: golden files (fidelidade) + gerador determinístico (patologia), no nível do transporte

## Contexto

A Fase 2 precisa testar chat com streaming e cancelamento sem queimar crédito de API e sem testes flaky. O `testing-strategy.md` §"Dados de teste" lista o dilema como decisão em aberto: "Provedor simulado: replay de fita (golden files) vs. gerador determinístico". O `AGENTS.md` repete que isso **não está decidido** e exige ADR antes de virar código.

Os dois candidatos resolvem problemas diferentes, e o histórico do projeto anterior mostra que os bugs de chat vivem quase todos em um lugar: o **parser de SSE** — onde bytes viram eventos. Ignorar esse lugar é garantir que os testes de Fase 2 deem verde sem que a Fase 3 (que vai ler `tool_call` do stream) funcione.

### O que cada ferramenta resolve

**Golden files (fidelidade)** capturam as esquisitices reais do provedor que você nunca inventaria de cabeça:

- como o OpenRouter fragmenta os deltas de `tool_calls` (o `arguments` chegando em pedaços que não são JSON válido isoladamente);
- o `reasoning_content` que o DeepSeek emite em stream paralelo ao `content`;
- comentários de keepalive no SSE (`: keepalive\n\n` que parece linha vazia mas é heartbeat);
- UTF-8 partido no meio de um caractere entre dois chunks (chunks HTTP não respeitam fronteira de caractere).

Um gerador determinístico em memória nunca reproduz isso — porque no gerador você só escreve o que já sabe que existe.

**Gerador determinístico (patologia)** roteiriza o que não dá para gravar sob demanda de um provedor real:

- stall no meio do stream (servidor para de responder, conexão TCP aberta);
- truncamento no meio de um `tool_call` (o cliente recebe metade e a conexão cai);
- degeneração por repetição (o modelo entra em loop, mesmos tokens);
- `429` no terceiro chunk (rate limit depois de já ter começado);
- `finish_reason` ausente (a stream termina sem fechar direito).

Essas patologias precisam ser injetadas sob controle. Um golden file não serve porque o provedor real não coopera.

## Decisão

O provedor simulado é **dual**, com separação estrita de papel. Os dois são **transport-level**, não trait-level. Esta é a decisão central do ADR, e dela derivam as outras.

### 1. Golden files (fidelidade) — transport-level

- Cenários vivem em `crates/provider-engine/fixtures/<provider>/<scenario>.jsonl`.
- Cada linha é um **chunk HTTP bruto** como o servidor real enviaria — bytes de SSE, com a fronteira do chunk explícita. Pode ser um evento completo (`data: {...}\n\n`), um keepalive (`: keepalive\n\n`), um comentário, ou metade de um caractere UTF-8 (a segunda metade vem na linha seguinte). O formato é o que está no cabo, não a estrutura do evento.
- Um cenário por arquivo. Cenários v1:
  - `openai/short_response.jsonl` — conversa curta, sem `tool_call`.
  - `openrouter/streaming_tool_call_deltas.jsonl` — `tool_call` chegando em deltas fragmentados, `arguments` inválido como JSON até o último delta.
  - `deepseek/reasoning_content.jsonl` — `reasoning_content` em paralelo ao `content`.
  - `openai/sse_keepalive_comments.jsonl` — keepalives intercalados com eventos reais.
  - `openai/utf8_split_across_chunks.jsonl` — caractere UTF-8 partido entre dois chunks.
- O `FakeProviderAdapter` (apenas para golden files) **não implementa `ProviderAdapter`**. Ele escreve os bytes do `.jsonl` em um `tokio::io::DuplexStream`, e o adapter real (de produção) lê desse mesmo `DuplexStream` como se fosse a response do `reqwest`. O parser de SSE é o mesmo nos dois casos.
- A consequência prática: o parser é exercitado byte a byte em todo PR.

### 2. Gerador determinístico (patologia) — transport-level

- `ScriptedProviderAdapter` aceita um `Script { events: Vec<ScriptedEvent> }`.
- `ScriptedEvent` é uma enum:

  ```rust
  enum ScriptedEvent {
      ChunkAt { offset: usize, bytes: Vec<u8> },
      StallAt { offset: usize, virtual_duration: VirtualDuration },
      TruncateAt { offset: usize },
      Then429,
      DropFinishReason,
      CloseConnection,
  }
  ```

  `VirtualDuration` é um marcador de tempo, não um `Duration` real — é uma contagem de "ticks de clock virtual" (ver §3.2).
- O script é executado pelo `ScriptedProviderAdapter` em um `tokio::io::DuplexStream`. O adapter real lê desse stream; o parser de SSE é o mesmo. Os bytes finais do stream (e o que falta) são exatamente o que o script define.
- Scripts v1:
  - `mid_stream_stall` — adapter trava após o terceiro delta; watchdog de 60s (virtual) tem que disparar.
  - `mid_tool_call_truncation` — conexão cai no meio do `arguments` de um `tool_call`; a próxima request tem que sobreviver.
  - `repetition_degeneration` — mesmos tokens repetidos 50 vezes; a UI tem que cortar.
  - `429_on_third_chunk` — primeiros dois chunks OK, terceiro vem com `429`; a request tem que ser retentada (ou o erro tem que aparecer em português com ação — ver §3.4 da Etapa 5).
  - `missing_finish_reason` — stream termina sem `data: [DONE]` e sem `finish_reason`; o adapter tem que fechar limpo.

### 3. Os três cuidados obrigatórios

Estes não são conveniência, são o **contrato** do ADR. Qualquer um deles sendo afrouxado quebra o propósito da decisão.

#### 3.1. Falsifique no nível do transporte, não do trait

O `ProviderAdapter` recebe **eventos já parseados**. A abstração de transporte (HTTP + SSE) é o que está sendo falsificado, não o que está sendo entregue.

- O golden file e o script de patologia produzem **bytes brutos**, com fronteiras de chunk explícitas, e os entregam ao mesmo parser de SSE que o adapter real consome. O trait recebe os eventos depois do parser.
- O fake **não implementa o trait `ProviderAdapter`**. Ele implementa (ou apenas expõe) a fonte de bytes que o adapter real lê.
- Um **fake em nível de trait** ainda existe, em `crates/provider-engine/src/fake/trait_level.rs`. Ele é menor e roda em testes rápidos das camadas **acima** do parser (lógica de orquestração, mapeamento de erro, cálculo de custo). Esses testes não exercitam o parser — eles substituem o adapter todo. Fica explícito no nome do módulo e nos comentários: "use este fake só para testar o que vem depois do parser".

A razão: a maioria dos bugs de chat está no parser, e um fake de trait pula o parser inteiro. Pular o parser é falsificar a coisa errada.

#### 3.2. Relógio virtual, sempre

O watchdog de 60s, a carência de replay de 90s, e qualquer timeout de stream custam **tempo virtual**, não tempo real.

- O `Clock` do `frederico-security` (trait `Clock` com `FakeClock` e `SystemClock`) é injetado em todos os adapters, incluindo os fakes.
- Os testes rodam com `FakeClock` + `tokio::time::pause()` + `advance()`. O `ScriptedEvent::StallAt` aceita `VirtualDuration` (contagem de ticks), e o adapter consulta o `Clock` injetado para saber quando "passou" o tempo — nenhum `tokio::time::sleep` real.
- A consequência prática: a suíte de chat completa (cobrindo todas as patologias) tem que rodar em **menos de 5 segundos de tempo real** na máquina de referência (i5-3570/16 GB do `testing-strategy.md`). Se passar disso, há `sleep` real escapando.

#### 3.3. Golden file apodrece em silêncio — é o risco real

Golden files passam verde em 2027 enquanto o provedor mudou o formato em 2026, e a suíte mente. Mitigação obrigatória, em quatro camadas:

1. **Cabeçalho em cada fixture.** A primeira linha de todo `.jsonl` é um JSON, não um chunk de SSE:

   ```json
   {"_fixture_header": {"provider": "openrouter", "model": "openai/gpt-4o", "scenario": "streaming_tool_call_deltas", "recorded_at": "2026-07-27T14:32:00Z", "recorder_version": "0.2.0", "source_endpoint": "https://openrouter.ai/api/v1/chat/completions"}}
   ```

   O loader rejeita arquivo sem esse cabeçalho (parse do JSON, presença de `_fixture_header`).

2. **`provider-recorder` versionado.** Binário em `crates/provider-engine/src/bin/recorder.rs`. Re-grava fixtures a partir de uma chave real (lida do env do runner, **nunca** commitada). **Não roda no CI de PR** — roda manual ou noturno, com a chave em variável de ambiente do runner, e produz um diff revisável. Semanal, gera PR-bot com diff das fixtures regravadas.

3. **Contrato contra provedores reais, fora do PR CI.** Suíte em `tests/contract/` que fala com provedor real (chave por env) e valida que os eventos parseados batem com a fixture gravada. Roda **noturna**. Quebra = alerta (issue, Slack, e-mail) — **não** bloqueia PR. A suíte é tolerante a divergência de timestamp, UUID e ordem de eventos secundários; estrita no formato do evento em si.

4. **Sanitização automática na gravação, com guarda no CI.** O recorder aplica regexes antes de escrever:

   ```text
   (?i)(authorization|api[_-]?key|x-api-key)\s*[:=]\s*\S+  → [REDACTED]
   \b(sk-|sk-ant-|gsk_|or-)[A-Za-z0-9_-]+                  → [REDACTED]
   Bearer\s+[A-Za-z0-9._-]+                               → Bearer [REDACTED]
   ```

   E o CI (extensão do `verify.ps1` ou script próprio) varre `crates/provider-engine/fixtures/**/*.jsonl` e falha o build se encontrar qualquer padrão desses em arquivo versionado. A rec gravada em 2027 não vaza a chave de 2026 — mesmo se a sanitização do recorder falhar, o CI pega.

Adicionalmente:

- Um cenário por arquivo. Nada de "vários cenários concatenados em um só `.jsonl`". Diff de fixture quebrada fica pequeno.
- JSONL legível por humanos. Nada de dump binário de log de rede. O reviewer de PR consegue ler a fixture.
- `.gitattributes` em `crates/provider-engine/fixtures/*.jsonl` com `diff` legível (uma linha por chunk).

### Onde os fakes vivem

- `crates/provider-engine/src/fake/` (dentro do crate, não em crate separado) — o test infrastructure vive ao lado do código que ele testa.
- `crates/provider-engine/fixtures/` — versionado, revisável, sanitizado.
- `crates/provider-engine/src/bin/recorder.rs` — binário do recorder, com seu próprio teste de "fita regravada com header correto".

## Alternativas descartadas

- **Fake só em nível de trait.** Descartada: pula o parser, que é onde mora a maioria dos bugs. Um teste de fluxo que substitui o adapter inteiro testa a lógica de orquestração, mas deixa o parser intocado — exatamente o componente que vai ser exercitado pela Fase 3 com `tool_call`.
- **Gerador determinístico sozinho (sem golden files).** Descartada: o gerador reproduz só o que você sabe que existe. Não pega bugs do tipo "ah, o OpenRouter emite um campo novo que ninguém previu".
- **Captura de rede gravada (pcap ou HAR).** Descartada: fragilidade do formato binário, dificuldade de sanitizar, dificuldade de revisar em PR, tamanho do arquivo. JSONL é legível, sanitizável e pequeno.
- **Provedor real em CI de PR.** Descartada: flaky, lento, caro, dependente de chave em CI, quebra o paralelismo do CI.
- **MSW (mock service worker) ou VCR (gravação de HTTP) genéricos.** Descartada: adicionam uma dependência de runtime ou de teste, e o ganho sobre o que está descrito aqui é marginal. O esquema de bytes com fronteiras explícitas é o que a realidade entrega; reproduzir isso direto é mais simples do que adaptar um framework genérico.

## Consequências

**Mais fácil:**

- Parser de SSE exercitado byte a byte em todo PR.
- Patologias roteirizadas viram testes reprodutíveis (não flaky).
- Golden files documentam o formato real dos provedores — referência de leitura humana para quem for implementar o adapter novo (v1.1 Gemini, por exemplo).
- Recorder e contrato noturno criam um ciclo virtuoso: a realidade puxa as fixtures, e o CI garante que ninguém gravou uma chave por acidente.

**Mais difícil:**

- Manter o recorder sincronizado com a realidade é trabalho contínuo. O contrato noturno é uma rede de segurança, não um substituto para alguém olhar a fixture quando ela muda.
- A suíte de contrato contra provedores reais está fora do PR gate. Alertas podem passar batido se ninguém olhar. Cultura de revisão do alarme é parte do trabalho.
- Golden files adicionam bytes ao repo. Mitigado por `.gitattributes` e revisão de PR (diff pequeno por cenário).
- Três coisas coexistem: golden file, scripted pathology, trait-level fake. A documentação em `docs/modules/provider-engine.md` (a ser criado na Etapa 1) tem que ser explícita sobre **quando usar qual**.
- O recorder é um binário a mais para empacotar (mesmo que não entre no instalador final, ele roda no pipeline noturno). Custo de build marginal, mas existente.
