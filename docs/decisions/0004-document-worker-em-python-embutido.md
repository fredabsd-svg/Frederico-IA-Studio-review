# 0004 — `document-worker` em Python embutido

## Contexto

O `PROMPT MESTRE` §5.3 lista uma série de bibliotecas "maduras" para o `document-worker`: `python-docx`, `openpyxl`, `reportlab` ou `PyMuPDF`, `matplotlib`, `pdfplumber`, `pytesseract` com Tesseract e dados de idioma pt-BR. O §16 detalha que o WordPro, ExcelPro e PDFPro Kits compartilham um "Document Artifact Engine" e que o mesmo `DocumentSpec` deve renderizar em Word e em PDF preservando hierarquia, títulos, tabelas e paginação.

A escolha da tecnologia do worker afeta três variáveis: paridade de funcionalidades com os requisitos do `PROMPT MESTRE` §16-§19, portabilidade do build (o usuário não pode precisar instalar Python), e custo de manutenção.

## Decisão

O `document-worker.exe` é distribuído com **Python embeddable** (também chamado de "Python ZIP"), gerenciado como pacote versionado e assinado. O instalador do Frederico IA Studio empacota:

- interpretador Python embeddable (Windows, 64 bits);
- bibliotecas: `python-docx`, `openpyxl`, `reportlab`, `pymupdf` (`fitz`), `matplotlib`, `pdfplumber`, `pytesseract`;
- binário do Tesseract OCR com dados de idioma `por` (pt-BR) e `eng` (en);
- fontes da identidade visual "Tinta & Latão" (Source Serif 4 e Source Sans 3), para embutir nos PDFs (§16.3).

O worker é executado como processo separado, sem API em `localhost` (`PROMPT MESTRE` §5.3, §22.5). Comunicação com o app principal via **named pipes** no Windows, com protocolo JSON versionado. O handshake (`PROMPT MESTRE` §7.3) verifica versão, capacidades, dependências e compatibilidade.

O worker **não altera o `PATH` global** do Windows. O app principal resolve o caminho do interpretador e passa para o worker. As bibliotecas vivem em um diretório gerenciado pelo próprio app (`%LOCALAPPDATA%\FredericoAIStudio\runtimes\python\` ou dentro do diretório de instalação).

## Alternativas descartadas

- **Rust puro para o `document-worker`** (ex: `docx-rs`, `rust_xlsxwriter`, `printpdf`, `lopdf`). Descartada: o ecossistema Rust para documentos Office e PDF é **significativamente menos maduro** que o Python — paridade 1:1 com `python-docx` (estilos avançados, listas multinível, controle de seção) e com `openpyxl` (formatação condicional, tabelas estruturadas, validação de dados) exigiria reescrever milhares de horas de biblioteca. O custo de manter isso internamente é proibitivo para a v1.
- **Node.js com `docx`, `excel4node`, `pdfkit` etc.** Descartada: paridade semelhante ao Rust para Word, e o caso de PDF é ainda pior (`pdfkit` é de baixo nível, não tem agregação tipográfica decente). Aprofundaria a dívida de manter bibliotecas próprias.
- **Chamar o Office via COM/Interop.** Descartada: viola o `PROMPT MESTRE` §5.2 ("o usuário não deverá instalar manualmente [...] Office"). Usuários sem Office não conseguiriam usar o app.
- **LibreOffice headless** como motor de conversão. Descartada: adiciona ~1.5 GB ao instalador, comportamento de renderização divergente do Office em casos de borda, e amarraria a qualidade visual a uma versão upstream que não controlamos.
- **PyInstaller para empacotar tudo em um `.exe` monolítico.** Descartada para o worker principal, mantida como opção para distribuição: ter o interpretador e bibliotecas como arquivos separados permite atualizar bibliotecas sem rebuildar o executável principal, e facilita a auditoria. **Reabrir** se a distribuição como arquivos separados virar problema de empacotamento.

## Consequências

**Mais fácil:**
- Paridade total com os requisitos do `PROMPT MESTRE` §16-§19 sem reescrever bibliotecas.
- Bibliotecas Python podem ser atualizadas independentemente do app principal.
- O mesmo Python pode ser reaproveitado por outros workers que precisem de ecossistema científico (futuro).
- `python-docx` e `openpyxl` são bibliotecas保守, com anos de uso em produção, e bugs reais tendem a ser corrigidos rapidamente.

**Mais difícil:**
- O instalador fica ~150 MB maior (Python embeddable + libs + Tesseract + dados de idioma + fontes). É um custo de download real e precisa ser comunicado honestamente no `README` quando a Fase 9 chegar.
- Build do worker é mais complexo: precisa de um ambiente Python rodando em CI só para empacotar e testar.
- Atualizar bibliotecas Python exige rebuildar o pacote versionado e revalidar paridade visual — risco de regressão sutil.
- O `document-worker` se torna um vetor de ataque a mais (processo separado, mas com ecossistema grande). A threat model de `docs/architecture/security-threat-model.md` precisa cobrir Python especificamente (ex: import de módulos perigosos, eval no código gerado).
- Duas linguagens no mesmo produto (Rust no núcleo + Python no worker) aumentam a complexidade de contratação e onboarding.
