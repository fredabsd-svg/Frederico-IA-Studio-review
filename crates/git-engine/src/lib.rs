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
//! **Estado:** Etapa 3, PR de implementação. As cinco operações do
//! [ADR-0039] §D1 existem: `status`, `diff`, `log`, `branch` e
//! `commit`.
//!
//! [ADR-0039]: ../docs/decisions/0039-fase-8-escopo-e-etapas.md

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
    #[error("o repositório ainda não tem commit — crie um antes de usar branch")]
    SemCommit,
    #[error("já existe um branch chamado {0}")]
    BranchJaExiste(String),
    #[error("não existe branch chamado {0}")]
    BranchNaoExiste(String),
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

/// O que mudou em um arquivo, do ponto de vista do Git.
///
/// Deliberadamente pequeno: o `git2::Status` é um bitfield com 14
/// combinações, e traduzi-lo inteiro para a fronteira exporia a
/// biblioteca — o que o ADR-0047 §D4 pede para evitar, porque trocar
/// de biblioteca depois arrastaria a UI junto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoArquivo {
    /// Não rastreado pelo Git.
    NaoRastreado,
    Novo,
    Modificado,
    Apagado,
    Renomeado,
    /// Merge com conflito não resolvido. O crate detecta e reporta;
    /// resolver é do usuário (spec §"O que este módulo NÃO faz").
    Conflito,
}

impl EstadoArquivo {
    /// Rótulo estável para o JSON da ferramenta. Não usa `Debug`:
    /// derivar contrato de saída do `Debug` faz renomear variante
    /// virar quebra de contrato silenciosa.
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::NaoRastreado => "nao_rastreado",
            Self::Novo => "novo",
            Self::Modificado => "modificado",
            Self::Apagado => "apagado",
            Self::Renomeado => "renomeado",
            Self::Conflito => "conflito",
        }
    }
}

/// Um arquivo com mudança pendente.
///
/// `staged` distingue o que já está no índice do que só existe na
/// árvore de trabalho — a diferença entre o que entraria no próximo
/// commit e o que não entraria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MudancaArquivo {
    pub caminho: String,
    pub estado: EstadoArquivo,
    pub staged: bool,
}

/// Um branch local do repositório.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub nome: String,
    pub atual: bool,
}

