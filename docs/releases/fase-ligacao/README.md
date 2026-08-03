# Fase de Ligação (entre Fase 5 e Fase 6): narrativas de release

<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-03
Fase correspondente: fase-ligacao (entre 5 e 6)
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
| Etapa 1 — composição + jail + tools | **fechada** (esta PR) | Etapa 2 | — |
| Etapa 2 — `docs.generate`/`docs.inspect` na casca | não iniciada | Etapa 1 | nenhuma |
| Etapa 3 — `MemoryExtractor` + embedding adapter reais | não iniciada | Etapa 1 | nenhuma |
| Etapa 4 — decidir `frederico-agent-engine` | não iniciada | nenhuma | nenhuma |
| Etapa 5 — `tests/e2e/` atravessando a casca | não iniciada | Etapa 1 | nenhuma |
| Etapa 6 — regra de "definição de pronto" + gate CI | não iniciada | nenhuma | nenhuma |
| Etapa 7 — `SecurityJailResolver` (modo desenvolvedor) | não iniciada | Etapa 1 | Fase 7 do PROMPT MESTRE |

Etapas 2-7 não dependem umas das outras (em sua maioria) e
podem entrar em PRs separadas conforme a capacidade de revisão
do momento. A regra de PRs empilhadas continua valendo: PR
aberta depois que a anterior entrou em main, sempre.
