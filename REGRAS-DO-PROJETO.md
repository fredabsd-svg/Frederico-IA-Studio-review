# REGRAS DO PROJETO — Frederico IA Studio

Estas regras complementam o PROMPT MESTRE e valem para **toda** IA ou pessoa que trabalhar no repositório. Elas não são sugestões: violação de regra é defeito, igual a bug de código. Em conflito entre uma regra e a conveniência do momento, a regra vence.

---

## REGRA 1 — DOCUMENTAÇÃO

### 1.1 Princípio único

**A documentação descreve o que o código FAZ hoje — nunca o que se pretende que ele faça.**

Documento que descreve intenção, funcionalidade futura ou comportamento que não existe mais é um defeito da mesma gravidade de um bug em produção. O projeto anterior morreu em parte por isso: a documentação prometia, o código divergia, e ninguém sabia mais o que era verdade. Se algo ainda não foi implementado, o lugar dele é o roadmap (`docs/development-roadmap.md`) — jamais a documentação técnica.

### 1.2 Estrutura obrigatória do repositório

```text
README.md                          ← porta de entrada honesta (§1.5)
CHANGELOG.md                       ← histórico por versão (§1.7)
REGRAS-DO-PROJETO.md               ← este arquivo
docs/
├── architecture/                  ← arquitetura real de cada área
├── decisions/                     ← ADRs numerados (§1.6)
├── modules/                       ← um documento por crate/worker/pacote (§1.4)
├── testing/                       ← estratégia e como rodar
├── security/                      ← modelo de ameaça e decisões de segurança
├── releases/                      ← notas de cada versão publicada
└── status.md                      ← estado real por fase (§1.8)
```

Os documentos de especificação exigidos pelo §33 do PROMPT MESTRE (tool-registry, memory, wordpro, excelpro, pdfpro etc.) vivem em `docs/architecture/`. Quando a implementação divergir da especificação original, **a especificação é atualizada no mesmo commit** — com uma nota "Alterado em relação ao plano original: motivo".

### 1.3 Documentação acompanha o código — sempre no mesmo commit

- Toda mudança de comportamento, contrato, schema, tabela, ferramenta, permissão ou fluxo atualiza a documentação afetada **no mesmo commit/PR**. Não existe "documento depois".
- Um PR que muda comportamento e não toca em `docs/` deve declarar explicitamente na descrição: "Sem impacto documental — motivo". Se o motivo não convencer, o PR não entra.
- Renomeou, moveu ou removeu algo? Procure o nome antigo em toda a documentação (`grep` no repositório inteiro) e corrija cada ocorrência. Referência a coisa que não existe mais é defeito.

### 1.4 Documento por módulo

Cada crate, worker e pacote tem um arquivo em `docs/modules/<nome>.md` respondendo, em uma página:

1. o que este módulo faz (2–4 frases);
2. o que ele expõe (API/contratos públicos);
3. do que ele depende e quem depende dele;
4. decisões não óbvias e armadilhas conhecidas;
5. como testá-lo isoladamente;
6. o que ele **não** faz (limites explícitos).

Módulo novo sem esse documento não é considerado entregue.

### 1.5 README honesto

O README responde apenas: o que o aplicativo é, o que **funciona hoje**, como instalar, como desenvolver, onde está o resto da documentação. Proibido no README: lista de funcionalidades futuras misturada com as existentes, badges decorativas de status que ninguém atualiza, promessas ("em breve"). Funcionalidade só entra no README quando passa nos critérios de aceite dela.

### 1.6 Decisões viram ADR

Toda decisão estrutural (stack, formato de contrato, mudança de arquitetura, troca de biblioteca central, exceção a uma regra do PROMPT MESTRE) gera um ADR em `docs/decisions/NNNN-titulo.md` com exatamente quatro seções: **Contexto** (o problema), **Decisão** (o que foi decidido), **Alternativas descartadas** (e por quê), **Consequências** (o que fica mais fácil e o que fica mais difícil).

