//! As duas ferramentas de GitHub ([ADR-0048] §D3).
//!
//! **Ambas `Critical`**, e o nível não é dramatização. `Critical` é o
//! único que força `ApprovalRequest.mandatory = true` mesmo sem UI de
//! escopo (`validate.rs::with_mandatory_for_risk`) — foi por isso que
//! o [ADR-0044] o escolheu para `exec.shell`. Aqui a razão é mais
//! forte: commit local se desfaz, push para o repositório de outras
//! pessoas não.
//!
//! ## Sem token ou sem matriz, elas não existem
//!
//! O catálogo, a allowlist de run e a permissão se movem juntos —
//! bump atômico ([ADR-0020] §3 D3), igual ao `exec.*`. O agente não
//! vê ferramenta que não pode funcionar, e a fila de aprovação não
//! recebe pedido de algo que falharia de qualquer jeito.
//!
//! ## A autorização não mora aqui
//!
//! A matriz é estado do `GithubEngine` e roda dentro dele, antes de
//! qualquer rede ([ADR-0041] §D2). Estas ferramentas não a consultam
//! nem a recebem por argumento: elas chamam o motor, e o motor
//! recusa. Duplicar a checagem aqui criaria uma segunda régua que
//! pode divergir da primeira.
//!
//! [ADR-0020]: ../../docs/decisions/0020-fase-5-etapa-4-excelpro-inspect.md
//! [ADR-0041]: ../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
//! [ADR-0044]: ../../docs/decisions/0044-exec-shell-com-resolucao-propria-de-programa.md
//! [ADR-0048]: ../../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md

use std::sync::Arc;

use frederico_github_engine::GithubEngine;

mod create_pr;
mod push;

pub use create_pr::GithubCreatePrTool;
pub use push::GithubPushTool;

/// O que as duas ferramentas precisam para existir.
///
/// O `GithubEngine` já vem construído com token e matriz — quem o
/// monta (a casca, a partir do `ServiceCredentialStore` e do perfil)
/// decide o alcance, e nenhuma chamada pode ampliá-lo.
#[derive(Clone)]
pub struct GithubDeps {
    pub engine: Arc<GithubEngine>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use frederico_github_engine::MatrizAutorizacao;
    use secrecy::SecretString;
    use serde_json::json;

    use crate::manifest::RiskLevel;
    use crate::tools::Tool;

    fn deps() -> GithubDeps {
        GithubDeps {
            engine: Arc::new(GithubEngine::new(
                SecretString::from("token".to_string()),
                MatrizAutorizacao::vazia(),
            )),
        }
    }

    /// **As duas são `Critical`, e o nível tem consequência
    /// mecânica** (ADR-0048 §D3): é o único que força
    /// `ApprovalRequest.mandatory = true` sem UI de escopo. Rebaixar
    /// para `High` afrouxaria a fila de aprovação sem que nada mais
    /// mudasse de aparência.
    #[test]
    fn ferramentas_de_github_sao_criticas_e_exigem_aprovacao() {
        let push = GithubPushTool::new(deps());
        let pr = GithubCreatePrTool::new(deps());

        for m in [push.manifest(), pr.manifest()] {
            assert_eq!(
                m.risk_level,
                RiskLevel::Critical,
                "{} tem que ser Critical",
                m.id.as_str()
            );
            assert!(m.requires_user_approval, "{}", m.id.as_str());
            assert!(m.requires_network, "{}", m.id.as_str());
        }
    }

    /// **Negação estrutural:** nenhuma das duas aceita caminho de
    /// workspace. O `github.push` opera sempre no workspace da
    /// conversa; `repositorio` nomeia o alvo **da matriz**, não um
    /// diretório.
    #[test]
    fn ferramentas_de_github_nao_aceitam_caminho() {
        for m in [
            GithubPushTool::new(deps()).manifest,
            GithubCreatePrTool::new(deps()).manifest,
        ] {
            let schema = &m.input_schema.0;
            assert_eq!(schema["additionalProperties"], json!(false));
            let props = schema["properties"].as_object().expect("properties");
            for proibido in ["caminho", "path", "workspace", "diretorio", "cwd"] {
                assert!(
                    !props.contains_key(proibido),
                    "{} declara `{proibido}`",
                    m.id.as_str()
                );
            }
        }
    }

    /// **Negação:** force não aparece no schema em forma nenhuma.
    #[test]
    fn schema_do_push_nao_tem_force() {
        let m = GithubPushTool::new(deps()).manifest;
        let texto = serde_json::to_string(&m.input_schema.0).expect("json");
        for proibido in ["force", "forcado", "forçado", "sobrescrever"] {
            assert!(!texto.contains(proibido), "schema menciona `{proibido}`");
        }
    }
}
