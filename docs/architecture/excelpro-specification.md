<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-28
Fase correspondente: 5
-->

> Última verificação: 2026-07-28. Reflete a Etapa 1 da Fase 5 — o
> catálogo de blocos (`DocumentSpec` com `Kpis`, `Table`, `Chart`,
> `KeyValue`) já está definido e validado; a restrição "Spreadsheet
> aceita apenas `Kpis`/`Table`/`Chart`" já é regra semântica
> validada em runtime. O ExcelPro **em si** (renderização via
> `openpyxl`, fórmulas auditáveis, memória de cálculo como aba
> oculta) entra na Etapa 4. A regra de moeda brasileira e datas
> brasileiras (`PROMPT MESTRE` §18.2) será definida na Etapa 4 com
> ADR próprio.

# Especificação do ExcelPro Kit (stub)

> Stub criado na Fase 0. Aprofundado na Etapa 1 da Fase 5 (catálogo
> de blocos + restrição Spreadsheet); renderização via `openpyxl`
> entra na Etapa 4.

## Decisão tomada

- Geração de `.xlsx` profissionais, funcionais e auditáveis a partir de `DocumentSpec` (`PROMPT MESTRE` §18).
- **Padrão visual** com azul escuro, verde de sucesso, cinza claro, branco, espaço em branco, cards de KPI, cabeçalhos destacados (`PROMPT MESTRE` §18.2).
- **Células de entrada** com preenchimento amarelo, fonte azul e proteção seletiva — distinguíveis visualmente das calculadas (`PROMPT MESTRE` §18.2).
- **Fórmulas auditáveis**: referências consistentes, sem valores fixos escondidos, fonte única da verdade, localização do Excel considerada, sem substituir fórmula por valor sem justificativa (`PROMPT MESTRE` §18.4).
- **Memória de cálculo** explícita: dados informados, importados, fórmulas, premissas, cálculos, ajustes manuais, pendências (`PROMPT MESTRE` §18.6).
- **Revisão multimodelo** sobre o arquivo real: abre, inspeciona fórmulas, examina estilos, analisa gráficos, encontra inconsistências, modifica, salva nova versão, produz relatório (`PROMPT MESTRE` §18.7).

## Contrato previsto

O ExcelPro consome `DocumentSpec` (com tipo `DocumentType::Spreadsheet` e blocos específicos — `Kpis`, `Table`, `Chart`) e produz `.xlsx` real no disco. Validação antes de marcar `valid` (§18.5): planilha abre, abas existem com nomes corretos, fórmulas presentes, sem referências quebradas, sem células com erro, intervalos válidos, tabelas, gráficos, validações, células protegidas, totalizações, saldos, coerência, sem duplicados, sem linhas vazias indevidas, sem colunas fora de ordem, sem corrupção.

## Modelos contábeis e financeiros previstos (`PROMPT MESTRE` §18.3)

DRE, balanço patrimonial, DFC pelo método direto, conciliação bancária, fluxo de caixa, orçamento, contas a pagar, contas a receber, análise de débitos, razão, balancete, projeções, dashboards, controle fiscal.

## Recursos mínimos (`PROMPT MESTRE` §18.1)

Tabelas estruturadas, fórmulas, referências, validação de dados, listas suspensas, filtros, congelamento de painéis, formatação condicional, gráficos, dashboards, indicadores, segmentação lógica, proteção, células de entrada, células calculadas, comentários, instruções, impressão, cabeçalhos, rodapés, áreas de impressão, moeda brasileira, datas brasileiras, percentuais, números negativos, conciliação.

## Não-objetivos

- Editor visual completo de Excel dentro do app.
- Macros VBA ou automação COM.
- Tabelas dinâmicas complexas com fontes de dados externas.
- Conexão ao vivo com banco de dados dentro do `.xlsx`.
- Trading ou simulação financeira (o app gera planilhas, não opera mercados).

## Aprofundar antes da Fase 5

- Catálogo de estilos de formatação condicional no `openpyxl`.
- Estratégia de proteção de células: onde o app consegue colocar senha, e onde depende do Office.
- Política de `moeda brasileira` e `datas brasileiras` vs. localização do Excel do usuário (a planilha abre com locale `pt-BR`).
- Formato de exportação da "memória de cálculo" para outro modelo revisar (§18.6) — embutido no `.xlsx` como aba oculta? Documento auxiliar?
- Política de revisão multimodelo: o que o segundo modelo recebe (o arquivo, um `DocumentSpec` modificado, ambos?).
- Procedimento de teste: `.xlsx` é aberto por `openpyxl` em modo round-trip; abas e fórmulas críticas são validadas.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado.

## Referências

- `PROMPT MESTRE` §18 (ExcelPro)
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`pdfpro-specification.md`](./pdfpro-specification.md) (fidelidade Excel → PDF)
- `docs/development-roadmap.md` (Fase 5)
