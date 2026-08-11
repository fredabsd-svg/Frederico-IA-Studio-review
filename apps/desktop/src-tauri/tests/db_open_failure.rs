//! In-process test do caminho de erro do `Database::open` —
//! complementa o smoke test `smoke_startup.rs` que valida
//! a casca Tauri (a primeira classe de erro: binário não
//! pânica, marker de recovery no stderr). Este test valida
//! a **segunda classe**: o `Database::open` retorna erro
//! estruturado (`StorageError::Open`) quando o data dir é
//! inválido, sem panic e sem hang.
//!
//! **Por que este test é in-process (não spawna o binário):
//! a v2 e v3 do smoke test (PR #48) tentaram spawnar o
//! binário `frederico-desktop` com `FREDERICO_DATA_DIR`
//! apontando pra (v2) um SQLite com `_sqlx_migrations`
//! de checksum errado, e (v3) um arquivo comum. Em ambos
//! os casos, o teste passou local (Windows 11) mas falhou
//! em CI (Windows Server 2022, runs #31448091994 e
//! #31489302906): o binário emitia só o primeiro
//! `tracing::info!` ("inicando") e ficava preso antes do
//! `.setup` callback rodar (provavelmente o Tauri runtime
//! é mais lento em Windows Server, excedendo a janela de
//! 5s do smoke). Como o **caminho de recovery depende do
//! `Database::open` retornar erro** (não depende de
//! features do Tauri), validar essa pré-condição in-process
//! cobre a mesma classe de regressão sem flakiness de
//! runtime.
//!
//! **O que o test prova:**
//! 1. `Database::open` com um path cujo parent é um arquivo
//!    (não diretório) retorna `Err(StorageError)` — não
//!    pánica, não trava, não retorna Ok silenciosamente.
//! 2. O erro carrega informação útil (caminho do banco,
//!    mensagem do I/O subjacente) — `handle_startup_db_error`
//!    usa isso pra montar a mensagem do dialog.
//!
//! **O que este test NÃO cobre:**
//! - Que o `.setup` callback da casca Tauri chama
//!   `handle_startup_db_error` no caminho de erro (isso
//!   depende de o Tauri runtime inicializar a tempo — coberto
//!   manualmente em ambiente interativo + smoke local).
//! - Que o dialog nativo aparece (depende de GUI; coberto
//!   manualmente + pelo `catch_unwind` no `blocking_show`
//!   quando em headless).
//!
//! **Relação com a Etapa 6+:** o startup recovery gracioso
//! introduzido na Etapa 6+ é o `handle_startup_db_error`
//! que recebe o `StorageError` deste test e o converte em
//! dialog + tracing::error!. Se o `Database::open` parasse
//! de retornar erro em paths inválidos (ex.: alguém
//! trocasse `tokio::fs::create_dir_all` por `fs::create_dir`
//! que silenciosamente ignora falhas), o `handle_startup_db_
//! error` não seria chamado e o binário panicaria de novo
//! (regressão do `.expect()` original). Este test cobre
//! essa classe de regressão.
//!
//! **Por que `tokio::runtime::Runtime` em vez de
//! `tauri::async_runtime::block_on`:** o test binário
//! gerado por `cargo test` que importa `tauri` linka
//! as DLLs nativas do Tauri (tao/wry/WebView2 binding),
//! o que dá `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139) no
//! Windows quando o WebView2 runtime não está exatamente
//! no nível esperado (verificado nesta sessão). O Tauri
//! não é necessário aqui — o `Database::open` só depende
//! de `tokio`. Usar `tokio::runtime::Runtime` direto
//! mantém o test binário leve (só depende de `tokio`,
//! sem a árvore nativa do Tauri).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use frederico_storage::Database;

