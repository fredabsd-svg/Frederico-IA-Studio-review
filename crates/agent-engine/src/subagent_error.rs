//! `SubagentError` — erros estruturados do `SubagentRunner` (Etapa 4
//! da Fase 6, ADR-0027 §D4).
//!
//! **Princípio fundamental (D4 do ADR-0027):** "Verificação no
//! spawn, erro legível, nunca panic nem silent fail."
//!
//! O `SubagentRunner::try_spawn` retorna `Result<SubagentHandle,
//! SubagentError>`. Cada variante carrega informação suficiente pro
//! modelo do pai **reformular** a decisão:
//!
//! - `GlobalLimitReached`: cancelar subagente existente ou
//!   reformular sem o 9º.
//! - `DepthExceeded`: reformular sem o neto, ou expandir o
//!   trabalho do filho existente.
//! - `AllocationExceedsParent`: reduzir a alocação ou liberar
//!   budget de outros subagentes.
//! - `Registry(RegistryError)`: usar um ID da lista `valid` (não
//!   inventar novo).
//! - `PermissionDenied`: ferramentas que o especialista pediu
//!   mas o pai não tem — abrir o diálogo com o usuário.
//! - `InternalError`: erro inesperado; reportar e tentar de novo
//!   com alocação menor.
//!
//! **Por que `Display` é legível pelo modelo, não só por humanos:**
//! o modelo do pai recebe o `Display` (via `to_string()`) e usa
//! como parte do contexto da próxima chamada. Texto claro +
//! nomes dos eixos + valores + sugestão de ação → modelo
//! consegue decidir sem ambiguidade.
//!
//! **Por que `thiserror`:** mesma família do `AllocationError`
//! (em `budget_allocation.rs`) e do `TransitionError` (Etapa 2
//! da Fase 6). Display + source automáticos.

use std::fmt;

use crate::budget_allocation::AllocationError;

/// Detalhe do `SubagentError::UnknownSpecialist` — usado como
/// `#[error(transparent)]` no enum pai (vide abaixo). `Display`
/// manual porque `thiserror` não implementa `Display` direto pra
/// `Vec<String>` em format string do `#[error("...")]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnknownSpecialistDetail {
    /// ID que o modelo pediu (não existe no registry).
    pub requested: String,
    /// Lista de IDs válidos (vem do `RegistryError::UnknownSpecialist { valid }`).
    pub valid: Vec<String>,
}

impl fmt::Display for UnknownSpecialistDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let valid_joined = self.valid.join(", ");
        write!(
            f,
            "não foi possível criar o subagente: '{}' não existe no SpecialistRegistry. \
             IDs válidos: [{}]. Escolha um da lista (não invente novo).",
            self.requested, valid_joined
        )
    }
}

impl std::error::Error for UnknownSpecialistDetail {}

