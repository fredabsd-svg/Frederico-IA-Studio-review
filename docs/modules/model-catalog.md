# `frederico-model-catalog`

## O que faz

Catálogo embutido de modelos: quais modelos cada provedor oferece, o que cada um sabe fazer (ferramentas, visão, JSON mode, cache de prompt) e quanto custa por token. O catálogo é versionado em `data/catalog.json`, validado durante a build pelo `build.rs` e embutido no binário via `include_str!` — não há chamada de rede em runtime. Atualizar o catálogo é abrir um PR, revisar e liberar no próximo release.

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

## Do que depende e quem depende dele

- **Depende de:** `frederico-core` (`ModelId`, `ProviderId`), `serde`/`serde_json`, `thiserror`. Nada de I/O, nada de rede, nada de banco.
- **Depende dele:** `frederico-provider-engine` (capacidades do adapter), `frederico-execution-engine` (cálculo de custo real ao fechar um `Run`), e a casca Tauri `frederico-desktop` (listagem de modelos na UI).

## Decisões não óbvias e armadilhas

- **Sem rede em runtime, por decisão.** Um catálogo buscado online seria mais fresco, mas transformaria a listagem de modelos em ponto de falha de rede e em vetor de dado não confiável. O preço é que modelo novo só aparece com release novo.
- **Custo em microcents, inteiro.** Ponto flutuante para dinheiro acumula erro ao somar milhares de chamadas. Toda a cadeia (`PriceTable` → `MessageRepo::set_usage_and_cost` → `ConversationRepo::add_cost`) trabalha com inteiro.
- **`CATALOG_HASH` vem do `build.rs`**, não do código. Editar `data/catalog.json` sem rebuildar deixa o hash defasado.
- **O catálogo é dado, não contrato.** Modelo ausente do catálogo não impede a chamada ao provedor; significa apenas que não há preço nem capacidade declarada, e o custo não é calculado.

## Como testar isoladamente

```bash
cargo test -p frederico-model-catalog
```

Os testes vivem em `#[cfg(test)] mod tests` no próprio `src/lib.rs` e usam `Catalog::from_json` com fixtures inline — não dependem do catálogo real, então mudar `data/catalog.json` não os quebra.

## O que ele não faz

- Não busca modelos em rede, nem em runtime nem em build.
- Não escolhe modelo por você — roteamento é do `frederico-agent-engine` / Fase 6.
- Não chama provedor: quem fala HTTP é o `frederico-provider-engine`.
- Não persiste nada; não conhece SQLite.
- Não valida se a chave de API dá acesso ao modelo listado — isso só se descobre chamando.
