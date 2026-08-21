<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-19
Fase correspondente: 0 (modelos, Etapa 2 da Fase 0) + 6 (SpecialistRegistry, Etapa 3) + 8 (fusão com o remoto, ADR-0052)
-->

# `frederico-model-catalog`


## O catálogo efetivo: embutido fundido com o que o provedor respondeu

Desde 2026-08-19 ([ADR-0052](../decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md)),
o app consulta o `/models` de cada provedor com credencial **na
abertura, em tarefa de fundo**, e funde o resultado com o embutido.

A regra, em uma frase: **o remoto decide quais modelos existem; o
embutido decide quanto custam.**

| Situação | Resultado |
|---|---|
| Provedor não respondeu (sem rede, sem credencial, erro) | lista embutida intacta |
| Modelo no remoto e não no embutido | entra, marcado como `Remoto` |
| Modelo no embutido e não no remoto | **sai** — aposentado pelo provedor |
| Modelo nos dois | fica, com os campos do embutido preservados |

### Por que o preço não vem do remoto

Medido em 2026-08-19: o `/models` do OpenRouter devolve preço e
janela de contexto; o da **OpenAI devolve só a lista de ids**. Se o
remoto mandasse em tudo, um refresh da OpenAI apagaria todos os
preços e nenhum modelo dela rodaria — `model_no_price` aborta o run
antes de qualquer I/O.

### Por que lista vazia é tratada como falha

Um provedor que responde `[]` faria a fusão apagar todos os modelos
embutidos dele. "Não consegui listar" é muito mais provável que "este
provedor não tem modelo nenhum", e o custo de errar para o lado
errado é o usuário perder acesso a tudo daquele provedor.

### O que a fusão nunca presume

Modelo que só o remoto conhece entra **sem capacidade nenhuma
declarada** e com janela de contexto mínima quando o provedor não a
informa. Declarar `tools` num modelo que não as suporta faz o run
falhar no meio, depois de gastar tokens.

### Sem persistência

O refresh vive em memória e refaz a cada abertura (ADR-0052 §D4).
Abrir offline depois de abrir online mostra o embutido, não a última
lista vista.

## A unidade de preço tem dois nomes errados

O campo se chama `input_microcents` e o comentário dizia "por mil
tokens". Nenhum dos dois está certo: é **por milhão**, e a unidade é
**10⁻⁵ de dólar** (milicentavo), não microcent.

Conferido contra seis entradas de preço público conhecido — GPT-4o
mini a US$ 0,15/1M gravado como `15000`; Claude 3.5 Haiku a
US$ 0,80/1M como `80000`. O comentário foi corrigido; o nome do campo
não, porque renomeá-lo toca schema, JSON, banco e migração.


## O que faz

Catálogo embutido de modelos + registro de especialistas. **Modelos** ([Etapa 2 da Fase 0](../decisions/0006-model-catalog-crate.md)): quais modelos cada provedor oferece, o que cada um sabe fazer (ferramentas, visão, JSON mode, cache de prompt) e quanto custa por token. O catálogo é versionado em `data/catalog.json`, validado durante a build pelo `build.rs` e embutido no binário via `include_str!` — não há chamada de rede em runtime. **Especialistas** ([Etapa 3 da Fase 6](../decisions/0030-specialist-registry-from-model-catalog.md)): a peça user-facing do catálogo, descrevendo "que papéis (com quais ferramentas e restrições) os modelos podem exercer" no Modo Equipe. O registry é versionado em `data/specialists/default.toml`, mesmo padrão de embedded. Atualizar qualquer um é abrir um PR, revisar e liberar no próximo release.

Decisão registrada no [ADR-0006](../decisions/0006-model-catalog-crate.md); o contexto de produto está em [`chat-and-providers.md`](../architecture/chat-and-providers.md).

## O que expõe