/// Erro estruturado do `SubagentRunner::try_spawn`. Cada variante
/// carrega informação suficiente pro modelo do pai reformular a
/// decisão (D4 do ADR-0027).
///
/// **`Display` e `std::error::Error` são manuais** (não usamos
/// `#[derive(thiserror::Error)]` no enum) porque uma das variantes
/// (`UnknownSpecialist`) carrega um `Vec<String>` que o `thiserror`
/// não consegue formatar no `#[error("...")]` direto. A
/// `UnknownSpecialistDetail` é um struct novo que tem `Display`
/// manual, e a variante do enum delega. Mesma estratégia do
/// `RegistryError` (Etapa 3 da Fase 6, ADR-0030).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SubagentError {
    /// Teto global de 8 subagentes por run atingido (D1 do
    /// ADR-0027). O modelo pode cancelar um subagente ativo ou
    /// reformular sem o 9º.
    GlobalLimitReached {
        /// Contador atual no momento da rejeição (igual a `max`).
        current: u32,
        /// Teto global (constante do projeto: 8 no ADR-0027 D1).
        max: u32,
        /// Próximo índice que o modelo tentou criar (current + 1).
        next: u32,
    },

    /// Teto de profundidade 2 excedido (D2 do ADR-0027).
    /// O pai tentou criar um neto (profundidade 3). O modelo
    /// reformula sem o neto, ou expande o trabalho do filho
    /// existente.
    DepthExceeded {
        /// Profundidade do pai (0 = raiz; 1 = filho direto; 2 = neto bloqueado).
        current: u32,
        /// Teto de profundidade (constante do projeto: 2 no ADR-0027 D2).
        max: u32,
    },

    /// A alocação pedida excede o `parent_remaining` (D3 + D5 do
    /// ADR-0027). O modelo reduz a alocação ou cancela outros
    /// subagentes pra liberar budget.
    AllocationExceedsParent {
        /// Erro de alocação original (carrega eixo, requested, available).
        cause: AllocationError,
    },

    /// O `SpecialistRegistry` rejeitou o ID (Etapa 3 da Fase 6,
    /// ADR-0030 §D4 + PROMPT MESTRE §9.2 — "zero fallback
    /// silencioso"). O modelo usa um ID da lista `valid` em vez
    /// de inventar novo. `Display` delega pro
    /// `UnknownSpecialistDetail` (mais simples que derivar manual).
    UnknownSpecialist(UnknownSpecialistDetail),

    /// As ferramentas que o especialista pediu excedem o que o
    /// pai tem. Diferente do `PermissionDenied` da
    /// `PermissionSet::is_subset_of` (que é falha em runtime) —
    /// aqui é falha **no spawn** (já nem deixa o subagente
    /// existir).
    PermissionDenied {
        /// Lista das ferramentas que faltam (formato legível).
        reason: String,
    },

    /// Erro interno inesperado (bug, I/O falhou, etc). Não é
    /// falha do modelo — reportar e tentar de novo com alocação
    /// menor. A Etapa 4 PR 2 (spawn real) vai popular isto em
    /// casos como: DB write falhou, portão `apply_transition`
    /// rejeitou (não deveria — o subagente nasce em `created`).
    InternalError(String),
}

impl SubagentError {
    /// `true` se a falha é por causa do modelo ter pedido algo
    /// que o portão rejeitou (limite, profundidade, alocação,
    /// registry, permissão). O modelo **pode** reformular.
    ///
    /// `false` se é bug interno — o modelo **não** pode
    /// reformular, precisa pedir ajuda do usuário ou abortar.
    #[must_use]
    pub fn is_model_recoverable(&self) -> bool {
        matches!(
            self,
            Self::GlobalLimitReached { .. }
                | Self::DepthExceeded { .. }
                | Self::AllocationExceedsParent { .. }
                | Self::UnknownSpecialist { .. }
                | Self::PermissionDenied { .. }
        )
    }
}

/// `Display` pro `SubagentError::UnknownSpecialist` agora vive no
/// `UnknownSpecialistDetail` (acima), via `#[error(transparent)]`.
/// Este bloco é só placeholder pra remover a implementação manual
/// anterior — `thiserror` já gera o `Display` pro enum inteiro.
impl std::fmt::Display for SubagentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `thiserror` já gera o `Display` pro enum; este impl é
        // necessário porque o `thiserror` 1.x não consegue
        // coexistir com a forma padrão quando há um
        // `#[error(transparent)]` — a macro emite `match` no
        // `source()` mas o `Display` é gerado. O impl manual
        // delega pra `format!("{}", ...)` que reusa o `Display`
        // do `UnknownSpecialistDetail`.
        match self {
            Self::UnknownSpecialist(detail) => write!(f, "{detail}"),
            Self::GlobalLimitReached { current, max, next } => write!(
                f,
                "não foi possível criar o subagente: limite global de {max} subagentes por run \
                 atingido (atual: {current}). Cancele um subagente ativo ou reformule sem o \
                 {next}º."
            ),
            Self::DepthExceeded { current, max } => write!(
                f,
                "não foi possível criar o subagente: profundidade máxima de {max} excedida \
                 (atual: {current}). Subagentes aninhados além de {max} não são suportados. \
                 Reformule: a tarefa do neto cabe no filho atual, ou use o pai direto."
            ),
            Self::AllocationExceedsParent { cause } => write!(
                f,
                "não foi possível criar o subagente: alocação excede budget disponível do pai. \
                 Detalhe: {cause}"
            ),
            Self::PermissionDenied { reason } => write!(
                f,
                "não foi possível criar o subagente: ferramentas exigidas pelo especialista \
                 não estão disponíveis no pai. Detalhe: {reason}"
            ),
            Self::InternalError(msg) => write!(f, "erro interno do SubagentRunner: {msg}"),
        }
    }
}

