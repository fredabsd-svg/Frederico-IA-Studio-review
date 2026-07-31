<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-31
Fase correspondente: 5 (Etapa 4 — ExcelPro v0.1)
-->

> Última verificação: 2026-07-31. Reflete a Etapa 4 da Fase 5 —
> `ExcelProKit` v0.1 implementado no crate `frederico-document-kits`
> (10/10 testes unit verde + 1 E2E full vertical com `openpyxl`
> round-trip). Renderização completa de `DocumentSpec`
> (`DocumentType::Spreadsheet`) em `.xlsx` real, cobrindo
> `Kpis` + `Table` + `Chart`, com formatos numéricos brasileiros
> (moeda R$, percentual, milhar — coluna `BRL`/`PCT`/`THOUSANDS`/`INT`
> via `column_formats` opcional, backward-compat). Bump atômico do
> enum `DocumentFormat::Xlsx` junto com o kit (REGRAS §1.9). O estado
> é **parcialmente implementado** porque a v0.1 entrega o esqueleto
> funcional (dados tabulares, formatos numéricos, sanitização de
> sheet name) mas **não** entrega o pacote visual/interativo que
> o spec promete (§18.1, §18.2). As lacunas estão nomeadas em
> "Lacunas do v0.1 (que impedem 'implementado')" abaixo.

# Especificação do ExcelPro Kit

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

- D1 (Etapa 4): **chart SEM aba `Charts_<n>`** — `openpyxl` tem
  `worksheet.add_chart(...)`, mas isso adiciona uma aba vazia (só com
  a figura) ao lado das abas com dados. Decidido **não** criar aba
  pra chart. Em vez disso:
  1. Procura a próxima `Table` compatível (mesmo nº de linhas que
     `labels`). Se encontrar, embute os dados do chart na próxima
     linha vazia da `Table`.
  2. Se **não** encontrar, registra no `Painel` (cumulativa, 1ª aba)
     com `kind`, `title` e `ref`.
  3. **Sempre** adiciona warning explícito:
     `"chart_<title> renderizado apenas como registro no Painel;
     chart nativo previsto para a Etapa 5/6"`.

- D2 (Etapa 4): **formatos numéricos brasileiros via `column_formats`**
  (extensão opcional do `xlsx.write` Python, backward-compat) —
  `BRL` → `"$#,##0.00"`, `PCT` → `"0.00%"`, `THOUSANDS` → `"#,##0"`,
  `INT` → `"0"`. Heurística no `ExcelProKit`: `Table.currency="BRL"`
  aplica `BRL` em todas as colunas de dados (pula 1ª se > 1);
  `Table.percent=true` aplica `PCT`; `Table.thousands=true` aplica
  `THOUSANDS`; `Kpis.format="BRL"` aplica `BRL` no valor de cada card.
  Limitação: `cell.number_format` é **visual** — o `value` Python
  continua sendo `int`/`float`. Caller que precisa de string
  formatada usa `docs.inspect` ou formatador explícito.

- D3 (Etapa 4): **mapeamento de blocos → sheets** — `Kpis` vai
  pro `Painel` (cumulativa, 1ª aba); `Table` (com `title`) vira
  sheet `<title>` sanitizado; `Table` (sem `title`) vira `Table_<i>`
  (i = posição do bloco no spec); `Chart` (com Table compatível)
  embute na Table; `Chart` (sem Table compatível) vira registro no
  Painel + warning. Sanitização de sheet name em Rust
  (`sheet_name.rs`): max 31 chars, remove `\ / ? * [ ] :`,
  strip whitespace, fallback `Table_<i>`, sufixo `_2.._999` em
  colisão, UTF-8 safe (chars, não bytes), acentos preservados.

- D4 (Etapa 4): **bump atômico do `DocumentFormat::Xlsx`** junto
  com o `ExcelProKit` real (REGRAS §1.9). Inventário cresceu de
  `["docx"]` para `["docx", "xlsx"]` no mesmo commit.

