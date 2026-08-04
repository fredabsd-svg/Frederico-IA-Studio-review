# Fase de Ligação (entre Fase 5 e Fase 6): narrativas de release

<!--
Estado: em andamento
Verificado contra o código em: 2026-08-03
Fase correspondente: fase-ligacao (entre 5 e 6)
Carimbo de estado por linha: a tabela abaixo mostra o status real
de cada etapa. A Etapa 5 foi movida pra PRÓXIMA (antes da Etapa 3)
e **fechou** no PR #24 — E2E atravessando a casca prova que
1, 2.A e 2.B funcionam no binário antes de empilhar mais
etapas por cima. A Etapa 7
(`SecurityJailResolver` modo desenvolvedor) foi REMOVIDA desta
fase — depende da Fase 7 do PROMPT MESTRE, e uma fase de
ligação que depende de fase futura nunca fecha. Virou
pendência nomeada dentro da Fase 7.
-->

Índice das narrativas de processo (descrições de PR, lições de
execução) associadas à **Fase de Ligação** do Frederico IA Studio.
Esta fase não é uma fase do PROMPT MESTRE — é o trabalho de
conectar o motor (`crates/` + `workers/`) à casca Tauri
(`apps/desktop/`) que a Fase 5 deixou pronto e a Fase 6 (chat
real) consome.

**Não duplica o `CHANGELOG.md`**, que registra só o efeito pro
usuário (§1.7 do `REGRAS-DO-PROJETO.md`). O que mora aqui é a
história técnica — o que aconteceu em cada PR, quais decisões
foram tomadas no caminho, e o que se aprendeu.

## Índice

| PR | Arquivo | Assunto |
|----|---------|--------|
| #21 | [`pr-fase-ligacao-etapa-1.md`](./pr-fase-ligacao-etapa-1.md) | Etapa 1 — `frederico-app` (camada de composição pura) + `JailResolver` + `ToolContext` + `build_chat_orchestrator` consumido pela casca Tauri. O caminho de produção do Frederico agora executa o que a suíte de testes dos crates já provava. |
| #22 | [`pr-fase-ligacao-etapa-2A.md`](./pr-fase-ligacao-etapa-2A.md) | Etapa 2.A — `DocumentWorkerLauncher` (ADR-0023) com lazy start + restart on death com teto + kill tree no app exit. 3 `tauri::command` novos: `DocumentWorkerStatus` / `DocumentWorkerInvoke` / `DocumentWorkerReset`. Resolvedor de runtime com 3 candidatos. Degradação declarada quando runtime ausente. |
| #23 | [`pr-fase-ligacao-etapa-2B.md`](./pr-fase-ligacao-etapa-2B.md) | Etapa 2.B — trait `WorkerInvoker` no `core` (ADR-0024) + `InvokeError` próprio + `impl` em `WorkerHandle` (orphan rule) + `impl` em `DocumentWorkerLauncher` + bump atômico do `documents: None → Full` + integração dos 3 kits (`WordPro` + `ExcelPro` + `PdfPro`) no `ToolRegistry` via `build_default_tools`. |
| #24 (este PR) | [`pr-fase-ligacao-etapa-5.md`](./pr-fase-ligacao-etapa-5.md) | Etapa 5 — nova crate `frederico-e2e` (14º membro, `publish = false`, path `crates/e2e/tests/`) com 5 testes E2E atravessando o caminho de produção **sem subir a casca Tauri** (consomem `frederico_app::build_chat_orchestrator` direto). Helper `build_orchestrator` em `tests/common/mod.rs` é o ponto único de montagem. 4 testes sempre rodam (`files.read`, degradação declarada, jail por conversa, `docs.generate` com `FakeWorker`); 1 teste `#[ignore]` (`docs.generate` com `DocumentWorkerLauncher` real — único que atravessa o Python e gera `.docx` de verdade). `testing-strategy.md` promovido a `parcialmente implementado` com 2 adições (fronteira do que os E2E cobrem + regra da composição compartilhada). `verify-external.ps1` Step 7 ativa o teste de worker real. |

## Por que Fase de Ligação existe

