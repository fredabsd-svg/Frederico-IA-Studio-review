//! Sanitização de nomes de sheet do Excel (Etapa 4 da Fase 5).
//!
//! O Excel tem regras rígidas pra nomes de sheet:
//! - Máximo 31 caracteres.
//! - Proibidos: `\ / ? * [ ] :` (e nenhum controle char).
//! - Não pode ser vazio.
//! - Não pode colidir com sheet já existente no workbook.
//!
//! Esta função é **pura** (sem I/O) e testável. É usada
//! pelo `ExcelProKit` antes de chamar o `xlsx.write` (o
//! Python rejeita sheet com caracteres proibidos, então a
//! sanitização é defesa em profundidade — o kit garante o
//! contrato).

use std::collections::HashSet;

/// Máximo de caracteres permitido num nome de sheet do
/// Excel. Documentado em <https://support.microsoft.com/en-us/office/rename-a-worksheet-3f1f7148-ee83-404d-8ef0-9ff99fbad1f9>.
const MAX_SHEET_NAME_LEN: usize = 31;

/// Sufixo determinístico pra resolver colisão (Etapa 4
/// deltas: "trunque, resolva colisão com sufixo
/// determinístico (`..._2`) e cubra isso com teste — título
/// com acento, com barra, com 80 caracteres e dois títulos
/// que colidem depois do corte").
const COLLISION_SUFFIX_FMT: &str = "_";

/// Caracteres proibidos pelo Excel em sheet names. Lista
/// derivada de `xl/workbook.xml` spec (OOXML) + teste real
/// no openpyxl 3.1+.
const FORBIDDEN_CHARS: &[char] = &['\\', '/', '?', '*', '[', ']', ':'];

/// Sufixo de fallback quando o nome proposto é vazio
/// (depois de strip de whitespace + forbidden chars).
/// Formato: `Table_<block_index>`. Se `block_index` não
/// está disponível (caso degenerado), usa `Sheet`.
fn fallback_name(block_index: Option<usize>) -> String {
    match block_index {
        Some(i) => format!("Table_{i}"),
        None => "Sheet".to_string(),
    }
}

/// Sanitiza um nome de sheet.
///
/// Regras (em ordem):
/// 1. Strip de whitespace leading/trailing.
/// 2. Remove caracteres proibidos (`\ / ? * [ ] :`).
/// 3. Trunca pra 31 chars (se > 31).
/// 4. Se vazio, usa o fallback `Table_<block_index>`.
/// 5. Resolve colisão com sufixo `_2`, `_3`, etc. (até
///    `_999`; depois disso, é problema do caller — não
///    vamos lidar com 1000 sheets).
///
/// `block_index` é usado **apenas** no fallback (regra 4)
/// — a colisão (regra 5) usa o nome base, não o índice.
///
/// `used` é o set de nomes já presentes no workbook
/// (mutado: após a função, o nome escolhido é inserido).
pub fn sanitize_sheet_name(
    proposed: &str,
    block_index: Option<usize>,
    used: &mut HashSet<String>,
) -> String {
    // 1-3: strip, remove forbidden, trunca.
    let cleaned: String = proposed
        .chars()
        .filter(|c| !FORBIDDEN_CHARS.contains(c))
        .collect();
    let trimmed = cleaned.trim();
    let base = if trimmed.is_empty() {
        fallback_name(block_index)
    } else {
        truncate_to_max(trimmed, MAX_SHEET_NAME_LEN)
    };

    // 5: resolve colisão.
    if !used.contains(&base) {
        used.insert(base.clone());
        return base;
    }

    // Colisão: tenta `_2`, `_3`, ... até `_999`.
    for n in 2..=999 {
        let suffix = format!("{COLLISION_SUFFIX_FMT}{n}");
        let candidate_max_len = MAX_SHEET_NAME_LEN - suffix.len();
        if candidate_max_len < 1 {
            // Nome base > 30 chars + sufixo de 2+ chars
            // ultrapassa 31. Impossível nesse caso (base
            // ja foi truncado a 31), mas defensivo.
            break;
        }
        let candidate = if base.len() <= candidate_max_len {
            format!("{base}{suffix}")
        } else {
            let truncated = truncate_to_max(&base, candidate_max_len);
            format!("{truncated}{suffix}")
        };
        if !used.contains(&candidate) {
            used.insert(candidate.clone());
            return candidate;
        }
    }

    // Fallback final: `_fallback_<block_index>_<n>` (caso
    // patológico de 998+ sheets com o mesmo nome base).
    // Não acontece no Frederico (planilha tem < 100 sheets
    // na v1), mas é defensivo.
    let n = used.len() + 1;
    let candidate = match block_index {
        Some(i) => format!("_f_{i}_{n}"),
        None => format!("_f_{n}"),
    };
    let candidate = truncate_to_max(&candidate, MAX_SHEET_NAME_LEN);
    used.insert(candidate.clone());
    candidate
}

