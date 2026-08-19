# 0052 — Refresh de catálogo no boot, em segundo plano, com o remoto mandando na lista do provedor

> **Substitui os §D2.1 e §D2.2 do [ADR-0043](0043-catalogo-embutido-com-refresh-opcional.md).** O §D1 (embutido como base), o §D3 (preço ausente não vira estimativa), o §D4 (origem visível) e o §D5 (fronteira de confiança) continuam valendo, inteiros.

## Contexto

O ADR-0043 decidiu que o refresh de catálogo é **opcional e explícito**:

> **Nunca automático.** Sem refresh no boot, sem refresh periódico, sem refresh no primeiro uso. O usuário pede; nada acontece sozinho.

E rejeitou a alternativa oposta com uma razão precisa:

> **Refresh automático no boot.** Rejeitado pelo §D2.1: quebra offline-first e faz o app depender de rede para abrir — exatamente o que o ADR-0006 evitou com razão.

O uso mostrou o custo da decisão. Em 2026-08-19 o catálogo embutido foi atualizado à mão: **13 modelos, nenhum posterior a 2024**, com `gpt-4o` e `claude-3-5-sonnet` como topo de linha e os oito especialistas apontando para um modelo que já não é vendido. A atualização exigiu buscar preço em três fontes diferentes, converter unidade, e consertar dois testes que estavam presos a nomes de modelo.

Ou seja: o refresh manual existe no papel desde a Etapa 1 e **nunca foi construído**, e enquanto isso o único caminho real de atualização foi o que o próprio ADR-0043 chamou de inaceitável — "recompile o aplicativo".

Há também um efeito que o §D2.2 produz e que o uso expõe: **modelo que o provedor aposentou continua na lista para sempre.** Um usuário que escolhe `gpt-4o` num catálogo embutido de 2024 recebe erro do provedor, não do app — e o app não tem como saber que o modelo saiu, porque o §D2.2 o proíbe de deixar o remoto mandar.

## Decisões

### D1 — O refresh acontece no boot, em segundo plano, sem bloquear nada

Substitui o §D2.1 do ADR-0043.

Ao abrir, o app dispara o refresh dos provedores que **têm credencial configurada**, em tarefa de fundo. A janela abre imediatamente, com o catálogo embutido completo, antes de qualquer resposta de rede. Quando (e se) a resposta chega, a lista de modelos é atualizada.

**A objeção do ADR-0043 é respondida, não ignorada.** Ela dizia que o boot refresh "faz o app depender de rede para abrir". Essa propriedade vem de *bloquear*, não de *disparar* — e aqui nada bloqueia:

| Cenário | Comportamento |
|---|---|
| Sem rede | O app abre normal, com o embutido. A falha é registrada e não vira modal. |
| Sem credencial no provedor | Ele nem é consultado. |
| Provedor lento ou fora do ar | O app já está aberto e utilizável; a lista simplesmente não muda. |
| Resposta inválida | Descartada pelo schema; o embutido continua. |

O §D1 do ADR-0043 — *"o app abre e funciona offline, com catálogo completo, sem nunca tocar a rede"* — precisa de uma emenda literal: o app **toca** a rede, mas não **depende** dela para abrir nem para funcionar. A garantia que importava era a segunda.

### D2 — Para o provedor consultado com sucesso, a lista remota manda

Substitui o §D2.2 do ADR-0043, que dizia que o remoto "nunca substitui o embutido".

Quando o provedor responde com uma lista válida, ela passa a ser a verdade sobre **quais modelos daquele provedor existem**:

- Modelo que está no remoto e não no embutido: **entra**.
- Modelo que está no embutido e não no remoto: **sai da lista** — é modelo aposentado, e mantê-lo só produz erro do provedor na hora do uso.
- Modelo nos dois: **fica, com os campos do embutido preservados** onde o remoto não traz dado. Esta parte do §D2.2 continua valendo, e o §D3 abaixo diz por quê.

A troca só vale para o provedor que respondeu. Provedor não consultado, ou que falhou, mantém o embutido intacto — a lista de um provedor nunca é afetada pelo silêncio de outro.

### D3 — Preço continua vindo do embutido, e modelo sem preço continua sem rodar

O §D3 do ADR-0043 é mantido sem mudança, e ganha uma razão a mais: **os endpoints `/models` dos provedores não são iguais.**

Medido em 2026-08-19: o `/models` do OpenRouter devolve preço e janela de contexto por modelo; o da OpenAI devolve **só a lista de ids**. Se o remoto substituísse o embutido por inteiro, um refresh da OpenAI apagaria todos os preços e o app pararia de rodar qualquer modelo dela — com a mensagem `model_no_price`, que o `chat-and-providers.md` §462 define como abortar antes de qualquer I/O.

Portanto: o remoto decide **quais** modelos existem; o embutido decide **quanto custam**, quando sabe. Modelo novo, que o embutido não conhece e cujo provedor não informa preço, aparece na lista marcado e **não roda** até o usuário preencher o preço — exatamente como o §D3 do ADR-0043 já mandava.

### D4 — Sem persistência nesta entrega, e isso é declarado

O ADR-0043 §D2 previa gravar o refresh no banco. **Não é feito aqui.** O refresh vive em memória e refaz a cada abertura, que é literalmente o que foi pedido.

O custo: a primeira abertura offline depois de uma online não lembra o que foi visto. É aceitável porque o embutido continua sendo a base completa — o usuário offline vê a lista de sempre, não uma lista vazia. Persistir é melhoria com valor próprio, e entra quando houver quem a peça.

## Alternativas descartadas

1. **Manter o §D2.1 e construir o refresh manual.** Rejeitado: o botão existe no papel desde a Etapa 1 e nunca foi construído, enquanto o catálogo envelhecia. Uma decisão que depende de alguém lembrar de clicar tem o mesmo resultado observado de não existir.
2. **Refresh bloqueante no boot, com tela de carregamento.** Rejeitado pelo §D1: é exatamente o que o ADR-0043 recusou com razão, e a razão continua boa.
3. **Refresh periódico durante a sessão.** Rejeitado por ora: catálogo de modelos muda em escala de semanas, não de minutos, e cada consulta gasta credencial do usuário. Uma vez por abertura é a menor frequência que resolve o problema observado.
4. **Deixar o remoto substituir tudo, inclusive preço.** Rejeitado pelo §D3, com a medição do `/models` da OpenAI como evidência.
5. **Manter modelo aposentado na lista, marcado como "pode não funcionar".** Rejeitado: transfere para o usuário uma decisão que o provedor já tomou. Se o modelo saiu, escolher não é opção — é armadilha.

## Consequências

- **Fica mais fácil:** usar modelo novo no dia em que ele sai, sem release do produto e sem clicar em nada.
- **Fica mais difícil:** reproduzir um bug de catálogo. A lista passa a depender do que o provedor respondeu naquela abertura. Mitigado pela marca de origem do ADR-0043 §D4, que diz de onde veio cada modelo.
- **O app passa a fazer rede na abertura**, com a credencial do usuário, para cada provedor configurado. É consulta de leitura ao mesmo host com que ele já fala — o §D5 do ADR-0043 continua respondendo por isso.
- **Uma lista pode encolher entre duas aberturas** sem o usuário ter feito nada. É o comportamento correto, e a marca de origem é o que o torna explicável.
- **Sem persistência** (§D4): abrir offline depois de abrir online mostra o embutido, não a última lista vista.

## Histórico de revisão

- 2026-08-19 — versão inicial. Substitui os §D2.1 e §D2.2 do ADR-0043.
