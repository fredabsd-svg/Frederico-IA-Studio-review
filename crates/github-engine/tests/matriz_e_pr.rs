//! Testes do `github-engine` — Etapa 5 da Fase 8.
//!
//! Os quatro nomes previstos em
//! `docs/architecture/github-integration-architecture.md`
//! §"Testes: o twin determinístico não é opcional" estão aqui, mais
//! as negações da matriz.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;

use frederico_github_engine::{
    GithubEngine, GithubError, MatrizAutorizacao, MatrizError, Operacao, RegraRepo, RepoRef,
};
use secrecy::SecretString;

fn repo() -> RepoRef {
    RepoRef::parse("fredabsd-svg/Frederico-IA-Studio-review").expect("repo")
}

fn ops(lista: &[Operacao]) -> BTreeSet<Operacao> {
    lista.iter().copied().collect()
}

/// Matriz que autoriza o repositório de teste em `feature/*` para as
/// três operações.
fn matriz_permissiva() -> MatrizAutorizacao {
    MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: repo(),
        branches: vec!["feature/*".into()],
        operacoes: ops(&[Operacao::Ler, Operacao::Push, Operacao::CriarPr]),
    }])
}

fn token() -> SecretString {
    SecretString::from("token-de-teste".to_string())
}

// ---------------------------------------------------------------------
// Twin determinístico (ADR-0041 §D5)
// ---------------------------------------------------------------------

/// Servidor HTTP local que fala o subconjunto usado da API do GitHub.
///
/// **Não é mock de biblioteca:** é um socket de verdade, e o
/// `GithubEngine` fala com ele pelo mesmo caminho de produção — mesmo
/// `reqwest`, mesmo cabeçalho, mesma serialização. O que muda é só o
/// `base_url`. Um mock no nível do cliente HTTP provaria que o teste
/// sabe chamar o mock.
///
/// Devolve o endereço e um canal com o corpo do request recebido,
/// para o teste conferir **o que foi enviado** — que é metade do
/// contrato.
fn sobe_stub_pr(
    status: u16,
    corpo_resposta: &'static str,
) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let Ok((mut fluxo, _)) = listener.accept() else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let n = fluxo.read(&mut buf).unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(request);

        let resposta = format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            corpo_resposta.len(),
            corpo_resposta
        );
        let _ = fluxo.write_all(resposta.as_bytes());
        let _ = fluxo.flush();
    });

    (format!("http://{addr}"), rx)
}

#[tokio::test]
async fn github_create_pr_against_local_stub() {
    let (base, rx) = sobe_stub_pr(
        201,
        r#"{"number": 42, "html_url": "http://local/pr/42", "title": "titulo"}"#,
    );
    let engine = GithubEngine::com_base_url(token(), matriz_permissiva(), base);

    let pr = engine
        .criar_pr(
            &repo(),
            "feature/minha-branch",
            "main",
            "titulo do PR",
            "corpo do PR",
        )
        .await
        .expect("criar PR contra o stub");

    assert_eq!(pr.numero, 42);
    assert_eq!(pr.url, "http://local/pr/42");

    // A outra metade do contrato: o que **saiu** daqui.
    let request = rx.recv().expect("o stub tem que ter recebido o request");
    assert!(
        request.starts_with("POST /repos/fredabsd-svg/Frederico-IA-Studio-review/pulls"),
        "caminho errado: {request}"
    );
    // Comparação sem caixa: o `hyper` serializa nome de cabeçalho em
    // minúsculas, e asserir `Authorization:` faria o teste falhar por
    // um detalhe de serialização em vez de por contrato quebrado.
    let minusculo = request.to_lowercase();
    assert!(
        minusculo.contains("authorization: bearer token-de-teste"),
        "o token tem que ir no cabeçalho: {request}"
    );
    assert!(
        request.contains("\"head\":\"feature/minha-branch\""),
        "corpo: {request}"
    );
    assert!(request.contains("\"base\":\"main\""), "corpo: {request}");
}