impl GitRepo {
    /// Arquivos com mudança pendente, ordenados por caminho.
    ///
    /// Inclui não rastreados: para quem usa o app, um arquivo que a
    /// IA acabou de criar e não aparece no status é indistinguível de
    /// um arquivo que não foi criado.
    pub fn status(&self) -> Result<Vec<MudancaArquivo>, GitError> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true);
        let statuses = self
            .inner
            .statuses(Some(&mut opts))
            .map_err(GitError::de("ler status"))?;

        let mut saida = Vec::new();
        for entry in statuses.iter() {
            // `path()` devolve `Result` no git2 0.21 (o caminho pode
            // não ser UTF-8 válido); caminho ilegível é pulado, não
            // reportado como mudança inexistente.
            let Ok(caminho) = entry.path() else {
                continue;
            };
            let s = entry.status();
            // Conflito primeiro: o bit coexiste com outros, e
            // reportar "modificado" para arquivo em conflito esconde
            // justamente o que o usuário precisa ver.
            let (estado, staged) = if s.is_conflicted() {
                (EstadoArquivo::Conflito, false)
            } else if s.is_index_new() {
                (EstadoArquivo::Novo, true)
            } else if s.is_index_modified() {
                (EstadoArquivo::Modificado, true)
            } else if s.is_index_deleted() {
                (EstadoArquivo::Apagado, true)
            } else if s.is_index_renamed() {
                (EstadoArquivo::Renomeado, true)
            } else if s.is_wt_new() {
                (EstadoArquivo::NaoRastreado, false)
            } else if s.is_wt_modified() {
                (EstadoArquivo::Modificado, false)
            } else if s.is_wt_deleted() {
                (EstadoArquivo::Apagado, false)
            } else if s.is_wt_renamed() {
                (EstadoArquivo::Renomeado, false)
            } else {
                continue; // ignorado, ou estado que não interessa reportar
            };
            saida.push(MudancaArquivo {
                caminho: caminho.to_string(),
                estado,
                staged,
            });
        }
        saida.sort_by(|a, b| a.caminho.cmp(&b.caminho));
        Ok(saida)
    }

    /// Patch unificado das mudanças pendentes.
    ///
    /// `staged = true` compara o índice com o `HEAD` (o que entraria
    /// no commit); `false` compara a árvore de trabalho com o índice
    /// (o que ficaria de fora). São perguntas diferentes, e é por
    /// isso que o spec expõe o booleano em vez de somar as duas.
    pub fn diff(&self, staged: bool) -> Result<String, GitError> {
        let head_tree = match self.inner.head() {
            Ok(h) => h.peel_to_tree().ok(),
            Err(_) => None,
        };
        let index = self.inner.index().map_err(GitError::de("abrir índice"))?;
        let diff = if staged {
            self.inner
                .diff_tree_to_index(head_tree.as_ref(), Some(&index), None)
                .map_err(GitError::de("diff do índice"))?
        } else {
            self.inner
                .diff_index_to_workdir(Some(&index), None)
                .map_err(GitError::de("diff da árvore de trabalho"))?
        };

        let mut patch = String::new();
        diff.print(git2::DiffFormat::Patch, |_, _, linha| {
            let origem = linha.origin();
            if matches!(origem, '+' | '-' | ' ') {
                patch.push(origem);
            }
            patch.push_str(&String::from_utf8_lossy(linha.content()));
            true
        })
        .map_err(GitError::de("formatar patch"))?;
        Ok(patch)
    }

    /// Branches locais, com o corrente marcado.
    pub fn branches(&self) -> Result<Vec<BranchInfo>, GitError> {
        let atual = self.branch_atual().unwrap_or_default();
        let it = self
            .inner
            .branches(Some(git2::BranchType::Local))
            .map_err(GitError::de("listar branches"))?;
        let mut saida = Vec::new();
        for b in it {
            let (branch, _) = b.map_err(GitError::de("ler branch"))?;
            if let Ok(Some(nome)) = branch.name() {
                saida.push(BranchInfo {
                    nome: nome.to_string(),
                    atual: nome == atual,
                });
            }
        }
        saida.sort_by(|a, b| a.nome.cmp(&b.nome));
        Ok(saida)
    }

    /// Nome do branch corrente. `None` em `HEAD` destacado.
    #[must_use]
    pub fn branch_atual(&self) -> Option<String> {
        let head = self.inner.head().ok()?;
        head.shorthand().ok().map(ToString::to_string)
    }

    /// Cria um branch a partir do `HEAD` e, se `trocar`, passa a ele.
    ///
    /// **Não apaga branch** — o spec exclui a operação, e a exclusão
    /// é deliberada: apagar branch é a única operação local desta
    /// família que descarta trabalho sem o usuário ver o que perdeu.
    pub fn criar_branch(&self, nome: &str, trocar: bool) -> Result<BranchInfo, GitError> {
        let commit = match self.inner.head() {
            Ok(h) => h.peel_to_commit().map_err(GitError::de("ler HEAD"))?,
            Err(_) => return Err(GitError::SemCommit),
        };
        if self
            .inner
            .find_branch(nome, git2::BranchType::Local)
            .is_ok()
        {
            return Err(GitError::BranchJaExiste(nome.to_string()));
        }
        self.inner
            .branch(nome, &commit, false)
            .map_err(GitError::de("criar branch"))?;
        if trocar {
            self.trocar_branch(nome)?;
        }
        Ok(BranchInfo {
            nome: nome.to_string(),
            atual: trocar,
        })
    }

    /// Troca para um branch existente.
    pub fn trocar_branch(&self, nome: &str) -> Result<(), GitError> {
        if self
            .inner
            .find_branch(nome, git2::BranchType::Local)
            .is_err()
        {
            return Err(GitError::BranchNaoExiste(nome.to_string()));
        }
        let referencia = format!("refs/heads/{nome}");
        let objeto = self
            .inner
            .revparse_single(&referencia)
            .map_err(GitError::de("resolver branch"))?;
        // `safe` (não `force`): trocar de branch por cima de mudança
        // pendente apagaria trabalho do usuário sem aviso. O `git2`
        // recusa, e a recusa é o comportamento certo.
        self.inner
            .checkout_tree(&objeto, Some(git2::build::CheckoutBuilder::new().safe()))
            .map_err(GitError::de("trocar de branch"))?;
        self.inner
            .set_head(&referencia)
            .map_err(GitError::de("mover HEAD"))?;
        Ok(())
    }
}
