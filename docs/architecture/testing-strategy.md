<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-04
Fase correspondente: 1-9
-->

# Estratégia de Testes

Cinco camadas (`PROMPT MESTRE` §28). Cada invariante declarada nos specs de `docs/architecture/` tem pelo menos um teste que a prova. O CI fecha o ciclo (`REGRAS §1.10` e `§1.13`).

## Camadas

### 1. Unit

- Roda sem I/O, sem rede, sem worker, sem banco.
- Cobertura: tipos, máquina de estados (`agent-state-machine.md` §6.1), permissões (`tool-permission-model.md` §8), schemas, validação de tool calls, normalização de caminhos.
- Framework: `cargo test` no Rust; `vitest` no TS.
- Localização: `#[cfg(test)]` em `crates/*/src/**.rs` e `*.test.ts` em `apps/desktop/src/`.

### 2. Integration

- Roda contra banco SQLite em modo WAL, com workers **simulados em processo** (mesma crate, mas com spawn desabilitado e adapter mockado).
- Cobertura: IPC entre núcleo e workers (com adapter mockado), persistência de runs e checkpoints, recuperação após crash, replay de eventos (`PROMPT MESTRE` §12.6), interseção de inventário.
- Localização: `tests/integration/`.

### 3. E2E

- Roda o app Tauri em modo headless (`tauri-driver`), contra workers reais ou simulados conforme o teste.
- Cobertura: **fluxos verticais do `PROMPT MESTRE` §33** (mensagem → execução → tool call → persistência → recuperação; planilha → revisão multimodelo); medições de desempenho (`PROMPT MESTRE` §23.7); recarga de janela durante execução (`PROMPT MESTRE` §12.6); LGPD (exportar e excluir conta).
- Localização: **`crates/e2e/tests/`** (Etapa 5 da Fase de Ligação, 2026-08-04 — crate dedicada `frederico-e2e` no workspace; o spec original dizia `tests/e2e/`, mas o Cargo só reconhece `tests/` dentro de um package — um diretório `tests/e2e/` na raiz do workspace não é compilado). Caminho fixado aqui pra Etapa 6 poder fazer o gate "E2E que atravessa a casca" por fase. **Não mudar sem ADR.**

### 4. Caos e recuperação (`PROMPT MESTRE` §28.4)

- Encerra forçadamente: app, modelo (corta conexão), stream, worker, ferramenta, document worker, sandbox, pipeline, banco (durante transação controlada).
- Verifica: integridade dos checkpoints, estado recuperável, ausência de duplicidade, arquivos preservados, processos encerrados, **zero processos órfãos**.
- Localização: `tests/recovery/`.

### 5. Máquina limpa (`PROMPT MESTRE` §28.5)

- Windows 10/11 64 bits **sem** Docker, WSL, Node, Python, PostgreSQL, Git, Office, compiladores.
- Apenas o instalador é executado. App abre, executa fluxo 1 e fluxo 2.
- Ambiente provisionado por CI runner descartável (imagem limpa a cada execução).
- Localização: `tests/installer/` (testa o instalador) e pipeline dedicada.

## Mapeamento invariante → teste

Cada spec em `docs/architecture/` lista invariantes com o rótulo "verificável em teste". Cada invariante gera pelo menos um teste na camada apropriada. Tabela mantida em `docs/testing/invariant-coverage.md` (a ser criado quando o primeiro spec for promovido a `parcialmente implementado`).

**Exemplos:**

- "Subagente nunca tem mais permissão que o pai" (`tool-permission-model.md`) → teste unit parametrizado: para todo par `(pai, filho)` válido, `perm(filho) ⊆ perm(pai)`.
- "Toda execução persiste no estado `created` antes de qualquer outra coisa" (`agent-state-machine.md`) → teste de integração que mata o processo imediatamente após submeter mensagem e verifica o `Run` no banco.
- "Path traversal é bloqueado" (`security-threat-model.md` I3) → teste E2E que tenta `..\..\etc\passwd` em cada tool de arquivo.

## Dados de teste

- Fixtures versionadas em `packages/testing-fixtures/`.
- **Provedor simulado**: `crates/provider-engine/src/fake/` — implementa `ProviderAdapter` retornando respostas determinísticas baseadas em fitas (golden files) versionadas.
- **Ferramenta simulada**: `crates/tool-registry/src/fake/` — implementa ferramentas triviais para testes de fluxo (eco, gerador de erro, gerador de arquivo).
- **Worker simulado**: `crates/process-architecture/src/fake/` — implementa o envelope IPC em processo, sem spawn real.

