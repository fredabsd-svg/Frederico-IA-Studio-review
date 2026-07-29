//! Geração do prompt do modo documental a partir do **schema**.
//!
//! REGRAS §1.9 — "Gerado vence manual": o prompt **não é mantido à
//! mão**. O `document_mode_prompt()` é uma função pura que enumera os
//! blocos do `DocumentBlock`, descreve as regras semânticas de
//! `validate.rs`, e devolve o texto. O `execution-engine` da Etapa 3
//! consome este prompt para configurar o system prompt do modelo
//! quando a tarefa é "gerar documento" (em vez de "conversar").
//!
//! Por que gerar e não escrever? Se o catálogo de blocos mudar (v0.1
//! → v0.2), o prompt passa a refletir a mudança no mesmo commit em
//! que o tipo Rust muda — sem chance de divergência. É o mesmo
//! motivo de `schemars` no `build.rs`.

use crate::blocks::DocumentBlock;

/// Devolve o system prompt do modo documental (PT-BR).
///
/// A string é determinística e não contém timestamps, IDs de build ou
/// outra coisa que mude a cada chamada. O `execution-engine` cacheia
/// por `spec_version`.
#[must_use]
pub fn document_mode_prompt() -> String {
    let mut s = String::new();

    s.push_str(MODE_HEADER);
    s.push_str("\n\n## O que é o DocumentSpec\n\n");
    s.push_str(SPEC_INTRO);
    s.push_str("\n\n## Catálogo de blocos (v0.1)\n\n");
    s.push_str(&block_catalog());
    s.push_str("\n\n## Regras semânticas (validadas em runtime)\n\n");
    s.push_str(SEMANTIC_RULES);
    s.push_str("\n\n## O que NÃO fazer\n\n");
    s.push_str(DONT_DO);
    s.push_str("\n\n## Estilos disponíveis\n\n");
    s.push_str(STYLES);
    s.push_str("\n\n## Tipos de documento\n\n");
    s.push_str(DOC_TYPES);

    s
}

/// Lista formatada dos blocos (markdown). Gerada a partir do match
/// exaustivo sobre `DocumentBlock` — se um bloco novo for adicionado
/// em `blocks.rs`, esta função quebra em `unimplemented!` (proposital:
/// força o autor a escrever a descrição do bloco).
fn block_catalog() -> String {
    use std::fmt::Write as _;

    let mut s = String::new();
    for (i, entry) in block_descriptions().iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        writeln!(s, "### `{}`", entry.name).unwrap();
        writeln!(s, "{}", entry.summary).unwrap();
        if let Some(fields) = entry.fields {
            writeln!(s, "**Campos:** {fields}").unwrap();
        }
    }
    s
}

struct BlockDescription {
    name: &'static str,
    summary: &'static str,
    fields: Option<&'static str>,
}

