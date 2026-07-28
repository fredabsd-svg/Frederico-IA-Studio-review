//! Testes E2E do fluxo de **correção, expiração e retomada**
//! (Etapa 4 da Fase 4, `ADR-0014`).
//!
//! Cobre:
//! - `apply_correction` atômico (insere nova + marca antiga
//!   superseded na mesma transação).
//! - Filtro de expiração: `list_by_scope` exclui memórias com
//!   `expires_at` no passado, **exceto** se `user_pinned`.
//! - Filtro de supersedência: `list_by_scope` exclui memórias
//!   com `superseded_by IS NOT NULL`, **mesmo** se
//!   `user_pinned` (correção é mais forte que pin).
//! - `purge_expired` deleta as elegíveis e preserva pinned.

use chrono::{DateTime, Duration, Utc};
use frederico_core::{MemoryOrigin, MemoryScopeType, MemorySourceType, MemoryType};
use frederico_memory::memory_repo::NewMemoryInput;
use frederico_memory::MemoryRepo;
use frederico_storage::Database;

/// Helper: cria um `NewMemoryInput` de teste com origem User.
fn user_input(scope: &str, content: &str) -> NewMemoryInput {
    NewMemoryInput {
        scope_type: MemoryScopeType::Project,
        scope_id: scope.into(),
        type_: MemoryType::Fact,
        content: content.into(),
        origin: MemoryOrigin::User,
        source_type: MemorySourceType::new("user_message"),
        source_id: None,
        confidence: 0.9,
        importance: 0.5,
        expires_at: None,
        user_confirmed: true,
        user_pinned: false,
    }
}

/// Helper: cria um `NewMemoryInput` do tipo `temporary` com TTL
/// `days_from_now` dias no futuro (negativo = no passado).
fn temporary_input(scope: &str, content: &str, days_from_now: i64) -> NewMemoryInput {
    let expires_at = Utc::now() + Duration::days(days_from_now);
    NewMemoryInput {
        scope_type: MemoryScopeType::Project,
        scope_id: scope.into(),
        type_: MemoryType::Temporary,
        content: content.into(),
        origin: MemoryOrigin::User,
        source_type: MemorySourceType::new("user_message"),
        source_id: None,
        confidence: 0.9,
        importance: 0.5,
        expires_at: Some(expires_at),
        user_confirmed: true,
        user_pinned: false,
    }
}

// ===========================================================================
// apply_correction
// ===========================================================================

#[tokio::test]
async fn apply_correction_marks_old_as_superseded_and_creates_new() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // Memória antiga: "uso MySQL".
    let old = repo
        .insert_user_confirmed(user_input("proj-1", "uso MySQL"))
        .await
        .expect("insert old");
    assert!(old.superseded_by.is_none(), "old não começa superseded");

    // Substituta: "uso Postgres" (tipo Correction, mesma origem).
    let new_input = NewMemoryInput {
        type_: MemoryType::Correction,
        ..user_input("proj-1", "uso Postgres")
    };
    let now = Utc::now();
    let result = repo
        .apply_correction(&old.id, new_input, now)
        .await
        .expect("apply_correction");

    // 1. `old_id` devolvido é o da antiga.
    assert_eq!(result.old_id, old.id);

    // 2. `new_record` é a substituta, com `user_confirmed = true`
    //    e `pending_review = false` (correção é confirmação).
    assert_eq!(result.new_record.content, "uso Postgres");
    assert!(result.new_record.user_confirmed);
    assert!(!result.new_record.pending_review);
    assert_eq!(result.new_record.type_, MemoryType::Correction);
    assert_eq!(result.superseded_at, now);

    // 3. A antiga está superseded no banco.
    let old_now = repo.get(&old.id).await.expect("get old").unwrap();
    assert_eq!(old_now.superseded_by, Some(result.new_record.id));
    assert_eq!(old_now.superseded_at, Some(now));

    // 4. `list_by_scope` exclui a antiga (filtro de supersedência).
    let visible = repo
        .list_by_scope(MemoryScopeType::Project, "proj-1", now)
        .await
        .expect("list");
    let visible_contents: Vec<&str> = visible.iter().map(|r| r.content.as_str()).collect();
    assert!(
        !visible_contents.contains(&"uso MySQL"),
        "antiga superseded não pode aparecer no retrieval"
    );
    assert!(
        visible_contents.contains(&"uso Postgres"),
        "nova deve aparecer no retrieval"
    );
}

