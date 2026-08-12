//! Implementação Windows do `CredentialStore` — Windows Credential
//! Manager (DPAPI).
//!
//! A chave é gravada como `CRED_TYPE_GENERIC` com
//! `CRED_PERSIST_LOCAL_MACHINE`, que faz o Credential Manager criptografar
//! o blob via DPAPI sob o perfil do usuário Windows. Cada provedor tem
//! um `TargetName` único: `Frederico-IA-Studio:provider:<provider_id>`.
//!
//! **Nenhum atalho de texto puro:** o blob chega como `&SecretString`,
//! é copiado pro `CredentialBlob` como bytes (UTF-8), e o
//! `CredWriteW` faz a encriptação via DPAPI. A leitura via `CredReadW`
//! devolve os bytes descriptografados, que viram `SecretString` e
//! voltam pro chamador sem nunca passar por `String`/`&str` acessível.
//!
//! **Por que `unsafe`?** A API Win32 (`CredWriteW`/`CredReadW`/
//! `CredDeleteW`/`CredEnumerateW`) opera com `PWSTR`/`PCWSTR` (wide
//! string pointers), `*mut u8` para o blob, e exige `CredFree` no
//! retorno. O `unsafe` fica isolado neste módulo; o resto do projeto
//! só vê a trait `CredentialStore` com `SecretString`.
//
// O crate inteiro tem `unsafe_code = "forbid"` no `Cargo.toml`; este
// é o **único** módulo onde `unsafe` é permitido e ele é isolado
// aqui por ser a única ponte com a Win32.
#![allow(unsafe_code)]

use super::{CredentialStore, SecurityError};
use async_trait::async_trait;
use frederico_core::ProviderId;
use secrecy::{ExposeSecret, SecretString};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// Etapa 2 da Fase 7 (ADR-0036): Job Objects para tree-kill
/// garantido. O módulo vive aqui (e nao em `windows/job_object.rs`
/// como decidi originalmente) porque o modulo `windows` ja e
/// `mod` no lib raiz, e adicionar submodulos no mesmo file funciona
/// em Rust 2018+ (ver `mod job_object;` abaixo — Rust procura
/// `src/windows/job_object.rs`).
mod job_object;
pub use job_object::{JobError, JobObject};

/// Etapa 2 da Fase 7 (ADR-0036 D4): Restricted Token para drop
/// dos 6 privilégios elevados. Mesma estrutura do `job_object`
/// (submódulo do `windows`).
mod restricted_token;
pub use restricted_token::{RestrictedToken, RestrictedTokenError, DROPPED_PRIVILEGE_NAMES};

/// Etapa 5+ da Fase 7 (path safety enforcement): helper pra
/// aplicar `Mandatory Label\Low` no workdir. Combinado com
/// `TokenIntegrityLevel = Low` no token (setado em
/// `RestrictedToken::set_integrity_level`), fecha path safety:
/// o processo Low só consegue acessar o workdir (que tem
/// label Low), não o parent (label Medium default).
mod integrity_label;
pub use integrity_label::{
    build_low_label_security_descriptor, set_low_integrity_label, IntegrityLabelError, LabelSd,
    INTEGRITY_LEVEL_LOW,
};

/// Prefixo do `TargetName`. Usado em `list_providers` como filtro
/// (`Frederico-IA-Studio:provider:*`).
const TARGET_PREFIX: &str = "Frederico-IA-Studio:provider:";

/// `WindowsError::NOT_FOUND` (1168). As funções Win32 retornam isso
/// como HRESULT `0x80070490`; extraímos o win32 code com `& 0xFFFF`
/// para comparar.
const ERROR_NOT_FOUND: u32 = 1168;

/// Extrai o win32 error de um HRESULT. HRESULTs têm o formato
/// `0x7FFx_xxxx` para erros de Win32, com o win32 code nos 16 bits
/// baixos. `& 0xFFFF` é a forma canônica de obter o código
/// subjacente.
const fn hresult_to_win32(hr: i32) -> u32 {
    (hr as u32) & 0xFFFF
}

pub struct WindowsCredentialStore;

