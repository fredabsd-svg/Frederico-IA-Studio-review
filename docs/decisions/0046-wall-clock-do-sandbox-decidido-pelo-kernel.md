# 0046 — O wall clock do sandbox é decidido pelo kernel, não pelo nosso escalonador

## Contexto

O teste `crates/e2e/tests/e2e_exec_python_under_sandbox.rs::wall_clock_kills_long_running_process` falhou sob carga em 2026-08-17, rodando `cargo test --workspace` com uma build concorrente na mesma máquina:

```text
[e2e_exec_python/wall-clock] elapsed=13.435221s ok=true err=None
```

O script Python dormiu os 10 s inteiros e a ferramenta devolveu **sucesso**, com `max_wall_clock_ms=2000`. Isolado, o teste passava em ~2,0 s.

Esse teste existe justamente para provar que o campo `wall_clock` deixou de ser "apenas informativo" na Etapa 4 da Fase 7. Havia duas explicações possíveis, com consequências muito diferentes: ou o teste era frágil (2 worker threads insuficientes sob carga), ou o `wait_with_timeout` não governava o caminho todo — caso em que o limite de tempo do sandbox dependeria de a máquina estar folgada.

### O que a medição mostrou (2026-08-17, Windows 11 26200, 16 CPUs)

A reprodução usou carga artificial de CPU (64 a 96 processos em busy-loop) e instrumentação temporária no `RawChild::wait_with_timeout`.

**1. O relógio do tokio não é o culpado.** Num runtime de 2 workers sob 96 spinners, `tokio::time::sleep(2s)` levou 2,015 s e 2,003 s. A hipótese "2 worker threads não bastam para o timer disparar" **não se sustenta** — a starvation atinge o processo inteiro, não o timer em particular.

**2. O orçamento crescia com o atraso de despacho.** A v1 fazia:

```rust
tokio::time::timeout(
    timeout,
    task::spawn_blocking(move || WaitForSingleObject(h, timeout_ms)),
)
```

O `WaitForSingleObject` só começa a contar os 2 s quando a thread do pool de bloqueio de fato roda. Medido: o despacho atrasou 1,66 s e 4,11 s. O orçamento efetivo virava `atraso + wall_clock`, sem teto — e o `spawn` do processo, por si só, custou de 2,0 s a 7,0 s sob a mesma carga.

**3. O estouro virava sucesso.** Instrumentado, o caminho da falha aparece inteiro:

```text
[rc] blocking iniciou em 1.6619658s
[rc] WaitForSingleObject=WAIT_EVENT(258) em 3.6651732s
[rc] tokio::timeout resolveu em 3.6654245s (err=false)
```

Os dois mecanismos são relógios **relativos**, que só começam a contar quando alguma thread nossa recebe CPU, e o veredito saía de quem fosse observado primeiro. Quando o atraso de despacho passa do tempo de vida do filho, o `WaitForSingleObject` encontra o processo **já encerrado**, devolve `WAIT_OBJECT_0` com exit code 0, e a ferramenta reporta sucesso. É exatamente o `elapsed=13.4s ok=true` do relatório.

**Conclusão: era defeito de produção.** `max_wall_clock_ms` não era um limite; era `wall_clock + atraso de escalonamento arbitrário`, e acima de certo atraso degradava silenciosamente para "sem limite nenhum" — fail-open num controle de segurança. O teste estava certo: o limite de tempo do sandbox dependia de a máquina estar folgada.

O teste também tinha um defeito próprio, menor e independente: ele cronometrava `tool.execute()` inteiro (proxy + `CreateProcessAsUserW` + rotulagem de integridade do workdir + teardown) e exigia `elapsed < 5s`. Esse intervalo não é o wall clock, e sob carga passou a medir a folga da máquina.

## Decisão

**Separar correção de prontidão, e tirar a correção do nosso escalonador.**

O `RawChild::wait_with_timeout` passa a ancorar o wall clock num **deadline absoluto**, tirado uma única vez antes de enfileirar qualquer thread, em duas leituras do mesmo instante: uma monotônica (`Instant`), que governa a espera e o timer, e uma no relógio de sistema (`FILETIME`), que é a única régua do veredito — porque é o relógio em que o kernel carimba o `ExitTime` do processo.

1. **Correção (fail-closed) — quem decide é o kernel.** Um ponto único, `veredito_do_wall_clock`, não pergunta que via da corrida venceu nem quando fomos escalonados. Pergunta ao kernel o que aconteceu com o processo e **quando**:
   - ainda vivo depois do deadline → `TerminateProcess` + `TimedOut`;
   - já encerrado **dentro** do orçamento → sucesso, com o exit code de verdade, mesmo que só tenhamos percebido muito depois (nada de falso positivo);
   - já encerrado **depois** do orçamento → `TimedOut`, ainda que o `WaitForSingleObject` tenha devolvido `WAIT_OBJECT_0`. É este ramo que fecha o fail-open.

   O `ExitTime` vem de `GetProcessTimes`. O deadline em `FILETIME` vem de `SystemTime::now()`, que no Windows é o mesmo relógio (`GetSystemTimePreciseAsFileTime` por baixo) — a comparação é exata e não exige feature nova do crate `windows`.