## Desempenho (`PROMPT MESTRE` §23.7)

- Testes E2E medem: tempo até janela visível (< 2 s), tempo até digitar (< 4 s), latência de digitação em conversa longa (< 50 ms).
- **Máquina de referência declarada**: i5-3570, 16 GB, Windows 10 64 bits. CI fixa essa máquina como gate.
- Medições ficam registradas em `docs/testing/perf-baseline.md` por versão, com tolerância de ±10% para evitar fragilidade.

## CI (REGRAS §1.10 + §1.13 + REGRA 2)

O que o pipeline verifica está abaixo. **Quando ele fica vermelho, quem manda é a REGRA 2**: `main` verde é pré-condição para mesclar, promover fase, promover spec, iniciar a fase seguinte ou publicar release; re-run diagnostica mas não absolve; teste instável é defeito bloqueante com prazo.

O pipeline falha em:

- qualquer teste falhando;
- cobertura de invariante nova sem teste;
- quebra de orçamento de desempenho;
- link interno de doc quebrado (`markdown-link-check` ou similar);
- spec com `Estado: implementado` e carimbo de verificação vencido;
- spec com `Estado: especificado` cuja fase está "em andamento" no `status.md` (§1.13);
- fase marcada "concluída" no `status.md` sem a suíte da fase verde (§1.10);
- arquivo gerado divergente da fonte (`REGRAS §1.9`).

### O que o pipeline cobra hoje

`scripts/check-docs.mjs` (passo "Docs guard") e `scripts/check-doc-impact.mjs` (passo "Doc-impact guard") implementam:

| Verificação | Situação |
|---|---|
| Cabeçalho de spec ausente, malformado ou com `Estado` fora da lista | cobrado |
| Carimbo de verificação vencido (60 dias) nos estados implementados | cobrado |
| Trava do §1.13, com a isenção de escopo global | cobrado |
| Crate/pacote sem o documento do §1.4 | cobrado |
| Link interno ou âncora quebrada | cobrado |
| PR que mexe em migrações / tool-registry / contratos sem tocar docs | cobrado, com a válvula do §1.3 |
| Fase "concluída" sem a suíte verde | implícito: `cargo test --workspace` roda antes, no mesmo job — job vermelho reprova o PR inteiro |
| Arquivo gerado divergente da fonte (§1.9) | **não cobrado** — não existe script de geração no repositório para comparar. Quando o primeiro existir, o check entra junto |

## Não-objetivos

- 100% de cobertura de linha cego (cobertura é meio, não fim; o que importa é invariante coberto).
- Testes flaky tolerados (qualquer teste flaky é bloqueante até estabilizar ou ser substituído).
- Testes de UI com snapshot pixel-a-pixel (testa-se comportamento e acessibilidade, não renderização exata).
- Mutation testing na v1 (caríssimo, retorno incerto).
- Teste manual de UI como substituto de E2E automatizado.

## Decisões

Nenhuma nova nesta versão. Decisões a tomar na Fase 1 (com ADR próprio):

- Stack de E2E: `tauri-driver` vs. Playwright vs. custom.
- Provedor simulado: replay de fita (golden files) vs. gerador determinístico (estado em memória).
- Onde rodar testes de "máquina limpa": runner self-hosted, GitHub Actions, Buildkite, ou outro.

## Fronteira do que os E2E cobrem (Etapa 5 da Fase de Ligação, 2026-08-04)

A Etapa 5 da Fase de Ligação fechou `tests/e2e/` na raiz do
repositório, com o objetivo explícito de provar que o **caminho
de produção do Frederico** (modelo → `ChatOrchestrator` →
`ToolRegistry` → kit → `WorkerToolDispatcher` → `WorkerInvoker` →
`document-worker` → arquivo) atravessa o motor e a casca
corretamente, sem subir a casca Tauri (a decisão de não subir o
binário está em ADR-0022 §D4 — a casca e os E2E consomem a mesma
função de composição, `frederico_app::build_chat_orchestrator`).

A escolha de stack (Rust, consumindo `frederico-app` direto) e de
provedor simulado (trait-level fake, ADR-0008) foi a aplicação
das duas decisões já tomadas acima, não decisões novas. **Esta
seção documenta o que os E2E atuais cobrem e o que
deliberadamente fica fora** — pra próxima sessão não ler "E2E
verde" como "documento gerado de verdade" sem checar a fronteira.

### O que os E2E cobrem (a maioria)