A Fase 5 fechou o `document-worker` (kit DocumentSpec, 3 formatos
de arquivo, 8 handlers) mas o caminho de produção do app ainda
não consumia nada disso. O `ChatOrchestrator` era construído
inline com 12 args posicionais, `ToolRegistry::new()` ficava
vazio, `PermissionSet::default()` deny-all bloqueava o Passo 5
do `validate_tool_call`, e `Jail::new(std::env::current_dir()?)`
abria o jail no cwd do app — não por conversa.

A Fase de Ligação fecha os 3 itens do diagnóstico do prompt
da fase: composição via `frederico-app` (sem inline na casca),
jail por conversa (sem cwd global), permission set real (sem
deny-all).

## Como esta fase é dividida

| Etapa | Status | Próxima | Bloqueia |
|-------|--------|---------|----------|
| Etapa 1 — composição + jail + tools | **fechada** (PR #21) | Etapa 2 | — |
| Etapa 2.A — `DocumentWorkerLauncher` + resolvedor de runtime + status/invoke direto | **fechada** (PR #22) | Etapa 2.B | nenhuma |
| Etapa 2.B — `docs.generate`/`docs.inspect` no `ToolRegistry` (bump atômico capability+permission) | **fechada** (PR #23) | Etapa 5 | nenhuma |
| **Etapa 5** — `tests/e2e/` atravessando a casca (modelo → ChatOrchestrator → ToolRegistry → kit → WorkerInvoker → document-worker → arquivo) | **fechada** (PR #24) | — | nenhuma |
| Etapa 3 — `MemoryExtractor` + embedding adapter reais | não iniciada | Etapa 1 | nenhuma |
| Etapa 4 — decidir `frederico-agent-engine` | não iniciada | nenhuma | nenhuma |
| Etapa 6 — regra de "definição de pronto" + gate CI | não iniciada | nenhuma | nenhuma |

**Ordem executada:** Etapa 5 antes da Etapa 3 — a Etapa 5
**fechou no PR #24** (4 de 6 etapas). As Etapas 1, 2.A e
2.B estavam validadas pelo `cargo test --workspace`, mas o
caminho end-to-end (modelo → casca → tool → kit → worker →
arquivo) só foi provado agora — 4 testes com `FakeWorker` +
1 teste com `DocumentWorkerLauncher` real (`#[ignore]`,
ativado pelo `verify-external.ps1`). A Etapa 6 (gate CI
"E2E por fase") vai depender do path `crates/e2e/tests/`
fixado nessa etapa. Se algo estiver mal ligado
(recurso, ciclo de vida do worker, bump atômico de permission,
rota no ToolRegistry), você descobre agora, com três PRs de
contexto fresco. Esperar a Etapa 3 primeiro empilha mais uma
etapa por cima antes de validar a base.

**Etapa 7 (`SecurityJailResolver` modo desenvolvedor) REMOVIDA**
desta fase. Depende da Fase 7 do PROMPT MESTRE; uma fase
de ligação que depende de fase futura nunca fecha. Virou
pendência nomeada dentro da Fase 7 (criar
`SecurityJailResolver` em `crates/security/src/jail.rs` com
Job Objects do Windows pra garantir kill-tree do child
quando o parent morre, mesmo em kill -9; substituir
`FileSystemJailResolver` no `setup` da casca).

Etapas 3-6 não dependem umas das outras (em sua maioria) e
podem entrar em PRs separadas conforme a capacidade de revisão
do momento. A regra de PRs empilhadas continua valendo: PR
aberta depois que a anterior entrou em main, sempre.

## O que a Etapa 2.B fecha (resumo)

A Etapa 2.A deixou o `DocumentWorkerLauncher` como um **botão
separado** (caminho de invoke direto, via 3 `tauri::command`),
mas o **caminho do modelo** (ChatOrchestrator → ToolRegistry)
ainda não enxergava `docs.generate`/`docs.inspect`. A Etapa 2.B
fecha esse gap com o trait `WorkerInvoker` no `core` (ADR-0024)
— o `WorkerHandle` (Fase 5) e o `DocumentWorkerLauncher`
(Etapa 2.A) ambos implementam o mesmo trait, e os 3 kits do
`document-kits` + o `WorkerToolDispatcher` consomem `Arc<dyn
WorkerInvoker>`. Bump atômico do `documents: None → Full` na
casca Tauri. Suíte do `frederico-app` continua **32/32 verde**,
workspace (excluindo `process-architecture` com 2 testes de
OCR flaky) **533/533 verde**.
