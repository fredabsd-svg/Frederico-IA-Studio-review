//! Testes do `project-engine` — Etapa 4 da Fase 8.
//!
//! Os quatro nomes previstos em
//! `docs/architecture/project-and-milestones-architecture.md`
//! §"Testes previstos" estão aqui, mais as negações que a medição
//! acrescentou.

use std::fs;

use frederico_core::ProjectId;
use frederico_git_engine::{Autor, GitRepo};
use frederico_project_engine::{ProjectEngine, ProjectError};
use frederico_storage::Database;

fn autor() -> Autor {
    Autor {
        nome: "Frederico".into(),
        email: "frederico@example.com".into(),
    }
}

/// Banco em memória com as migrações aplicadas — inclusive a `0032`,
/// que é o que este crate exercita.
async fn banco() -> Database {
    Database::open_in_memory().await.expect("abrir banco")
}

/// Workspace já sob Git, com um commit.
fn workspace_com_git(dir: &std::path::Path, conteudo: &str) {
    let repo = GitRepo::iniciar(dir).expect("iniciar git");
    fs::write(dir.join("a.txt"), conteudo).expect("escrever");
    repo.commitar("estado inicial", &autor()).expect("commit");
}

#[tokio::test]
async fn project_open_and_list_roundtrip() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");

    let projeto = engine
        .abrir_projeto(tmp.path(), "Meu Projeto", None)
        .await
        .expect("abrir");
    assert_eq!(projeto.nome, "Meu Projeto");
    assert_eq!(projeto.caminho, tmp.path());

    let lista = engine.listar_projetos().await.expect("listar");
    assert_eq!(lista.len(), 1);
    assert_eq!(lista[0].id, projeto.id);
}

/// Reabrir o mesmo caminho não cria projeto duplicado — é o caso
/// comum (o usuário volta ao projeto de ontem), não um erro.
#[tokio::test]
async fn reabrir_o_mesmo_caminho_nao_duplica_projeto() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");

    let primeiro = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir 1");
    let segundo = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir 2");

    assert_eq!(primeiro.id, segundo.id, "é o mesmo projeto, não um novo");
    assert_eq!(engine.listar_projetos().await.expect("listar").len(), 1);
}

/// **Negação:** nome vazio é recusado antes de tocar o banco.
#[tokio::test]
async fn projeto_sem_nome_e_recusado() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");

    for nome in ["", "   "] {
        assert!(matches!(
            engine.abrir_projeto(tmp.path(), nome, None).await,
            Err(ProjectError::NomeVazio)
        ));
    }
    assert!(engine.listar_projetos().await.expect("listar").is_empty());
}

#[tokio::test]
async fn milestone_create_then_restore() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    workspace_com_git(tmp.path(), "versao boa\n");

    let projeto = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir");

    let marco = engine
        .criar_marco(
            projeto.id,
            "v1",
            "entrega ao cliente",
            &autor(),
            Some("conv-1"),
        )
        .await
        .expect("criar marco");
    assert_eq!(marco.nome, "v1");
    assert!(!marco.automatico);
    assert_eq!(marco.conversa_origem.as_deref(), Some("conv-1"));

    // A tag existe **no repositório**, não só no banco. É a metade
    // que sustenta "marco é um commit com nome, conferível com
    // `git log`" (ADR-0042 §D2).
    let repo = GitRepo::abrir(tmp.path()).expect("abrir repo");
    assert_eq!(repo.tags().expect("tags").len(), 1);

    // Trabalho depois do marco, commitado.
    fs::write(tmp.path().join("a.txt"), "versao ruim\n").expect("modificar");
    repo.commitar("mudança que deu errado", &autor())
        .expect("commit");

    let restauracao = engine
        .restaurar_marco(projeto.id, "v1", &autor())
        .await
        .expect("restaurar");

    let conteudo = fs::read_to_string(tmp.path().join("a.txt")).expect("ler");
    assert_eq!(conteudo.replace("\r\n", "\n"), "versao boa\n");
    assert!(
        restauracao.marco_automatico.is_none(),
        "árvore estava limpa; não havia trabalho pendente a salvar"
    );
}