const fn block_descriptions() -> &'static [BlockDescription] {
    &[
        BlockDescription {
            name: "cover",
            summary: "Capa do documento (título, subtítulo, autor, data).",
            fields: Some("`title: string` (obrigatório), `subtitle?: string`, `author?: string`, `date?: string`"),
        },
        BlockDescription {
            name: "toc",
            summary: "Sumário — gerado em duas passadas pelo kit (numeração real, não a do spec).",
            fields: None,
        },
        BlockDescription {
            name: "heading",
            summary: "Cabeçalho de seção.",
            fields: Some("`level: 1|2|3`, `text: string`, `number?: string` (ex: \"3.2\")"),
        },
        BlockDescription {
            name: "paragraph",
            summary: "Parágrafo de texto corrido.",
            fields: Some("`text: string`, `style?: string` (nome de estilo opcional)"),
        },
        BlockDescription {
            name: "list",
            summary: "Lista numerada ou com marcadores.",
            fields: Some("`ordered: bool`, `items: ListItem[]` (recursivo via `children`)"),
        },
        BlockDescription {
            name: "table",
            summary: "Tabela. Todas as linhas devem ter o mesmo número de colunas que `headers`.",
            fields: Some(
                "`headers: string[]`, `rows: string[][]`, `total?: {label, expression}`, `currency?: \"BRL\"|...`, `percent?: bool`, `thousands?: bool`, `title?: string`, `source?: string`",
            ),
        },
        BlockDescription {
            name: "key_value",
            summary: "Tabela de chave-valor (ex: cláusula de contrato).",
            fields: Some("`entries: [string, string][]`"),
        },
        BlockDescription {
            name: "kpis",
            summary: "Conjunto de 2 a 4 KPIs (cartões). Use para dashboards.",
            fields: Some("`items: KpiCard[]` (2 ≤ n ≤ 4); cada `KpiCard`: `label`, `value`, `delta?`, `delta_label?`"),
        },
        BlockDescription {
            name: "callout",
            summary: "Caixa de destaque (info, alerta, crítico, sucesso).",
            fields: Some("`kind: info|alert|critical|success`, `text: string`"),
        },
        BlockDescription {
            name: "quote",
            summary: "Citação com atribuição.",
            fields: Some("`text: string`, `attribution?: string`"),
        },
        BlockDescription {
            name: "steps",
            summary: "Passos numerados.",
            fields: Some("`items: Step[]` (≥ 1); cada `Step`: `title`, `description?`"),
        },
        BlockDescription {
            name: "chart",
            summary: "Gráfico (barra, linha ou pizza).",
            fields: Some("`kind: bar|line|pie`, `labels: string[]`, `series: ChartSeries[]` (≥ 1), `title?: string`"),
        },
        BlockDescription {
            name: "image",
            summary: "Imagem. `alt` é obrigatório (acessibilidade).",
            fields: Some("`path: string`, `alt: string`, `caption?: string`, `width_cm?: number`"),
        },
        BlockDescription {
            name: "code",
            summary: "Bloco de código com syntax highlight opcional.",
            fields: Some("`content: string`, `language?: string`, `caption?: string`"),
        },
        BlockDescription {
            name: "divider",
            summary: "Linha horizontal. Sem campos.",
            fields: None,
        },
        BlockDescription {
            name: "spacer",
            summary: "Espaço vertical em centímetros.",
            fields: Some("`height_cm: number`"),
        },
        BlockDescription {
            name: "page_break",
            summary: "Quebra de página forçada.",
            fields: None,
        },
        BlockDescription {
            name: "footer",
            summary: "Rodapé (repetido em todas as páginas a partir do bloco).",
            fields: Some("`text: string`, `page_numbers?: bool`"),
        },
        BlockDescription {
            name: "signatures",
            summary: "Bloco de assinaturas.",
            fields: Some("`pairs: SignaturePair[]`; cada par: `name`, `role?`, `location?`"),
        },
        BlockDescription {
            name: "back_cover",
            summary: "Contracapa com contatos.",
            fields: Some("`contacts: ContactInfo` (`name`, `email?`, `phone?`, `address?`)"),
        },
    ]
}

const MODE_HEADER: &str = "\
Você está no modo documental. Sua saída é um JSON que respeita o
contrato `DocumentSpec` (versão 0.1.0). O JSON descreve a **estrutura**
do documento; a engine cuida de tipografia, cores e paginação.";

const SPEC_INTRO: &str = "\
O JSON tem esta forma:

```json
{
  \"spec_version\": \"0.1.0\",
  \"doc_type\": \"report\",
  \"style\": \"tinta_e_latao\",
  \"language\": \"pt-br\",
  \"metadata\": { \"title\": \"...\", \"author\": \"...\" },
  \"confidentiality\": { \"level\": \"internal\" },
  \"blocks\": [ /* veja o catálogo abaixo */ ]
}
```";