#[tokio::test]
async fn apply_correction_is_idempotent_for_old_when_already_superseded() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    let old = repo
        .insert_user_confirmed(user_input("proj-1", "uso MySQL"))
        .await
        .expect("insert old");

    let now = Utc::now();

    // Primeira correção: cria `correction_1` e marca `old` superseded.
    let correction_1 = repo
        .apply_correction(
            &old.id,
            NewMemoryInput {
                content: "uso Postgres v1".into(),
                ..user_input("proj-1", "x")
            },
            now,
        )
        .await
        .expect("correction 1");

    // Segunda correção na MESMA antiga: `old` já está
    // superseded → `mark_superseded` é no-op, mas o método
    // **ainda cria** uma nova (cada correção é uma memória
    // nova). A antiga continua apontando pra `correction_1`.
    let correction_2 = repo
        .apply_correction(
            &old.id,
            NewMemoryInput {
                content: "uso Postgres v2".into(),
                ..user_input("proj-1", "x")
            },
            now + Duration::seconds(1),
        )
        .await
        .expect("correction 2");

    assert_ne!(correction_1.new_record.id, correction_2.new_record.id);
    let old_now = repo.get(&old.id).await.expect("get old").unwrap();
    assert_eq!(old_now.superseded_by, Some(correction_1.new_record.id));
    // A `correction_1` em si NÃO foi marcada superseded por
    // `correction_2` (mark só atua em `old`).
    let c1 = repo
        .get(&correction_1.new_record.id)
        .await
        .expect("get c1")
        .unwrap();
    assert!(c1.superseded_by.is_none(), "c1 não foi tocada");
    assert!(c1.superseded_at.is_none());
}

#[tokio::test]
async fn apply_correction_with_invalid_input_rejects_before_opening_tx() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    let old = repo
        .insert_user_confirmed(user_input("proj-1", "uso MySQL"))
        .await
        .expect("insert old");

    // Replacement inválido: content vazio.
    let bad = NewMemoryInput {
        content: "   ".into(),
        ..user_input("proj-1", "x")
    };
    let result = repo.apply_correction(&old.id, bad, Utc::now()).await;
    assert!(result.is_err(), "content vazio deve rejeitar");

    // `old` continua intacta (a transação nem abriu).
    let old_now = repo.get(&old.id).await.expect("get old").unwrap();
    assert!(old_now.superseded_by.is_none());
    assert!(old_now.superseded_at.is_none());
}

// ===========================================================================
// Filtro de expiração
// ===========================================================================

#[tokio::test]
async fn list_by_scope_excludes_expired_temporary() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // Memória já expirada (`days_from_now = -1`).
    let _ = repo
        .insert_user_confirmed(temporary_input("proj-1", "vou viajar ontem", -1))
        .await
        .expect("insert expired");

    let now = Utc::now();
    let visible = repo
        .list_by_scope(MemoryScopeType::Project, "proj-1", now)
        .await
        .expect("list");
    assert!(
        visible.is_empty(),
        "memória expirada deve ser filtrada; visíveis = {visible:?}"
    );
}

#[tokio::test]
async fn list_by_scope_includes_pinned_even_if_expired() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // Memória expirada MAS pinned pelo usuário — `user_pinned`
    // bypassa `expires_at` (regra do `ADR-0014 §1`).
    let input = NewMemoryInput {
        user_pinned: true,
        ..temporary_input("proj-1", "lembrar até 2025-01-01", -100)
    };
    let pinned = repo
        .insert_user_confirmed(input)
        .await
        .expect("insert pinned");
    assert!(pinned.user_pinned);

    let now = Utc::now();
    let visible = repo
        .list_by_scope(MemoryScopeType::Project, "proj-1", now)
        .await
        .expect("list");
    assert_eq!(visible.len(), 1, "pinned expirada deve aparecer");
    assert_eq!(visible[0].id, pinned.id);
}

#[tokio::test]
async fn list_by_scope_excludes_superseded_even_if_pinned() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // Memória pinned que vai ser superseded. Regra do
    // `ADR-0014 §2`: correção é mais forte que pin.
    let old = repo
        .insert_user_confirmed(NewMemoryInput {
            user_pinned: true,
            ..user_input("proj-1", "lembrar: uso MySQL")
        })
        .await
        .expect("insert old");

    let now = Utc::now();
    let _ = repo
        .apply_correction(
            &old.id,
            NewMemoryInput {
                content: "na verdade uso Postgres".into(),
                ..user_input("proj-1", "x")
            },
            now,
        )
        .await
        .expect("apply_correction");

    let visible = repo
        .list_by_scope(MemoryScopeType::Project, "proj-1", now)
        .await
        .expect("list");
    assert_eq!(visible.len(), 1, "só a substituta aparece");
    assert_eq!(visible[0].content, "na verdade uso Postgres");
}