2. **Prontidão (matar cedo).** `timeout_at` no mesmo deadline absoluto (não `timeout`, que reancoraria o timer no instante em que a linha por acaso executasse), e a espera bloqueante limitada ao que **resta** do orçamento. Se o despacho atrasou além do deadline, o restante é zero e a espera vira um poll: o atraso não estende mais o orçamento do filho.

3. **Erros estruturais não passam pelo wall clock.** Falha de join do pool de bloqueio ou retorno inesperado do Win32 viram erro próprio. Confundi-los com "o filho demorou" esconderia bug nosso atrás do limite de tempo.

**O teste E2E passa a distinguir o que cada asserção vale.** `!ok` + erro contendo `"wall-clock"` é o contrato, e agora é determinístico. A asserção de tempo vira uma **guarda de latência** declarada como tal: o trabalho do filho sobe de 10 s para 60 s e o limite de 5 s vira 20 s, de modo que a guarda discrimina com folga **maior** que a da v1 (a v1 separava 5 s de 10 s, fator 2; a v2 separa 20 s de 60 s, fator 3).

**Um teste novo constrói a starvation em vez de torcer por ela.** `wall_clock_verdict_survives_starved_runtime` monta um runtime de 1 worker e 1 thread de bloqueio e ocupa as duas (um `spawn_blocking` de 8 s e um `std::thread::sleep(9s)` dentro do `join!`), de modo que o filho de 4 s morre sozinho antes de conseguirmos olhar. Visto vermelho contra o código da v1, com a mensagem exata do defeito relatado (`RawExitStatus { code: 0 }`), e verde depois.

## Alternativas descartadas

**Aumentar `worker_threads` no teste.** Trataria o sintoma no arnês e deixaria a produção como estava — e a medição mostra que não era isso: o timer do tokio estava preciso. A casca Tauri não roda com 2 workers, então a exposição em produção é real e independente do arnês.

**Afrouxar a asserção ou aumentar o `max_wall_clock_ms`.** Proibido pela REGRA §2.4, e erraria o alvo: o defeito não era o limite ser apertado demais, era ele não ser um limite.

**`WaitForMultipleObjects([timer, processo])` com um waitable timer armado no início do wall clock.** Atraente porque o timer é absoluto e armado pelo kernel, mas não resolve o desempate: com `bWaitAll = FALSE` o retorno é o **menor índice sinalizado**, então quando somos despachados tarde e os dois objetos já estão sinalizados, o índice fixo decide. Timer em 0 gera falso positivo (filho que coube no orçamento vira estouro); processo em 0 gera falso negativo (o mesmo fail-open de hoje). O desempate precisa de **quando**, não de **se** — e isso só o `ExitTime` responde.

**Limite de tempo via Job Object.** O Windows oferece `JOB_OBJECT_LIMIT_JOB_TIME` e `PerProcessUserTimeLimit`, mas ambos são tempo de **CPU**, não wall clock. Um processo dormindo não consome CPU e nunca seria morto — inútil justamente para o caso que o teste cobre.

**Registrar o instante de saída na própria thread de espera** (`Instant::now()` ao ver `WAIT_OBJECT_0`). Não serve: essa thread também é escalonada, e sob starvation o carimbo dela é tão tardio quanto a observação.

## Consequências

**Fica mais fácil.** `max_wall_clock_ms` passa a ser um limite de verdade: um run que ultrapassa o orçamento é sempre recusado, em qualquer condição de carga, e um run que cabe no orçamento nunca é recusado por engano. O veredito deixou de ter corrida — há um ponto único de decisão, com três ramos explícitos, em vez de dois relógios competindo. A starvation, que antes só aparecia como flake sob carga, agora tem teste determinístico.

**Fica mais difícil.** O crate `security` ganha dependência de `GetProcessTimes` e da aritmética de `FILETIME`, incluindo a constante da época de 1601. A comparação usa o relógio de sistema, não o monotônico: um salto de NTP dentro da janela do wall clock pode classificar mal uma invocação. Aceito porque é o único relógio em que o kernel carimba o `ExitTime`, e a janela típica é de segundos.

**O que continua verdadeiro depois desta decisão.** A *latência* do kill continua dependendo de escalonamento: matar o filho exige que uma thread nossa rode, e sob starvation ele pode sobreviver além do orçamento até conseguirmos o `TerminateProcess`. Nenhum desenho evita isso no Windows, pelo motivo do parágrafo sobre Job Object. O que esta decisão garante é que o **resultado** devolvido continua correto nesse intervalo. Medido depois da correção, com 80 processos em busy-loop e as 4 provas do arquivo em paralelo, `elapsed` ficou entre 7,0 s e 11,0 s em 4 rodadas — dentro da guarda de 20 s, com o veredito correto em todas.

**Lacuna vizinha, não fechada aqui.** O `collect_output` espera o `tokio::join!` inteiro, e os leitores de stdout/stderr só terminam quando o write end do pipe fecha. Um **neto** que herde esses handles e sobreviva ao filho segura os leitores além do wall clock, ainda que o veredito já esteja correto. É um gatilho diferente do medido em 2026-08-17 (exige neto), e fechá-lo exige cancelar os leitores no estouro. Registrado como pendência da Fase 7 no `docs/status.md`.