/// Erro do GitHub vira mensagem com a causa, não "falhou".
#[tokio::test]
async fn recusa_do_github_traz_a_mensagem_do_servico() {
    let (base, _rx) = sobe_stub_pr(
        422,
        r#"{"message": "A pull request already exists for feature/x."}"#,
    );
    let engine = GithubEngine::com_base_url(token(), matriz_permissiva(), base);

    let erro = engine
        .criar_pr(&repo(), "feature/x", "main", "t", "c")
        .await
        .expect_err("422 tem que virar erro");

    match erro {
        GithubError::Recusa { status, mensagem } => {
            assert_eq!(status, 422);
            assert!(mensagem.contains("already exists"), "mensagem: {mensagem}");
        }
        outro => panic!("esperava Recusa, veio {outro:?}"),
    }
}

// ---------------------------------------------------------------------
// Negações da matriz
// ---------------------------------------------------------------------

/// **Negação:** repositório fora da matriz é recusado **antes** de
/// qualquer rede.
///
/// O stub não é sequer iniciado: se a autorização acontecesse depois
/// da chamada HTTP, este teste falharia por timeout em vez de passar.
#[tokio::test]
async fn github_rejects_repo_outside_matrix() {
    let engine = GithubEngine::com_base_url(
        token(),
        matriz_permissiva(),
        "http://127.0.0.1:1", // porta inválida de propósito
    );
    let outro = RepoRef::parse("outra-pessoa/repositorio-alheio").expect("repo");

    let erro = engine
        .criar_pr(&outro, "feature/x", "main", "t", "c")
        .await
        .expect_err("repositório fora da matriz tem que ser recusado");

    assert!(
        matches!(
            &erro,
            GithubError::Autorizacao(MatrizError::RepoForaDaMatriz(r))
                if r == "outra-pessoa/repositorio-alheio"
        ),
        "veio {erro:?}"
    );
}

/// **Negação:** matriz vazia nega tudo. É o default.
#[test]
fn matriz_vazia_nega_tudo() {
    let m = MatrizAutorizacao::vazia();
    for op in [Operacao::Ler, Operacao::Push, Operacao::CriarPr] {
        assert!(m.autoriza(&repo(), op, Some("feature/x")).is_err());
    }
}

/// **Negação — a regra que protege a branch principal:** curinga não
/// alcança `main` nem `master`.
///
/// Sem isso, um `*` digitado para liberar branches de trabalho
/// passaria a cobrir a branch de produção sem ninguém perceber. A
/// menção nominal é o consentimento.
#[test]
fn curinga_nao_alcanca_branch_protegida() {
    let m = MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: repo(),
        branches: vec!["*".into()],
        operacoes: ops(&[Operacao::Push]),
    }]);

    for protegida in ["main", "master"] {
        let erro = m
            .autoriza(&repo(), Operacao::Push, Some(protegida))
            .expect_err("curinga não pode alcançar branch protegida");
        assert!(matches!(erro, MatrizError::BranchNegada { .. }));
    }

    // Controle positivo: o curinga funciona para o resto.
    m.autoriza(&repo(), Operacao::Push, Some("feature/x"))
        .expect("curinga vale para branch comum");

    // E a menção nominal libera.
    let nominal = MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: repo(),
        branches: vec!["main".into()],
        operacoes: ops(&[Operacao::Push]),
    }]);
    nominal
        .autoriza(&repo(), Operacao::Push, Some("main"))
        .expect("menção nominal autoriza");
}

/// **Negação:** operação ausente da regra é negada, mesmo com o
/// repositório e a branch certos.
#[test]
fn operacao_ausente_e_negada() {
    let m = MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: repo(),
        branches: vec!["feature/*".into()],
        operacoes: ops(&[Operacao::Ler]),
    }]);

    m.autoriza(&repo(), Operacao::Ler, Some("feature/x"))
        .expect("ler está autorizado");

    for negada in [Operacao::Push, Operacao::CriarPr] {
        assert!(matches!(
            m.autoriza(&repo(), negada, Some("feature/x")),
            Err(MatrizError::OperacaoNegada { .. })
        ));
    }
}

