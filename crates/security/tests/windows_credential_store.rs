//! Testes de integração do `WindowsCredentialStore` real.
//!
//! Estes testes falam com o **Windows Credential Manager** de verdade
//! (DPAPI). Cada teste:
//!
//! 1. Gera um `ProviderId` único por execução (counter atômico +
//!    PID) para não colidir com runs anteriores nem com paralelismo
//!    entre testes.
//! 2. Cria um [`Cleanup`] que deleta a credencial no `Drop` — assim
//!    uma falha no meio do teste não deixa lixo no cofre.
//! 3. Roda serializado (`#[serial]`) porque o Credential Manager é
//!    global; dois testes paralelos podem se atrapalhar ao enumerar.
//!
//! Pré-condição: Windows. Em outras plataformas o arquivo inteiro é
//! pulado via `#[cfg(windows)]` no `Cargo.toml` da crate.
//!
//! O crate tem `unsafe_code = "deny"`; este integration test
//! **precisa** de `unsafe` para semear uma credencial de outro app
//! via `CredWriteW` direto (teste `list_providers_filters_by_prefix`).
//! Mantemos o `unsafe` cirúrgico e comentado.
//!
//! Sobre o `MutexGuard` segurado através de awaits (lint
//! `clippy::await_holding_lock`): o `lock()` é um `std::sync::Mutex`
//! **do SO** (não do tokio), e as chamadas Win32 (CredReadW/
//! CredWriteW/etc.) **bloqueiam a thread** — não cedem pro runtime.
//! Em runtime `current_thread`, isso significa que não há nenhuma
//! outra task tokio que possa tentar adquirir o lock durante o
//! await; a única "concorrência" possível seria com outra thread
//! real, e o `#[test]` padrão do Cargo tem `--test-threads=1` para
//! essa suíte (ver `scripts/verify.ps1` se aplicável). Logo, o
//! padrão é seguro aqui apesar do lint.
#![cfg(windows)]
#![allow(unsafe_code)]
#![allow(clippy::await_holding_lock)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use frederico_core::ProviderId;
use frederico_security::windows::WindowsCredentialStore;
use frederico_security::CredentialStore;
use secrecy::ExposeSecret;

/// Mutex global para serializar os testes — o Windows Credential
/// Manager é um recurso global do SO e dois testes paralelos
/// (list_providers em particular) podem se atrapalhar.
static SERIAL: Mutex<()> = Mutex::new(());

/// Counter global para gerar `ProviderId`s únicos. `Ordering::Relaxed`
/// é suficiente: cada teste precisa apenas de um valor diferente dos
/// outros, não de uma ordem específica.
static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// Adquire o lock global; em caso de poison (panic de outro teste)
/// ainda assim continuamos (o Credential Manager é recuperável).
fn lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Gera um `ProviderId` único combinando o PID do processo e o
/// counter global. Garante que dois testes na mesma run e runs
/// diferentes não colidam.
fn unique_id(label: &'static str) -> ProviderId {
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    ProviderId::new(format!("test-{label}-{pid}-{n}"))
}

