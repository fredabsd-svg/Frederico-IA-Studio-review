//! As três ferramentas de marco de projeto ([ADR-0048] §D2).
//!
//! `milestone.list` é leitura e não pede aprovação; `milestone.create`
//! e `milestone.restore` mudam estado e pedem por invocação — a mesma
//! assimetria do [ADR-0034] que separa `files.read` de `files.write`.
//!
//! ## O agente não escolhe o projeto
//!
//! Nenhuma das três aceita projeto no schema. Elas operam sobre o
//! projeto **do workspace da conversa**, encontrado pelo caminho do
//! Jail. Workspace que não é projeto registrado faz a ferramenta
//! recusar com essa mensagem.
//!
//! É o [ADR-0048] §D1 em código, e a mesma proteção estrutural das
//! ferramentas de Git da Etapa 3: a fronteira é garantida por
//! ausência de parâmetro, não por validação. `abrir_projeto` **não**
//! virou ferramenta porque registrar projeto amplia o que o
//! **usuário** alcança pela UI ([ADR-0042] §D4) — uma ferramenta
//! inverteria a direção.
//!
//! [ADR-0034]: ../../docs/decisions/0034-fase-7-write-exec-approval-policy.md
//! [ADR-0042]: ../../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
//! [ADR-0048]: ../../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md

use std::sync::Arc;

use frederico_core::{ProjectId, ToolId};
use frederico_git_engine::Autor;
use frederico_project_engine::{ProjectEngine, ProjectError};

use crate::tools::{ToolContext, ToolResult};

mod create;
mod list;
mod restore;

pub use create::MilestoneCreateTool;
pub use list::MilestoneListTool;
pub use restore::MilestoneRestoreTool;

/// Identidade que assina os marcos criados pelo app.
///
/// Fixa e fora do alcance do modelo, pelo mesmo motivo do
/// `git.commit` da Etapa 3: o marco vira commit e tag no histórico,
/// que é o registro que se consulta para saber quem fez o quê.
pub(crate) const AUTOR_NOME: &str = "Frederico IA Studio";
pub(crate) const AUTOR_EMAIL: &str = "frederico-ia-studio@localhost";

pub(crate) fn autor_do_app() -> Autor {
    Autor {
        nome: AUTOR_NOME.to_string(),
        email: AUTOR_EMAIL.to_string(),
    }
}

/// O que as três ferramentas precisam para existir.
///
/// Sem o pool, elas **não entram no catálogo** — bump atômico
/// ([ADR-0020] §3 D3). O agente não vê ferramenta que não pode
/// funcionar.
///
/// [ADR-0020]: ../../docs/decisions/0020-fase-5-etapa-4-excelpro-inspect.md
#[derive(Clone)]
pub struct MarcoDeps {
    pub pool: Arc<sqlx::SqlitePool>,
}

/// Encontra o projeto do workspace da conversa.
///
/// A recusa é declarada: sem projeto registrado para este caminho, a
/// ferramenta diz isso em vez de criar um projeto por conta própria —
/// registrar é ação do usuário ([ADR-0048] §D1).
///
/// [ADR-0048]: ../../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md
pub(crate) async fn projeto_da_conversa(
    tool_id: &ToolId,
    deps: &MarcoDeps,
    ctx: &ToolContext,
) -> Result<ProjectId, ToolResult> {
    let engine = ProjectEngine::new(&deps.pool);
    let raiz = ctx.jail.root();
    let projetos = match engine.listar_projetos().await {
        Ok(p) => p,
        Err(e) => return Err(ToolResult::err(tool_id.clone(), e.to_string())),
    };
    projetos
        .into_iter()
        .find(|p| p.caminho == raiz)
        .map(|p| p.id)
        .ok_or_else(|| {
            ToolResult::err(
                tool_id.clone(),
                "o workspace desta conversa não é um projeto registrado. \
                 Abra-o como projeto pela interface antes de usar marcos.",
            )
        })
}

