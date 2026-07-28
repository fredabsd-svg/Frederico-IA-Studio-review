//! Erros estruturados da máquina de estados.
//!
//! Cada variante traz os dados necessários para diagnóstico: estado de
//! origem, estado de destino, evento. A invariante do spec
//! `agent-state-machine.md` §"Invariantes" é que **transições inválidas
//! são erro estruturado, nunca ignoradas** — esse módulo é onde o erro
//! mora.

use std::error::Error;
use std::fmt;

use crate::event::RunEventKind;
use crate::state::RunState;

/// Erro retornado por [`crate::transition::apply_transition`] quando a
/// transição pedida não está na tabela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// Tentou-se aplicar qualquer evento a partir de um estado
    /// terminal (`completed`, `failed`, `cancelled`, `interrupted`).
    /// Nenhuma transição sai de um estado terminal — o
    /// `apply_transition` rejeita antes mesmo de consultar a tabela.
    FromTerminal {
        /// Estado terminal do qual o caller tentou sair.
        from: RunState,
    },

    /// O par (`from`, `event`) não casa com nenhuma aresta da tabela
    /// estrutural nem com nenhuma aresta global. O `to` que o caller
    /// tinha em mente (ou `None` se não foi checado) é incluído para
    /// diagnóstico.
    InvalidTransition {
        /// Estado de origem da transição tentada.
        from: RunState,
        /// Evento que o caller tentou aplicar.
        event: RunEventKind,
        /// Estado de destino que o caller tinha em mente, se
        /// conhecido. `None` se o caller só tinha o par (`from`,
        /// `event`).
        to: Option<RunState>,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FromTerminal { from } => {
                write!(
                    f,
                    "estado terminal {from} é imutável; nenhuma transição sai dele"
                )
            }
            Self::InvalidTransition { from, event, to } => match to {
                Some(t) => write!(
                    f,
                    "transição inválida de {from} para {t} via evento {event}"
                ),
                None => write!(f, "transição inválida de {from} via evento {event}"),
            },
        }
    }
}

impl Error for TransitionError {}

/// Erro de parsing de `RunState` (string → enum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStateParseError(pub String);

impl fmt::Display for RunStateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RunState desconhecido: {:?}", self.0)
    }
}

impl Error for RunStateParseError {}

/// Erro de parsing de `RunEventKind` (string → enum).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEventKindParseError(pub String);

impl fmt::Display for RunEventKindParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RunEventKind desconhecido: {:?}", self.0)
    }
}

impl Error for RunEventKindParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_terminal_display_is_descriptive() {
        let err = TransitionError::FromTerminal {
            from: RunState::Completed,
        };
        let s = err.to_string();
        assert!(s.contains("completed"));
        assert!(s.contains("terminal"));
    }

    #[test]
    fn invalid_transition_display_with_to() {
        let err = TransitionError::InvalidTransition {
            from: RunState::Created,
            event: RunEventKind::FirstToken,
            to: Some(RunState::Completed),
        };
        let s = err.to_string();
        assert!(s.contains("created"));
        assert!(s.contains("completed"));
        assert!(s.contains("first_token"));
    }

    #[test]
    fn invalid_transition_display_without_to() {
        let err = TransitionError::InvalidTransition {
            from: RunState::Created,
            event: RunEventKind::FirstToken,
            to: None,
        };
        let s = err.to_string();
        assert!(s.contains("created"));
        assert!(s.contains("first_token"));
    }
}
