//! As cinco ferramentas de Git local (Etapa 3 da Fase 8).
//!
//! `git.status`, `git.diff` e `git.log` são leitura e não pedem
//! aprovação; `git.branch` e `git.commit` mudam estado e pedem por
//! invocação. A assimetria é a do [ADR-0034], a mesma que separa
//! `files.read` de `files.write`.
//!
//! ## O repositório é o workspace, e isso não é configurável
//!
//! Nenhuma das cinco aceita caminho de repositório no schema. Elas
//! abrem `ctx.jail.root()` e só. É o [ADR-0040] §D3 em código: se
//! não existe parâmetro para apontar outro lugar, não existe
//! argumento que o modelo possa construir para sair do workspace da
//! conversa — nem por `..`, nem por caminho absoluto, nem por UNC.
//!
//! O `frederico_git_engine::GitRepo::abrir` fecha a segunda metade
//! da mesma porta: ele não sobe diretórios procurando `.git`, então
//! um workspace que por acaso esteja dentro de um repositório maior
//! não passa a operar sobre o repositório de fora.
//!
//! [ADR-0034]: ../../docs/decisions/0034-fase-7-write-exec-approval-policy.md
//! [ADR-0040]: ../../docs/decisions/0040-git-engine-biblioteca-e-fronteira.md

use frederico_core::ToolId;
use frederico_git_engine::GitRepo;

use crate::tools::{ToolContext, ToolResult};

mod branch;
mod commit;
mod diff;
mod log;
mod status;

pub use branch::GitBranchTool;
pub use commit::GitCommitTool;
pub use diff::GitDiffTool;
pub use log::GitLogTool;
pub use status::GitStatusTool;