- ADRs são numerados em sequência e **imutáveis**: decisão revista gera um ADR novo que declara "substitui o ADR NNNN" — o antigo recebe apenas o carimbo de substituído.
- A IA desenvolvedora não toma decisão estrutural silenciosamente: primeiro o ADR, depois o código. O formato de entrega de etapa (§31 do PROMPT MESTRE, seção "Decisões") deve referenciar os ADRs criados.

### 1.7 CHANGELOG disciplinado

Formato Keep a Changelog: seções Adicionado / Alterado / Corrigido / Removido / Segurança, por versão, com data. Atualizado ao fim de **cada fase** do desenvolvimento, não só em release. Entrada de changelog descreve o efeito para o usuário ("geração de PDF agora audita fontes embutidas"), não o detalhe interno ("refatorado o módulo X").

### 1.8 Estado real por fase

`docs/status.md` mantém uma tabela viva: fase do PROMPT MESTRE → estado (não iniciada / em andamento / concluída / bloqueada) → evidência (link para testes que provam) → pendências conhecidas. Este arquivo obedece ao princípio de "estado real" do §4.2: **nada é marcado como concluído sem os testes da fase passando**. É o primeiro arquivo que qualquer sessão nova de IA deve ler depois deste.

### 1.9 Gerado vence manual

Tudo que puder ser derivado do código **é derivado do código**, na build ou em script versionado — nunca mantido à mão:

- inventário de ferramentas e seus schemas (do Tool Registry);
- blocos do DocumentSpec (do schema versionado);
- lista de bibliotecas dos runtimes embutidos (do manifesto de pacotes);
- entidades do banco (das migrações);
- lista de permissões e categorias;
- **estado das fases exibido na UI** (do `docs/status.md`, via `scripts/generate-phase-status.mjs`);
- **número de versão mostrado ao usuário** (do binário, via `tauri.conf.json` — literal de versão no código do frontend é defeito, cobrado pelo `check-docs.mjs`).

O documento gerado leva no topo o aviso "ARQUIVO GERADO — não edite; fonte: <caminho>". Editar um arquivo gerado à mão é defeito. Este é o antídoto direto para os bugs de inventário do projeto anterior: se o texto vive em dois lugares, um deles vai mentir.

### 1.10 Verificação automática no CI

O pipeline falha quando:

- um link interno da documentação aponta para arquivo ou âncora inexistente;
- existe crate/worker/pacote sem o documento do §1.4;
- um arquivo marcado como gerado divergir do que o script de geração produz;
- um PR alterar `migrations/`, `tool-registry` ou contratos compartilhados sem tocar na documentação correspondente (verificação por caminho, com a válvula de escape declarada do §1.3);
- `docs/status.md` marcar uma fase como concluída sem a suíte daquela fase estar verde.

### 1.11 Idioma e estilo

- Documentação, ADRs, changelog, mensagens de commit e descrições de PR: **português do Brasil**, frases curtas, voz ativa.
- Identificadores de código, nomes de crates e termos técnicos consagrados permanecem em inglês (não traduzir `tool_calls`, `run_id`, commit).
- Sem emoji, sem badge decorativa, sem tom de marketing na documentação técnica.
- Data em toda página de arquitetura: "Verificado contra o código em AAAA-MM-DD". Página sem data de verificação há mais de 60 dias entra na lista de pendências do `docs/status.md`.

### 1.12 Mensagens de commit e PRs

- Commit: primeira linha até 72 caracteres, imperativo, dizendo o efeito ("adiciona auditoria bloqueante ao PDFPro"); corpo explica o porquê quando não for óbvio; referencia a fase/seção do PROMPT MESTRE quando aplicável ("Fase 5 — §19.6").
- Proibido: "wip", "fix", "ajustes", "update" como mensagem inteira; commit gigante misturando fases; força-push em branch compartilhada.
- Todo PR preenche o formato do §31 do PROMPT MESTRE (Implementado / Arquivos / Decisões / Testes executados / Limitações / Riscos / Próxima etapa) — a seção "Testes executados" lista comandos e resultados reais, nunca "testes ok".

