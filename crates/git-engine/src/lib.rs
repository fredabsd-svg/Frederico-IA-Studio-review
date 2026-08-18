//! Operações de Git local sobre o workspace da conversa.
//!
//! **Fronteira (ADR-0040 §D3):** este crate não conhece o sistema de
//! arquivos além do caminho que recebe. O workspace chega já resolvido
//! pelo `JailResolver`; não existe API que aceite caminho absoluto
//! arbitrário, e é por isso que o agente não consegue operar Git fora
//! da pasta da conversa.
//!
//! **Biblioteca (ADR-0040 §D1 e ADR-0047):** Git vem de biblioteca
//! linkada. `Command::new("git")` está proibido — processo externo
//! contornaria o sandbox inteiro da Fase 7. A escolha do `git2` sobre
//! o `gix` foi medida pelo spike da Etapa 3, não presumida.
//!
//! **Estado:** Etapa 3, PR de spike. Só o caminho de escrita
//! (`commit`) e a leitura que o valida (`log`) existem. `status`,
//! `diff` e `branch` entram no PR de implementação.

use std::path::{Path, PathBuf};

/// Erros do `git-engine`.
///
/// A mensagem do `git2` é preservada em `detalhe` porque ela vem
/// estruturada da biblioteca — e não como prosa em stderr na locale
/// da máquina, que é um dos motivos do ADR-0040 §D1 ponto 2.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("o caminho não é um repositório Git: {0}")]
    NaoEhRepositorio(PathBuf),
    #[error("nada para commitar — a árvore de trabalho está limpa")]
    NadaParaCommitar,
    #[error("falha do Git em {operacao}: {detalhe}")]
    Biblioteca {
        operacao: &'static str,
        detalhe: String,
    },
}

impl GitError {
    fn de(operacao: &'static str) -> impl Fn(git2::Error) -> GitError {
        move |e| GitError::Biblioteca {
            operacao,
            detalhe: e.message().to_string(),
        }
    }
}

/// Quem assina o commit.
///
/// Vem sempre de fora. O `git2` aceita assinatura explícita, e é isso
/// que este crate usa: depender de `user.name`/`user.email` no config
/// da máquina reintroduziria a dependência de ambiente que o
/// ADR-0040 §D1 ponto 1 rejeita.
#[derive(Debug, Clone)]
pub struct Autor {
    pub nome: String,
    pub email: String,
}

/// Um commit lido de volta do repositório.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: String,
    pub resumo: String,
    pub autor: String,
    pub pais: usize,
}

/// Repositório Git aberto sobre o workspace da conversa.
pub struct GitRepo {
    inner: git2::Repository,
}

impl GitRepo {
    /// Abre um repositório existente.
    ///
    /// Não descobre repositório subindo diretórios: o caminho é o
    /// workspace resolvido, e procurar acima dele sairia do Jail.
    pub fn abrir(workspace: &Path) -> Result<Self, GitError> {
        let inner = git2::Repository::open(workspace)
            .map_err(|_| GitError::NaoEhRepositorio(workspace.to_path_buf()))?;
        Ok(Self { inner })
    }

    /// Cria um repositório novo no workspace.
    pub fn iniciar(workspace: &Path) -> Result<Self, GitError> {
        let inner = git2::Repository::init(workspace).map_err(GitError::de("init"))?;
        Ok(Self { inner })
    }

    /// Registra no índice tudo que mudou no workspace e cria o commit.
    ///
    /// **O índice é escrito antes da árvore, e isso não é detalhe.** O
    /// spike da Etapa 3 mediu que uma implementação que grava o objeto
    /// de commit sem atualizar o `.git/index` deixa o repositório num
    /// estado que qualquer outro cliente Git lê como "o arquivo
    /// commitado foi apagado" (ADR-0047 §Medição). O commit fica
    /// válido e o usuário vê um workspace quebrado.
    pub fn commitar(&self, mensagem: &str, autor: &Autor) -> Result<CommitInfo, GitError> {
        let mut index = self.inner.index().map_err(GitError::de("abrir índice"))?;
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(GitError::de("registrar arquivos"))?;
        index.write().map_err(GitError::de("escrever índice"))?;

        let tree_id = index
            .write_tree()
            .map_err(GitError::de("escrever árvore"))?;
        let tree = self
            .inner
            .find_tree(tree_id)
            .map_err(GitError::de("ler árvore"))?;

        let pai = match self.inner.head() {
            Ok(h) => h.peel_to_commit().ok(),
            Err(_) => None, // repositório sem commit ainda
        };
        if let Some(p) = &pai {
            if p.tree_id() == tree_id {
                return Err(GitError::NadaParaCommitar);
            }
        }

        let assinatura = git2::Signature::now(&autor.nome, &autor.email)
            .map_err(GitError::de("montar assinatura"))?;
        let pais: Vec<&git2::Commit<'_>> = pai.iter().collect();
        let id = self
            .inner
            .commit(
                Some("HEAD"),
                &assinatura,
                &assinatura,
                mensagem,
                &tree,
                &pais,
            )
            .map_err(GitError::de("commitar"))?;

        self.ler_commit(id)
    }

    /// Últimos `limite` commits a partir do `HEAD`, do mais novo para
    /// o mais antigo. Repositório sem commit devolve lista vazia.
    pub fn historico(&self, limite: usize) -> Result<Vec<CommitInfo>, GitError> {
        if self.inner.head().is_err() {
            return Ok(Vec::new());
        }
        let mut walk = self.inner.revwalk().map_err(GitError::de("revwalk"))?;
        walk.push_head().map_err(GitError::de("revwalk push"))?;
        let mut saida = Vec::new();
        for id in walk.take(limite) {
            let id = id.map_err(GitError::de("percorrer histórico"))?;
            saida.push(self.ler_commit(id)?);
        }
        Ok(saida)
    }

    fn ler_commit(&self, id: git2::Oid) -> Result<CommitInfo, GitError> {
        let c = self
            .inner
            .find_commit(id)
            .map_err(GitError::de("ler commit"))?;
        let resumo = c.summary().ok().flatten().unwrap_or_default().to_string();
        let assinatura = c.author();
        let autor = assinatura.name().unwrap_or_default().to_string();
        Ok(CommitInfo {
            id: id.to_string(),
            resumo,
            autor,
            pais: c.parent_count(),
        })
    }
}