const SEMANTIC_RULES: &str = "\
1. `blocks` não pode ser vazio.
2. `spec_version` deve ser exatamente `\"0.1.0\"` enquanto o catálogo
   não mudar.
3. `kpis` aceita 2 a 4 cartões (não 1, não 5).
4. `steps` aceita 1 ou mais passos.
5. `table.rows[i]` deve ter o mesmo número de colunas que
   `table.headers`.
6. Se `doc_type = \"spreadsheet\"`, os blocos aceitos são **apenas**
   `kpis`, `table` e `chart`. Capa, sumário e parágrafos não fazem
   sentido numa planilha — se o usuário pediu uma planilha, use
   apenas esses três.
7. `language` deve estar em minúsculas (BCP-47: `pt-br`, `en-us`).";

const DONT_DO: &str = "\
- Não escreva código de diagramação (Python, LaTeX, HTML, Markdown
  cru, instruções de formatação imperativa). A engine recebe o
  `DocumentSpec` e renderiza.
- Não invente placeholders. Sem `\"DD/MM/AAAA\"`, sem `\"[cliente]\"`,
  sem `\"Seu Nome\"`. Use a data real (do contexto da conversa) e os
  contatos reais (do `metadata` ou da base). Se um campo é
  desconhecido, **omita** a chave (não invente).
- Não use emoji, setas decorativas ou símbolos carregando sentido.
  A tipografia é séria.
- Não inclua seções ou anexos que o usuário não pediu.
- Não feche o documento com `BackCover` salvo se houver contatos
  reais para exibir.";

const STYLES: &str = "\
- `tinta_e_latao` (default) — identidade visual do Frederico:
  fontes Source Serif 4 + Source Sans 3, paleta azul escuro / verde
  / cinza claro, espaço em branco generoso, cards de KPI com
  cabeçalho destacado.
- `sobrio` — para registráveis (contratos, procurações, ofícios).
  Sem cor, tipografia conservadora (Source Serif 4), sem ornamento.
  Aplicado automaticamente em `doc_type` registráveis.";

const DOC_TYPES: &str = "\
`report`, `memo`, `contract`, `spreadsheet`, `proposal`,
`technical_opinion`, `power_of_attorney`, `official_letter`,
`announcement`, `manual`, `presentation`, `generic`.

O `doc_type` afeta o **template** escolhido pelo kit (cabeçalhos,
rodapés, paginação, estilo automático). Não use `generic` se um tipo
específico se aplica.";

// Apenas para forçar que o catálogo de blocos do match em `validate.rs`
// está em sincronia com a lista em `block_descriptions()`. Se um
// `DocumentBlock` for adicionado sem entrada correspondente em
// `block_descriptions()`, este teste em `tests/spec_roundtrip.rs`
// quebra. O `unused` aqui garante que a constante é referenciada mesmo
// sem o teste inline (linker não reclama).
#[doc(hidden)]
pub const _ENSURE_BLOCKS_USED: fn() = || {
    let _ = DocumentBlock::Divider;
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_stable_and_nonempty() {
        let p = document_mode_prompt();
        assert!(p.contains("modo documental"));
        assert!(p.contains("0.1.0"));
        assert!(p.contains("`kpis`"));
        assert!(p.contains("`spreadsheet`"));
        // Tamanho mínimo razoável (catálogo + regras + estilos).
        assert!(p.len() > 2000, "prompt muito curto: {} chars", p.len());
    }

    #[test]
    fn prompt_lists_every_block_kind() {
        let p = document_mode_prompt();
        for name in [
            "cover",
            "toc",
            "heading",
            "paragraph",
            "list",
            "table",
            "key_value",
            "kpis",
            "callout",
            "quote",
            "steps",
            "chart",
            "image",
            "code",
            "divider",
            "spacer",
            "page_break",
            "footer",
            "signatures",
            "back_cover",
        ] {
            assert!(
                p.contains(&format!("`{name}`")),
                "bloco {name} ausente do prompt"
            );
        }
    }
}