- D5 (Etapa 4): **round-trip via `docs.inspect`** (cobre `.xlsx`
  também) — o `sheets: [{block_index, sheet_name}]` do `generate`
  fecha o ciclo com o inspect. E2E do ExcelPro: gera via
  `docs.generate`, reabre via `docs.inspect` (modo resumo),
  afirma Painel 1ª aba + 1 sheet por Table + linha de TOTAL +
  formato de moeda aplicado. **Round-trip pela mesma porta que o
  modelo usa** (não pelo handler direto).

## Lacunas do v0.1 (que impedem "implementado")

A Etapa 4 entrega o esqueleto funcional do ExcelPro: dados
tabulares (Painel + 1 sheet por Table), sanitização de sheet
name, formatos numéricos brasileiros via `column_formats`
(BRL/PCT/THOUSANDS/INT), e mapeamento/blocos cobertos. **Não**
entrega o pacote visual/interativo que o spec promete abaixo em
"Decisão tomada" e "Recursos mínimos" (PROMPT MESTRE §18.1,
§18.2, §18.6). Estas são as lacunas que mantêm o spec em
**parcialmente implementado** (REGRAS §1.13 — vocabulário de
3 valores, sync com o código):

1. **Chart visual nativo** (`openpyxl.chart.BarChart` /
   `LineChart` / `PieChart`) — Etapa 5/6. v0.1 registra o chart
   no Painel e (quando possível) embute os dados na próxima
   Table compatível — **mas não há chart visual no `.xlsx`**.
   Alguém abrindo o arquivo no Excel não vê a figura do chart,
   só os dados. O spec promete "gráficos, dashboards, indicadores"
   (§18.1); v0.1 entrega os dados por trás deles, não a figura.
2. **Identidade visual Excel** (cores dos cards KPI, fill do
   header, borders, freeze panes na 1ª linha, largura automática
   de coluna) — Etapa 5/6. v0.1 é "funcional mas sem graça" —
   mesma lógica do WordPro v0.1 ser "feio em tipografia" (Etapa 3).
   O spec promete "padrão visual com azul escuro, verde de sucesso,
   cinza claro, branco, espaço em branco, cards de KPI, cabeçalhos
   destacados" (§18.2); v0.1 não aplica nada disso.
3. **Tabela visual estilizada** (zebrado, header com fill, bordas
   entre células) — Etapa 5/6. v0.1 gera tabela sem formatação
   visual no `openpyxl` (default = sem fill, sem border).
4. **Fórmulas Excel** (campo `formula` no `Table`) — o `xlsx.write`
   Python v0.3.0 não aceita `formula` no payload (só `rows`).
   Caller que precisa de fórmulas calcula fora e bota o valor
   numérico. Fórmulas como 1ª classe entram na Etapa 5/6. O spec
   promete "fórmulas auditáveis: referências consistentes, sem
   valores fixos escondidos, fonte única da verdade" (§18.4);
   v0.1 não tem isso.
5. **Memória de cálculo como aba oculta** (PROMPT MESTRE §18.6) —
   Etapa 5/6. v0.1 não embute memória de cálculo.
6. **Filtros / tabelas estruturadas / validação de dados** (PROMPT
   MESTRE §18.1) — Etapa 5/6. v0.1 cobre os blocos do `DocumentSpec`
   (`Kpis`/`Table`/`Chart`); extensões Excel-only (filtros, listas
   suspensas, proteção) entram depois.

A promoção pra `implementado` exige que **todos** os 6 itens
acima estejam fechados. A Etapa 5 (PDFPro completo) **não** é
pré-requisito para isso — o trabalho de identidade visual
Excel e chart visual nativo pode ser puxado pra Etapa 5/6 em
qualquer ordem, contanto que entrem juntos (chart sem identidade
visual ainda é funcional mas feio; identidade visual sem chart
também).

## Referências

- `PROMPT MESTRE` §18 (ExcelPro)
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`pdfpro-specification.md`](./pdfpro-specification.md) (fidelidade Excel → PDF)
- `docs/development-roadmap.md` (Fase 5)
- [ADR-0020](../decisions/0020-fase-5-etapa-4-excelpro-inspect.md) — decisão completa da Etapa 4 (D1-D5 + alternativas descartadas + pendências).