/// Abre o repositório do workspace da conversa.
///
/// Erro vira `ToolResult::err` com a mensagem do `git-engine`, que
/// já é PT-BR e estruturada — não é stderr do `git` na locale da
/// máquina, que é o ponto 2 do [ADR-0040] §D1.
///
/// [ADR-0040]: ../../docs/decisions/0040-git-engine-biblioteca-e-fronteira.md
fn abrir_repo(tool_id: &ToolId, ctx: &ToolContext) -> Result<GitRepo, ToolResult> {
    GitRepo::abrir(ctx.jail.root()).map_err(|e| ToolResult::err(tool_id.clone(), e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use frederico_core::{ConversationId, MessageId, RunId};
    use serde_json::json;
    use uuid::Uuid;

    use crate::manifest::RiskLevel;
    use crate::tools::Tool;
    use crate::workspace::Jail;

    fn contexto(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(
            ConversationId(Uuid::nil()),
            RunId(Uuid::nil()),
            MessageId(Uuid::nil()),
            Jail::new(dir).expect("jail"),
        )
    }

    /// O ciclo que o usuário enxerga: a IA cria um arquivo, pergunta
    /// o que mudou, commita e confere o histórico — tudo pelo
    /// contrato da ferramenta, não pela API do crate.
    #[tokio::test]
    async fn ciclo_status_commit_log_pelo_contrato_da_ferramenta() {
        let dir = tempfile::tempdir().expect("tempdir");
        frederico_git_engine::GitRepo::iniciar(dir.path()).expect("iniciar");
        fs::write(dir.path().join("a.txt"), "linha um\n").expect("escrever");
        let ctx = contexto(dir.path());

        let status = GitStatusTool::new().execute(&ctx, &json!({})).await;
        assert!(status.ok, "status falhou: {:?}", status.error_message);
        assert_eq!(status.output["limpo"], json!(false));
        assert_eq!(status.output["mudancas"][0]["caminho"], json!("a.txt"));
        assert_eq!(
            status.output["mudancas"][0]["estado"],
            json!("nao_rastreado")
        );

        let commit = GitCommitTool::new()
            .execute(&ctx, &json!({"mensagem": "adiciona a.txt"}))
            .await;
        assert!(commit.ok, "commit falhou: {:?}", commit.error_message);
        assert_eq!(
            commit.output["pais"],
            json!(0),
            "primeiro commit não tem pai"
        );
        assert_eq!(commit.output["arquivos"], json!(1));
        assert_eq!(
            commit.output["autor"],
            json!("Frederico IA Studio"),
            "o commit é do app; atribuí-lo ao usuário seria falsificar autoria"
        );

        let log = GitLogTool::new().execute(&ctx, &json!({})).await;
        assert!(log.ok);
        assert_eq!(log.output["total"], json!(1));
        assert_eq!(log.output["commits"][0]["resumo"], json!("adiciona a.txt"));

        // Depois do commit o status volta limpo — é o que prova que
        // o índice foi escrito, e não só o objeto (ADR-0047 §D3).
        let status = GitStatusTool::new().execute(&ctx, &json!({})).await;
        assert_eq!(status.output["limpo"], json!(true));
    }

    /// **Negação:** árvore limpa não vira commit vazio.
    #[tokio::test]
    async fn commit_de_arvore_limpa_e_recusado() {
        let dir = tempfile::tempdir().expect("tempdir");
        frederico_git_engine::GitRepo::iniciar(dir.path()).expect("iniciar");
        fs::write(dir.path().join("a.txt"), "conteudo\n").expect("escrever");
        let ctx = contexto(dir.path());
        let tool = GitCommitTool::new();

        assert!(
            tool.execute(&ctx, &json!({"mensagem": "primeiro"}))
                .await
                .ok
        );

        let segundo = tool.execute(&ctx, &json!({"mensagem": "de novo"})).await;
        assert!(!segundo.ok, "commit vazio deveria ser recusado");
        assert!(
            segundo.error_message.unwrap_or_default().contains("limpa"),
            "a recusa precisa dizer o motivo"
        );
    }

    /// **Negação:** mensagem vazia é recusada antes de tocar o
    /// repositório.
    #[tokio::test]
    async fn commit_sem_mensagem_e_recusado() {
        let dir = tempfile::tempdir().expect("tempdir");
        frederico_git_engine::GitRepo::iniciar(dir.path()).expect("iniciar");
        let ctx = contexto(dir.path());

        for args in [
            json!({}),
            json!({"mensagem": ""}),
            json!({"mensagem": "   "}),
        ] {
            let r = GitCommitTool::new().execute(&ctx, &args).await;
            assert!(!r.ok, "deveria recusar {args}");
        }
    }

    /// **Negação:** workspace que não é repositório devolve erro
    /// explicando, não pânico nem sucesso silencioso.
    #[tokio::test]
    async fn workspace_sem_repositorio_devolve_erro_util() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = contexto(dir.path());

        let r = GitStatusTool::new().execute(&ctx, &json!({})).await;
        assert!(!r.ok);
        assert!(
            r.error_message
                .unwrap_or_default()
                .contains("não é um repositório Git"),
            "mensagem precisa nomear a causa"
        );
    }

    /// **Negação estrutural — a que sustenta o ADR-0040 §D3.**
    ///
    /// Nenhuma das cinco pode aceitar caminho no schema. Se alguém
    /// acrescentar `repo`, `path`, `cwd` ou `workspace` para "deixar
    /// mais flexível", a fronteira do Jail deixa de ser garantida
    /// pela ausência de parâmetro e passa a depender de validação —
    /// que é uma garantia mais fraca. Este teste quebra antes.
    #[test]
    fn nenhuma_ferramenta_de_git_aceita_caminho_de_repositorio() {
        let manifestos = [
            GitStatusTool::new().manifest,
            GitDiffTool::new().manifest,
            GitLogTool::new().manifest,
            GitBranchTool::new().manifest,
            GitCommitTool::new().manifest,
        ];
        for m in &manifestos {
            let schema = &m.input_schema.0;
            assert_eq!(
                schema["additionalProperties"],
                json!(false),
                "{}: schema aberto deixaria passar caminho não declarado",
                m.id.as_str()
            );
            let props = schema["properties"].as_object().expect("properties");
            for proibido in ["repo", "repositorio", "path", "caminho", "cwd", "workspace"] {
                assert!(
                    !props.contains_key(proibido),
                    "{} declara `{proibido}`: o repositório é sempre o workspace da conversa",
                    m.id.as_str()
                );
            }
        }
    }

    /// A assimetria do ADR-0034, fixada: ler não pede aprovação,
    /// escrever pede. Sem isso, um refactor que zere o flag por
    /// engano passa despercebido.
    #[test]
    fn aprovacao_e_risco_seguem_o_adr_0034() {
        let status = GitStatusTool::new().manifest;
        let diff = GitDiffTool::new().manifest;
        let log = GitLogTool::new().manifest;
        for m in [&status, &diff, &log] {
            assert!(
                !m.requires_user_approval,
                "{} é leitura e não pode pedir aprovação",
                m.id.as_str()
            );
            assert_eq!(m.risk_level, RiskLevel::Safe);
        }

        let branch = GitBranchTool::new().manifest;
        let commit = GitCommitTool::new().manifest;
        for m in [&branch, &commit] {
            assert!(
                m.requires_user_approval,
                "{} muda estado e exige aprovação por invocação",
                m.id.as_str()
            );
        }
        assert_eq!(branch.risk_level, RiskLevel::Moderate);
        assert_eq!(commit.risk_level, RiskLevel::High);
    }

    /// **Negação:** `git.branch` não conhece ação de apagar. O valor
    /// não está no enum do schema, e o `execute` recusa qualquer
    /// ação fora das três.
    #[tokio::test]
    async fn git_branch_nao_apaga() {
        let dir = tempfile::tempdir().expect("tempdir");
        frederico_git_engine::GitRepo::iniciar(dir.path()).expect("iniciar");
        fs::write(dir.path().join("a.txt"), "x\n").expect("escrever");
        let ctx = contexto(dir.path());
        GitCommitTool::new()
            .execute(&ctx, &json!({"mensagem": "base"}))
            .await;

        let m = GitBranchTool::new().manifest;
        let acoes = m.input_schema.0["properties"]["acao"]["enum"]
            .as_array()
            .expect("enum de acao")
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(acoes, vec!["listar", "criar", "trocar"]);

        let r = GitBranchTool::new()
            .execute(&ctx, &json!({"acao": "apagar", "nome": "main"}))
            .await;
        assert!(!r.ok, "apagar não pode ser aceito nem por caminho lateral");
    }

    /// `git.diff` responde perguntas diferentes conforme `staged`, e
    /// o output diz qual delas respondeu.
    #[tokio::test]
    async fn git_diff_declara_qual_pergunta_respondeu() {
        let dir = tempfile::tempdir().expect("tempdir");
        frederico_git_engine::GitRepo::iniciar(dir.path()).expect("iniciar");
        fs::write(dir.path().join("a.txt"), "linha um\n").expect("escrever");
        let ctx = contexto(dir.path());
        GitCommitTool::new()
            .execute(&ctx, &json!({"mensagem": "base"}))
            .await;
        fs::write(dir.path().join("a.txt"), "linha um\nlinha dois\n").expect("modificar");

        let worktree = GitDiffTool::new().execute(&ctx, &json!({})).await;
        assert!(worktree.ok);
        assert_eq!(worktree.output["staged"], json!(false));
        assert_eq!(worktree.output["vazio"], json!(false));
        assert!(worktree.output["patch"]
            .as_str()
            .unwrap_or_default()
            .contains("+linha dois"));

        let staged = GitDiffTool::new()
            .execute(&ctx, &json!({"staged": true}))
            .await;
        assert_eq!(staged.output["staged"], json!(true));
        assert_eq!(
            staged.output["vazio"],
            json!(true),
            "nada foi para o índice, então nada entraria no commit"
        );
    }
}