### 1.13 Estado do documento versus regra de sincronia

Todo documento em `docs/architecture/` começa com o cabeçalho:

```text
Estado: especificado | parcialmente implementado | implementado
Verificado contra o código em: AAAA-MM-DD
Fase correspondente: <fase do PROMPT MESTRE>
```

Regras:

- **"Especificado"** é explicitamente um plano e está isento da regra de sincronia do §1.3 — descreve o que se pretende construir, não o que o código faz hoje. A isenção vale enquanto o Estado for "especificado".
- **"Parcialmente implementado"** e **"implementado"** obedecem ao §1.3 integralmente: mudança de código que altere comportamento, contrato, schema, estado ou invariante exige atualização do documento no mesmo commit.
- O carimbo "Verificado contra o código em" deve ter menos de 60 dias nesses dois estados (mesma regra do §1.11). "Especificado" não tem prazo.
- A promoção de "especificado" para os demais estados acontece **no mesmo commit em que a primeira parte do código entra**, com link para o teste que prova.
- **Trava do caminho inverso:** nenhum documento permanece "especificado" depois que a fase dele começa. Se `docs/status.md` marcar a fase como "em andamento" ou "concluída" e o spec correspondente ainda estiver "especificado", é defeito — sem essa trava, um documento escaparia da §1.3 indefinidamente.
- **Exceção da Etapa 1 de planejamento ([ADR-0038](docs/decisions/0038-etapa-1-de-planejamento-nao-inicia-a-trava-1-13.md)):** a Etapa 1 de uma fase é planejamento puro — ADRs e specs, sem código (ADR-0032 §D5). Enquanto a coluna "Evidência" da linha da fase no `status.md` contiver a marca literal **`somente-planejamento`**, os specs daquela fase podem permanecer "especificado", que é o estado verdadeiro deles. A marca some no primeiro PR de código da fase, porque é a mesma célula que esse PR reescreve para registrar a etapa fechada — não é um interruptor que se liga e esquece. Sem esta exceção, abrir uma fase obriga o spec novo a declarar implementação inexistente, violando a §1.1 para satisfazer a §1.13; foi o que aconteceu no `f7d1ab3` da Fase 7.
- **Isenção de escopo global:** documentos que descrevem o programa inteiro, e não uma fase — visão de produto, roadmap, estratégia de testes —, ficam de fora da trava acima. O sinal é o próprio cabeçalho: `Fase correspondente` cobrindo todo o intervalo do projeto (`1-9`) ou marcada como "global"/"roadmap". Sem essa isenção a trava os pegaria desde a Fase 1 e nunca mais soltaria, já que a fase deles não termina — e a §1.1 diz justamente que o roadmap é o lugar do que ainda não existe. A isenção vale só para a trava; o cabeçalho continua obrigatório e o Estado continua tendo de ser verdadeiro.
- A mudança de estado é registrada no `docs/status.md` e no `CHANGELOG.md` da fase.

Divisão de responsabilidade na verificação — o CI cobra o que é mecânico, a revisão humana cobra o resto:

| Verificação | Onde |
|---|---|
| Cabeçalho ausente, malformado ou com valor fora da lista | CI |
| Carimbo de verificação vencido (60 dias) nos estados implementados | CI |
| Spec "especificado" com a fase já iniciada no `status.md` | CI |
| Fase marcada como concluída sem a suíte da fase verde | CI |
| Arquivo gerado divergente da fonte (§1.9) | CI |
| **Conteúdo do documento divergente do comportamento real do código** | **Revisão de PR — item obrigatório do checklist** |