/// A interseção só restringe — nunca soma.
#[test]
fn intersecao_e_fail_closed() {
    let usuario = MatrizAutorizacao::com_regras(vec![
        RegraRepo {
            repo: repo(),
            branches: vec!["main".into(), "feature/*".into()],
            operacoes: ops(&[Operacao::Ler, Operacao::Push, Operacao::CriarPr]),
        },
        RegraRepo {
            repo: RepoRef::parse("eu/outro").expect("repo"),
            branches: vec!["main".into()],
            operacoes: ops(&[Operacao::Push]),
        },
    ]);
    let projeto = MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: repo(),
        branches: vec!["feature/*".into()],
        operacoes: ops(&[Operacao::Ler, Operacao::Push]),
    }]);

    let efetiva = usuario.intersecao(&projeto);

    // Repositório que só existe de um lado sai inteiro.
    assert_eq!(efetiva.regras().len(), 1);
    assert!(efetiva
        .autoriza(
            &RepoRef::parse("eu/outro").expect("repo"),
            Operacao::Push,
            Some("main")
        )
        .is_err());

    // Branch que só existe de um lado sai.
    assert!(efetiva
        .autoriza(&repo(), Operacao::Push, Some("main"))
        .is_err());
    efetiva
        .autoriza(&repo(), Operacao::Push, Some("feature/x"))
        .expect("branch comum aos dois sobrevive");

    // Operação que só existe de um lado sai.
    assert!(efetiva
        .autoriza(&repo(), Operacao::CriarPr, Some("feature/x"))
        .is_err());

    // E a interseção com a vazia zera.
    assert!(efetiva.intersecao(&MatrizAutorizacao::vazia()).esta_vazia());
}

/// **Negação:** `owner/repo` mal formado não vira referência.
#[test]
fn repo_mal_formado_e_recusado() {
    for texto in ["", "sembarra", "owner/", "/repo", "a/b/c", " / "] {
        assert!(
            matches!(RepoRef::parse(texto), Err(MatrizError::RepoMalFormado(_))),
            "{texto:?} deveria ser recusado"
        );
    }
    RepoRef::parse("owner/repo").expect("formato válido tem que passar");
}