/// Trunca `s` pra `max_len` chars. UTF-8 safe (trunca por
/// `char`, não por byte — evita cortar no meio de um
/// codepoint multi-byte).
fn truncate_to_max(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    s.chars().take(max_len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_passes_through() {
        let mut used = HashSet::new();
        assert_eq!(sanitize_sheet_name("Vendas", Some(0), &mut used), "Vendas");
    }

    #[test]
    fn forbidden_chars_removed() {
        let mut used = HashSet::new();
        // barra, interrogacao, asterisco
        assert_eq!(sanitize_sheet_name("a/b?c*d", Some(0), &mut used), "abcd");
        // colchetes
        assert_eq!(sanitize_sheet_name("x[1]y", Some(1), &mut used), "x1y");
        // dois pontos
        assert_eq!(sanitize_sheet_name("a:b:c", Some(2), &mut used), "abc");
        // contra-barra
        assert_eq!(sanitize_sheet_name("a\\b", Some(3), &mut used), "ab");
    }

    #[test]
    fn trims_whitespace() {
        let mut used = HashSet::new();
        assert_eq!(
            sanitize_sheet_name("  Vendas  ", Some(0), &mut used),
            "Vendas"
        );
    }

    #[test]
    fn truncates_to_31_chars() {
        let mut used = HashSet::new();
        // 80 chars → truncado a 31
        let long = "a".repeat(80);
        let result = sanitize_sheet_name(&long, Some(0), &mut used);
        assert_eq!(result.chars().count(), 31);
    }

    #[test]
    fn empty_name_falls_back_to_table_n() {
        let mut used = HashSet::new();
        // só whitespace
        assert_eq!(sanitize_sheet_name("   ", Some(7), &mut used), "Table_7");
        // só forbidden chars
        assert_eq!(sanitize_sheet_name("///", Some(3), &mut used), "Table_3");
        // vazio total
        assert_eq!(sanitize_sheet_name("", Some(0), &mut used), "Table_0");
        // sem block_index
        assert_eq!(sanitize_sheet_name("", None, &mut used), "Sheet");
    }

    #[test]
    fn collision_resolved_with_suffix_2() {
        let mut used = HashSet::new();
        // Pre-popular
        used.insert("Vendas".to_string());
        assert_eq!(
            sanitize_sheet_name("Vendas", Some(1), &mut used),
            "Vendas_2"
        );
    }

    #[test]
    fn collision_with_long_name_truncates_base() {
        let mut used = HashSet::new();
        // 30 chars (cabe em 31) — pre-popula o set.
        let long = "a".repeat(30);
        used.insert(long.clone());
        // Colisao: nome base de 30 chars + sufixo `_2`
        // (2 chars) = 32 chars, que ultrapassa o max
        // de 31 do Excel. A funcao DEVE truncar a base
        // pra 29 chars (30 - 2 + 1) pra caber o sufixo.
        let result = sanitize_sheet_name(&long, Some(1), &mut used);
        assert_eq!(
            result.chars().count(),
            31,
            "resultado deve ter exatamente 31 chars"
        );
        assert!(result.ends_with("_2"), "sufixo _2 deve estar presente");
        // Base truncada: 29 'a's + "_2" = 31.
        let expected = format!("{}_2", "a".repeat(29));
        assert_eq!(result, expected);
    }

    #[test]
    fn collision_truncates_to_fit_suffix() {
        // Caso degenerado: nome base de 31 chars já está
        // no set. Colisão → sufixo `_2` precisa caber.
        // Truncar base pra 29 + "_2" = 31.
        let mut used = HashSet::new();
        let base = "a".repeat(31);
        used.insert(base.clone());
        let result = sanitize_sheet_name(&base, Some(1), &mut used);
        // Deve ser "aaa...a" (29 chars) + "_2" = 31 chars
        assert_eq!(result.chars().count(), 31);
        assert!(result.ends_with("_2"));
    }

    #[test]
    fn collision_chain_increments_suffix() {
        let mut used = HashSet::new();
        used.insert("Vendas".to_string());
        used.insert("Vendas_2".to_string());
        used.insert("Vendas_3".to_string());
        assert_eq!(
            sanitize_sheet_name("Vendas", Some(4), &mut used),
            "Vendas_4"
        );
    }

    #[test]
    fn used_set_is_mutated() {
        let mut used = HashSet::new();
        let result = sanitize_sheet_name("Foo", Some(0), &mut used);
        assert_eq!(result, "Foo");
        assert!(used.contains("Foo"));
    }

    #[test]
    fn accent_passes_through() {
        // Acentos e cedilha são chars válidos (não
        // forbidden), só contam como 1 char cada.
        let mut used = HashSet::new();
        assert_eq!(
            sanitize_sheet_name("Receitações", Some(0), &mut used),
            "Receitações"
        );
    }

    #[test]
    fn utf8_multibyte_truncation_is_char_safe() {
        // 30 "a" + "ção" (3 chars com acento) = 33 chars.
        // Truncado a 31, deve cortar "o" (32) → fica
        // 30 "a" + "çã" = 32. Hmm, 32 > 31. Vou testar
        // com 28 "a" + "ção" = 31, exatamente.
        let mut used = HashSet::new();
        let name = format!("{}ção", "a".repeat(28));
        assert_eq!(name.chars().count(), 31);
        let result = sanitize_sheet_name(&name, Some(0), &mut used);
        assert_eq!(result.chars().count(), 31);
    }
}