/// `std::error::Error` manual (tiramos o `thiserror::Error`
/// derive). `source()` propaga o `AllocationError` quando o erro
/// raiz é um `AllocationExceedsParent`, e `None` nos outros casos
/// (o `UnknownSpecialist` é o último elo da cadeia — o
/// `RegistryError` original vem **de baixo** quando a Etapa 4 PR 2
/// construir via `?`).
impl std::error::Error for SubagentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AllocationExceedsParent { cause } => Some(cause),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_limit_reached_display_lists_active_count() {
        let err = SubagentError::GlobalLimitReached {
            current: 8,
            max: 8,
            next: 9,
        };
        let s = err.to_string();
        assert!(s.contains("8"));
        assert!(s.contains("9"));
        assert!(s.contains("limite global"));
    }

    #[test]
    fn depth_exceeded_display_suggests_reformulation() {
        let err = SubagentError::DepthExceeded { current: 2, max: 2 };
        let s = err.to_string();
        assert!(s.contains("2"));
        assert!(s.contains("Reformula") || s.contains("use o pai"));
    }

    #[test]
    fn allocation_exceeds_parent_wraps_cause() {
        let cause = AllocationError::ExceedsParent {
            axis: "max_cost_microcents".to_string(),
            requested: 5_000,
            available: 1_000,
        };
        let err = SubagentError::AllocationExceedsParent { cause };
        let s = err.to_string();
        assert!(s.contains("max_cost_microcents"));
        assert!(s.contains("5000"));
        assert!(s.contains("1000"));
    }

    #[test]
    fn unknown_specialist_lists_valid_ids() {
        let err = SubagentError::UnknownSpecialist(UnknownSpecialistDetail {
            requested: "revisor-final".into(),
            valid: vec!["revisor".into(), "pesquisador".into(), "testador".into()],
        });
        let s = err.to_string();
        assert!(s.contains("revisor-final"));
        assert!(s.contains("revisor"));
        assert!(s.contains("pesquisador"));
        assert!(s.contains("testador"));
    }

    #[test]
    fn permission_denied_display_includes_reason() {
        let err = SubagentError::PermissionDenied {
            reason: "tools: [docs.generate, web.browse] não estão no pai".into(),
        };
        let s = err.to_string();
        assert!(s.contains("docs.generate"));
        assert!(s.contains("web.browse"));
    }

    #[test]
    fn internal_error_is_not_model_recoverable() {
        let err = SubagentError::InternalError("DB write falhou".into());
        assert!(!err.is_model_recoverable());
    }

    #[test]
    fn portao_errors_are_model_recoverable() {
        let cases = [
            SubagentError::GlobalLimitReached {
                current: 8,
                max: 8,
                next: 9,
            },
            SubagentError::DepthExceeded { current: 2, max: 2 },
            SubagentError::AllocationExceedsParent {
                cause: AllocationError::SplitZero,
            },
            SubagentError::UnknownSpecialist(UnknownSpecialistDetail {
                requested: "x".into(),
                valid: vec!["y".into()],
            }),
            SubagentError::PermissionDenied { reason: "x".into() },
        ];
        for err in cases {
            assert!(
                err.is_model_recoverable(),
                "expected {:?} to be model-recoverable",
                err
            );
        }
    }

    #[test]
    fn allocation_error_serialization_roundtrips_in_cause() {
        // Garante que `AllocationError` serializa dentro do
        // `SubagentError` (o `cause` precisa sobreviver a um
        // JSON dump se a UI quiser mostrar o detalhe).
        let cause = AllocationError::SplitTooLarge {
            requested: 10,
            max_steps: 5,
        };
        let err = SubagentError::AllocationExceedsParent {
            cause: cause.clone(),
        };
        let json = serde_json::to_string(&err).expect("serializa");
        let back: SubagentError = serde_json::from_str(&json).expect("deserializa");
        assert_eq!(err, back);
    }
}