| Item | Para que serve |
|---|---|
| `Catalog::load()` | Instância única (`OnceLock`) do catálogo embutido |
| `Catalog::from_json(&str)` | Constrói a partir de JSON — usado nos testes |
| `Catalog::find_model(&ProviderId, &ModelId)` | Descritor de um modelo específico |
| `Catalog::list_for_provider(&ProviderId)` / `list_all()` / `models()` | Listagens para a UI de seleção |
| `Catalog::pricing_for(&ProviderId, &ModelId)` | Tabela de preços do modelo |
| `ModelDescriptor::cost_microcents(prompt, completion)` | Custo de uma chamada, em microcents |
| `PriceTable::cost_microcents(prompt, completion)` | Mesmo cálculo, a partir da tabela solta |
| `ModalitySet` / `CapabilitySet` + `Modality`, `Capability` | Consulta tipada de modalidade e capacidade |
| `CATALOG_HASH` | Hash do catálogo canônico, gerado pelo `build.rs` |
| `SpecialistId` / `SpecialistDefinition` / `SpecialistSummary` (Etapa 3) | Tipos do registro de especialistas. `SpecialistSummary` é a view leve pro UI. |
| `SpecialistRegistry` trait + `DefaultSpecialistRegistry` (Etapa 3) | Interface do registry + impl default (bundled + override). |
| `parse_specialists_toml(&str) -> Result<Vec<SpecialistDefinition>, String>` (Etapa 3) | Parse puro do TOML (testável isoladamente com fixture inline). |
| `registry::list_summaries(&dyn SpecialistRegistry, impl Fn)` (Etapa 3) | Helper free que monta `Vec<SpecialistSummary>` resolvendo capabilities via closure do caller. |

## Do que depende e quem depende dele

- **Depende de:** `frederico-core` (`ModelId`, `ProviderId`), `serde`/`serde_json`, `thiserror`. Nada de I/O, nada de rede, nada de banco.
- **Depende dele:** `frederico-provider-engine` (capacidades do adapter), `frederico-execution-engine` (cálculo de custo real ao fechar um `Run`), e a casca Tauri `frederico-desktop` (listagem de modelos na UI).

## Decisões não óbvias e armadilhas

- **Sem rede em runtime, por decisão.** Um catálogo buscado online seria mais fresco, mas transformaria a listagem de modelos em ponto de falha de rede e em vetor de dado não confiável. O preço é que modelo novo só aparece com release novo.
- **Custo em microcents, inteiro.** Ponto flutuante para dinheiro acumula erro ao somar milhares de chamadas. Toda a cadeia (`PriceTable` → `MessageRepo::set_usage_and_cost` → `ConversationRepo::add_cost`) trabalha com inteiro.
- **`CATALOG_HASH` vem do `build.rs`**, não do código. Editar `data/catalog.json` sem rebuildar deixa o hash defasado.
- **O catálogo é dado, não contrato.** Modelo ausente do catálogo não impede a chamada ao provedor; significa apenas que não há preço nem capacidade declarada, e o custo não é calculado.
- **`SpecialistRegistry` é a interface, `DefaultSpecialistRegistry` é a impl** (mesma divisão do `WorkerInvoker` / `JailResolver` — ADR-0024, ADR-0022). O `SubagentRunner` da Etapa 4 consome `Arc<dyn SpecialistRegistry>`, não a struct concreta. Permite mock em testes e registry de arquivo de projeto no futuro sem mexer no runner.
- **Override do usuário não invade bundled** (regra D1 do [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md)). Se o `~/.config/frederico/specialists.toml` redefine um ID bundled, **bundled vence** com warning explícito. Mesma família do "mais restritivo vence" do `permission_loader` (PR 2 da Etapa 3). Hard fail silencioso seria pior (app não inicia por config ruim do usuário).
- **`RegistryError::UnknownSpecialist` sempre carrega `valid: Vec<SpecialistId>`** (§D4 do ADR-0030). Sem o campo, o erro não compila (variant com campo obrigatório) — defesa em profundidade contra o "esqueci de listar os válidos". A UI da Etapa 6 renderiza a lista direto.
- **Parse do `default.toml` é inline no `build.rs`** (não reusa `parse_specialists_toml` do runtime) porque `build.rs` roda antes do `lib.rs` ser compilado. Duplicação justificada (~25 linhas, mesma estratégia do `validate_minimal` do `catalog.json`). O teste E2E `registry_loads_specialists_from_catalog` pega divergência entre build-time e runtime.
- **`list_summaries` é função free, não método do trait** (dyn-compatibility). Trait methods com generics quebram dyn-compatibility em Rust. Helper `list_summaries(&dyn SpecialistRegistry, impl Fn)` resolve capabilities por definição.

