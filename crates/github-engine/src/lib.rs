//! GitHub: `push`, criação de PR e a matriz que autoriza os dois.
//!
//! **Este é o primeiro módulo do produto que executa operação
//! destrutiva em serviço externo por conta do agente.** Todas as
//! anteriores eram locais e reversíveis por construção: `files.write`
//! tem backup `.bak` e hashes na auditoria ([ADR-0035]); `exec.*`
//! roda em sandbox que morre com o run.
//!
//! Aqui não há desfazer. Um `push` errado altera o repositório de
//! outras pessoas; um PR criado por engano notifica revisores e fica
//! no histórico mesmo depois de fechado. Todo o desenho decorre
//! disso.
//!
//! ## O que este módulo **não** faz
//!
//! - **Não faz force-push.** Não é opção com aprovação reforçada — é
//!   **ausência de API** ([ADR-0041] §D3). Quem precisa tem
//!   `exec.shell`, com o comando à vista, denylist e aprovação por
//!   invocação. A diferença entre as duas portas é que numa o usuário
//!   está lendo o comando e na outra o agente decidiu sozinho.
//! - **Não apaga branch, não fecha issue, não faz merge.** A
//!   superfície é `read`, `push`, `create_pr`.
//! - **Não guarda token.** Recebe um [`SecretString`] de quem já o
//!   leu do Windows Credential Manager, e **nunca** o coloca no
//!   ambiente do processo ([ADR-0041] §D1) — a Fase 7 provou por
//!   teste que credencial no ambiente do pai vaza para o filho do
//!   sandbox, em silêncio.
//! - **Não assume repositório.** Sem entrada na matriz, nada
//!   funciona.
//!
//! [ADR-0035]: ../docs/decisions/0035-fase-7-file-ops-overwrite-semantics.md
//! [ADR-0041]: ../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md

pub mod matriz;

use std::path::Path;

use secrecy::{ExposeSecret, SecretString};

pub use matriz::{
    MatrizAutorizacao, MatrizError, Operacao, RegraRepo, RepoRef, BRANCHES_PROTEGIDAS,
};

/// Base da API do GitHub em produção.
pub const BASE_URL_PADRAO: &str = "https://api.github.com";

/// Erros do `github-engine`.
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error(transparent)]
    Autorizacao(#[from] MatrizError),
    #[error("falha de rede ao falar com o GitHub: {0}")]
    Rede(String),
    #[error("o GitHub recusou ({status}): {mensagem}")]
    Recusa { status: u16, mensagem: String },
    #[error("resposta do GitHub em formato inesperado: {0}")]
    RespostaInvalida(String),
    #[error("falha do Git ao empurrar: {0}")]
    Git(String),
    #[error("o repositório local não tem a branch {0}")]
    BranchLocalInexistente(String),
    #[error(
        "o remoto `{remote}` aponta para {url}, que não é o repositório autorizado {esperado}"
    )]
    RemotoNaoCorresponde {
        remote: String,
        url: String,
        esperado: String,
    },
    #[error("o remoto `{0}` não tem URL configurada")]
    RemotoSemUrl(String),
}

/// Um pull request criado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub numero: u64,
    pub url: String,
    pub titulo: String,
}

/// Resultado de um push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushFeito {
    pub repo: String,
    pub branch: String,
    /// Quantos commits locais existiam à frente do remoto no momento
    /// do push. É o número que o pedido de aprovação mostra
    /// ([ADR-0041] §D4).
    ///
    /// [ADR-0041]: ../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
    pub commits: usize,
}

/// Cliente do GitHub, com a matriz embutida.
///
/// A matriz não é parâmetro de cada chamada: ela é **estado do
/// cliente**. Quem constrói o cliente decide o alcance, e nenhuma
/// chamada pode ampliá-lo — se a autorização viajasse por argumento,
/// bastaria um caminho de código esquecer de passá-la.
pub struct GithubEngine {
    token: SecretString,
    matriz: MatrizAutorizacao,
    base_url: String,
    http: reqwest::Client,
}

