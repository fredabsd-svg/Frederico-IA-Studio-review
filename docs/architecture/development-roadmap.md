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
| 7 | Execução isolada (Modo Desenvolvedor: núcleo) | Sandbox (Jail + Job Object + Restricted Token + env zeroed + proxy de rede), runtimes portáteis (Python + Node), `exec.python` / `exec.node` / `exec.shell` no Tool Registry, `files.write` / `files.edit` / `files.list` no Tool Registry | Sandbox isola execução (teste de negação verde); `pip install` e `npm install` rodam via proxy com allowlist; `I1` do threat model fecha com teste de regressão; `exec.shell` com `Denylist` recusa comandos destrutivos; **rede do sandbox é `#[ignore]` (noturno) por natureza** |
| 8 | Modo Desenvolvedor integrado | Git portátil (biblioteca linkada, nunca o `git` do PATH), GitHub (auth + push + PR), diff, projetos, marcos nomeados | PR criado pelo app (E2E noturno — `#[ignore]`) **com o `CI Nightly` provado verde e o run citado no `status.md`**; twin determinístico do E2E de GitHub rodando em todo PR; diff viewer funcional; projetos com workspace dedicado; marcos nomeados sobre o `git-engine` |
| 9 | Produção | Testes completos, segurança, assinatura, instalador, atualização, documentação, máquina limpa, versão estável | Todos os critérios de aceite do `PROMPT MESTRE` §32 marcados, instalador roda em máquina limpa |

**Alterado em relação ao plano original (2026-08-08, Etapa 1 da Fase 7, ADR-0032):**

- **Fase 7** mudou de escopo: sai "Git, GitHub, diff, projetos, checkpoints, testes". Entra "execução isolada" (sandbox + runtimes + file ops + exec tools). O critério de "PR criado pelo app" migra para a Fase 8.
- **Fase 8** mudou de escopo: absorve Git, GitHub, diff, projetos, checkpoints. O conteúdo "Copiloto, tarefas, refinamento" (Nino + sugestões + acessibilidade) vira subdivisão da Fase 8, não a fase inteira.
- **Fase 8 herda a dependência da Fase 7** (sem sandbox da Fase 7, o `exec.shell` da Fase 8 é inseguro). Pré-requisito atualizado: `8 → 3 + 4 + 6 + 7`.
- **Fase 7 ganha E2E de noturno** (`pip install`, `npm install` rodam contra a rede real): twin determinístico no PR + `#[ignore]` noturno, regra D2 do ADR-0026.

**Alterado em relação ao plano (2026-08-16, Etapa 1 da Fase 8, [ADR-0038](../decisions/0038-fase-8-escopo-e-etapas.md)):**

- **Copiloto (Nino) e tarefas saem da Fase 8.** É uma terceira natureza — produto e interação —, com critério de aceite qualitativo (`PROMPT MESTRE` §24.1) que não fecha por teste. Colá-lo a uma fase que já tem primitiva local e integração externa autenticada é o padrão que o ADR-0032 desmontou. Vira item próprio abaixo.
- **"PR criado pelo app" ganha pré-condição**: o `CI Nightly` precisa de um run verde citável antes da promoção da fase. Em 2026-08-16 ele acumulava 12 falhas consecutivas desde 2026-08-05 por secret ausente — a cobertura noturna era inexistente, com aparência de cobertura.
- **Checkpoints viram "marcos de projeto"** ([ADR-0041](../decisions/0041-projetos-e-checkpoints-nomeados.md)): o `CheckpointRepo` que o ADR-0032 §D2 mandava estender nunca foi escrito, e o checkpoint de run da migração `0003` tem semântica diferente (morre com o run).

## Itens com fase própria a definir

Trabalho reconhecido, com dono documental, sem fase atribuída — para que nenhum spec precise especular sobre o "quando":

| Item | Origem | Por que não tem fase ainda |
|---|---|---|
| **Copiloto (Nino) e tarefas** | `PROMPT MESTRE` §24.1; tirado da Fase 8 pelo ADR-0038 §D1 | Critério de aceite qualitativo; precisa de um ADR que o torne verificável antes de virar fase |
| **Filtro de rede no nível de processo (WFP/WDAC)** | Fase 7 — fecharia DNS exfiltration e o bypass por socket raw | Natureza de kernel/política do Windows; exige ADR próprio |
| **OAuth device flow para GitHub** | ADR-0040, alternativa 1 | Exige registrar um GitHub App e manter `client_id` do produto — decisão de produto, não de engenharia |
| **Retomada de run a partir de checkpoint** | Tabela `checkpoints` (migração `0003`) sem dono em código | Nada a consome hoje; construir por simetria seria mais estrutura sem dono |

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
