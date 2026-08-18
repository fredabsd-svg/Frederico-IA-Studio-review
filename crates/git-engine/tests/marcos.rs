//! Testes das primitivas de marco (tags anotadas) — Etapa 4 da
//! Fase 8, [ADR-0042](../../../docs/decisions/0042-projetos-e-checkpoints-nomeados.md).

use std::fs;

use frederico_git_engine::{Autor, GitError, GitRepo};

fn autor() -> Autor {
    Autor {
        nome: "Frederico".into(),
        email: "frederico@example.com".into(),
    }
}

fn repo_com_commit(dir: &std::path::Path, conteudo: &str) -> GitRepo {
    let repo = GitRepo::iniciar(dir).expect("iniciar");
    fs::write(dir.join("a.txt"), conteudo).expect("escrever");
    repo.commitar("primeiro commit", &autor()).expect("commit");
    repo
}

#[test]
fn marco_criado_aparece_na_listagem_com_o_commit_certo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "versao um\n");

    let criado = repo
        .criar_tag("v1-entrega", "primeira entrega ao cliente", &autor())
        .expect("criar marco");

    let tags = repo.tags().expect("listar");
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].nome, "v1-entrega");
    assert_eq!(tags[0].commit_id, criado.commit_id);
    assert_eq!(
        tags[0].mensagem.trim(),
        "primeira entrega ao cliente",
        "a mensagem do marco é dado do usuário e tem que voltar inteira"
    );
}

/// **Negação:** nome que o `git` do usuário não conseguiria ler é
/// recusado na criação, e não depois.
///
/// O `git2` aceita alguns desses e o `git` recusa na leitura — criar
/// a referência assim produziria um marco que só o app enxerga.
#[test]
fn marco_com_nome_invalido_e_recusado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "x\n");

    for nome in [
        "",
        "  ",
        "com espaço",
        "til~aqui",
        "dois..pontos",
        "-comeca-com-traco",
    ] {
        let r = repo.criar_tag(nome, "msg", &autor());
        assert!(
            matches!(r, Err(GitError::NomeDeTagInvalido(_))),
            "nome {nome:?} deveria ser recusado"
        );
    }

    // Controle positivo: um nome comum passa.
    repo.criar_tag("v1.0-final", "ok", &autor())
        .expect("nome válido tem que passar");
}

/// **Negação:** dois marcos com o mesmo nome não podem existir, e o
/// segundo não pode sobrescrever o primeiro em silêncio.
#[test]
fn marco_com_nome_repetido_e_recusado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "x\n");
    repo.criar_tag("marco", "primeiro", &autor())
        .expect("criar");

    let r = repo.criar_tag("marco", "segundo", &autor());
    assert!(matches!(r, Err(GitError::TagJaExiste(n)) if n == "marco"));

    // O primeiro continua intacto.
    assert_eq!(repo.tag("marco").expect("ler").mensagem.trim(), "primeiro");
}

/// **Negação:** repositório sem commit não tem o que marcar.
#[test]
fn marco_sem_commit_recusa_em_vez_de_panicar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = GitRepo::iniciar(tmp.path()).expect("iniciar");
    assert!(matches!(
        repo.criar_tag("marco", "msg", &autor()),
        Err(GitError::SemCommit)
    ));
}

/// **O contrato central do ADR-0042 §D3: restaurar não descarta
/// nada.**
///
/// Restaurar cria um commit novo com a árvore do marco, em vez de
/// mover o `HEAD` para trás. O teste confere as duas metades: o
/// conteúdo volta, **e** o commit que havia depois do marco continua
/// no histórico.
#[test]
fn restaurar_marco_traz_o_conteudo_de_volta_sem_apagar_historico() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "versao um\n");
    repo.criar_tag("v1", "estado bom", &autor()).expect("marco");

    // Trabalho depois do marco, commitado.
    fs::write(tmp.path().join("a.txt"), "versao dois quebrada\n").expect("modificar");
    repo.commitar("mudança que deu errado", &autor())
        .expect("commit 2");

    let restaurado = repo.restaurar_tag("v1", &autor()).expect("restaurar");

    // Metade 1: o conteúdo do arquivo voltou ao do marco.
    //
    // A comparação normaliza CRLF, e o motivo importa: com
    // `core.autocrlf=true` — o que vem de fábrica no Git for Windows
    // e o que está nesta máquina —, o checkout materializa o arquivo
    // com CRLF enquanto o blob no repositório guarda LF. Restaurar
    // devolve **o conteúdo**, não os bytes exatos do arquivo de
    // origem, e é exatamente o que o `git checkout` do usuário faz.
    // Asserir bytes crus quebraria em máquina com Git padrão.
    let conteudo = fs::read_to_string(tmp.path().join("a.txt")).expect("ler");
    assert_eq!(conteudo.replace("\r\n", "\n"), "versao um\n");

    // Metade 2: nada foi apagado. O commit ruim continua lá, e a
    // restauração é mais um commit em cima — não um `reset`.
    let historico = repo.historico(10).expect("log");
    let resumos: Vec<&str> = historico.iter().map(|c| c.resumo.as_str()).collect();
    assert_eq!(
        resumos,
        vec![
            "restaura o marco \"v1\"",
            "mudança que deu errado",
            "primeiro commit"
        ],
        "restaurar tem que somar ao histórico, não reescrevê-lo"
    );
    assert_eq!(restaurado.resumo, "restaura o marco \"v1\"");
    assert_eq!(restaurado.pais, 1);
}

/// **Negação:** restaurar por cima de trabalho não commitado é
/// recusado.
///
/// É o §D3 do ADR-0042 na prática: quem chama (o `project-engine`)
/// cria um marco automático antes. O motor não sobrescreve por conta
/// própria — se sobrescrevesse, a garantia dependeria de o caller
/// lembrar.
#[test]
fn restaurar_com_arvore_suja_e_recusado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "versao um\n");
    repo.criar_tag("v1", "estado bom", &autor()).expect("marco");
    fs::write(tmp.path().join("a.txt"), "versao dois\n").expect("commit 2 pendente");
    repo.commitar("segunda versão", &autor()).expect("commit");

    // Agora suja a árvore sem commitar.
    fs::write(tmp.path().join("rascunho.txt"), "trabalho em andamento\n").expect("sujar");

    let r = repo.restaurar_tag("v1", &autor());
    assert!(
        matches!(r, Err(GitError::ArvoreSujaNaRestauracao)),
        "restaurar com árvore suja tem que ser recusado, veio: {r:?}"
    );

    // E o rascunho continua onde estava.
    assert!(tmp.path().join("rascunho.txt").exists());
}

/// **Negação:** restaurar marco inexistente nomeia o que faltou.
#[test]
fn restaurar_marco_inexistente_e_recusado() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "x\n");
    assert!(matches!(
        repo.restaurar_tag("nao-existe", &autor()),
        Err(GitError::TagNaoExiste(n)) if n == "nao-existe"
    ));
}

/// Restaurar o marco que já é o estado corrente não gera commit
/// vazio — mesma regra do `commitar`.
#[test]
fn restaurar_marco_que_ja_e_o_estado_atual_nao_cria_commit_vazio() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_com_commit(tmp.path(), "x\n");
    repo.criar_tag("agora", "estado corrente", &autor())
        .expect("marco");

    assert!(matches!(
        repo.restaurar_tag("agora", &autor()),
        Err(GitError::NadaParaCommitar)
    ));
}