impl GithubEngine {
    /// Cliente apontando para a API pública.
    #[must_use]
    pub fn new(token: SecretString, matriz: MatrizAutorizacao) -> Self {
        Self::com_base_url(token, matriz, BASE_URL_PADRAO)
    }

    /// Cliente com `base_url` injetável.
    ///
    /// Existe para o **twin determinístico** do [ADR-0041] §D5: o
    /// mesmo código de produção roda contra um servidor HTTP local
    /// que fala o subconjunto usado da API, em todo PR, sem tocar o
    /// GitHub. Twin não é opcional — a REGRA §3.3 proíbe promover
    /// fase com cobertura só-noturna sem ele.
    ///
    /// [ADR-0041]: ../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
    #[must_use]
    pub fn com_base_url(
        token: SecretString,
        matriz: MatrizAutorizacao,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            token,
            matriz,
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn matriz(&self) -> &MatrizAutorizacao {
        &self.matriz
    }

    /// Cria um pull request.
    ///
    /// A branch autorizada é a **de origem** (`head`), que é a que o
    /// trabalho vem: abrir PR não escreve em `base`, só propõe. A
    /// escrita em `base` é decisão de quem faz o merge, no GitHub.
    pub async fn criar_pr(
        &self,
        repo: &RepoRef,
        head: &str,
        base: &str,
        titulo: &str,
        corpo: &str,
    ) -> Result<PullRequest, GithubError> {
        self.matriz.autoriza(repo, Operacao::CriarPr, Some(head))?;

        let url = format!(
            "{}/repos/{}/{}/pulls",
            self.base_url.trim_end_matches('/'),
            repo.owner,
            repo.repo
        );
        let corpo_json = serde_json::json!({
            "title": titulo,
            "head": head,
            "base": base,
            "body": corpo,
        });

        let resposta = self
            .http
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Frederico-IA-Studio")
            .bearer_auth(self.token.expose_secret())
            .json(&corpo_json)
            .send()
            .await
            .map_err(|e| GithubError::Rede(e.to_string()))?;

        let status = resposta.status().as_u16();
        let texto = resposta
            .text()
            .await
            .map_err(|e| GithubError::Rede(e.to_string()))?;

        if !(200..300).contains(&status) {
            // O corpo do GitHub traz `message`; se não trouxer, o
            // texto cru é melhor que uma mensagem genérica.
            let mensagem = serde_json::from_str::<serde_json::Value>(&texto)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or(texto);
            return Err(GithubError::Recusa { status, mensagem });
        }

        let v: serde_json::Value = serde_json::from_str(&texto)
            .map_err(|e| GithubError::RespostaInvalida(e.to_string()))?;
        let numero = v
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| GithubError::RespostaInvalida("faltou `number`".into()))?;
        let url_pr = v
            .get("html_url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(PullRequest {
            numero,
            url: url_pr,
            titulo: titulo.to_string(),
        })
    }

    /// Empurra uma branch local para o remoto.
    ///
    /// **Nunca com force.** O refspec é montado aqui e não aceita o
    /// prefixo `+` — não há parâmetro que o produza, o que torna o
    /// force ausente por construção e não por validação.
    pub async fn push(
        &self,
        workspace: &Path,
        repo: &RepoRef,
        branch: &str,
        remote: &str,
    ) -> Result<PushFeito, GithubError> {
        self.matriz.autoriza(repo, Operacao::Push, Some(branch))?;

        let repositorio = git2::Repository::open(workspace)
            .map_err(|e| GithubError::Git(e.message().to_string()))?;

        // Confirma que a branch existe localmente antes de falar com
        // a rede: erro de digitação vira mensagem clara em vez de
        // recusa do servidor.
        if repositorio
            .find_branch(branch, git2::BranchType::Local)
            .is_err()
        {
            return Err(GithubError::BranchLocalInexistente(branch.to_string()));
        }

        let commits = contar_commits(&repositorio, branch, remote);

        let mut remoto = repositorio
            .find_remote(remote)
            .map_err(|e| GithubError::Git(e.message().to_string()))?;

        // **ADR-0048 §D4.** A matriz autoriza `owner/repo`; sem esta
        // conferência, o push iria para onde o remoto apontar,
        // carregando a autorização do repositório certo. Enquanto o
        // motor não tinha porta para o agente, o cenário exigia
        // alguém alterar o remoto à mão — mas `.git/config` fica no
        // workspace, e o agente escreve no workspace.
        // `url()` devolve `Result<&str, _>` no git2 0.21 quando a URL
        // não é UTF-8 válida; os dois casos viram a mesma recusa.
        let url = remoto
            .url()
            .map_err(|_| GithubError::RemotoSemUrl(remote.to_string()))?;
        let alvo = repo_de_url(url);
        if alvo.as_ref() != Some(&repo.completo()) {
            return Err(GithubError::RemotoNaoCorresponde {
                remote: remote.to_string(),
                url: url.to_string(),
                esperado: repo.completo(),
            });
        }

        empurrar(&mut remoto, branch, self.token.expose_secret())?;

        Ok(PushFeito {
            repo: repo.completo(),
            branch: branch.to_string(),
            commits,
        })
    }
}