impl WindowsCredentialStore {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for WindowsCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Codifica uma `&str` em wide string com terminador null. O vetor
/// retornado **precisa** viver até o final da chamada Win32 que o
/// consome (Win32 lê a string durante a chamada, não mantém
/// referência).
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decodifica uma wide string terminada em null num `String`.
/// `wide` deve apontar para uma região terminada por `0`.
///
/// **Defesa em profundidade:** limita a leitura a `MAX_WIDE_CHARS`
/// chars. Se uma `CREDENTIALW` malformada retornar um ponteiro sem
/// null terminator dentro desse limite, evitamos ler memória não
/// mapeada (heap corruption) e devolvemos o que pudemos.
unsafe fn from_wide(wide: *const u16) -> String {
    const MAX_WIDE_CHARS: usize = 512; // 1024 bytes, espaço de sobra.
    if wide.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < MAX_WIDE_CHARS && *wide.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(wide, len);
    String::from_utf16_lossy(slice)
}

/// Constrói o `TargetName` para um provedor: `Frederico-IA-Studio:provider:<id>`.
fn target_name_for(provider: &ProviderId) -> Vec<u16> {
    to_wide(&format!("{TARGET_PREFIX}{}", provider.as_str()))
}

#[async_trait]
impl CredentialStore for WindowsCredentialStore {
    async fn get(&self, provider: &ProviderId) -> Result<Option<SecretString>, SecurityError> {
        let target = target_name_for(provider);
        let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
        let result =
            unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0, &mut cred_ptr) };
        match result {
            Ok(()) => {
                if cred_ptr.is_null() {
                    return Ok(None);
                }
                let cred = unsafe { *cred_ptr };
                let size = cred.CredentialBlobSize as usize;
                let s = if size == 0 {
                    String::new()
                } else {
                    let bytes = unsafe { std::slice::from_raw_parts(cred.CredentialBlob, size) };
                    String::from_utf8_lossy(bytes).into_owned()
                };
                // CredFree libera o bloco alocado por CredReadW.
                unsafe { CredFree(creds_to_void(cred_ptr)) };
                Ok(Some(SecretString::new(s.into())))
            }
            Err(e) => {
                if hresult_to_win32(e.code().0) == ERROR_NOT_FOUND {
                    return Ok(None);
                }
                Err(SecurityError::CredentialStore(format!("CredReadW: {e}")))
            }
        }
    }

    async fn set(&self, provider: &ProviderId, value: &SecretString) -> Result<(), SecurityError> {
        let target = target_name_for(provider);
        let secret = value.expose_secret();
        let blob = secret.as_bytes();

        // `CredWriteW` em `windows` v0.58 toma `(credential, flags)`.
        // O segundo argumento é `CRED_PRESERVE_CREDENTIAL_BLOB` ou 0.
        // Usamos 0 — substituição por cima.
        let result = unsafe {
            let target_pwstr = PWSTR(target.as_ptr() as *mut u16);
            let cred = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: target_pwstr,
                Comment: PWSTR::null(),
                LastWritten: std::mem::zeroed(),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_ptr() as *mut u8,
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: std::ptr::null_mut(),
                TargetAlias: PWSTR::null(),
                UserName: PWSTR::null(),
            };
            CredWriteW(&cred, 0)
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) => Err(SecurityError::CredentialStore(format!("CredWriteW: {e}"))),
        }
    }

    async fn delete(&self, provider: &ProviderId) -> Result<(), SecurityError> {
        let target = target_name_for(provider);
        let result = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0) };
        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                if hresult_to_win32(e.code().0) == ERROR_NOT_FOUND {
                    // Idempotente: deletar algo que não existe não é erro.
                    return Ok(());
                }
                Err(SecurityError::CredentialStore(format!("CredDeleteW: {e}")))
            }
        }
    }

    async fn list_providers(&self) -> Result<Vec<ProviderId>, SecurityError> {
        // Filtro com wildcard: `Frederico-IA-Studio:provider:*`.
        let filter = to_wide(&format!("{TARGET_PREFIX}*"));
        let mut count: u32 = 0;
        let mut creds_ptr: *mut *mut CREDENTIALW = std::ptr::null_mut();
        let result = unsafe {
            CredEnumerateW(
                PCWSTR(filter.as_ptr()),
                windows::Win32::Security::Credentials::CRED_ENUMERATE_FLAGS(0),
                &mut count,
                &mut creds_ptr,
            )
        };
        match result {
            Ok(()) => {
                if creds_ptr.is_null() || count == 0 {
                    // Em alguns retornos Ok com count=0, o sistema
                    // devolve um pointer não-nulo que não devemos
                    // CredFree-zar (não há bloco válido). Em outros
                    // devolve null. Devolvemos vetor vazio em ambos
                    // os casos.
                    return Ok(Vec::new());
                }
                // O Credential Manager aloca o array de ponteiros E
                // cada CREDENTIALW num **único bloco**; basta uma
                // chamada `CredFree` no `creds_ptr` para liberar
                // tudo. Chamar `CredFree` em cada `cred_ptr` causa
                // double-free e STATUS_HEAP_CORRUPTION.
                let creds = unsafe { std::slice::from_raw_parts(creds_ptr, count as usize) };
                let mut providers = Vec::new();
                for &cred_ptr in creds {
                    let cred = unsafe { *cred_ptr };
                    let target = unsafe { from_wide(cred.TargetName.0) };
                    if let Some(rest) = target.strip_prefix(TARGET_PREFIX) {
                        if !rest.is_empty() {
                            providers.push(ProviderId::new(rest));
                        }
                    }
                }
                unsafe { CredFree(creds_to_void(creds_ptr)) };
                Ok(providers)
            }
            Err(e) => {
                if hresult_to_win32(e.code().0) == ERROR_NOT_FOUND {
                    return Ok(Vec::new());
                }
                Err(SecurityError::CredentialStore(format!(
                    "CredEnumerateW: {e}"
                )))
            }
        }
    }
}

/// Converte um ponteiro alocado por `CredReadW`/`CredEnumerateW` em
/// `*const c_void` (o que `CredFree` espera no `windows` v0.58).
///
/// O cast é seguro do ponto de vista de tipos: `CredFree` aceita
/// `*const c_void` justamente porque diferentes APIs Win32 alocam
/// estruturas diferentes (CREDENTIAL, CREDENTIALW, ENUMERATE…) e a
/// liberação é feita por tamanho de alocação, não por tipo.
fn creds_to_void<T>(p: *mut T) -> *const std::ffi::c_void {
    p.cast::<std::ffi::c_void>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_name_format() {
        let n = target_name_for(&ProviderId::new("openai"));
        let s = String::from_utf16_lossy(&n[..n.len() - 1]); // sem null
        assert_eq!(s, "Frederico-IA-Studio:provider:openai");
    }

    #[test]
    fn to_wide_includes_null_terminator() {
        let w = to_wide("ab");
        assert_eq!(w, vec![b'a' as u16, b'b' as u16, 0]);
    }

    #[test]
    fn from_wide_handles_null_terminator() {
        let w: Vec<u16> = vec![b'h' as u16, b'i' as u16, 0, b'x' as u16];
        let s = unsafe { from_wide(w.as_ptr()) };
        assert_eq!(s, "hi");
    }

    #[test]
    fn from_wide_handles_null_pointer() {
        let s = unsafe { from_wide(std::ptr::null()) };
        assert_eq!(s, "");
    }
}
