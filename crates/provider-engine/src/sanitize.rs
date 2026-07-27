//! Sanitização de fixtures gravadas pelo `provider-recorder`.
//!
//! Toda fixture `.jsonl` em `fixtures/` é gerada por alguém que fez
//! uma chamada real a um provedor. Se essa fixture vazar pro
//! repositório com um header de autorização ou uma chave de API, é
//! game over. Por isso o recorder (e a checagem de CI) passam
//! **todo byte** por [`check`] antes de aceitar a fixture.
//!
//! ## Política
//!
//! Bloqueamos (case-insensitive) os tokens que costumam aparecer em
//! credenciais de LLM:
//!
//! - `Authorization` — header HTTP canônico.
//! - `api_key` — header de alguns provedores (Gemini, OpenRouter antigo).
//! - `x-api-key` — header da Anthropic (`x-api-key: sk-ant-...`).
//! - `Bearer ` — formato do header `Authorization: Bearer ...` (com
//!   espaço à direita, evita falsos positivos como "Bearer Smith").
//! - `sk-` — prefixo de chaves OpenAI e similares.
//! - `sk-ant-` — prefixo de chaves Anthropic.
//! - `gsk_` — prefixo de chaves Groq.
//! - `or-` — prefixo de chaves OpenRouter (formato `sk-or-...`,
//!   mas aparece como `or-` depois de `sk-`).
//!
//! Se qualquer um desses tokens aparecer em qualquer linha da
//! fixture, [`check`] devolve `Err` com o número da linha e o token
//! achado. O recorder aborta a gravação e **deleta** o arquivo.
//!
//! A função é deliberadamente simples — sem regex, sem normalização
//! Unicode exótica — para que o comportamento seja óbvio e auditável.
//! Adicionar mais tokens é uma mudança de uma linha.

use std::fmt;

/// Tokens proibidos (todos lowercase; a checagem é case-insensitive).
const FORBIDDEN_TOKENS: &[&str] = &[
    "authorization",
    "api_key",
    "x-api-key",
    "bearer ",
    "sk-",
    "sk-ant-",
    "gsk_",
    "or-",
];

/// Erro de sanitização: o `token` proibido apareceu na linha `line`
/// (1-indexed). A fixture **deve** ser rejeitada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizeError {
    pub line: usize,
    pub token: &'static str,
}

impl fmt::Display for SanitizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fixture rejeitada: token proibido `{}` encontrado na linha {}",
            self.token, self.line
        )
    }
}

impl std::error::Error for SanitizeError {}

/// Verifica se o conteúdo da fixture está limpo. Retorna
/// `Ok(())` se nenhum token proibido aparecer, ou `Err` com a
/// primeira violação encontrada (linha 1-indexed).
///
/// A varredura é byte-a-byte mas case-insensitive: cada linha é
/// convertida para lowercase uma única vez e os tokens
/// (já lowercase) são procurados via `contains`. Custo O(n) no
/// tamanho da fixture, sem alocação por linha.
pub fn check(content: &str) -> Result<(), SanitizeError> {
    for (idx, raw_line) in content.lines().enumerate() {
        let line = raw_line.to_lowercase();
        for &token in FORBIDDEN_TOKENS {
            if line.contains(token) {
                return Err(SanitizeError {
                    line: idx + 1,
                    token,
                });
            }
        }
    }
    Ok(())
}