/// A mecânica do push, separada da política.
///
/// A política (matriz e conferência do remoto, [ADR-0048] §D4) fica
/// no [`GithubEngine::push`]. Esta função só empurra — e a separação
/// existe para que a mecânica seja exercitada contra um repositório
/// local nos testes de unidade, **sem** abrir uma porta que
/// contorne a política na API pública. Ela é privada de propósito.
///
/// [ADR-0048]: ../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md
fn empurrar(remoto: &mut git2::Remote<'_>, branch: &str, token: &str) -> Result<(), GithubError> {
    let mut callbacks = git2::RemoteCallbacks::new();
    let token = token.to_string();
    callbacks.credentials(move |_url, _usuario, _tipos| {
        // PAT como senha, com usuário fixo — é o que o GitHub aceita
        // para HTTPS. O token **não** vai para o ambiente do
        // processo em nenhum momento (ADR-0041 §D1).
        git2::Cred::userpass_plaintext("x-access-token", &token)
    });
    let mut opcoes = git2::PushOptions::new();
    opcoes.remote_callbacks(callbacks);

    // Refspec sem `+`: sem force, por construção.
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remoto
        .push(&[refspec.as_str()], Some(&mut opcoes))
        .map_err(|e| GithubError::Git(e.message().to_string()))
}

/// Extrai `owner/repo` de uma URL de remoto do GitHub.
///
/// Aceita as formas que o GitHub publica — `https://github.com/o/r`,
/// `git@github.com:o/r`, com ou sem `.git`, com ou sem barra final —
/// e devolve `None` para qualquer outra coisa, **inclusive outro
/// host**. Um remoto apontando para outro serviço não é o
/// repositório da matriz, por definição, e devolver `None` faz o
/// caller recusar em vez de comparar nomes iguais em servidores
/// diferentes.
fn repo_de_url(url: &str) -> Option<String> {
    let sem_esquema = url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("https://")
        .or_else(|| url.trim().trim_end_matches('/').strip_prefix("http://"))
        .or_else(|| url.trim().trim_end_matches('/').strip_prefix("ssh://git@"))
        .or_else(|| url.trim().trim_end_matches('/').strip_prefix("git@"))
        .unwrap_or(url.trim().trim_end_matches('/'));

    // Depois do host vem `/owner/repo` ou `:owner/repo`.
    let resto = sem_esquema
        .strip_prefix("github.com/")
        .or_else(|| sem_esquema.strip_prefix("github.com:"))?;

    let resto = resto.strip_suffix(".git").unwrap_or(resto);
    let partes: Vec<&str> = resto.split('/').collect();
    if partes.len() != 2 || partes[0].is_empty() || partes[1].is_empty() {
        return None;
    }
    Some(format!("{}/{}", partes[0], partes[1]))
}