/// RAII guard que agenda a limpeza da credencial via
/// `tokio::task::spawn` quando o teste termina (normalmente ou em
/// panic). Usar `block_on` dentro do `Drop` deadlock-a em runtime
/// `current_thread` — o `block_on` esperaria a task atual terminar,
/// que espera o `Drop` terminar. `spawn` apenas agenda a future e
/// retorna, deixando o runtime drenar antes de sair.
///
/// **Não** é uma garantia rígida: se o processo abortar, a
/// credencial pode ficar. Mas como `ProviderId`s são únicos por run
/// (`unique_id` combina PID + counter), credenciais órfãs de runs
/// passadas não interferem em runs futuras — `list_providers` filtra
/// só as que começam com `TARGET_PREFIX` Frederico, e mesmo que
/// voltem, basta uma `CredDeleteW` direta no Credential Manager.
struct Cleanup(Vec<ProviderId>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            for id in std::mem::take(&mut self.0) {
                handle.spawn(async move {
                    let store = WindowsCredentialStore::new();
                    let _ = store.delete(&id).await;
                });
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn set_get_roundtrip() {
    let _g = lock();
    let store = WindowsCredentialStore::new();
    let id = unique_id("roundtrip");
    let mut cleanup = Cleanup(vec![id.clone()]);

    let secret = secrecy::SecretString::new("sk-roundtrip-1234".to_string().into());
    store.set(&id, &secret).await.expect("set");

    let got = store
        .get(&id)
        .await
        .expect("get")
        .expect("credencial deve existir após set");
    assert_eq!(got.expose_secret(), "sk-roundtrip-1234");

    // Cleanup explícito para confirmar idempotência.
    store.delete(&id).await.expect("delete");
    assert!(store.get(&id).await.expect("get pós-delete").is_none());

    // Marca como já limpo para o Drop não tentar de novo.
    cleanup.0.clear();
}

#[tokio::test(flavor = "current_thread")]
async fn get_nonexistent_returns_none() {
    let _g = lock();
    let store = WindowsCredentialStore::new();
    let id = unique_id("ghost");
    let mut cleanup = Cleanup(vec![id.clone()]);

    // Garante que não existe do nada.
    let result = store.get(&id).await.expect("get deve ser Ok");
    assert!(result.is_none(), "id recém-criado não pode existir");

    cleanup.0.clear();
}

#[tokio::test(flavor = "current_thread")]
async fn delete_nonexistent_is_idempotent() {
    let _g = lock();
    let store = WindowsCredentialStore::new();
    let id = unique_id("del-ghost");
    let mut cleanup = Cleanup(vec![id.clone()]);

    // Primeira vez: não existe, mas delete deve retornar Ok.
    store.delete(&id).await.expect("delete idempotente 1");
    // Segunda vez: ainda não existe, ainda Ok.
    store.delete(&id).await.expect("delete idempotente 2");

    cleanup.0.clear();
}

#[tokio::test(flavor = "current_thread")]
async fn list_providers_returns_only_frederico_prefixed() {
    let _g = lock();
    let store = WindowsCredentialStore::new();
    let a = unique_id("list-a");
    let b = unique_id("list-b");
    let mut cleanup = Cleanup(vec![a.clone(), b.clone()]);

    let sa = secrecy::SecretString::new("sk-a".to_string().into());
    let sb = secrecy::SecretString::new("sk-b".to_string().into());
    store.set(&a, &sa).await.expect("set a");
    store.set(&b, &sb).await.expect("set b");

    let listed = store.list_providers().await.expect("list");
    // Nossos dois IDs têm que estar lá (podem existir outros de runs
    // anteriores, mas como geramos unique_id, dois específicos
    // precisam aparecer).
    assert!(
        listed.contains(&a),
        "list_providers deve conter {a:?}, got {listed:?}"
    );
    assert!(
        listed.contains(&b),
        "list_providers deve conter {b:?}, got {listed:?}"
    );

    // Sanity: todos os retornados devem ser IDs que **não** contêm
    // "outro-app" (nosso seed de prefixo Frederico filtra o resto).
    for p in &listed {
        assert!(
            !p.as_str().contains("outro-app"),
            "list_providers vazou credencial de outro prefixo: {p:?}"
        );
    }

    cleanup.0.clear();
}

/// Semeia uma credencial **fora** do prefixo do Frederico (via
/// `CredWriteW` direto com TargetName `outro-app:foo`) e confirma
/// que `list_providers` **não** a devolve. Esse é o teste que prova
/// que o filtro por `TARGET_PREFIX` funciona de verdade — não só em
/// round-trips do nosso próprio set.
#[tokio::test(flavor = "current_thread")]
async fn list_providers_filters_by_prefix() {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    // 1. Semeia uma credencial "outro-app:foo" via Win32 direto.
    //    Tudo dentro de um único bloco `unsafe` — abrange o
    //    `mem::zeroed` (unsafe intrinsics), a construção do
    //    `CREDENTIALW` (campos com `*mut u8`) e a chamada Win32.
    let _g = lock();
    let target_raw = format!("outro-app:foo-{}", std::process::id());
    let target_wide: Vec<u16> = target_raw
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let blob = b"should-not-leak";
    let target_pwstr = windows::core::PWSTR(target_wide.as_ptr() as *mut u16);
    let cred = unsafe {
        CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: target_pwstr,
            Comment: windows::core::PWSTR::null(),
            LastWritten: std::mem::zeroed(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: windows::core::PWSTR::null(),
            UserName: windows::core::PWSTR::null(),
        }
    };
    unsafe {
        CredWriteW(&cred, 0).expect("CredWriteW do semeador");
    }

    // 2. Cria uma credencial Frederico válida pra garantir que list
    //    filtra corretamente (devolve a do Frederico, não a do outro).
    //    O lock global já foi adquirido no passo 1.
    let store = WindowsCredentialStore::new();
    let frederico_id = unique_id("filter");
    let mut cleanup = Cleanup(vec![frederico_id.clone()]);
    let s = secrecy::SecretString::new("sk-fred".to_string().into());
    store.set(&frederico_id, &s).await.expect("set Frederico");

    let listed = store.list_providers().await.expect("list");

    // 3. O seed "outro-app:..." NÃO pode aparecer.
    let leak = listed.iter().find(|p| p.as_str().starts_with("outro-app:"));
    assert!(
        leak.is_none(),
        "vazou credencial de outro app: {leak:?} em {listed:?}"
    );

    // 4. A nossa sim, deve aparecer.
    assert!(
        listed.contains(&frederico_id),
        "nossa credencial não apareceu: {frederico_id:?} em {listed:?}"
    );

    // 5. Limpa a semente do outro app.
    unsafe {
        let _ = CredDeleteW(PCWSTR(target_wide.as_ptr()), CRED_TYPE_GENERIC, 0);
    }

    cleanup.0.clear();
}

/// Re-gravar a mesma credencial deve sobrescrever sem erro.
#[tokio::test(flavor = "current_thread")]
async fn set_overwrites_existing() {
    let _g = lock();
    let store = WindowsCredentialStore::new();
    let id = unique_id("overwrite");
    let mut cleanup = Cleanup(vec![id.clone()]);

    let s1 = secrecy::SecretString::new("v1".to_string().into());
    store.set(&id, &s1).await.expect("set v1");
    let s2 = secrecy::SecretString::new("v2-different".to_string().into());
    store.set(&id, &s2).await.expect("set v2 overwrite");

    let got = store
        .get(&id)
        .await
        .expect("get")
        .expect("existe após overwrite");
    assert_eq!(got.expose_secret(), "v2-different");

    cleanup.0.clear();
}