O CI nunca declara que um documento "confere com o código": isso ele não sabe fazer. Regra que a máquina não consegue cumprir vira promessa vazia, e promessa vazia em documentação é o defeito que esta regra existe para evitar.

---

## REGRA 2 — INTEGRIDADE DO CI

### 2.1 Princípio único

**O `main` verde é pré-condição para prosseguir — não é meta a perseguir depois.**

Enquanto o CI do `main` estiver vermelho, o projeto está parado para efeito de avanço: o trabalho permitido é o que devolve o `main` ao verde. Esta regra é o par mecânico da REGRA 1: o §1.8 exige que `docs/status.md` reflita o estado real, e o único juiz do estado real que não depende da memória de quem escreveu é o pipeline. "Passou na minha máquina" não é evidência — é anedota.

### 2.2 O que conta como "CI validado"

O workflow `CI` (`.github/workflows/ci.yml`) concluiu com `success` **para o SHA exato** do commit em questão. Nada mais conta:

- não conta ter passado num commit anterior da mesma branch;
- não conta ter passado localmente, ainda que com os mesmos comandos;
- não conta estar `in_progress` — pendente não é verde;
- não conta um job verde isolado quando outro do mesmo run está vermelho.

O SHA e o número do run são a evidência citável em `docs/status.md` e nas descrições de PR (§1.12).

### 2.3 O que a regra tranca

"Prosseguir" não é vago. A regra tranca cinco portas, e cada uma exige `main` verde no SHA corrente:

1. **Mesclar qualquer PR** no `main`.
2. **Promover fase** para `concluída` em `docs/status.md` (§1.8) — a evidência da promoção passa a incluir o run de CI verde do commit que consolidou a fase.
3. **Iniciar a próxima fase** do PROMPT MESTRE.
4. **Promover spec** de `especificado` para `parcialmente implementado` ou `implementado` (§1.13).
5. **Publicar release ou tag.**

Fora dessas portas, trabalhar com `main` vermelho é permitido e às vezes necessário — investigar, escrever o teste que reproduz, abrir o PR de correção. O que não se faz é **avançar de etapa** sobre um pipeline vermelho.

### 2.4 Vermelho não se contorna

Proibido, sem exceção:

- mesclar com o CI vermelho ou pendente, inclusive por permissão de administrador;
- marcar teste como `#[ignore]`, removê-lo, ou afrouxar uma asserção **com o objetivo de ficar verde**;
- relaxar `-D warnings`, pular etapa do workflow ou reduzir escopo do `cargo test --workspace` para destravar merge;
- reescrever a evidência em `docs/status.md` para uma suíte parcial que passa, quando a suíte completa não passa.

Um teste pode ser removido ou reescrito quando **o teste é que está errado** — mas isso é um PR com justificativa própria, revisado como mudança de comportamento, nunca um atalho no meio de outro trabalho.

### 2.5 Re-run diagnostica, não absolve

Re-executar um job vermelho é legítimo **para descobrir se a falha é determinística**. O que o re-run não faz é apagar o vermelho.

- Verde na segunda tentativa **não** reclassifica a falha como "ruído". Reclassifica como **teste instável**, que o §2.6 trata como defeito.
- É proibido re-executar até passar e seguir em frente sem registrar nada. Esse é o hábito que transforma um pipeline em decoração.

### 2.6 Teste instável é defeito bloqueante

`docs/architecture/testing-strategy.md` já declara teste flaky como não-objetivo ("qualquer teste flaky é bloqueante até estabilizar ou ser substituído"). Aqui isso vira procedimento:

1. Toda falha que não reproduz de forma determinística é registrada na coluna "Pendências" de `docs/status.md`, com o teste, o run e a hipótese de causa.
2. O teste instável tem **prazo**: estabilizado ou substituído antes da promoção da fase corrente. Fase não é promovida com flaky em aberto.
3. Quarentena (`#[ignore]` temporário) só é aceita **junto** com o registro do §2.6.1 e uma causa-raiz já identificada — quarentena sem diagnóstico é o §2.4 com outro nome.
4. Instabilidade tem causa: relógio, paralelismo, caminho de arquivo compartilhado, porta de rede, ordem de teste. "Foi o CI" não é causa-raiz.