/// Quantos commits a branch local tem à frente do remoto.
///
/// Best-effort: se o remoto ainda não conhece a branch (push
/// inicial), devolve o total local. Erro de leitura vira `0` — este
/// número alimenta o texto do pedido de aprovação, e falhar a
/// operação inteira porque a contagem não saiu seria trocar uma
/// informação por uma indisponibilidade.
fn contar_commits(repo: &git2::Repository, branch: &str, remote: &str) -> usize {
    let Ok(local) = repo.revparse_single(&format!("refs/heads/{branch}")) else {
        return 0;
    };
    let remoto = repo.revparse_single(&format!("refs/remotes/{remote}/{branch}"));

    let Ok(mut walk) = repo.revwalk() else {
        return 0;
    };
    if walk.push(local.id()).is_err() {
        return 0;
    }
    if let Ok(r) = remoto {
        let _ = walk.hide(r.id());
    }
    walk.count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mecânica do push, contra um repositório bare local.
    ///
    /// Vive aqui, e não em `tests/`, por uma razão de desenho: a
    /// política do [ADR-0048] §D4 recusa remoto que não seja
    /// `github.com`, e um repositório local nunca será. Testar a
    /// mecânica pela API pública exigiria uma porta que contorne a
    /// política — e uma porta dessas, mesmo marcada "só para teste",
    /// é a porta. Aqui o teste alcança a função privada sem que ela
    /// exista para o mundo.
    ///
    /// [ADR-0048]: ../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md
    #[test]
    fn mecanica_do_push_entrega_a_branch_ao_remoto() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bare = tmp.path().join("remoto.git");
        git2::Repository::init_bare(&bare).expect("bare");

        let trabalho = tmp.path().join("trabalho");
        std::fs::create_dir_all(&trabalho).expect("mkdir");
        let repo = git2::Repository::init(&trabalho).expect("init");
        std::fs::write(trabalho.join("a.txt"), "conteudo\n").expect("escrever");
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add");
        index.write().expect("write");
        let arvore = repo
            .find_tree(index.write_tree().expect("tree"))
            .expect("find");
        let sig = git2::Signature::now("Frederico", "f@example.com").expect("sig");
        repo.commit(Some("HEAD"), &sig, &sig, "primeiro", &arvore, &[])
            .expect("commit");
        let head = repo.head().expect("head").peel_to_commit().expect("c");
        repo.branch("feature/entrega", &head, false)
            .expect("branch");

        let mut remoto = repo
            .remote("origin", &bare.to_string_lossy())
            .expect("remote");

        empurrar(&mut remoto, "feature/entrega", "token").expect("push");

        let destino = git2::Repository::open_bare(&bare).expect("abrir bare");
        destino
            .find_reference("refs/heads/feature/entrega")
            .expect("a branch tem que ter chegado");
    }

    /// O refspec montado não tem `+`, e é montado aqui.
    #[test]
    fn refspec_nao_tem_prefixo_de_force() {
        let branch = "feature/x";
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        assert!(!refspec.starts_with('+'));
        assert_eq!(refspec, "refs/heads/feature/x:refs/heads/feature/x");
    }

    /// As formas de URL que o GitHub publica são reconhecidas, e as
    /// demais **não** — inclusive outro host com o mesmo caminho.
    #[test]
    fn repo_de_url_reconhece_as_formas_do_github_e_recusa_o_resto() {
        for url in [
            "https://github.com/owner/repo",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo/",
            "git@github.com:owner/repo",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(
                repo_de_url(url).as_deref(),
                Some("owner/repo"),
                "não reconheceu {url}"
            );
        }

        for url in [
            // Outro host com o mesmo caminho: **não** é o repositório
            // da matriz, e devolver `None` faz o caller recusar em vez
            // de comparar nomes iguais em servidores diferentes.
            "https://gitlab.com/owner/repo",
            "https://github.com.attacker.example/owner/repo",
            "https://exemplo.com/owner/repo",
            "C:\\caminho\\local\\remoto.git",
            "/caminho/local/remoto.git",
            "https://github.com/owner",
            "https://github.com/owner/repo/extra",
            "",
        ] {
            assert_eq!(repo_de_url(url), None, "deveria recusar {url}");
        }
    }
}
