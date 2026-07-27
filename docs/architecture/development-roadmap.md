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
| 7 | Modo desenvolvedor | Projetos, arquivos, diff, sandbox, runtimes, Git, GitHub, testes, checkpoints | Sandbox isola execução, Git portátil embutido, PR criado pelo app |
| 8 | Copiloto, tarefas e refinamento | Nino, sugestões, tarefas, notificações, acessibilidade, desempenho, atualização | Copiloto cumpre `PROMPT MESTRE` §24.1 (1-6), medições de `§23.7` atingem orçamento |
| 9 | Produção | Testes completos, segurança, assinatura, instalador, atualização, documentação, máquina limpa, versão estável | Todos os critérios de aceite do `PROMPT MESTRE` §32 marcados, instalador roda em máquina limpa |

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