/// **O §D3 do ADR-0042 na prática:** trabalho não commitado vira
/// marco automático antes da restauração, em vez de sumir.
#[tokio::test]
async fn restaurar_salva_trabalho_pendente_num_marco_automatico() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    workspace_com_git(tmp.path(), "versao boa\n");

    let projeto = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir");
    engine
        .criar_marco(projeto.id, "v1", "bom", &autor(), None)
        .await
        .expect("marco");

    // Trabalho pendente, **não** commitado — o caso em que um
    // `reset --hard` apagaria tudo sem aviso.
    fs::write(tmp.path().join("rascunho.txt"), "ideia importante\n").expect("rascunho");

    let restauracao = engine
        .restaurar_marco(projeto.id, "v1", &autor())
        .await
        .expect("restaurar");

    let auto = restauracao
        .marco_automatico
        .expect("tinha trabalho pendente, logo tem marco automático");
    assert!(auto.automatico, "o marco tem que se declarar automático");
    assert!(auto.nome.starts_with("auto-antes-de-v1-"));

    // A prova: o rascunho não sumiu — está no commit do marco
    // automático, recuperável pelo `git`.
    let repo = GitRepo::abrir(tmp.path()).expect("abrir repo");
    let tag = repo.tag(&auto.nome).expect("tag do marco automático");
    assert_eq!(tag.commit_id, auto.commit_id);

    let resumos: Vec<String> = repo
        .historico(10)
        .expect("log")
        .into_iter()
        .map(|c| c.resumo)
        .collect();
    assert!(
        resumos
            .iter()
            .any(|r| r.contains("automaticamente antes de restaurar")),
        "o commit do marco automático tem que aparecer no histórico: {resumos:?}"
    );
}

/// **Negação — a que o spec nomeia:** sem repositório Git, criar
/// marco é recusado com erro que explica, e nada é criado pela
/// metade.
#[tokio::test]
async fn milestone_requires_git_workspace() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    // Sem `GitRepo::iniciar` — workspace comum.

    let projeto = engine
        .abrir_projeto(tmp.path(), "Sem Git", None)
        .await
        .expect("abrir projeto funciona sem Git");

    let erro = engine
        .criar_marco(projeto.id, "v1", "tentativa", &autor(), None)
        .await
        .expect_err("marco sem Git tem que falhar");
    assert!(
        matches!(erro, ProjectError::WorkspaceSemGit { .. }),
        "esperava WorkspaceSemGit, veio {erro:?}"
    );

    // Nada pela metade: nenhum metadado ficou no banco.
    assert!(engine
        .listar_marcos(projeto.id)
        .await
        .expect("listar")
        .is_empty());
}

/// **Negação:** dois marcos com o mesmo nome no mesmo projeto.
#[tokio::test]
async fn marco_com_nome_repetido_e_recusado() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    workspace_com_git(tmp.path(), "x\n");

    let projeto = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir");
    engine
        .criar_marco(projeto.id, "v1", "primeiro", &autor(), None)
        .await
        .expect("criar");

    let erro = engine
        .criar_marco(projeto.id, "v1", "segundo", &autor(), None)
        .await
        .expect_err("nome repetido tem que falhar");
    assert!(matches!(erro, ProjectError::MarcoJaExiste(n) if n == "v1"));

    // E o primeiro continua intacto, com a descrição original.
    let marco = engine.marco(projeto.id, "v1").await.expect("ler");
    assert_eq!(marco.descricao, "primeiro");
}

/// **Negação:** restaurar marco inexistente não pode criar marco
/// automático pelo caminho — o alvo é verificado antes de qualquer
/// escrita.
#[tokio::test]
async fn restaurar_marco_inexistente_nao_deixa_lixo() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    workspace_com_git(tmp.path(), "x\n");

    let projeto = engine
        .abrir_projeto(tmp.path(), "Projeto", None)
        .await
        .expect("abrir");
    fs::write(tmp.path().join("pendente.txt"), "algo\n").expect("sujar");

    let erro = engine
        .restaurar_marco(projeto.id, "nao-existe", &autor())
        .await
        .expect_err("marco inexistente tem que falhar");
    assert!(matches!(erro, ProjectError::MarcoNaoEncontrado(n) if n == "nao-existe"));

    // Nenhum marco automático foi criado, e o pendente continua
    // pendente — a operação falhou sem efeito colateral.
    assert!(engine
        .listar_marcos(projeto.id)
        .await
        .expect("listar")
        .is_empty());
    assert!(tmp.path().join("pendente.txt").exists());
}

/// **Negação:** projeto inexistente.
#[tokio::test]
async fn operacao_em_projeto_inexistente_e_recusada() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let fantasma = ProjectId::new();

    assert!(matches!(
        engine.projeto(fantasma).await,
        Err(ProjectError::ProjetoNaoEncontrado)
    ));
    assert!(matches!(
        engine.criar_marco(fantasma, "v1", "", &autor(), None).await,
        Err(ProjectError::ProjetoNaoEncontrado)
    ));
}