/// Traduz erro do `project-engine` para `ToolResult`, preservando a
/// causa — inclusive a de workspace sem Git, que é a recusa mais
/// provável e a mais acionável.
pub(crate) fn erro_para_resultado(tool_id: ToolId, erro: &ProjectError) -> ToolResult {
    ToolResult::err(tool_id, erro.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::manifest::RiskLevel;
    use crate::tools::Tool;

    /// A assimetria do ADR-0034 nas três de marco, e o risco de cada
    /// uma conforme o ADR-0048 §D2.
    // `tokio::test`: o `SqlitePoolOptions::connect_lazy` exige
    // contexto Tokio já na construção, mesmo sem tocar o banco.
    #[tokio::test]
    async fn aprovacao_e_risco_das_ferramentas_de_marco() {
        let deps = MarcoDeps {
            pool: Arc::new(
                sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite::memory:")
                    .expect("pool"),
            ),
        };
        let lista = MilestoneListTool::new(deps.clone());
        let criar = MilestoneCreateTool::new(deps.clone());
        let restaurar = MilestoneRestoreTool::new(deps);

        assert!(
            !lista.manifest().requires_user_approval,
            "listar é leitura e não pode pedir aprovação"
        );
        assert_eq!(lista.manifest().risk_level, RiskLevel::Safe);

        assert!(criar.manifest().requires_user_approval);
        assert_eq!(criar.manifest().risk_level, RiskLevel::Moderate);

        assert!(restaurar.manifest().requires_user_approval);
        // `High`, não `Critical`: o ADR-0042 §D3 garante que
        // restaurar não descarta trabalho, então o dano máximo é um
        // commit indesejado, desfazível pelo Git do usuário.
        // `Critical` fica para o que não tem desfazer.
        assert_eq!(restaurar.manifest().risk_level, RiskLevel::High);
    }

    /// **Negação estrutural — o ADR-0048 §D1.**
    ///
    /// Nenhuma das três pode aceitar projeto, caminho ou repositório
    /// no schema. Elas operam sobre o projeto do workspace da
    /// conversa, e a fronteira é garantida por ausência de parâmetro.
    #[tokio::test]
    async fn nenhuma_ferramenta_de_marco_aceita_projeto_ou_caminho() {
        let deps = MarcoDeps {
            pool: Arc::new(
                sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite::memory:")
                    .expect("pool"),
            ),
        };
        let manifestos = [
            MilestoneListTool::new(deps.clone()).manifest,
            MilestoneCreateTool::new(deps.clone()).manifest,
            MilestoneRestoreTool::new(deps).manifest,
        ];
        for m in &manifestos {
            let schema = &m.input_schema.0;
            assert_eq!(
                schema["additionalProperties"],
                json!(false),
                "{}: schema aberto deixaria passar parâmetro não declarado",
                m.id.as_str()
            );
            let props = schema["properties"].as_object().expect("properties");
            for proibido in [
                "projeto",
                "project",
                "project_id",
                "caminho",
                "path",
                "workspace",
                "repositorio",
            ] {
                assert!(
                    !props.contains_key(proibido),
                    "{} declara `{proibido}`: o projeto é sempre o do workspace da conversa",
                    m.id.as_str()
                );
            }
        }
    }

    /// **Negação:** apagar marco não existe em lugar nenhum do
    /// schema — mesma regra do `git.branch` da Etapa 3.
    #[tokio::test]
    async fn apagar_marco_nao_existe() {
        let deps = MarcoDeps {
            pool: Arc::new(
                sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite::memory:")
                    .expect("pool"),
            ),
        };
        for m in [
            MilestoneListTool::new(deps.clone()).manifest,
            MilestoneCreateTool::new(deps.clone()).manifest,
            MilestoneRestoreTool::new(deps).manifest,
        ] {
            let texto = serde_json::to_string(&m.input_schema.0).expect("json");
            for proibido in ["apagar", "delete", "remover", "excluir"] {
                assert!(
                    !texto.contains(proibido),
                    "{} menciona `{proibido}` no schema",
                    m.id.as_str()
                );
            }
        }
    }
}
