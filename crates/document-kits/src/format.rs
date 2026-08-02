//! `DocumentFormat` — o enum que o modelo vê como inventário.
//!
//! ## Regra de ouro (REGRAS §1.9)
//!
//! O enum é a **fonte da verdade** dos formatos disponíveis. A
//! enumeração que aparece no `input_schema` do `docs.generate`
//! (campo `format`) é **gerada** a partir de
//! `KitRegistry::implemented_formats()` — **nunca** mantida à
//! mão. Adicionar uma variante exige que o `Kit`
//! correspondente esteja **implementado** (não skeleton) **e**
//! registrado no `KitRegistry`; até lá, o modelo não sabe que
//! o formato é uma opção (precedente do ADR-0020 D3 — o Xlsx
//! só entrou no enum junto com o `ExcelProKit` real;
//! idem para o `Pdf` no Etapa 5 PR 2 com o `PdfProKit` real).
//!
//! Inventário que mente é o defeito que derrubou o app
//! anterior — a disciplina é: o que o modelo enxerga
//! corresponde ao que existe. Esta enum é a
//! materialização Rust dessa regra.
//!
//! **Estado atual (Etapa 5 PR 2):** `Docx`, `Xlsx` e `Pdf`.
//! `Pdf` entrou no mesmo commit do `PdfProKit` real
//! (render + glifo-check pre-render, Etapa 5 PR 2) — a
//! auditoria bloqueante do §19.6 (visual + estrutural) entra
//! nos PRs 3 e 4. Bump atômico mantido (precedente do
//! ADR-0020 §3 D3).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Formato de documento que o `docs.generate` pode produzir.
/// **Adicionar uma variante exige**:
///
/// 1. Um `Kit` implementado (não skeleton) que
///    `target_format() == DocumentFormat::Xxx`.
/// 2. O kit registrado no `KitRegistry`.
///
/// Sem os dois, a variante **não** aparece no schema do
/// `docs.generate` — o modelo não pode pedir. Precedente:
/// ADR-0020 §3 (D3) — `DocumentFormat::Xlsx` só entrou no
/// enum junto com o `ExcelProKit` real (Etapa 4);
/// `DocumentFormat::Pdf` entrou no PR 2 da Etapa 5 junto
/// com o `PdfProKit` real (mesma disciplina).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    /// Microsoft Word (`.docx`).
    ///
    /// Implementado na Etapa 3 da Fase 5 (WordPro mínimo,
    /// v0.1 do kit).
    #[serde(alias = "docx")]
    Docx,

    /// Microsoft Excel (`.xlsx`).
    ///
    /// Implementado na Etapa 4 da Fase 5 (ExcelPro v0.1:
    /// Spreadsheet com Kpis/Table/Chart em .xlsx, com
    /// formatos numéricos brasileiros — BRL, PCT, milhar).
    /// Chart real (bar/line/pie com cores) fica pra
    /// Etapa 5/6 com extensão do `xlsx.write` ou handler
    /// novo; Etapa 4 v0.1 ancorada no primitivo.
    #[serde(alias = "xlsx")]
    Xlsx,

    /// PDF (`.pdf`).
    ///
    /// Implementado na Etapa 5 PR 2 da Fase 5 (`PdfProKit`
    /// v0.1): `render` real com `reportlab` Platypus +
    /// fontes Tinta & Latão embutidas (sem fallback) +
    /// identidade visual "Tinta & Latão" + modo Sóbrio +
    /// 20 blocos cobertos + glifo-check via `fontTools`
    /// antes de renderizar (D-GLYPH-1).
    ///
    /// **Limitações v0.1 (registradas como lacunas, NÃO
    /// silenciadas):**
    /// - Auditoria bloqueante do §19.6 (visual pypdfium2 +
    ///   estrutural pikepdf) entra nos PRs 3 e 4.
    /// - Tagged PDF automático: fraco no `reportlab` —
    ///   registrado como pendência 5.x.
    /// - Chart visual nativo no PDF: placeholder textual
    ///   em v0.1, real em Etapa 5.x.
    /// - Sumário automático em duas passadas: placeholder
    ///   em v0.1.
    /// - `docs.inspect` cobrindo `.pdf` (round-trip
    ///   spec→pdf→spec): pendência 5.x.
    #[serde(alias = "pdf")]
    Pdf,
}

impl DocumentFormat {
    /// Serializa como string no formato aceito pelo schema
    /// (`"docx"`, `"xlsx"`, `"pdf"` — snake_case).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pdf => "pdf",
        }
    }

    /// Extensão de arquivo padrão (com ponto).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => ".docx",
            Self::Xlsx => ".xlsx",
            Self::Pdf => ".pdf",
        }
    }

    /// MIME type aproximado.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Pdf => "application/pdf",
        }
    }
}

impl fmt::Display for DocumentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_serde() {
        // O que sai no schema (snake_case) tem que bater com
        // o que a serde produz. Se um dia alguém mudar o
        // rename, este teste pega.
        for fmt in [
            DocumentFormat::Docx,
            DocumentFormat::Xlsx,
            DocumentFormat::Pdf,
        ] {
            let json = serde_json::to_string(&fmt).unwrap();
            let expected = format!("\"{}\"", fmt.as_str());
            assert_eq!(json, expected, "DocumentFormat::{fmt:?} divergente");
        }
    }

    #[test]
    fn extension_includes_dot() {
        assert_eq!(DocumentFormat::Docx.extension(), ".docx");
        assert_eq!(DocumentFormat::Xlsx.extension(), ".xlsx");
        assert_eq!(DocumentFormat::Pdf.extension(), ".pdf");
    }

    #[test]
    fn pdf_format_basics() {
        // Guardas do bump atômico (Etapa 5 PR 2): o enum
        // entrou com `Pdf` no mesmo commit do `PdfProKit`
        // real. Se alguém remover a variante sem remover
        // o kit, esse teste pega (a outra metade da guarda
        // está em `pdfpro.rs` testando `target_format`).
        assert_eq!(DocumentFormat::Pdf.as_str(), "pdf");
        assert_eq!(DocumentFormat::Pdf.extension(), ".pdf");
        assert_eq!(DocumentFormat::Pdf.mime_type(), "application/pdf");
    }
}