A maioria dos testes em `tests/e2e/` consome o `FakeWorker`
in-process (definido em `crates/process-architecture/src/fake.rs`,
spawnado por `WorkerManager::spawn_in_process`). O `FakeWorker`
implementa o envelope IPC sobre `tokio::sync::mpsc` — sem pipes
reais, sem Python, sem `document-worker`. **Ele exercita o
contrato do `WorkerInvoker`** (ADR-0024) e o caminho do motor
(modelo → `ChatOrchestrator` → `ToolRegistry` → kit → dispatcher
→ invoker), mas **para antes do Python**: o que volta do
`invoke` é o que o `FakeWorker` devolve (`{ok: true, echo:
<args>, env_received: ...}`), não é um arquivo gerado de
verdade.

Isso **prova que o motor e a casca estão bem ligados** —
exercita o bump atômico do `documents: None → Full` (ADR-0020
§3 D3), o `Arc<dyn WorkerInvoker>` no `setup`, o
`ToolRegistry` com 3 manifestos, a allowlist, o `PermissionSet`,
o `JailResolver` por conversa, o `RecordingEventSink`, a
persistência de `Message` e `Run` no SQLite, o journal
de eventos. **O que NÃO prova** é que o `document-worker`
Python real gera um `.docx` válido — isso é a próxima fronteira.

### O que 1 teste cobre (o "até o fim")

**Um único teste** em `tests/e2e/` (E2E-5, marcado
`#[ignore]` com mensagem explícita) **vai até o fim do
caminho de produção**: usa o `DocumentWorkerLauncher` real
(Etapa 2.A, ADR-0023) com o `document-worker` Python real
(`workers/document-worker/document-worker.py`), e gera um
arquivo `.docx` de verdade. Esse teste é `#[ignore]` por
default — **não roda em todo PR**. Ele é ativado pelo
`scripts/verify-external.ps1` (que garante o
`bootstrap.ps1` antes) e conta como a evidência "a Fase de
Ligação fechou" — sem ele, a Etapa 5 fecha com
`cargo test --workspace` verde, mas sem nunca ter gerado
um documento pelo caminho do produto.

A próxima evolução dessa fronteira está em duas direções
(pendências nomeadas, não nesta fase):

- **Mais 1 E2E com worker real por kit** (Fase 5 fechou
  `docx`/`xlsx`/`pdf` separadamente; a Etapa 5 só testa
  `docx` ponta-a-ponta — a Fase 6 ou uma Etapa 6 da fase-
  ligação cobre `xlsx` e `pdf`).
- **Subir o binário Tauri** (decidir `tauri-driver` vs.
  Playwright vs. custom) — sai do escopo da Fase de
  Ligação, é a próxima fase intermediária ou a Fase 9.

### Regra da composição compartilhada (o invariante que impede a divergência)

**Os E2E chamam a mesma função que a casca Tauri chama.** O
`apps/desktop/src-tauri/src/main.rs` constrói o
`ChatOrchestrator` via
`frederico_app::build_chat_orchestrator(parts)`; os E2E
fazem o mesmo. As funções de composição
(`build_tool_registry`, `build_default_tools`,
`build_default_allowed_for_run`,
`initial_permission_set`,
`initial_permission_set_for_capable_launcher`) são
**as mesmas** — uma diverge, ambas divergem. Isso é o
que torna o "E2E verde" significativo: não estamos
testando um código que a casca não usa.

A regra é mecânica:

- Toda função de composição que a casca Tauri consome
  mora em `frederico-app`, **nunca** em `apps/desktop`.
- Os E2E importam de `frederico-app`, **nunca** tentam
  `use` em `apps/desktop/src-tauri` (que é binário).
- O `check-core-purity.ps1` garante que `frederico-app`
  continua puro (sem `tauri`, sem `windows`) — o gate
  pega qualquer regressão nessa fronteira.

Próxima sessão que mexer em composição: ler o ADR-0022
§D4 e o `composition.rs:18-24` antes de mover código
pra casca. O §1.3 da `REGRAS-DO-PROJETO.md` exige
atualizar este spec **no mesmo commit** se a fronteira
mudar.

## Referências

- `PROMPT MESTRE` §23.7 (desempenho), §28 (testes), §32 (critérios de aceite)
- `REGRAS-DO-PROJETO.md` §1.10, §1.13
- [`security-threat-model.md`](./security-threat-model.md) — testes de segurança
- [`docs/status.md`](../status.md) — fases e critério de promoção
