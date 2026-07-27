<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 4
-->

# Arquitetura de Memória (stub)

> Stub criado na Fase 0. Será aprofundado antes do início da Fase 4 (Memória e continuidade).

## Decisão tomada

- **Recuperação híbrida** combinando escopo + recência + FTS5 + similaridade semântica + importância + confirmação do usuário (`PROMPT MESTRE` §10.6).
- **Embeddings por provedor configurado** por padrão; modelo local ONNX apenas sob demanda, **nunca** na inicialização do app (`PROMPT MESTRE` §10.13).
- **Recuperação semântica com prazo máximo de 2 s**; estourado, a execução segue sem ela e o fato é registrado — memória nunca trava uma resposta (`PROMPT MESTRE` §10.13).
- **Busca lexical FTS5 funciona mesmo sem embeddings** disponíveis (`PROMPT MESTRE` §10.13).
- **Mensagens recentes da conversa têm prioridade** sobre memória semântica (`PROMPT MESTRE` §10.4).
- **Zero memórias é resposta válida** — não se recupera conteúdo só para preencher espaço (`PROMPT MESTRE` §10.7).
- **Memória é dado, não instrução** — não pode alterar system prompt, permissões, identidade do agente (`PROMPT MESTRE` §10.10).

## Contrato previsto

Tipo `MemoryRecord` (`PROMPT MESTRE` §10.3) — `id`, `scopeType`, `scopeId`, `type`, `content`, `sourceType`, `sourceId`, `confidence`, `importance`, `createdAt`, `updatedAt`, `lastUsedAt`, `expiresAt`, `supersededBy`, `userConfirmed`, `userPinned`, `active`.

**Escopos** (`PROMPT MESTRE` §10.1): `profile`, `preference`, `assistant`, `project`, `client`, `conversation`, `document`, `task`, `session`.

**Tipos** (`PROMPT MESTRE` §10.2): `preference`, `fact`, `decision`, `correction`, `project_instruction`, `client_context`, `procedure`, `delivery_pattern`, `temporary`, `conversation_summary`, `document_reference`, `user_pinned`.

## Não-objetivos

- Memória como busca puramente vetorial.
- Auto-salvamento de toda mensagem como memória.
- Memória alterar system prompt, permissões ou identidade do agente.
- Sincronização de memória entre dispositivos na v1.
- Memória como feature exposta ao usuário final como "base de conhecimento editável" — o usuário gerencia via UI, mas não é o caso de uso principal.

## Aprofundar antes da Fase 4

- Algoritmo de pontuação final: pesos exatos para cada fator da fórmula do `PROMPT MESTRE` §10.6.
- Política de classificação de candidatos a memória (`PROMPT MESTRE` §10.9): o que vira memória, em que escopo, com que importância inicial.
- Critérios e métricas do conjunto de avaliação do `PROMPT MESTRE` §10.12 (precisão, relevância, falsos positivos, falsos negativos, recuperação cruzada indevida, uso de correções, latência).
- Estratégia de reindexação quando o modelo de embeddings muda (`PROMPT MESTRE` §10.13) — com progresso visível e uso do app durante o processo.
- Critérios de expiração e descarte de memórias temporárias.
- Painel de memória na UI: o que mostrar, como explicar uma recuperação (`PROMPT MESTRE` §10.11).
- Política de `supersededBy` quando o usuário corrige algo (`PROMPT MESTRE` §10.8).

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado.

## Referências

- `PROMPT MESTRE` §10 (memória), §10.13 (motor de embeddings e indexação)
- [`testing-strategy.md`](./testing-strategy.md) — testes de memória (camada unit + avaliação do §10.12)
- [`security-threat-model.md`](./security-threat-model.md) — I4 (vazamento entre projetos), E2 (memória como instrução)
- `docs/development-roadmap.md` (Fase 4)
