//! Testes das operações de leitura e de branch (PR de implementação
//! da Etapa 3). O caminho de escrita está em `spike_escrita_real.rs`,
//! que é o critério de saída do spike e não se mistura com estes.

use std::fs;

use frederico_git_engine::{Autor, EstadoArquivo, GitError, GitRepo};

fn autor() -> Autor {
    Autor {
        nome: "Frederico".into(),
        email: "frederico@example.com".into(),
    }
}

/// Repositório com um commit e um arquivo `a.txt` rastreado.
fn repo_com_commit(dir: &std::path::Path) -> GitRepo {
    let repo = GitRepo::iniciar(dir).expect("iniciar");
    fs::write(dir.join("a.txt"), "linha um\n").expect("escrever");
    repo.commitar("primeiro commit", &autor()).expect("commit");
    repo
}

#[test]
fn git_status_distingue_rastreado_de_nao_rastreado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());

    fs::write(tmp.path().join("a.txt"), "linha um\nlinha dois\n").expect("modificar");
    fs::write(tmp.path().join("b.txt"), "novo\n").expect("criar");

    let status = repo.status().expect("status");
    assert_eq!(status.len(), 2, "esperava 2 mudanças, veio {status:?}");

    assert_eq!(status[0].caminho, "a.txt");
    assert_eq!(status[0].estado, EstadoArquivo::Modificado);
    assert!(!status[0].staged, "a.txt não foi para o índice");

    assert_eq!(status[1].caminho, "b.txt");
    assert_eq!(
        status[1].estado,
        EstadoArquivo::NaoRastreado,
        "arquivo que a IA acabou de criar precisa aparecer; se sumir do \
         status, ele é indistinguível de um arquivo que não foi criado"
    );
}

#[test]
fn git_status_de_arvore_limpa_e_vazio() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());
    // Controle positivo do teste acima: sem mudança, a lista é
    // vazia — e não "vazia porque o status não enxerga nada".
    assert!(repo.status().expect("status").is_empty());
}

#[test]
fn git_diff_mostra_a_linha_acrescentada() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());
    fs::write(tmp.path().join("a.txt"), "linha um\nlinha dois\n").expect("modificar");

    let patch = repo.diff(false).expect("diff");
    assert!(patch.contains("+linha dois"), "patch veio: {patch}");
    assert!(
        !patch.contains("+linha um"),
        "linha que não mudou não pode aparecer como acréscimo: {patch}"
    );
}

#[test]
fn git_diff_staged_e_worktree_respondem_perguntas_diferentes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());
    fs::write(tmp.path().join("a.txt"), "linha um\nlinha dois\n").expect("modificar");

    // Nada foi para o índice: o que entraria no commit é vazio, e o
    // que ficaria de fora tem a linha nova. É essa assimetria que
    // justifica o booleano do spec em vez de um diff só.
    assert!(repo.diff(true).expect("diff staged").is_empty());
    assert!(repo
        .diff(false)
        .expect("diff worktree")
        .contains("+linha dois"));
}

#[test]
fn git_branch_cria_troca_e_lista() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());

    let criado = repo.criar_branch("trabalho", true).expect("criar branch");
    assert_eq!(criado.nome, "trabalho");
    assert_eq!(repo.branch_atual().as_deref(), Some("trabalho"));

    let branches = repo.branches().expect("listar");
    let nomes: Vec<&str> = branches.iter().map(|b| b.nome.as_str()).collect();
    assert!(nomes.contains(&"trabalho"), "veio {nomes:?}");
    assert_eq!(
        branches.iter().filter(|b| b.atual).count(),
        1,
        "exatamente um branch é o corrente"
    );
}

#[test]
fn git_branch_recusa_nome_repetido_e_branch_inexistente() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path());
    let antes = repo.branch_atual();
    repo.criar_branch("trabalho", false).expect("criar");

    // **Negação 1:** criar duas vezes o mesmo nome não pode silenciar
    // nem sobrescrever o branch existente.
    assert!(matches!(
        repo.criar_branch("trabalho", false),
        Err(GitError::BranchJaExiste(n)) if n == "trabalho"
    ));

    // **Negação 2:** trocar para branch que não existe falha com o
    // nome na mensagem, em vez de deixar o HEAD em estado estranho.
    assert!(matches!(
        repo.trocar_branch("nao-existe"),
        Err(GitError::BranchNaoExiste(n)) if n == "nao-existe"
    ));
    assert_eq!(
        repo.branch_atual(),
        antes,
        "HEAD não pode ter se movido depois de uma troca recusada"
    );
}

#[test]
fn git_branch_sem_commit_recusa_em_vez_de_panicar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = GitRepo::iniciar(tmp.path()).expect("iniciar");
    // **Negação:** repositório recém-criado não tem HEAD resolvível.
    // O erro precisa dizer isso, não estourar no `unwrap` interno.
    assert!(matches!(
        repo.criar_branch("trabalho", false),
        Err(GitError::SemCommit)
    ));
}

/// **Negação de fuga do Jail — a que o spec nomeia.**
///
/// O `git2` oferece `Repository::discover`, que sobe diretórios até
/// achar um `.git`. Usá-la faria o workspace da conversa operar sobre
/// o repositório do **pai** — que fora do Jail é qualquer coisa,
/// inclusive o repositório do próprio usuário.
///
/// Este teste fixa que `abrir` recusa. O controle positivo na mesma
/// função impede que ele passe por acidente (por exemplo, se `abrir`
/// passasse a recusar tudo).
#[test]
fn git_rejects_path_outside_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pai = tmp.path();
    GitRepo::iniciar(pai).expect("repo do pai");

    let workspace = pai.join("workspace-da-conversa");
    fs::create_dir(&workspace).expect("criar subdir");

    // Negação: subdiretório não é repositório, mesmo com um `.git`
    // logo acima.
    // `match` e não `expect_err`: o `GitRepo` guarda um
    // `git2::Repository`, que não implementa `Debug` — e derivar
    // `Debug` no nosso tipo só para o teste vazaria a biblioteca
    // pela fronteira que o ADR-0047 §D4 quer manter fechada.
    let erro = match GitRepo::abrir(&workspace) {
        Ok(_) => panic!("abrir subiu diretório e achou o repositório do pai"),
        Err(e) => e,
    };
    assert!(
        matches!(erro, GitError::NaoEhRepositorio(p) if p == workspace),
        "esperava NaoEhRepositorio no caminho do workspace"
    );

    // Controle positivo: no diretório que é repositório de verdade,
    // `abrir` funciona.
    GitRepo::abrir(pai).expect("o pai é repositório e abre");
}
