//! Critério de saída do spike da Etapa 3 (ADR-0040 §D2): um commit
//! real num repositório temporário, lido de volta.
//!
//! O que a medição do spike acrescentou ao critério original está no
//! [ADR-0047]: ler o próprio commit de volta pela mesma biblioteca
//! que o escreveu **não basta**. A `gix` passava nesse crivo e ainda
//! assim deixava o repositório num estado que o `git` real lê como
//! "o arquivo commitado foi apagado", porque não escrevia o
//! `.git/index`. Por isso o teste de roundtrip aqui também confere o
//! índice.

use frederico_git_engine::{Autor, GitError, GitRepo};
use std::fs;

fn autor() -> Autor {
    Autor {
        nome: "Frederico".into(),
        email: "frederico@example.com".into(),
    }
}

/// Caminho feliz: escreve dois commits e os lê de volta.
#[test]
fn git_commit_then_log_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = GitRepo::iniciar(dir.path()).expect("init");

    fs::write(dir.path().join("a.txt"), "linha um\n").expect("escrever a.txt");
    let c1 = repo
        .commitar("primeiro commit", &autor())
        .expect("commit 1");
    assert_eq!(c1.resumo, "primeiro commit");
    assert_eq!(c1.pais, 0, "o primeiro commit não tem pai");

    fs::write(dir.path().join("a.txt"), "linha um\nlinha dois\n").expect("editar a.txt");
    fs::write(dir.path().join("b.txt"), "novo\n").expect("escrever b.txt");
    let c2 = repo.commitar("segundo commit", &autor()).expect("commit 2");
    assert_eq!(c2.pais, 1, "o segundo commit tem exatamente um pai");
    assert_ne!(c1.id, c2.id);

    let hist = repo.historico(10).expect("histórico");
    let resumos: Vec<&str> = hist.iter().map(|c| c.resumo.as_str()).collect();
    assert_eq!(resumos, vec!["segundo commit", "primeiro commit"]);
    assert_eq!(hist[0].autor, "Frederico");
}

/// **A prova que o spike acrescentou.** Depois do commit, o
/// `.git/index` existe e descreve os arquivos commitados.
///
/// Sem isso, o objeto de commit é válido e o repositório é inútil:
/// `git status` de qualquer outro cliente mostra os arquivos como
/// apagados do índice e não rastreados na árvore. Foi exatamente o
/// que a `gix` produziu no spike (ADR-0047 §Medição).
#[test]
fn git_commit_escreve_o_indice_e_nao_so_o_objeto() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = GitRepo::iniciar(dir.path()).expect("init");
    fs::write(dir.path().join("a.txt"), "conteudo\n").expect("escrever");
    repo.commitar("commit com índice", &autor())
        .expect("commit");

    let indice = dir.path().join(".git").join("index");
    assert!(
        indice.exists(),
        "o commit não escreveu .git/index — o repositório fica quebrado \
         para qualquer outro cliente Git (ADR-0047)"
    );

    // O índice tem de casar com a árvore do HEAD: nada pendente.
    let reaberto = GitRepo::abrir(dir.path()).expect("reabrir");
    let erro = reaberto.commitar("nada mudou", &autor());
    assert!(
        matches!(erro, Err(GitError::NadaParaCommitar)),
        "com índice e árvore em dia, um commit sem mudança deve ser \
         recusado; veio {erro:?}"
    );
}

/// **Negação.** Caminho que não é repositório é recusado com erro
/// nomeado, não com pânico nem com criação silenciosa de repositório.
#[test]
fn git_abrir_recusa_caminho_que_nao_e_repositorio() {
    let dir = tempfile::tempdir().expect("tempdir");
    let erro = GitRepo::abrir(dir.path()).err();
    assert!(
        matches!(erro, Some(GitError::NaoEhRepositorio(_))),
        "esperava NaoEhRepositorio, veio {erro:?}"
    );
    assert!(
        !dir.path().join(".git").exists(),
        "abrir não pode criar repositório"
    );
}

/// **Negação, e a que protege a decisão.** O crate não pode spawnar
/// processo — se alguém reintroduzir `Command::new("git")`, o Git
/// passa a rodar fora do sandbox da Fase 7 (ADR-0040 §D1 ponto 3).
///
/// A verificação é sobre o texto do fonte, e é deliberado: uma
/// asserção comportamental não pega o caso em que o `Command` só é
/// alcançado por um caminho de erro raro. Regra que só vive em prosa
/// é regra que volta na primeira urgência.
///
/// **A primeira versão deste teste falhou contra a própria
/// documentação:** o doc-comment do `lib.rs` cita
/// `Command::new("git")` justamente para dizer que é proibido, e a
/// busca no texto cru não distingue código de comentário. Por isso o
/// comentário é removido antes da busca — um guard que não sabe ler
/// o que guarda vira ruído, e guard ruidoso é desligado.
#[test]
fn git_has_no_process_spawn() {
    let fonte = include_str!("../src/lib.rs");
    let codigo: String = fonte
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        );

    for p in ["std::process", "Command::new", "process::Command"] {
        assert!(
            !codigo.contains(p),
            "`{p}` apareceu no código de git-engine/src/lib.rs — o              ADR-0040 §D1 proíbe processo externo neste crate"
        );
    }

    // Controles positivos: a leitura funciona e o stripper não comeu
    // o código junto com o comentário.
    assert!(
        codigo.contains("git2::Repository"),
        "o controle falhou — a leitura do fonte não está funcionando"
    );
    assert!(
        fonte.contains("Command::new"),
        "o controle falhou — o doc-comment que cita a proibição sumiu          do lib.rs, e com ele o caso que este teste precisa distinguir"
    );
}
