//! Projetos e marcos nomeados.
//!
//! **Projeto** é um diretório de workspace que o usuário nomeou
//! ([ADR-0042] §D4): quatro campos numa tabela, sem formato
//! proprietário, sem importação e sem migração.
//!
//! **Marco** é uma tag anotada no repositório Git do workspace
//! ([ADR-0042] §D2), mais uma linha de metadados. O dado de verdade
//! é a tag — o banco guarda o que o Git não tem onde guardar (qual
//! conversa originou, se foi automático). "Marco é um commit com
//! nome" é conferível pelo usuário com `git log`, sem confiar no app.
//!
//! ## O que este crate **não** faz
//!
//! - **Não copia árvore de arquivos.** Rejeitado pelo [ADR-0042]
//!   §D2: duplicaria dados do usuário, não escalaria com o tamanho
//!   do workspace e reimplementaria mal o que o Git faz bem.
//! - **Não constrói o `CheckpointRepo`.** A tabela `checkpoints` da
//!   migração `0003` continua sem dono em código ([ADR-0042] §D5).
//!   Nada a consome, e construir por simetria é criar estrutura sem
//!   dono — o defeito que aquele ADR nomeia.
//! - **Não descarta trabalho.** Ver [`ProjectEngine::restaurar_marco`].
//!
//! [ADR-0042]: ../docs/decisions/0042-projetos-e-checkpoints-nomeados.md

use std::path::{Path, PathBuf};

use frederico_core::ProjectId;
use frederico_git_engine::{Autor, GitError, GitRepo};

/// Linha crua de `projects`. Existe para o `query_as` — o tipo de
/// domínio é [`Projeto`].
type LinhaProjeto = (String, String, String, Option<String>, String, String);

/// Linha crua de `project_milestones`. Tipo de domínio: [`Marco`].
type LinhaMarco = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    String,
);

/// Erros do `project-engine`.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("já existe um projeto neste caminho: {0}")]
    CaminhoJaRegistrado(PathBuf),
    #[error("projeto não encontrado")]
    ProjetoNaoEncontrado,
    #[error("o nome do projeto não pode ser vazio")]
    NomeVazio,
    #[error("o caminho não existe ou não é um diretório: {0}")]
    CaminhoInvalido(PathBuf),
    #[error(
        "o workspace deste projeto não é um repositório Git — marcos exigem Git ({caminho}). \
         Motivo: {origem}"
    )]
    WorkspaceSemGit { caminho: PathBuf, origem: GitError },
    #[error("já existe um marco chamado {0} neste projeto")]
    MarcoJaExiste(String),
    #[error("marco não encontrado: {0}")]
    MarcoNaoEncontrado(String),
    #[error("falha do Git: {0}")]
    Git(#[from] GitError),
    #[error("falha no banco: {0}")]
    Banco(#[from] sqlx::Error),
}

/// Um projeto — o workspace com nome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projeto {
    pub id: ProjectId,
    pub caminho: PathBuf,
    pub nome: String,
    pub perfil_permissao: Option<String>,
    pub criado_em: String,
    pub ultimo_acesso: String,
}

/// Um marco — a tag, mais o que o Git não guarda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marco {
    pub id: String,
    pub projeto_id: ProjectId,
    pub nome: String,
    pub descricao: String,
    pub commit_id: String,
    pub conversa_origem: Option<String>,
    /// `true` quando o app criou o marco sozinho, antes de uma
    /// restauração ([ADR-0042] §D3).
    ///
    /// [ADR-0042]: ../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
    pub automatico: bool,
    pub criado_em: String,
}

/// Resultado de uma restauração.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restauracao {
    /// Commit criado pela restauração.
    pub commit_id: String,
    /// Marco automático criado antes, quando havia trabalho
    /// pendente. `None` quando a árvore já estava limpa.
    pub marco_automatico: Option<Marco>,
}

