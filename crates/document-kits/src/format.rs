//! `DocumentFormat` — o enum que o modelo vê como inventário.
//!
//! ## Regra de ouro (REGRAS §1.9)
//!
//! O enum é a **fonte da verdade** dos formatos disponíveis. A
//! enumeração que aparece no `input_schema` do `docs.generate`
//! (campo `format`) é **gerada** a partir de
//! `KitRegistry::implemented_formats()` — **nunca** mantida à
//! mão. Adicionar `DocumentFormat::Xlsx` exige que o
//! `ExcelProKit` esteja implementado e registrado; até lá, o
//! modelo não sabe que `.xlsx` é uma opção.
//!
//! Inventário que mente é o defeito que derrubou o app
//! anterior — a disciplina é: o que o modelo enxerga
//! corresponde ao que existe. Esta enum é a
//! materialização Rust dessa regra.

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
/// `docs.generate` — o modelo não pode pedir.
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
}

impl DocumentFormat {
    /// Serializa como string no formato aceito pelo schema
    /// (`"docx"`, `"xlsx"`, `"pdf"` — snake_case).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
        }
    }

    /// Extensão de arquivo padrão (com ponto).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => ".docx",
            Self::Xlsx => ".xlsx",
        }
    }

    /// MIME type aproximado.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
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
        for fmt in [DocumentFormat::Docx, DocumentFormat::Xlsx] {
            let json = serde_json::to_string(&fmt).unwrap();
            let expected = format!("\"{}\"", fmt.as_str());
            assert_eq!(json, expected, "DocumentFormat::{fmt:?} divergente");
        }
    }

    #[test]
    fn extension_includes_dot() {
        assert_eq!(DocumentFormat::Docx.extension(), ".docx");
        assert_eq!(DocumentFormat::Xlsx.extension(), ".xlsx");
    }
}