## Como testar isoladamente

```bash
cargo test -p frederico-model-catalog
```

Os testes do catálogo vivem em `#[cfg(test)] mod tests` no próprio `src/lib.rs` e usam `Catalog::from_json` com fixtures inline — não dependem do catálogo real, então mudar `data/catalog.json` não os quebra. Os testes do `SpecialistRegistry` (Etapa 3) vivem em `registry::tests` e `specialist::tests` e usam `DefaultSpecialistRegistry::from_parts` (construtor de teste) com fixtures inline — não dependem do `default.toml` real, então mudar o TOML não os quebra.

## O que ele não faz

- Não busca modelos em rede, nem em runtime nem em build.
- Não escolhe modelo por você — roteamento é do `frederico-agent-engine` / Fase 6.
- Não chama provedor: quem fala HTTP é o `frederico-provider-engine`.
- Não persiste nada; não conhece SQLite.
- Não valida se a chave de API dá acesso ao modelo listado — isso só se descobre chamando.
- **Não invoca subagente.** O `SpecialistRegistry` é o **catálogo** dos papéis disponíveis; quem delega é o `SubagentRunner` da Etapa 4 da Fase 6. A Etapa 3 só lista e resolve capabilities — spawn é trabalho de outra etapa.
- **Não carrega o `PermissionSet` real** (assistant/project/user). Isso é o **PR 2 da Etapa 3** — `permission_loader` em `crates/tool-registry` com `PermissionSet::merge` fail-closed + migração `0028_profiles.sql`. O Etapa 3 PR 1 entrega só o registry; o `PermissionSet` carrega o default deny até o PR 2 entrar.
- **Não tem hot-reload de override.** Override é lido no startup. Hot-reload via filesystem watch é trabalho de fase futura (Etapa 6+).

## Specialists (Etapa 3, ADR-0030)

O registro de especialistas vive em `data/specialists/default.toml` (8 entries bundled: revisor, pesquisador, testador, validador, sumador, arquiteto, crítico, executor). O `build.rs` embute no binário via `include_str!` no `OUT_DIR/specialists.toml` + expõe `SPECIALISTS_TOML_PATH` (mesmo padrão do `CATALOG_JSON_PATH`).

O `DefaultSpecialistRegistry` carrega bundled + override de `~/.config/frederico/specialists.toml` (Linux/macOS) ou `%APPDATA%\Frederico\specialists.toml` (Windows), resolvido via `directories::ProjectDirs`. Override-fail-closed: parse ou I/O falhando → warning + fallback bundled (degradação declarada, mesma família do `OpenRouter API key ausente` da Fase de Ligação Etapa 3).

O `SpecialistBundle` em `crates/app/src/composition.rs` parea o registry com o `Catalog` e resolve as capabilities do `default_model` por definição (a UI renderiza badges "tem tools" / "tem visão" sem precisar consultar o catálogo de novo).

Consumidores atuais: Tauri command `list_specialists` → `<SpecialistPicker>` React. Consumidores futuros: `SubagentRunner` (Etapa 4, vai plugar aqui), UI completa do Modo Equipe (Etapa 6 da Fase 6).