/// Fachada de projetos e marcos sobre o banco e o `git-engine`.
pub struct ProjectEngine<'a> {
    pool: &'a sqlx::SqlitePool,
}

impl<'a> ProjectEngine<'a> {
    #[must_use]
    pub fn new(pool: &'a sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    // ---------------------------------------------------------------
    // Projetos
    // ---------------------------------------------------------------

    /// Registra um diretório de workspace como projeto.
    ///
    /// O caminho é gravado como veio. **Não resolve nem canonicaliza
    /// caminho aqui**: quem decide o que o agente alcança é o
    /// `JailResolver` ([ADR-0022]/[ADR-0036]), e duplicar essa
    /// decisão neste crate criaria uma segunda régua que pode
    /// divergir da primeira. Abrir projeto amplia o que o **usuário**
    /// alcança pela UI, não o que o agente alcança ([ADR-0042] §D4).
    ///
    /// [ADR-0022]: ../docs/decisions/0022-jail-resolver-v1.md
    /// [ADR-0036]: ../docs/decisions/0036-security-jail-resolver-windows-job-objects.md
    /// [ADR-0042]: ../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
    pub async fn abrir_projeto(
        &self,
        caminho: &Path,
        nome: &str,
        perfil_permissao: Option<&str>,
    ) -> Result<Projeto, ProjectError> {
        if nome.trim().is_empty() {
            return Err(ProjectError::NomeVazio);
        }
        // Guarda de usabilidade, não de segurança: um caminho digitado
        // errado viraria linha permanente no banco apontando para
        // lugar nenhum. Quem limita o alcance do **agente** é o
        // `JailResolver`, não isto.
        if !caminho.is_dir() {
            return Err(ProjectError::CaminhoInvalido(caminho.to_path_buf()));
        }
        let caminho_str = caminho.to_string_lossy().to_string();

        if let Some(existente) = self.projeto_por_caminho(caminho).await? {
            // Reabrir projeto conhecido não é erro — é o caso comum.
            // Só o registro em duplicata seria.
            self.tocar_ultimo_acesso(existente.id).await?;
            return self
                .projeto_por_caminho(caminho)
                .await?
                .ok_or(ProjectError::ProjetoNaoEncontrado);
        }

        let id = ProjectId::new();
        sqlx::query(
            "INSERT INTO projects (id, caminho, nome, perfil_permissao) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(id.as_uuid().to_string())
        .bind(&caminho_str)
        .bind(nome)
        .bind(perfil_permissao)
        .execute(self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                ProjectError::CaminhoJaRegistrado(caminho.to_path_buf())
            }
            _ => ProjectError::Banco(e),
        })?;

        self.projeto_por_caminho(caminho)
            .await?
            .ok_or(ProjectError::ProjetoNaoEncontrado)
    }