/// **Negação estrutural — o [ADR-0041] §D3.**
///
/// Force-push não é opção com aprovação reforçada: é ausência de
/// API. Este teste falha se alguém acrescentar o `+` no refspec, um
/// parâmetro `force`, ou o `Remote::push` com refspec vindo de fora.
///
/// [ADR-0041]: ../../../docs/decisions/0041-github-auth-e-matriz-de-autorizacao.md
#[test]
fn github_has_no_force_push_api() {
    let arquivo = include_str!("../src/lib.rs");
    // Corta no módulo de teste: ele contém
    // `refspec_nao_tem_prefixo_de_force`, cujo **nome** casaria com a
    // busca. Varrer o teste que prova a regra e acusá-lo de violá-la
    // seria o teste se mordendo.
    let fonte = match arquivo.find("#[cfg(test)]") {
        Some(i) => &arquivo[..i],
        None => arquivo,
    };
    let codigo: String = fonte
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");

    for proibido in ["force", "+refs/", "\"+\""] {
        assert!(
            !codigo.contains(proibido),
            "`{proibido}` apareceu no código do github-engine — o ADR-0041 §D3 \
             proíbe force-push, e a proibição é por ausência de API"
        );
    }

    // O refspec é montado aqui, com prefixo fixo, e não vem de fora.
    assert!(
        codigo.contains(r#"format!("refs/heads/{branch}:refs/heads/{branch}")"#),
        "o refspec deixou de ser montado literalmente — se ele passar a vir \
         de parâmetro, o force volta pela porta dos fundos"
    );

    // Controle positivo: o stripper não comeu o código, e o
    // doc-comment que cita a proibição continua no arquivo.
    assert!(codigo.contains("git2::Repository::open"));
    assert!(
        fonte.contains("force-push"),
        "o doc-comment que declara a proibição sumiu do lib.rs"
    );
}

// ---------------------------------------------------------------------
// E2E noturno (ADR-0041 §D5) — twin acima é quem roda em todo PR
// ---------------------------------------------------------------------

/// Cria um PR **de verdade** no GitHub.
///
/// `#[ignore]` por natureza: rede, secret e serviço externo (REGRA
/// §3.3). Roda no `CI Nightly`. O twin
/// `github_create_pr_against_local_stub` cobre o mesmo caminho de
/// produção em todo PR — e é ele que a REGRA §3.3 exige para a fase
/// poder ser promovida.
///
/// Exige `GITHUB_TOKEN_E2E` e `GITHUB_REPO_E2E` (`owner/repo`). Sem
/// eles, **falha** em vez de pular: um teste de cobertura que pula
/// por ausência daquilo que deveria testar é fail-open com outra
/// roupa — a lição que renomeou o `memory_real_providers_or_skip`
/// para `_or_fail` na PR #63.
#[tokio::test]
#[ignore = "noturno: exige GITHUB_TOKEN_E2E e GITHUB_REPO_E2E"]
async fn github_create_pr_against_real_service() {
    let token_real = std::env::var("GITHUB_TOKEN_E2E")
        .expect("GITHUB_TOKEN_E2E ausente — teste de cobertura não pula por falta do que testa");
    let repo_texto =
        std::env::var("GITHUB_REPO_E2E").expect("GITHUB_REPO_E2E ausente (formato owner/repo)");
    let alvo = RepoRef::parse(&repo_texto).expect("GITHUB_REPO_E2E mal formado");

    let matriz = MatrizAutorizacao::com_regras(vec![RegraRepo {
        repo: alvo.clone(),
        branches: vec!["e2e/*".into()],
        operacoes: ops(&[Operacao::CriarPr]),
    }]);
    let engine = GithubEngine::new(SecretString::from(token_real), matriz);

    // A branch `e2e/noturno` precisa existir no repositório alvo.
    let resultado = engine
        .criar_pr(
            &alvo,
            "e2e/noturno",
            "main",
            "E2E noturno — PR criado pelo app",
            "Criado pelo `github_create_pr_against_real_service`. Pode fechar.",
        )
        .await;

    match resultado {
        Ok(pr) => assert!(pr.numero > 0, "PR sem número: {pr:?}"),
        // PR já aberto para a mesma branch é o estado normal da
        // segunda noite em diante, e não é falha do caminho.
        Err(GithubError::Recusa {
            status: 422,
            mensagem,
        }) if mensagem.contains("already exists") => {}
        Err(e) => panic!("criação de PR falhou: {e}"),
    }
}

// ---------------------------------------------------------------------
// Push — twin contra repositório bare local
// ---------------------------------------------------------------------

/// Twin do `push`: empurra para um repositório **bare de verdade** no
/// disco, via `file://`.
///
/// O que isto prova: a autorização roda antes, o refspec montado aqui
/// funciona, e o commit chega ao outro lado. O que **não** prova: o
/// callback de credencial, que só existe no transporte HTTPS — está
/// declarado como limitação no `docs/modules/github-engine.md`, e é
/// o `github_create_pr_against_real_service` noturno que o exercita.
fn repo_local_com_remoto(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bare = dir.join("remoto.git");
    git2::Repository::init_bare(&bare).expect("bare");

    let trabalho = dir.join("trabalho");
    std::fs::create_dir_all(&trabalho).expect("mkdir");
    let repo = git2::Repository::init(&trabalho).expect("init");
    std::fs::write(trabalho.join("a.txt"), "conteudo\n").expect("escrever");

    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .expect("add");
    index.write().expect("write index");
    let arvore = repo
        .find_tree(index.write_tree().expect("tree"))
        .expect("find tree");
    let sig = git2::Signature::now("Frederico", "frederico@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "primeiro", &arvore, &[])
        .expect("commit");

    // Branch de trabalho, que é a que a matriz autoriza.
    let head = repo.head().expect("head").peel_to_commit().expect("commit");
    repo.branch("feature/entrega", &head, false)
        .expect("branch");
    // Caminho local direto, e não `file://`: no Windows o
    // `file://C:/...` faz o libgit2 ler `C:` como host e recusar. O
    // transporte local aceita o caminho cru, que é o que o `git` também
    // aceita em `git remote add origin C:\caminho`.
    let url = bare.to_string_lossy().to_string();
    repo.remote("origin", &url).expect("remote");

    (trabalho, bare)
}

/// **Negação — o [ADR-0048] §D4.**
///
/// A matriz autoriza `owner/repo`, mas o push vai para onde o remoto
/// apontar. Sem esta conferência, um remoto trocado empurraria para
/// outro lugar carregando a autorização do repositório certo — e
/// `.git/config` fica no workspace, onde o agente escreve.
///
/// Este teste era o twin do push até 2026-08-19. Ele deixou de poder
/// empurrar de verdade porque o remoto local **não** é `github.com`,
/// que é exatamente o que a proteção nova recusa. A mecânica do push
/// passou a ser exercitada no teste de unidade
/// `mecanica_do_push_entrega_a_branch_ao_remoto`, que alcança a
/// função privada sem abrir porta que contorne a política.
///
/// [ADR-0048]: ../../../docs/decisions/0048-superficie-de-ferramentas-de-marco-e-github.md
#[tokio::test]
async fn push_recusa_remoto_que_nao_e_o_repositorio_autorizado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (trabalho, bare) = repo_local_com_remoto(tmp.path());
    let engine = GithubEngine::com_base_url(token(), matriz_permissiva(), "http://127.0.0.1:1");

    let erro = engine
        .push(&trabalho, &repo(), "feature/entrega", "origin")
        .await
        .expect_err("remoto que não é o repositório autorizado tem que ser recusado");

    match &erro {
        GithubError::RemotoNaoCorresponde { esperado, .. } => {
            assert_eq!(esperado, "fredabsd-svg/Frederico-IA-Studio-review");
        }
        outro => panic!("esperava RemotoNaoCorresponde, veio {outro:?}"),
    }

    // E nada chegou ao remoto.
    let remoto = git2::Repository::open_bare(&bare).expect("abrir bare");
    assert!(remoto.find_reference("refs/heads/feature/entrega").is_err());
}

/// **Negação:** push para branch fora da matriz não toca a rede.
#[tokio::test]
async fn push_para_branch_nao_autorizada_e_recusado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (trabalho, bare) = repo_local_com_remoto(tmp.path());
    let engine = GithubEngine::com_base_url(token(), matriz_permissiva(), "http://127.0.0.1:1");

    // `main` existe localmente, mas a matriz só autoriza `feature/*`
    // — e curinga não alcança branch protegida.
    let erro = engine
        .push(&trabalho, &repo(), "master", "origin")
        .await
        .expect_err("branch protegida sem menção nominal tem que ser recusada");
    assert!(
        matches!(
            &erro,
            GithubError::Autorizacao(MatrizError::BranchNegada { .. })
        ),
        "veio {erro:?}"
    );

    // E nada chegou ao remoto.
    let remoto = git2::Repository::open_bare(&bare).expect("abrir bare");
    assert!(remoto.find_reference("refs/heads/master").is_err());
}

/// **Negação:** branch que não existe localmente falha antes da rede,
/// com o nome no erro.
#[tokio::test]
async fn push_de_branch_inexistente_falha_antes_da_rede() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (trabalho, _bare) = repo_local_com_remoto(tmp.path());
    let engine = GithubEngine::com_base_url(token(), matriz_permissiva(), "http://127.0.0.1:1");

    let erro = engine
        .push(&trabalho, &repo(), "feature/nao-existe", "origin")
        .await
        .expect_err("branch inexistente tem que falhar");
    assert!(
        matches!(&erro, GithubError::BranchLocalInexistente(b) if b == "feature/nao-existe"),
        "veio {erro:?}"
    );
}