/// Versão "info" do check: devolve **todos** os tokens proibidos
/// encontrados (uma vez por token, sem número de linha). Útil pra
/// reportar a um humano o que precisa ser limpo. Não usada no gate
/// do CI — só no recorder, se quisermos dar uma mensagem mais
/// detalhada antes de abortar.
#[allow(dead_code)]
pub fn list_violations(content: &str) -> Vec<&'static str> {
    let lower = content.to_lowercase();
    let mut out = Vec::new();
    for &token in FORBIDDEN_TOKENS {
        if lower.contains(token) {
            out.push(token);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_fixture_passes() {
        let fixture = "# recorded_at=2026-07-27\n\
                       data: {\"id\":\"chatcmpl-abc\",\"object\":\"chat.completion.chunk\"}\n\
                       data: [DONE]\n";
        assert!(check(fixture).is_ok());
    }

    #[test]
    fn rejects_authorization_header() {
        let bad = "Authorization: Bearer sk-fake\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.line, 1);
        // `authorization` é o primeiro token proibido que casa.
        assert_eq!(err.token, "authorization");
    }

    #[test]
    fn rejects_api_key_header() {
        // Variação: `api_key` com underscore.
        let bad = "{\"api_key\":\"sk-fake\"}\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.token, "api_key");
    }

    #[test]
    fn rejects_x_api_key_header() {
        let bad = "x-api-key: sk-ant-fake\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.token, "x-api-key");
    }

    #[test]
    fn rejects_bearer_token() {
        // Cuidando para a string `bearer ` ter espaço à direita,
        // evitando "Bearer Smith" como falso positivo.
        let bad = "{\"header\":\"Bearer xyz\"}\n";
        let err = check(bad).unwrap_err();
        // O `bearer ` (com espaço) bate, mas `sk-` (caso o token
        // tivesse `sk-` no meio) também poderia bater. Aqui só
        // tem `bearer `, então é esse.
        assert_eq!(err.token, "bearer ");
    }

    #[test]
    fn rejects_openai_style_sk_prefix() {
        let bad = "{\"key\":\"sk-proj-abc123\"}\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.token, "sk-");
    }

    #[test]
    fn rejects_anthropic_sk_ant_prefix() {
        let bad = "sk-ant-api03-fake\n";
        let err = check(bad).unwrap_err();
        // `sk-` casa antes de `sk-ant-` (na ordem do array), e
        // ambos casariam aqui. A função pega o primeiro.
        assert_eq!(err.token, "sk-");
    }

    #[test]
    fn rejects_groq_gsk_prefix() {
        let bad = "{\"key\":\"gsk_abc\"}\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.token, "gsk_");
    }

    #[test]
    fn rejects_openrouter_or_prefix() {
        // `or-` casa em `sk-or-v1-...`, mas o token `sk-` casa
        // primeiro. Verificamos que o `or-` é detectado quando
        // está sozinho, fora de `sk-`.
        let bad = "openrouter token: or-v1-abc\n";
        let err = check(bad).unwrap_err();
        assert_eq!(err.token, "or-");
    }

    #[test]
    fn case_insensitive() {
        let bad = "AUTHORIZATION: BEARER sk-FAKE\n";
        let err = check(bad).unwrap_err();
        // `authorization` (lowercase) bate primeiro.
        assert_eq!(err.token, "authorization");
    }

    #[test]
    fn reports_line_number() {
        let fixture = "# header limpo\n\
                       data: ok\n\
                       data: Authorization leaked\n";
        let err = check(fixture).unwrap_err();
        assert_eq!(err.line, 3);
    }

    #[test]
    fn list_violations_returns_all() {
        let fixture = "Authorization: Bearer sk-fake\n\
                       data: ok\n";
        let violations = list_violations(fixture);
        // Pelo menos `authorization`, `bearer ` e `sk-` casam.
        assert!(violations.contains(&"authorization"));
        assert!(violations.contains(&"bearer "));
        assert!(violations.contains(&"sk-"));
    }

    #[test]
    fn empty_fixture_passes() {
        assert!(check("").is_ok());
    }

    #[test]
    fn bearer_without_trailing_space_does_not_match() {
        // O token `bearer ` exige espaço à direita. Sozinho, sem
        // espaço, não deve casar — evita falsos positivos.
        let fixture = "{\"role\":\"bearer\"}\n";
        assert!(check(fixture).is_ok());
    }
}