    /// Projetos registrados, do último acesso para o mais antigo.
    pub async fn listar_projetos(&self) -> Result<Vec<Projeto>, ProjectError> {
        let linhas: Vec<LinhaProjeto> = sqlx::query_as(
            "SELECT id, caminho, nome, perfil_permissao, criado_em, ultimo_acesso \
             FROM projects ORDER BY ultimo_acesso DESC",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(linhas.into_iter().map(linha_para_projeto).collect())
    }

    async fn projeto_por_caminho(&self, caminho: &Path) -> Result<Option<Projeto>, ProjectError> {
        let linha: Option<LinhaProjeto> = sqlx::query_as(
            "SELECT id, caminho, nome, perfil_permissao, criado_em, ultimo_acesso \
                 FROM projects WHERE caminho = ?1",
        )
        .bind(caminho.to_string_lossy().to_string())
        .fetch_optional(self.pool)
        .await?;
        Ok(linha.map(linha_para_projeto))
    }

    /// Projeto pelo id.
    pub async fn projeto(&self, id: ProjectId) -> Result<Projeto, ProjectError> {
        let linha: Option<LinhaProjeto> = sqlx::query_as(
            "SELECT id, caminho, nome, perfil_permissao, criado_em, ultimo_acesso \
                 FROM projects WHERE id = ?1",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(self.pool)
        .await?;
        linha
            .map(linha_para_projeto)
            .ok_or(ProjectError::ProjetoNaoEncontrado)
    }

    async fn tocar_ultimo_acesso(&self, id: ProjectId) -> Result<(), ProjectError> {
        sqlx::query("UPDATE projects SET ultimo_acesso = datetime('now') WHERE id = ?1")
            .bind(id.as_uuid().to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }

    // ---------------------------------------------------------------
    // Marcos
    // ---------------------------------------------------------------

    /// Abre o repositório do projeto, ou explica por que não dá.
    ///
    /// A recusa é declarada e não contornada: marco exige workspace
    /// sob Git ([ADR-0042] §D2). A UI mostra isso em vez de oferecer
    /// um botão que falha.
    ///
    /// [ADR-0042]: ../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
    fn repo_do_projeto(projeto: &Projeto) -> Result<GitRepo, ProjectError> {
        GitRepo::abrir(&projeto.caminho).map_err(|origem| ProjectError::WorkspaceSemGit {
            caminho: projeto.caminho.clone(),
            origem,
        })
    }

    /// Cria um marco: tag anotada no repositório + metadados.
    ///
    /// A tag vem primeiro. Se o banco falhar depois, sobra uma tag
    /// sem metadados — visível pelo `git tag` do usuário, que é
    /// perda recuperável. A ordem inversa deixaria metadados
    /// apontando para uma tag inexistente, que é registro mentindo.
    pub async fn criar_marco(
        &self,
        projeto_id: ProjectId,
        nome: &str,
        descricao: &str,
        autor: &Autor,
        conversa_origem: Option<&str>,
    ) -> Result<Marco, ProjectError> {
        self.criar_marco_interno(projeto_id, nome, descricao, autor, conversa_origem, false)
            .await
    }

    async fn criar_marco_interno(
        &self,
        projeto_id: ProjectId,
        nome: &str,
        descricao: &str,
        autor: &Autor,
        conversa_origem: Option<&str>,
        automatico: bool,
    ) -> Result<Marco, ProjectError> {
        let projeto = self.projeto(projeto_id).await?;
        let repo = Self::repo_do_projeto(&projeto)?;

        let existente: Option<(String,)> =
            sqlx::query_as("SELECT id FROM project_milestones WHERE project_id = ?1 AND nome = ?2")
                .bind(projeto_id.as_uuid().to_string())
                .bind(nome)
                .fetch_optional(self.pool)
                .await?;
        if existente.is_some() {
            return Err(ProjectError::MarcoJaExiste(nome.to_string()));
        }

        let mensagem = if descricao.trim().is_empty() {
            format!("marco: {nome}")
        } else {
            descricao.to_string()
        };
        let tag = repo.criar_tag(nome, &mensagem, autor)?;

        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO project_milestones \
             (id, project_id, nome, descricao, commit_id, conversa_origem, automatico) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&id)
        .bind(projeto_id.as_uuid().to_string())
        .bind(nome)
        .bind(descricao)
        .bind(&tag.commit_id)
        .bind(conversa_origem)
        .bind(i64::from(automatico))
        .execute(self.pool)
        .await?;

        self.marco(projeto_id, nome).await
    }

    /// Marcos do projeto, do mais novo para o mais antigo.
    pub async fn listar_marcos(&self, projeto_id: ProjectId) -> Result<Vec<Marco>, ProjectError> {
        let linhas: Vec<LinhaMarco> = sqlx::query_as(
                "SELECT id, project_id, nome, descricao, commit_id, conversa_origem, automatico, criado_em \
                 FROM project_milestones WHERE project_id = ?1 ORDER BY criado_em DESC, rowid DESC",
            )
            .bind(projeto_id.as_uuid().to_string())
            .fetch_all(self.pool)
            .await?;
        Ok(linhas.into_iter().map(linha_para_marco).collect())
    }

    /// Um marco pelo nome.
    pub async fn marco(&self, projeto_id: ProjectId, nome: &str) -> Result<Marco, ProjectError> {
        let linha: Option<LinhaMarco> = sqlx::query_as(
                "SELECT id, project_id, nome, descricao, commit_id, conversa_origem, automatico, criado_em \
                 FROM project_milestones WHERE project_id = ?1 AND nome = ?2",
            )
            .bind(projeto_id.as_uuid().to_string())
            .bind(nome)
            .fetch_optional(self.pool)
            .await?;
        linha
            .map(linha_para_marco)
            .ok_or_else(|| ProjectError::MarcoNaoEncontrado(nome.to_string()))
    }

    /// Restaura o workspace ao estado de um marco.
    ///
    /// **Nada é descartado, e a garantia é estrutural.** O
    /// [ADR-0042] §D3 exige que nenhuma API descarte mudanças sem
    /// marco automático anterior. Aqui isso acontece em duas
    /// camadas:
    ///
    /// 1. Se houver trabalho pendente, um **marco automático** é
    ///    criado antes — o trabalho vira commit com nome, não lixo
    ///    perdido.
    /// 2. A restauração em si é um **commit novo** com a árvore do
    ///    marco ([`GitRepo::restaurar_tag`]), não um `reset`. O
    ///    histórico continua inteiro.
    ///
    /// O motor recusa restaurar com árvore suja, então a camada 1
    /// não é cortesia: sem ela, a operação falha. A garantia não
    /// depende de quem chama lembrar.
    ///
    /// [ADR-0042]: ../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
    pub async fn restaurar_marco(
        &self,
        projeto_id: ProjectId,
        nome: &str,
        autor: &Autor,
    ) -> Result<Restauracao, ProjectError> {
        // Confirma que o marco existe **antes** de mexer em qualquer
        // coisa: criar marco automático para depois descobrir que o
        // alvo não existe deixaria lixo pelo caminho.
        let alvo = self.marco(projeto_id, nome).await?;
        let projeto = self.projeto(projeto_id).await?;
        let repo = Self::repo_do_projeto(&projeto)?;

        let marco_automatico = if repo.status()?.is_empty() {
            None
        } else {
            let carimbo = chrono::Utc::now().format("%Y%m%d-%H%M%S");
            let nome_auto = format!("auto-antes-de-{nome}-{carimbo}");
            let descricao = format!(
                "estado do workspace salvo automaticamente antes de restaurar o marco \"{nome}\""
            );
            repo.commitar(&descricao, autor)?;
            Some(
                self.criar_marco_interno(projeto_id, &nome_auto, &descricao, autor, None, true)
                    .await?,
            )
        };

        let commit = repo.restaurar_tag(&alvo.nome, autor)?;
        Ok(Restauracao {
            commit_id: commit.id,
            marco_automatico,
        })
    }
}

fn linha_para_projeto(l: (String, String, String, Option<String>, String, String)) -> Projeto {
    let (id, caminho, nome, perfil_permissao, criado_em, ultimo_acesso) = l;
    Projeto {
        id: ProjectId(uuid::Uuid::parse_str(&id).unwrap_or_default()),
        caminho: PathBuf::from(caminho),
        nome,
        perfil_permissao,
        criado_em,
        ultimo_acesso,
    }
}

fn linha_para_marco(
    l: (
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
        String,
    ),
) -> Marco {
    let (id, projeto_id, nome, descricao, commit_id, conversa_origem, automatico, criado_em) = l;
    Marco {
        id,
        projeto_id: ProjectId(uuid::Uuid::parse_str(&projeto_id).unwrap_or_default()),
        nome,
        descricao,
        commit_id,
        conversa_origem,
        automatico: automatico != 0,
        criado_em,
    }
}