// ===========================================================================
// purge_expired
// ===========================================================================

#[tokio::test]
async fn purge_expired_deletes_expired_and_superseded() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // 1. Memória expirada (`temporary` com `days_from_now = -1`).
    let _ = repo
        .insert_user_confirmed(temporary_input("proj-1", "expirada 1", -1))
        .await
        .expect("insert expired 1");
    // 2. Memória expirada adicional.
    let _ = repo
        .insert_user_confirmed(temporary_input("proj-1", "expirada 2", -1))
        .await
        .expect("insert expired 2");
    // 3. Memória superseded (correção).
    let old = repo
        .insert_user_confirmed(user_input("proj-1", "uso MySQL"))
        .await
        .expect("insert old");
    let _ = repo
        .apply_correction(
            &old.id,
            NewMemoryInput {
                content: "uso Postgres".into(),
                ..user_input("proj-1", "x")
            },
            Utc::now(),
        )
        .await
        .expect("apply_correction");
    // 4. Memória ativa (não deve ser tocada).
    let alive = repo
        .insert_user_confirmed(user_input("proj-1", "memória viva"))
        .await
        .expect("insert alive");

    let now = Utc::now();
    let deleted = repo.purge_expired(now).await.expect("purge");
    assert_eq!(
        deleted, 3,
        "2 expiradas + 1 superseded = 3 deletadas (alive preservada)"
    );

    // A viva continua.
    let alive_now = repo.get(&alive.id).await.expect("get alive").unwrap();
    assert!(alive_now.active);
    assert!(alive_now.superseded_by.is_none());

    // Antiga superseded foi deletada fisicamente.
    let old_now = repo.get(&old.id).await.expect("get old");
    assert!(old_now.is_none(), "superseded foi purgeada");
}

#[tokio::test]
async fn purge_expired_keeps_pinned_expired() {
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    // Pinned expirada — `user_pinned` bypassa `expires_at`
    // E bypassa `purge_expired` (a query do `purge_expired`
    // filtra `user_pinned = 0`).
    let pinned = repo
        .insert_user_confirmed(NewMemoryInput {
            user_pinned: true,
            ..temporary_input("proj-1", "lembrar até 2025", -100)
        })
        .await
        .expect("insert pinned");

    let deleted = repo.purge_expired(Utc::now()).await.expect("purge");
    assert_eq!(deleted, 0, "pinned expirada preservada");

    let pinned_now = repo.get(&pinned.id).await.expect("get pinned").unwrap();
    assert!(pinned_now.active, "pinned continua ativa");
    assert!(pinned_now.user_pinned);
}

// ===========================================================================
// Retomada: clock determinístico (FakeClock-friendly)
// ===========================================================================

#[tokio::test]
async fn correction_timeline_preserves_superseded_at_for_audit() {
    // A Etapa 5 do UI vai mostrar "essa memória foi substituída
    // por X há 3 dias". Pra isso, `superseded_at` tem que estar
    // correto. Cobertura explícita.
    let db = Database::open_in_memory().await.expect("db");
    let repo = MemoryRepo::new(&db);

    let old = repo
        .insert_user_confirmed(user_input("proj-1", "lembrar: 1+1=2"))
        .await
        .expect("insert old");

    // O caller passa `now` explícito (no app de produção, vem
    // do `SystemClock`; em teste, é um `now` fixo).
    let correction_time: DateTime<Utc> = Utc::now();
    let result = repo
        .apply_correction(
            &old.id,
            NewMemoryInput {
                content: "na verdade, vai de 1+1=3 no nosso projeto".into(),
                ..user_input("proj-1", "x")
            },
            correction_time,
        )
        .await
        .expect("apply_correction");

    // A `old` tem `superseded_at = correction_time` exato.
    let old_now = repo.get(&old.id).await.expect("get old").unwrap();
    assert_eq!(
        old_now.superseded_at,
        Some(correction_time),
        "superseded_at deve ser o `now` do caller"
    );
    // A `new` tem `created_at = correction_time` (consistente).
    let new_now = repo
        .get(&result.new_record.id)
        .await
        .expect("get new")
        .unwrap();
    assert_eq!(new_now.created_at, correction_time);
}
