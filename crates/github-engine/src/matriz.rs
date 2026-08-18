//! Matriz de autorização — o coração do [ADR-0041] §D2.
//!
//! Permissão de GitHub **não é booleano**. "Pode usar GitHub"
//! autorizaria tanto ler um repositório público quanto empurrar para
//! o `main` de produção, e a assimetria de dano entre os dois é o
//! motivo de a matriz existir.
//!
//! Três dimensões, todas fail-closed: repositório, branch e
//! operação. Vazio significa **nenhum**, nunca "todos".
//!
//! [ADR-0041]: ../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md

use std::collections::BTreeSet;

/// Branches que exigem menção nominal ([ADR-0041] §D2).
///
/// Um padrão com curinga **não** as alcança. Quem quer empurrar para
/// `main` escreve `main`, e a escrita é o consentimento — não um
/// `*` digitado para outra finalidade que passou a cobrir a branch
/// principal sem ninguém perceber.
///
/// [ADR-0041]: ../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
pub const BRANCHES_PROTEGIDAS: &[&str] = &["main", "master"];

/// Operação sobre um repositório.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operacao {
    Ler,
    Push,
    CriarPr,
}

impl Operacao {
    #[must_use]
    pub const fn como_str(self) -> &'static str {
        match self {
            Self::Ler => "read",
            Self::Push => "push",
            Self::CriarPr => "create_pr",
        }
    }
}

/// Referência a um repositório, no formato `owner/repo`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    /// Constrói a partir de `owner/repo`.
    ///
    /// Recusa qualquer coisa que não seja exatamente duas partes não
    /// vazias. Aceitar `owner/repo/extra` ou `owner/` deixaria a
    /// comparação da matriz depender de normalização implícita — e a
    /// comparação é o que autoriza a operação.
    pub fn parse(texto: &str) -> Result<Self, MatrizError> {
        let partes: Vec<&str> = texto.split('/').collect();
        if partes.len() != 2 || partes[0].trim().is_empty() || partes[1].trim().is_empty() {
            return Err(MatrizError::RepoMalFormado(texto.to_string()));
        }
        Ok(Self {
            owner: partes[0].to_string(),
            repo: partes[1].to_string(),
        })
    }

    #[must_use]
    pub fn completo(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Erros da matriz.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MatrizError {
    #[error("repositório mal formado (esperado `owner/repo`): {0}")]
    RepoMalFormado(String),
    #[error("o repositório {0} não está na matriz de autorização")]
    RepoForaDaMatriz(String),
    #[error("a operação {operacao} não está autorizada para {repo}")]
    OperacaoNegada { repo: String, operacao: String },
    #[error(
        "a branch {branch} de {repo} não está autorizada. \
         Branch protegida exige menção nominal — curinga não a alcança."
    )]
    BranchNegada { repo: String, branch: String },
}

/// Regra de um repositório dentro da matriz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegraRepo {
    pub repo: RepoRef,
    /// Padrões de branch. Vazio = nenhuma branch.
    ///
    /// Suporta nome exato (`main`, `desenvolvimento`) e sufixo com
    /// curinga (`feature/*`). O curinga **não** alcança as branches
    /// de [`BRANCHES_PROTEGIDAS`].
    pub branches: Vec<String>,
    /// Operações permitidas. Vazio = nenhuma.
    pub operacoes: BTreeSet<Operacao>,
}

impl RegraRepo {
    fn branch_autorizada(&self, branch: &str) -> bool {
        let protegida = BRANCHES_PROTEGIDAS.contains(&branch);
        self.branches.iter().any(|padrao| {
            if padrao == branch {
                return true; // menção nominal sempre vale
            }
            if protegida {
                return false; // curinga não alcança protegida
            }
            match padrao.strip_suffix('*') {
                Some(prefixo) => branch.starts_with(prefixo),
                None => false,
            }
        })
    }
}

/// A matriz de autorização.
///
/// **Vazia nega tudo.** É o default, e é o comportamento certo: sem
/// entrada, o app não sabe para onde empurrar, e inventar um destino
/// é o tipo de dano que não tem desfazer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatrizAutorizacao {
    regras: Vec<RegraRepo>,
}

impl MatrizAutorizacao {
    /// Matriz vazia — nega tudo.
    #[must_use]
    pub fn vazia() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn com_regras(regras: Vec<RegraRepo>) -> Self {
        Self { regras }
    }

    #[must_use]
    pub fn regras(&self) -> &[RegraRepo] {
        &self.regras
    }

    #[must_use]
    pub fn esta_vazia(&self) -> bool {
        self.regras.is_empty()
    }

    /// Autoriza — ou explica exatamente o que faltou.
    ///
    /// A ordem das verificações é deliberada: repositório, depois
    /// operação, depois branch. Ela determina qual mensagem o usuário
    /// vê, e "o repositório não está na matriz" é acionável enquanto
    /// "negado" não é.
    ///
    /// `branch` é opcional porque [`Operacao::Ler`] não tem branch
    /// alvo.
    pub fn autoriza(
        &self,
        repo: &RepoRef,
        operacao: Operacao,
        branch: Option<&str>,
    ) -> Result<(), MatrizError> {
        let regra = self
            .regras
            .iter()
            .find(|r| &r.repo == repo)
            .ok_or_else(|| MatrizError::RepoForaDaMatriz(repo.completo()))?;

        if !regra.operacoes.contains(&operacao) {
            return Err(MatrizError::OperacaoNegada {
                repo: repo.completo(),
                operacao: operacao.como_str().to_string(),
            });
        }

        if let Some(branch) = branch {
            if !regra.branch_autorizada(branch) {
                return Err(MatrizError::BranchNegada {
                    repo: repo.completo(),
                    branch: branch.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Interseção de duas matrizes, fail-closed.
    ///
    /// Segue a regra dos demais eixos do `PermissionSet`: o efetivo é
    /// `usuário ∩ projeto`. Repositório ausente de qualquer um dos
    /// lados sai; branch e operação idem. **Nada é somado** — a
    /// interseção só pode restringir.
    #[must_use]
    pub fn intersecao(&self, outra: &Self) -> Self {
        let mut regras = Vec::new();
        for minha in &self.regras {
            let Some(dela) = outra.regras.iter().find(|r| r.repo == minha.repo) else {
                continue; // repositório que só existe de um lado sai
            };
            let branches: Vec<String> = minha
                .branches
                .iter()
                .filter(|b| dela.branches.contains(b))
                .cloned()
                .collect();
            let operacoes: BTreeSet<Operacao> = minha
                .operacoes
                .intersection(&dela.operacoes)
                .copied()
                .collect();
            // Regra que sobrou sem branch ou sem operação não
            // autoriza nada; mantê-la só produziria mensagem de erro
            // pior ("operação negada" em vez de "fora da matriz").
            if branches.is_empty() || operacoes.is_empty() {
                continue;
            }
            regras.push(RegraRepo {
                repo: minha.repo.clone(),
                branches,
                operacoes,
            });
        }
        Self { regras }
    }
}