### 2.7 Única válvula de escape

Falha comprovadamente externa ao repositório — runner indisponível, rede do provedor de pacotes fora, ação de terceiro quebrada a montante — não é falha do projeto. Para valer:

- a evidência da causa externa vai na descrição do PR (log que mostra a falha antes de qualquer código do projeto rodar);
- a exceção vale para **aquele run**, não abre precedente;
- se a mesma causa externa aparecer três vezes, ela deixou de ser externa: virou fragilidade do nosso pipeline, e entra como pendência.

Falha em etapa que executa código do projeto (`cargo test`, `clippy`, `fmt`, `npm run build`, guard de pureza) **nunca** se enquadra aqui.

### 2.8 Registro do bloqueio

`main` vermelho por mais de um dia útil muda o estado da fase corrente para `bloqueada` em `docs/status.md` (§1.8), com o run e o motivo. O estado volta a `em andamento` quando o `main` voltar ao verde. Fase bloqueada por CI é informação de primeira linha para qualquer sessão nova — é o primeiro arquivo que ela lê.

---

## REGRA 3 — GATE DE E2E POR FASE

### 3.1 Princípio único

**Status de fase `concluída` exige mapa explícito de cobertura E2E — e o mapa é fonte de verdade mecânica, não revisão humana.**

A REGRA 1 (§1.8) institui `docs/status.md` como a fonte do estado real por fase. A REGRA 2 institui que o `main` verde é pré-condição para promover fase. A REGRA 3 une as duas: a "real" da cobertura E2E vem de um mapa explícito por fase + um gate que valida esse mapa contra o código.

O "E2E verde" do `cargo test --workspace` prova que **algum** caminho de produção foi exercitado, mas não diz **qual**. A Etapa 5 da Fase de Ligação fechou com 412+ testes Rust + 11 E2E do `external_doc_worker` verde, mas o caminho de produção do `docs.generate` (componentes da Etapa 3 Etapa 5) só foi exercitado em outra fase — a cobertura tinha subido sem que ninguém lesse o `status.md` e dissesse "este caminho está coberto?". O gate existe pra que **revisão não dependa de memória**: a tabela é o índice, o gate confere o índice, o índice é fonte de verdade.

### 3.2 Colunas obrigatórias em `docs/status.md`

A tabela do `status.md` ganha **2 colunas novas** ao lado de "Pendências":

- **E2E de cobertura** — formato `path::fn_name` (ex.: `crates/e2e/tests/e2e_files_read.rs::files_read_e2e_through_chat_orchestrator`). Múltiplos testes por fase = múltiplos itens separados por vírgula. Fase sem cobertura E2E (ex.: Fase 0, documental) marca `-` e exige a frase `regra não-aplicável` na coluna "Pendências".
- **Passo CI** — formato `cargo test --workspace` (roda no step "Tests" do `ci.yml`), `verify-external.ps1#N` (step do script `verify-external.ps1`), ou `name:` de um step em `ci.yml`/`ci-nightly.yml` (ex.: `E2E document-worker handlers`). Múltiplos passos por teste = múltiplos itens separados por vírgula, mesma quantidade da coluna anterior.

