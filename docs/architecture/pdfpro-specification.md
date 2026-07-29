<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-28
Fase correspondente: 5
-->

> Última verificação: 2026-07-28. Reflete a Etapa 1 da Fase 5 — o
> catálogo de blocos do `DocumentSpec` já está definido; o PDFPro
> **em si** (escolha entre `reportlab` e `PyMuPDF` — ADR aberto da
> Fase 0, decisão na Etapa 5; fontes embutidas; validação visual e
> estrutural; **auditoria bloqueante** dentro do salvamento —
> `PROMPT MESTRE` §19.6, sem interruptor) entra na Etapa 5. Fidelidade
> Word → PDF e Excel → PDF entra junto.

# Especificação do PDFPro Kit (stub)

> Stub criado na Fase 0. Aprofundado na Etapa 1 da Fase 5 (catálogo
> de blocos); renderização, validação visual/estrutural e auditoria
> bloqueante entram na Etapa 5.

## Decisão tomada

- Geração, revisão, combinação e validação de PDFs a partir de `DocumentSpec` (`PROMPT MESTRE` §19).
- **Fontes embutidas no PDF final** — nenhum documento pode depender de fonte instalada na máquina do usuário (`PROMPT MESTRE` §5.3 final).
- **Identidade "Tinta & Latão"** + **modo Sóbrio** para registráveis, idênticos aos outros kits.
- **Validação visual** das páginas via renderização para imagens temporárias: conteúdo cortado, tabela ultrapassando margem, sobreposição, página vazia, fonte ausente, caractere quebrado, espaçamento, resolução, cabeçalho, rodapé, alinhamento (`PROMPT MESTRE` §19.3).
- **Validação estrutural**: abertura, quantidade de páginas, metadados, fontes, texto, imagens, links, bookmarks, tamanho, corrupção (`PROMPT MESTRE` §19.4).
- **Auditoria bloqueante** (`PROMPT MESTRE` §19.6): as duas validações executam dentro do salvamento do artefato; reprovação deixa em `invalid` e impede a entrega. **Sem interruptor** para desligar.
- **Fidelidade ao criar PDF a partir de Word/Excel**: hierarquia, títulos, tabelas, gráficos, paginação preservados, arquivo de origem registrado (`PROMPT MESTRE` §19.5).

## Contrato previsto

O PDFPro consome `DocumentSpec` (com `DocumentType::Pdf`) ou um `.docx`/`.xlsx` já gerado (no caso de fidelidade), e produz um `.pdf` real no disco. O `.pdf` passa pelas duas validações (§19.3 e §19.4) **antes** de o artefato ser marcado como `valid`. A auditoria é parte do salvamento, não uma etapa opcional depois.

## Recursos mínimos (`PROMPT MESTRE` §19.1)

Relatórios, demonstrações, documentos para apresentação, capas, sumários, tabelas, gráficos, imagens, cabeçalhos, rodapés, paginação, marca d'água, anexos, metadados, bookmarks, divisão, união, compressão, proteção opcional.

## Não-objetivos

- Editor de PDF dentro do app (anotação leve pode ser considerada depois).
- Assinatura digital de PDF com certificado A1/A3 na v1.
- OCR de PDF escaneado de entrada (a entrada do app é o `DocumentSpec`; OCR é para anexos do usuário, `PROMPT MESTRE` §15).
- Geração de PDF "impressa" de uma página web (a não ser via `DocumentSpec`).

## Aprofundar antes da Fase 5

- Engine de renderização: `reportlab` vs. `PyMuPDF` (a definir com benchmark na Fase 5; `PROMPT MESTRE` §5.3 lista os dois como opção).
- Sumário automático em duas passadas: como renderizar, medir páginas, reescrever sumário com paginação real (`PROMPT MESTRE` §16.4).
- Política de marca d'água (padrão em todos os docs? opt-in? opt-out?).
- Política de compressão de imagens no PDF final.
- Política de proteção (senha) — quando oferecer, com que algoritmo, e como recuperar senha.
- Política de acessibilidade (PDF/A, PDF/UA) — opt-in ou padrão em algum tipo de documento.
- Procedimento de teste de auditoria: injetar falha de fonte, conteúdo cortado, página vazia; o teste prova que a auditoria reprova e bloqueia a entrega.

## Decisões

Nenhuma nova. Decisões serão tomadas quando o spec for aprofundado (especificamente a escolha entre `reportlab` e `PyMuPDF`, que merece ADR).

## Referências

- `PROMPT MESTRE` §16.4 (sumário em duas passadas), §19 (PDFPro)
- [`document-engine-architecture.md`](./document-engine-architecture.md)
- [`wordpro-specification.md`](./wordpro-specification.md) (fidelidade Word → PDF)
- [`excelpro-specification.md`](./excelpro-specification.md) (fidelidade Excel → PDF)
- `docs/development-roadmap.md` (Fase 5)
