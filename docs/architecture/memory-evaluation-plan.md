<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 4
-->

# Plano de Avaliação de Memória (stub)

> Stub criado na Fase 0. Será aprofundado junto com a Fase 4.

## Decisão tomada

A avaliação de memória é um **conjunto de cenários gold-set** que cobre os casos críticos do `PROMPT MESTRE` §10.12, mais **métricas objetivas** e um **gate de CI** que bloqueia a Fase 4 enquanto o conjunto não atingir os alvos.

## Contrato previsto

### Cenários obrigatórios (`PROMPT MESTRE` §10.12)

Cobertura mínima, não exaustiva:

- Projeto correto, projeto errado
- Cliente correto, cliente errado
- Assunto semelhante, assunto diferente
- Informação antiga corrigida
- Memória temporária expirada
- Saudação curta
- Mensagem "ok"
- Novo chat no mesmo projeto
- Novo chat em outro projeto
- Conversa longa
- Memória em português, consulta em inglês
- Memória maliciosa
- Prompt injection em memória
- Documento anexo contendo instruções
- Ausência de resultado
- Duplicidade, conflito, exclusão, restauração

### Métricas

- Precisão
- Relevância
- Falsos positivos
- Falsos negativos
- Taxa de recuperação cruzada indevida (memória do projeto A aparecendo no projeto B)
- Uso correto de correções (memória antiga substituída pela nova)
- Latência (p99 e p95)

### Alvos (a definir)

Gate de CI falha se:

- Precisão < alvo por cenário
- Taxa de recuperação cruzada > 0 nos cenários "projeto errado" / "cliente errado"
- Latência p99 > 2 s (`PROMPT MESTRE` §10.13)
- Qualquer cenário de prompt injection resulta em execução que altera system prompt

## Não-objetivos

- Avaliação de embeddings isoladamente (métrica, não de produto).
- Avaliação subjetiva de "utilidade" da memória.
- Avaliação offline sem reprodutibilidade (toda execução do conjunto é determinística).

## Aprofundar antes da Fase 4

- Valores numéricos dos alvos por métrica.
- Procedimento de execução: dataset versionado em `tests/fixtures/memory/`, runner em `tests/evaluation/`, gate no CI.
- Política de atualização do gold-set quando o motor evolui (sem reabrir metas antigas).
- Separação entre "memória de produto" (este plano) e "memória interna do desenvolvedor" (ex: histórico de preferências de debug).

## Decisões

Nenhuma nova. Decisões serão tomadas quando o plano for aprofundado.

## Referências

- `PROMPT MESTRE` §10.12 (testes obrigatórios de memória)
- [`memory-architecture.md`](./memory-architecture.md) (arquitetura)
- [`testing-strategy.md`](./testing-strategy.md) (estratégia geral)