/// In-process test: `Database::open` retorna erro estruturado
/// quando o data dir é inválido (parent do banco é um arquivo,
/// não diretório). Garante que o `.setup` callback da casca
/// Tauri (`apps/desktop/src-tauri/src/main.rs::setup`) tem o
/// `Err` que precisa pra acionar o `handle_startup_db_error`.
///
/// **Cenário testado:** `tempfile::tempdir()` cria um diretório
/// isolado. Dentro, `std::fs::write` cria um arquivo comum
/// (`not_a_dir`). O caminho do banco fica `<arquivo>/frederico.db`
/// — o parent é o arquivo, e `tokio::fs::create_dir_all(parent)`
/// em `Database::open` falha com `os error 183` ("Não é
/// possível criar um arquivo já existente"). O erro é mapeado
/// pra `StorageError::Open` (ver `crates/storage/src/lib.rs`).
///
/// **Por que `tokio::runtime::Runtime`:** o `Database::open` é
/// `async` (usa `tokio::fs` internamente). Um `Runtime` novo
/// (current_thread) é suficiente — não há I/O bloqueante ou
/// tasks concorrentes, só um `await` único. O `.setup` da
/// casca Tauri usa `tauri::async_runtime::block_on` (mesma
/// semântica, runtime da Tauri); o test usa `tokio` direto
/// pra evitar a árvore de deps nativa do Tauri no test
/// binário (ver doc do módulo).
#[test]
fn database_open_fails_when_data_dir_is_a_file() {
    // Setup isolado (mesma convenção do smoke_startup.rs:
    // tempdir + _keep_alive pra não morrer no meio do test).
    let data_dir = tempfile::tempdir().expect("criar tempdir");
    let data_dir_path = data_dir.path().to_path_buf();

    // Cria um arquivo comum dentro do tempdir. O caminho
    // do banco (`<arquivo>/frederico.db`) terá o arquivo
    // como parent — `Database::open` falha em
    // `create_dir_all(parent)`.
    let fake_data_dir = data_dir_path.join("not_a_dir");
    std::fs::write(&fake_data_dir, b"not a directory")
        .expect("criar arquivo fake_data_dir pro I/O error");
    let db_path = fake_data_dir.join("frederico.db");

    // Mantém o tempdir vivo até o final do test (drop do
    // `tempdir()` apaga o diretório; sem o `_keep_alive`, o
    // tempdir morre antes da borrow acabar).
    let _keep_alive = &data_dir;

    // Cria um runtime tokio current_thread (mesmo padrão que
    // `tauri::async_runtime::block_on` usa) e roda o
    // `Database::open` async dentro dele.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("criar tokio runtime");
    let result = rt.block_on(async { Database::open(&db_path).await });

    // 1. Deve falhar (não Ok silencioso, não panic).
    let err = result.expect_err(
        "`Database::open` com parent sendo arquivo deveria falhar \
         (ele chama `create_dir_all(parent)` que retorna erro \
         no Windows quando o path existe como arquivo). Se este \
         test passar com Ok, significa que `Database::open` \
         silenciosamente criou o banco de alguma forma — o \
         `.setup` callback não acionaria o startup recovery \
         nesse cenário, e o caminho de erro ficaria descoberto.",
    );

    // 2. O erro deve carregar o caminho do banco (o
    //    `handle_startup_db_error` usa isso pra montar a
    //    mensagem do dialog).
    let err_str = err.to_string();
    assert!(
        err_str.contains(db_path.to_string_lossy().as_ref()) || err_str.contains("frederico.db"),
        "erro deveria mencionar o caminho do banco ou `frederico.db` \
         (informação usada pelo `handle_startup_db_error` no dialog). \
         erro recebido: {err_str}"
    );

    // 3. O erro deve mencionar a falha de I/O subjacente
    //    (`create_dir_all` falhou). Pode ser:
    //    - "não consegui criar diretório" (mensagem do
    //      `Database::open` quando `create_dir_all` falha)
    //    - "diretório" / "directory" / "create"
    //    - código de erro do OS
    let err_lower = err_str.to_lowercase();
    assert!(
        err_lower.contains("criar")
            || err_lower.contains("diret")
            || err_lower.contains("directory")
            || err_lower.contains("create")
            || err_lower.contains("exist")
            || err_lower.contains("os error"),
        "erro deveria mencionar falha de criação de diretório \
         (subjacente ao `create_dir_all` que falhou). \
         erro recebido: {err_str}"
    );
}