/// Os marcos de um projeto não vazam para outro.
#[tokio::test]
async fn marcos_nao_vazam_entre_projetos() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let a = tempfile::tempdir().expect("tempdir a");
    let b = tempfile::tempdir().expect("tempdir b");
    workspace_com_git(a.path(), "a\n");
    workspace_com_git(b.path(), "b\n");

    let pa = engine.abrir_projeto(a.path(), "A", None).await.expect("a");
    let pb = engine.abrir_projeto(b.path(), "B", None).await.expect("b");

    engine
        .criar_marco(pa.id, "v1", "de A", &autor(), None)
        .await
        .expect("marco em A");

    assert_eq!(engine.listar_marcos(pa.id).await.expect("a").len(), 1);
    assert!(
        engine.listar_marcos(pb.id).await.expect("b").is_empty(),
        "marco de um projeto não pode aparecer em outro"
    );

    // O mesmo nome em projetos diferentes é permitido — a unicidade
    // é por projeto, não global.
    engine
        .criar_marco(pb.id, "v1", "de B", &autor(), None)
        .await
        .expect("mesmo nome em outro projeto tem que funcionar");
}

/// **Negação:** caminho que não existe não vira projeto.
///
/// Guarda de usabilidade, não de segurança — um caminho digitado
/// errado viraria linha permanente apontando para lugar nenhum.
#[tokio::test]
async fn projeto_com_caminho_inexistente_e_recusado() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let tmp = tempfile::tempdir().expect("tempdir");
    let inexistente = tmp.path().join("nao-existe");

    assert!(matches!(
        engine.abrir_projeto(&inexistente, "Projeto", None).await,
        Err(ProjectError::CaminhoInvalido(_))
    ));

    // Arquivo também não serve — projeto é diretório.
    let arquivo = tmp.path().join("arquivo.txt");
    fs::write(&arquivo, "x").expect("escrever");
    assert!(matches!(
        engine.abrir_projeto(&arquivo, "Projeto", None).await,
        Err(ProjectError::CaminhoInvalido(_))
    ));

    assert!(engine.listar_projetos().await.expect("listar").is_empty());
}

/// **O invariante que o spec quis nomear, corrigido.**
///
/// O spec prevê `project_path_stays_inside_jail`. Esse teste, como o
/// nome sugere, é incompatível com o [ADR-0042] §D4: o caminho de um
/// projeto é escolha do **usuário** e vive fora de qualquer jail — o
/// jail é resolvido por conversa (ADR-0022), não por projeto. Um
/// projeto obrigado a ficar dentro do jail seria um projeto que só
/// existe dentro da pasta da conversa, o que não é um projeto.
///
/// O invariante verdadeiro é o outro lado: **registrar um projeto não
/// amplia o alcance do agente**. Este teste fixa isso pelo que o
/// crate expõe — `Projeto.caminho` é dado, e não há API aqui que o
/// transforme em alcance. A outra metade da prova está na Etapa 3:
/// `nenhuma_ferramenta_de_git_aceita_caminho_de_repositorio` garante
/// que nenhuma ferramenta aceita caminho, e todas abrem
/// `ctx.jail.root()`.
///
/// [ADR-0042]: ../../../docs/decisions/0042-projetos-e-checkpoints-nomeados.md
#[tokio::test]
async fn abrir_projeto_nao_amplia_o_alcance_do_agente() {
    let db = banco().await;
    let engine = ProjectEngine::new(db.pool());
    let fora = tempfile::tempdir().expect("tempdir");
    workspace_com_git(fora.path(), "conteudo\n");

    // Um caminho arbitrário do disco, deliberadamente fora de
    // qualquer jail de conversa.
    let projeto = engine
        .abrir_projeto(fora.path(), "Projeto Fora do Jail", None)
        .await
        .expect("abrir projeto em caminho arbitrário é o comportamento correto");

    // O que o crate devolve é dado: caminho, nome, datas. Nenhum
    // `Jail`, nenhum resolvedor, nenhuma ferramenta.
    assert_eq!(projeto.caminho, fora.path());

    // E o marco criado por aqui vive no repositório daquele caminho,
    // porque foi o **usuário** que apontou para ele — não o agente.
    engine
        .criar_marco(projeto.id, "v1", "", &autor(), None)
        .await
        .expect("marco");
    let repo = GitRepo::abrir(fora.path()).expect("repo");
    assert_eq!(repo.tags().expect("tags").len(), 1);
}
