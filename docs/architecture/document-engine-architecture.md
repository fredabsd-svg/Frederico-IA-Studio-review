<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 5
-->

# Arquitetura do Document Engine (stub)

> Stub criado na Fase 0. Será aprofundado antes do início da Fase 5 (Documentos).

## Decisão tomada

- **Camada central de processamento documental** antes do conteúdo ser enviado aos modelos (`PROMPT MESTRE` §15).
- **`document-worker.exe`** carregado sob demanda, com Python embutido (ver [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md)).
- **Integração com Docling** (ou tecnologia equivalente) como motor central de compreensão documental (`PROMPT MESTRE` §15.3), gerando Markdown otimizado e JSON estruturado.
- **Cache de extração** chaveado por `hash do arquivo + versão do processador + configuração + idioma + opções de OCR` (`PROMPT MESTRE` §15.4).
- **Identidade visual "Tinta & Latão"** aplicada automaticamente nos três kits, com fontes embutidas (`PROMPT MESTRE` §16.3) e modo **Sóbrio** para documentos registráveis (`PROMPT MESTRE` §16.6).
- **Três kits independentes** (WordPro, ExcelPro, PDFPro) sobre o **Document Artifact Engine** comum (`PROMPT MESTRE` §16).
- **`DocumentSpec` declarativo em JSON** validado por schema versionado — o modelo de IA **não escreve código de diagramação** (`PROMPT MESTRE` §16.4 e §16.5).
- **Auditoria bloqueante** no PDFPro: as validações visual e estrutural executam dentro do salvamento do artefato, reprovação impede a entrega (`PROMPT MESTRE` §19.6).

## Contrato previsto

```rust
struct DocumentSpec {
    spec_version: SemVer,
    doc_type: DocumentType,           // "report" | "memo" | "contract" | "spreadsheet" | ...
    style: DocumentStyle,             // "tinta-e-latao" | "sobrio"
    language: String,                 // BCP-47
    blocks: Vec<DocumentBlock>,
    metadata: DocumentMetadata,
    confidentiality: Option<ConfidentialityMark>,
}

enum DocumentBlock {
    Cover { title, subtitle, ... },
    Toc,
    Heading { level: u8, text, number: Option<String> },
    Paragraph { text, style: Option<String> },
    List { ordered: bool, items: Vec<ListItem> },
    Table { headers, rows, total: Option<TotalSpec>, currency, percent, thousands, title, source },
    KeyValue { entries: Vec<(String, String)> },
    Kpis { items: Vec<KpiCard> },     // 2 a 4 cartões
    Callout { kind: Info|Alert|Critical|Success, text },
    Quote { text, attribution },
    Steps { items: Vec<Step> },
    Chart { kind: Bar|Line|Pie, data, ... },
    Image { path, caption, alt },
    Code { language, content },
    Divider,
    Spacer,
    PageBreak,
    Footer { text, page_numbers: bool },
    Signatures { pairs: Vec<SignaturePair> },
    BackCover { contacts: ContactInfo },
}
```

## Não-objetivos

- Geração de documento "na mão" (reportlab cru, openpyxl estilizando célula a célula, python-docx para layout — todos proibidos pelo `PROMPT MESTRE` §16.5).
- Placeholders do tipo "DD/MM/AAAA", "[cliente]", "Seu Nome" — data é a data real, campo desconhecido é omitido, contracapa só com contatos reais (`PROMPT MESTRE` §16.5).
- Setas, símbolos decorativos ou emojis carregando sentido na tipografia.
- Edição de documento Word/Excel **dentro** do app como editor visual completo — o app gera, valida e abre o artefato; edição rica é no Office.
- Conversão de HTML improvisado em PDF.
- "Kit Genérico" — cada kit tem responsabilidade clara; não diluir.

## Aprofundar antes da Fase 5

- Schema JSON exato do `DocumentSpec` (com `jsonschema`).
- Política de versionamento do schema e migração de specs antigos.
- Catálogo de blocos completo (acima é inicial, há blocos a detalhar).
- Política de fallback quando o `document-worker` não está saudável — o que a UI mostra, o que o app faz.
- Estratégia de cache distribuída ou local (decidir e justificar).
- Política de retry e idempotência de `docs.generate`.
- Como o prompt do modo documental (`PROMPT MESTRE` §16.7) é **gerado a partir do schema** (não mantido à mão — `REGRAS §1.9`).
- Conjunto de modelos (`PROMPT MESTRE` §17.2 para WordPro, §18.3 para ExcelPro) — quais são, como versionar, como o usuário escolhe.

## Decisões

- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — por que Python no worker.

## Referências

- `PROMPT MESTRE` §15 (inteligência documental), §16 (suíte profissional), §17-§19 (Word/Excel/PDF)
- [`process-architecture.md`](./process-architecture.md) (como o worker se conecta)
- [`security-threat-model.md`](./security-threat-model.md) (vetor de ataque adicional)
- `docs/development-roadmap.md` (Fase 5)
