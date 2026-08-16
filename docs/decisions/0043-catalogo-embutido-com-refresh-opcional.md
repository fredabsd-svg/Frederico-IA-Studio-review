# 0043 — Catálogo embutido como base, com refresh opcional do provedor

> **Substitui parcialmente o [ADR-0006](0006-model-catalog-crate.md)** — mantém o catálogo embutido e a estrutura do `ModelDescriptor`; revisa a decisão de que o catálogo é *exclusivamente* embutido.

## Contexto

O ADR-0006 decidiu embutir o catálogo no binário em build time e rejeitou buscá-lo por rede:

> A v1 desktop é Windows-only e roda offline-first. Buscar catálogo de um servidor central em runtime adiciona dependência de rede, fronteira de confiança nova e quebra o modo offline — sacrifícios não justificados para um conjunto de dados que cabe em kilobytes.

O raciocínio continua correto no que ele decidiu. Mas o mesmo ADR já enunciava a tensão, duas frases antes:

> Esses dados mudam com frequência: provedores lançam modelo novo, ajustam preço, depreciam modelo antigo. Manter a lista em código viola `REGRAS §1.9` ("gerado vence manual") e envelhece mal.

A verificação de 2026-08-16 mostrou o envelhecimento em números: **13 modelos**, 9 provedores, nenhum HTTP no crate, `/models` inexistente em todo o código Rust. O catálogo é do dia em que foi escrito. Um usuário com credencial da OpenAI não alcança nenhum modelo lançado depois — não porque o produto decidiu não suportá-lo, mas porque o JSON não sabe que ele existe.

E o efeito piora com o resto do sistema. O `chat-and-providers.md` §462 registra que **modelo sem preço no catálogo aborta o run** antes de qualquer I/O, com a ação sugerida "adicione o preço no spec e regere o binário". Ou seja, a consequência prática de um modelo novo é: recompile o aplicativo. Para um estúdio de IA cujo valor é acesso a modelos, essa é a lacuna funcional mais visível do produto.

Há um agravante de arquitetura que o ADR-0006 não previu: o provedor **OpenRouter** existe para dar acesso a centenas de modelos de dezenas de fornecedores. Embutir uma lista fixa de 3 ou 4 deles é usar o provedor contra o propósito dele.

## Decisões

### D1 — O catálogo embutido continua, e continua sendo a base

Nada do ADR-0006 é desfeito no essencial: `catalog.json` versionado, validado por schema no `build.rs`, `include_str!` no runtime, `catalog_hash` no log de diagnóstico. **O app abre e funciona offline, com catálogo completo, sem nunca tocar a rede.** Essa propriedade não é negociada.

### D2 — Refresh do provedor é opcional, explícito e aditivo

Um `ModelCatalog.Refresh { provider }` consulta o endpoint de listagem do provedor (`/models` nos provedores OpenAI-compat) e **acrescenta** ao catálogo em memória e no banco. Três regras o cercam:

1. **Nunca automático.** Sem refresh no boot, sem refresh periódico, sem refresh no primeiro uso. O usuário pede; nada acontece sozinho. Isto preserva a garantia do §D1 — o modo offline não é degradado, porque nada tenta rede sem pedido.
2. **Nunca substitui o embutido.** Modelo do refresh entra ao lado, marcado com origem. Se a resposta do provedor for pior que o catálogo (campo faltando, preço ausente), o embutido continua sendo a verdade daquele modelo.
3. **Falha é visível e inofensiva.** Sem rede, sem credencial ou com erro do provedor: mensagem traduzida, catálogo embutido intacto. Refresh que falha nunca deixa o app pior do que antes.

### D3 — Preço ausente não vira estimativa

O `chat-and-providers.md` §462 é mantido: modelo sem preço aborta o run com `model_no_price`. O refresh **não** relaxa isso, e **não** inventa preço por heurística.

O que muda é a saída: hoje a ação sugerida é "regere o binário", o que só um desenvolvedor consegue fazer. Passa a ser preenchimento manual do preço pelo usuário, persistido no banco. Custo declarado, e explícito na UI — melhor que um custo silenciosamente errado, que é a razão de o `PriceTable` usar microcents desde o ADR-0006.

### D4 — Origem do modelo é visível na UI

Cada modelo mostra se veio do catálogo embutido ou de refresh, e quando. Um usuário que vê modelo que não funciona precisa distinguir "o produto suporta isto" de "seu provedor listou isto e nós repassamos". Sem essa marca, o app assume implicitamente uma garantia sobre dados que não são dele.

### D5 — A fronteira de confiança do ADR-0006 é respondida, não ignorada

O ADR-0006 rejeitou rede citando "fronteira de confiança nova". A objeção era contra um **servidor central do produto** — infraestrutura a manter, com poder de dizer a todos os usuários o que existe. O §D2 não cria isso: consulta o provedor **com quem o usuário já decidiu falar**, com a credencial dele, no endpoint que o adapter já usa para completions. Não há host novo, e o dado chega tratado como não confiável (validado contra o mesmo schema; o que não passar, não entra).

## Alternativas descartadas

1. **Manter o ADR-0006 sem mudança.** Rejeitado: 13 modelos fixos num produto multi-provedor é limitação funcional, não decisão de arquitetura, e "regere o binário" não é ação que um usuário execute.
2. **Refresh automático no boot.** Rejeitado pelo §D2.1: quebra offline-first e faz o app depender de rede para abrir — exatamente o que o ADR-0006 evitou com razão.
3. **Substituir o embutido pelo remoto.** Rejeitado: primeiro uso sem rede ficaria sem catálogo nenhum, e um provedor fora do ar viraria um app sem modelos.
4. **Catálogo central mantido pelo projeto.** Rejeitado pelo ADR-0006, e o §D5 explica por que a decisão dele continua valendo.
5. **Deixar o usuário editar `catalog.json`.** Rejeitado: é arquivo embutido no binário; editar exige recompilar. E editar arquivo gerado à mão é defeito pela §1.9.

## Consequências

- **Fica mais fácil:** usar modelo novo. Deixa de exigir release do produto.
- **Fica mais difícil:** garantir que todo modelo listado funciona. Modelo vindo de refresh pode faltar campo ou não suportar tool calling; a marca de origem do §D4 é o que impede a UI de prometer no lugar do provedor.
- **Superfície nova:** resposta de provedor externo passa a alimentar dado que o motor consome. Tratada como entrada não confiável, com o mesmo schema do embutido.
- **Migração e op nova de IPC** — implementação na Etapa 7 da Fase 8 (ADR-0039 §D5).
- **O ADR-0006 recebe o carimbo de substituído parcialmente**, e continua valendo no que este não toca (estrutura do `ModelDescriptor`, `build.rs`, `catalog_hash`).

## Histórico de revisão

- 2026-08-16 — versão inicial. Etapa 1 da Fase 8, após a verificação do catálogo estático em 2026-08-16.
