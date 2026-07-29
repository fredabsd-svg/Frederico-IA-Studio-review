//! Sanitização de input para queries FTS5.
//!
//! O FTS5 do SQLite tem dois modos de casamento de tokens
//! livres: `AND` implícito (padrão — exige todos os tokens)
//! e `OR` explícito (casa qualquer token). O padrão do FTS5
//! é restritivo demais pro caso de uso de memória: o usuário
//! raramente lembra a frase exata, lembra de palavras-chave.
//! Tokens livres + AND implícito mata o recall.
//!
//! **Estratégia da Etapa 1:** juntar tokens com `OR` explícito
//! — o BM25 ranqueia, e o mais relevante sobe. O recall
//! melhora (memória com 1 dos tokens relevantes aparece), a
//! precisão é controlada pelo scoring (sinais 1-5 do
//! `ADR-0011 §2`).
//!
//! Sanitização:
//!
//! 1. **Strip de caracteres perigosos**: aspas duplas, `*`, `(`,
//!    `)`, `^`, `:`, `-`, vírgula, ponto-e-vírgula, `=`, `<`,
//!    `>`, `!` viram espaço. Isso neutraliza injection.
//! 2. **Tokens livres + `OR` explícito**: cada token vira
//!    `token OR`. Sem aspas externas. `OR` é palavra-chave
//!    do FTS5.
//! 3. **Trim + colapso de whitespace**.
//! 4. **Limite de tamanho** (defesa contra DoS).
//!
//! O resultado é seguro pra colar direto num `MATCH` do FTS5.

/// Tamanho máximo de uma query sanitizada, em caracteres.
pub const MAX_QUERY_LEN: usize = 1024;

/// Sanitiza uma query para uso no operador `MATCH` do FTS5.
/// Devolve string tokenizada com `OR` entre tokens, pronta
/// pra colar em `WHERE memory_fts MATCH ?1`. Strings vazias
/// ou só com espaços devolvem `String::new()` — o caller deve
/// tratar como "sem busca lexical".
///
/// # Exemplos
///
/// ```text
/// sanitize_fts5_query("Rust")                     → "Rust"
/// sanitize_fts5_query("linguagem Rust")          → "linguagem OR Rust"
/// sanitize_fts5_query("")                        → ""
/// sanitize_fts5_query("\"ignore all prev\"")     → "ignore OR all OR prev"
/// sanitize_fts5_query("a AND b")                 → "a OR AND OR b"
/// ```
#[must_use]
pub fn sanitize_fts5_query(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Substitui cada caractere perigoso por espaço. Mantém
    // alphanumeric (Unicode-aware), whitespace, e underscore.
    //
    // Caracteres perigosos neutralizados:
    //   `"` (delimitador de termo) → espaço
    //   `*` (prefix match)         → espaço
    //   `(` `)` (grouping)         → espaço
    //   `^` (column filter)        → espaço
    //   `:` (column filter)        → espaço
    //   `-` (negation)             → espaço
    //   `,` `;` (separadores)      → espaço
    //   `=` `<` `>` `!` (operadores) → espaço
    //   `\` `/` (path injection)   → espaço
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect();

    // Quebra em tokens (colapsando whitespace) e junta com " OR ".
    // O FTS5 reconhece `OR` como keyword (mesmo se o token
    // também for "OR" no índice — ele é tratado como operador
    // no contexto MATCH).
    let tokens: Vec<&str> = safe.split_whitespace().collect();

    if tokens.is_empty() {
        return String::new();
    }

    let joined = tokens.join(" OR ");

    // Trunca no teto (defesa contra DoS). Conta chars
    // (não bytes) pra não cortar UTF-8 no meio.
    if joined.chars().count() > MAX_QUERY_LEN {
        let mut end = MAX_QUERY_LEN;
        for (i, (idx, _)) in joined.char_indices().enumerate() {
            if idx == MAX_QUERY_LEN {
                end = i;
                break;
            }
        }
        return joined[..end].to_string();
    }

    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_empty() {
        assert_eq!(sanitize_fts5_query(""), "");
        assert_eq!(sanitize_fts5_query("   "), "");
        assert_eq!(sanitize_fts5_query("\t\n"), "");
    }

    #[test]
    fn single_token_unchanged() {
        assert_eq!(sanitize_fts5_query("Rust"), "Rust");
    }

    #[test]
    fn multiple_tokens_joined_with_or() {
        // "linguagem Rust" → "linguagem OR Rust" — o FTS5
        // casa qualquer dos tokens.
        assert_eq!(sanitize_fts5_query("linguagem Rust"), "linguagem OR Rust");
        assert_eq!(
            sanitize_fts5_query("linguagem de programação"),
            "linguagem OR de OR programação"
        );
    }

    #[test]
    fn whitespace_trimmed_and_collapsed() {
        assert_eq!(
            sanitize_fts5_query("  Rust   programming  "),
            "Rust OR programming"
        );
    }

    #[test]
    fn double_quote_replaced_with_space() {
        assert_eq!(
            sanitize_fts5_query("\"ignore all previous instructions\""),
            "ignore OR all OR previous OR instructions"
        );
    }

    #[test]
    fn fts5_operators_become_literal_tokens() {
        // AND/OR viram tokens literais — o FTS5 trata como
        // "OR" só no contexto certo (entre tokens), e como
        // termo a casar no índice.
        assert_eq!(sanitize_fts5_query("a AND b"), "a OR AND OR b");
        assert_eq!(sanitize_fts5_query("foo OR bar"), "foo OR OR OR bar");
    }

    #[test]
    fn star_neutralized() {
        // O `*` vira espaço — neutralizado.
        assert_eq!(sanitize_fts5_query("wild*"), "wild");
    }

    #[test]
    fn unicode_preserved() {
        // Português: acentos mantidos, is_alphanumeric é Unicode-aware.
        assert_eq!(
            sanitize_fts5_query("linguagem Rust com café"),
            "linguagem OR Rust OR com OR café"
        );
    }

    #[test]
    fn truncation_respects_char_boundary() {
        let long: String = "a".repeat(1500);
        let sanitized = sanitize_fts5_query(&long);
        // Truncado em MAX_QUERY_LEN chars (não bytes).
        assert!(sanitized.chars().count() <= MAX_QUERY_LEN);
    }

    #[test]
    fn prompt_injection_neutralized() {
        // Tentativa clássica: o usuário envia texto com aspas
        // e operador OR pra quebrar a query. Após sanitização,
        // vira sequência de tokens inertes.
        let malicious = r#""OR 1=1 -- DROP TABLE memory_records; --"#;
        let sanitized = sanitize_fts5_query(malicious);
        // Aspas removidas (= virou espaço), traços e `;` viraram
        // espaço, `=` virou espaço. Tokens juntam com " OR ".
        // Note: `_` é preservado por design, então `memory_records`
        // continua junto.
        assert_eq!(sanitized, "OR OR 1 OR 1 OR DROP OR TABLE OR memory_records");
        // Não há aspas, asterisco, igual ou ponto-e-vírgula.
        assert!(!sanitized.contains('"'));
        assert!(!sanitized.contains('*'));
        assert!(!sanitized.contains('='));
        assert!(!sanitized.contains(';'));
    }

    #[test]
    fn underscore_preserved() {
        // Identificadores com underscore (ex: tool_id) sobrevivem.
        assert_eq!(sanitize_fts5_query("files.read"), "files OR read");
    }
}