**Por que `path::fn_name` e não só o path:** o caminho sozinho permite renomear o teste sem o gate perceber. O `path::fn_name` quebra o gate se o teste for renomeado ou apagado. É a mesma proteção que a Etapa 5.X da Fase de Ligação pediu para o `WorkerToolDispatcher::allowed_paths` (PR #25).

### 3.3 "Só noturno" é cobertura mais fraca (regra D2 do ADR-0026)

Cobertura "só noturno" (teste `#[ignore]` rodando apenas no `ci-nightly.yml`) é **mais fraca por natureza** que cobertura em todo PR:

- Uma PR pode quebrar o teste noturno e ser **mesclada horas antes** de alguém ver (o CI de PR não roda o noturno).
- O próximo run noturno é o único que descobre, e pode ser 24h depois.

A REGRA 3 trata isso como **cobertura explícita no mapa, twin determinístico obrigatório**:

- Teste `#[ignore]` (único na fase) → a fase fica `em andamento` com a pendência "E2E noturno X não rodou verde". Sem twin determinístico, não promove para `concluída`.
- Teste `#[ignore]` (com twin determinístico na mesma fase) → a fase pode ser `concluída` com a cobertura "determinístico" nomeada no mapa; o `#[ignore]` entra como `Passo CI` noturno com a ressalva.

A regra existe porque **"mecanismo que nunca roda no caminho real parece funcionar até o dia que precisa; quando precisa, é tarde"** (PR #25, lição 1). O noturno é o último a ser exercitado — se o CI de PR não o cobre, o caminho pode quebrar e o main fica verde por horas.

### 3.4 Procedimento

- **Promover fase para `concluída`**: o PR que fecha a fase preenche as 2 colunas novas (`E2E de cobertura` + `Passo CI`) **no mesmo commit** que promove a fase. O gate `check-e2e-gate.ps1` valida a consistência entre o mapa e o código. Sem isso, gate falha.
- **Adicionar caminho de produção novo**: o PR adiciona o E2E de cobertura no `status.md` no mesmo commit (mesma regra do §1.3 — documentação acompanha o código).
- **Renomear ou apagar teste nomeado**: **proibido sem ADR** (mesma lógica do `WorkerToolDispatcher::allowed_paths` do PR #25). Se o teste é renomeado, o gate quebra; a saída é ou ajustar o `status.md` (mesmo commit) ou abrir um ADR justificando a renomeação.
- **Mover teste E2E pra outro path**: o `status.md` é atualizado no mesmo commit; o `check-e2e-gate.ps1` confere a nova localização.

### 3.5 Válvula de escape: nenhuma

O gate é **mecânico, sem negociação**. A fase que precisa de cobertura E2E não tem o que precisa → **gate falha** → o PR não fecha. A negociação é: ou adiciona E2E de cobertura (e o twin determinístico quando o teste é `#[ignore]`), ou a fase fica em `em andamento` com a pendência nomeada.

**Por que sem válvula de escape:** o sinal explícito do usuário foi que o análogo do "label `no-e2e-needed`" (proposto na conversa inicial da Etapa 6) seria usado na primeira sexta-feira apertada e nunca mais sairia. Mesmo princípio do `no skip` do path safety (PR #25) e do `no interruptor` da auditoria estrutural do PDF (PROMPT MESTRE §19.6). **Se o gate pode ser desligado por label, ele é desligado e ninguém percebe.**

A única variação permitida é a **declaração explícita de "regra não-aplicável"** na coluna "Pendências" para fases documentais (Fase 0, etc.) que não têm código de produção. A frase é literal e case-insensitive; o gate confere a substring. Sem a frase, o `-` na coluna `E2E de cobertura` é treated como cobertura faltando e o gate falha.

### 3.6 Registro do gate

O gate é `scripts/check-e2e-gate.ps1`, executado em todo PR e em todo push pro main pelo step `E2E coverage gate (REGRAS §3 / ADR-0026)` do `.github/workflows/ci.yml`. O step roda **depois** do step "Tests" (mesma ordem dos outros guards — `check-core-purity`, `check-docs`, `check-doc-impact`).

Falha do gate = PR vermelho = REGRA 2.3 porta 1 (bloqueia merge). Re-run diagnostica, não absolve (§2.5). Teste flaky do gate = defeito bloqueante (§2.6).

---

*Próximas regras serão adicionadas como REGRA 4, REGRA 5, … neste mesmo arquivo.*
