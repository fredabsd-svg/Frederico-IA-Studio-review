<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 1-9 (roadmap global)
-->

# Roadmap de Desenvolvimento

Este documento é o **depósito de tudo que é futuro**. Documentos em `docs/architecture/` que descrevem algo ainda não construído fazem referência a este roadmap para marcar o "quando" — assim, os specs não precisam especular sobre prazo e não violam `REGRAS §1.1` por acidente.

A tabela de estado vivo está em [`docs/status.md`](../status.md). Este roadmap é o **planejado**; o `status.md` é o **real**.

## Fases

| # | Fase | Objetivo (resumo) | Critério de "done" (resumo) |
|---|------|-------------------|------------------------------|
| 1 | Fundação | Tauri + React + TS + Rust + SQLite + migrações + navegação + logs + instalador de desenvolvimento | App abre, navegação funciona, SQLite cria schema inicial, instalador empacota |
| 2 | Chat e provedores | Adaptadores, catálogo, credenciais, streaming, conversas, custos, cancelamento | Conversa com provedor real + provedor simulado, com streaming e cancelamento |
| 3 | Motor de execução e ferramentas | Máquina de estados, Tool Registry, manifestos, permissões, watchdog, checkpoints | **Fluxo vertical 1** do `PROMPT MESTRE` §33 funciona de ponta a ponta com provedor e ferramenta simulados |
| 4 | Memória e continuidade | Resumos, escopos, busca híbrida, explicabilidade, correções, retomada | Conjunto de avaliação do `PROMPT MESTRE` §10.12 atinge precisão alvo |
| 5 | Documentos | Document Worker, Docling, cache, OCR, WordPro, ExcelPro, PDFPro, validações | **Fluxo vertical 2** do `PROMPT MESTRE` §33 funciona com kit Excel; depois Word e PDF |
| 6 | Multimodelo e subagentes | Comparação, conselho, debate, pipeline, especialistas, dependências, cancelamento hierárquico | Pipeline sequencial do `PROMPT MESTRE` §14.4 executa com 2+ modelos reais; subagente herda permissões do pai |
| 7 | Execução isolada (Modo Desenvolvedor: núcleo) | Sandbox (Jail + Job Object + Restricted Token + env zeroed + proxy de rede), runtimes portáteis (Python + Node), `exec.python` / `exec.node` no Tool Registry, `files.write` / `files.edit` / `files.list` no Tool Registry (`exec.shell` foi tentado e descartado em 2026-08-14 — ver `exec-tools-specification.md`) | Sandbox isola execução (teste de negação verde); `pip install` e `npm install` rodam via proxy com allowlist; `I1` do threat model fecha com teste de regressão; **rede do sandbox é `#[ignore]` (noturno) por natureza** |
| 8 | Modo Desenvolvedor integrado | Git portátil, GitHub (auth + push + PR), diff, projetos, checkpoints, copiloto (Nino), tarefas | PR criado pelo app (E2E noturno — `#[ignore]`); diff viewer funcional; projetos com workspace dedicado; checkpoints nomeados; copiloto cumpre `PROMPT MESTRE` §24.1 (1-6) |
| 9 | Produção | Testes completos, segurança, assinatura, instalador, atualização, documentação, máquina limpa, versão estável | Todos os critérios de aceite do `PROMPT MESTRE` §32 marcados, instalador roda em máquina limpa |

**Alterado em relação ao plano original (2026-08-08, Etapa 1 da Fase 7, ADR-0032):**

- **Fase 7** mudou de escopo: sai "Git, GitHub, diff, projetos, checkpoints, testes". Entra "execução isolada" (sandbox + runtimes + file ops + exec tools). O critério de "PR criado pelo app" migra para a Fase 8.
- **Fase 8** mudou de escopo: absorve Git, GitHub, diff, projetos, checkpoints. O conteúdo "Copiloto, tarefas, refinamento" (Nino + sugestões + acessibilidade) vira subdivisão da Fase 8, não a fase inteira.
- **Fase 8 herda a dependência da Fase 7** (sem sandbox da Fase 7, execução de comandos de terminal na Fase 8 é insegura). Pré-requisito atualizado: `8 → 3 + 4 + 6 + 7`.
- **Fase 7 ganha E2E de noturno** (`pip install`, `npm install` rodam contra a rede real): twin determinístico no PR + `#[ignore]` noturno, regra D2 do ADR-0026.

## Pré-requisitos entre fases

```text
2 → 1
3 → 2
4 → 3
5 → 3
6 → 3 + 4
7 → 3
8 → 3 + 4 + 6 + 7
9 → todas
```

A regra é dura: pular pré-requisito exige ADR.

## Marcos verificáveis por fase

- **Fase 3 concluída** ⇒ primeiro fluxo vertical do `PROMPT MESTRE` §33 comprovado por teste E2E em `tests/e2e/`.
- **Fase 5 concluída** ⇒ segundo fluxo vertical do `PROMPT MESTRE` §33 comprovado por teste E2E em `tests/e2e/`.
- **Fase 8 concluída** ⇒ `PROMPT MESTRE` §23.7 (orçamento de desempenho) atinge i5-3570/16 GB.
- **Fase 9 concluída** ⇒ todos os 49 itens de `PROMPT MESTRE` §32 verificados; instalador roda em máquina limpa do `PROMPT MESTRE` §28.5.

## Adiamentos (fora do escopo da v1)

Listados aqui para que specs e READMEs não precisem especular sobre eles:

- **Modo servidor** (VPS Linux, acesso via navegador, multiusuário) — núcleo é desacoplado (ADR-0003), mas a casca servidor não é construída.
- **Marketplace de ferramentas** — apenas o catálogo do `PROMPT MESTRE` §7.11.
- **Treinamento de modelos / fine-tuning** — o app consome modelos, não treina.
- **Sincronização entre dispositivos** — sem nuvem obrigatória.
- **Internacionalização além de pt-BR** — a estrutura é preparada (ver `software-architecture.md`) mas apenas pt-BR é ativado.
- **Emulação textual de tool calling** como produto — apenas experimental, atrás de flag.
- **Compatibilidade com banco/código/API do projeto anterior** — `PROMPT MESTRE` §2 proíbe expressamente.

## Decisões

- [ADR-0001](../decisions/0001-spec-vs-as-built.md) — explica por que specs podem descrever fases ainda não iniciadas sem violar `REGRAS §1.1`.

## Referências

- `PROMPT MESTRE` §29 (fases), §32 (critérios de aceite), §33 (primeira ação)
- [`docs/status.md`](../status.md) — fonte da verdade do que está em andamento hoje
- [`product-vision.md`](./product-vision.md) — âncora de princípios e não-objetivos
