//! `RestrictedToken` — primitiva de Windows que descarta privilégios
//! elevados do token de processo. É a **segunda camada** do sandbox
//! (a primeira é o Jail de path safety; a terceira é o Job Object
//! para tree-kill).
//!
//! Ver [ADR-0031 §D4](../../../decisions/0031-fase-7-isolation-model-windows.md)
//! e [ADR-0036 §D4](../../../decisions/0036-security-jail-resolver-windows-job-objects.md).
//!
//! ## Os 6 privilégios descartados
//!
//! Cada um representa uma classe diferente de escalada que poderia
//! acontecer **se** um filho malicioso/invadido conseguisse privilégio
//! de admin (via exploit de Python/Node, por exemplo):
//!
//! - **`SeDebugPrivilege`** — `DebugActiveProcess` / `OpenProcess`
//!   com `PROCESS_ALL_ACCESS` em qualquer processo. Sem ele, o filho
//!   **não** consegue atachar no pai (mesmo rodando como admin no
//!   usuário), o que fecha o vetor clássico de "ler a memória do
//!   pai e roubar credenciais em cache".
//! - **`SeBackupPrivilege`** — leitura de arquivos de sistema via
//!   `BackupRead` (ignora DACL). Fecha vetor de "ler `SAM`/`SECURITY`
//!   sem ser admin" (que é o que ferramentas como `secretsdump.py`
//!   exploram em pós-exploração).
//! - **`SeRestorePrivilege`** — escrita em arquivos de sistema via
//!   `BackupWrite`. Complementar ao SeBackup: fecha o vetor de
//!   "sobrescrever `cmd.exe` por uma versão maliciosa".
//! - **`SeTakeOwnershipPrivilege`** — `SetSecurityInfo` para "roubar"
//!   ownership de arquivos de sistema (mesmo sem SeRestore).
//! - **`SeLoadDriverPrivilege`** — `NtLoadDriver`. Fecha vetor clássico
//!   de rootkit (carrega driver malicioso que roda em kernel mode).
//! - **`SeShutdownPrivilege`** — `InitiateSystemShutdown`. Defesa
//!   contra `exec.shell` malicioso que tenta desligar o host.
//!
//! ## Aplicação ao processo filho
//!
//! O `RestrictedToken` produz um HANDLE que o `SecurityJailResolver`
//! (peça 4) passa para `CreateProcessAsUser` (não `CreateProcess`,
//! que não aceita token). A v1 não usa `CreateProcessAsUser` — o
//! aplicação real é responsabilidade da peça 4 (orchestrator). Por
//! enquanto, o `restricted_token` é construído e o handle fica
//! disponível via [`Self::handle`].
//!
//! ## Compatibilidade com Python/Node
//!
//! AppContainer (a primitiva mais forte) **quebra** rotinas comuns
//! de Python/Node. Restricted Token **não** quebra — Python roda
//! normal sob ele (verificado pelo teste
//! `python_runs_under_restricted_token` da Etapa 2, em
//! `crates/security/tests/restricted_token.rs`).
//!
//! ## ADR-0007: `windows` confinado a este módulo
//!
//! O `Cargo.toml` do `frederico-security` tem `unsafe_code = "deny"`
//! no nível do crate. Este módulo (e só este) tem
//! `#![allow(unsafe_code)]`. Cada bloco `unsafe` tem comentário
//! explicando a invariante de segurança.

#![allow(unsafe_code)]

use thiserror::Error;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    CreateRestrictedToken, DuplicateTokenEx, LookupPrivilegeValueW, SecurityAnonymous,
    SetTokenInformation, TokenPrimary, CREATE_RESTRICTED_TOKEN_FLAGS, LUID_AND_ATTRIBUTES,
    SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS, TOKEN_MANDATORY_LABEL, TOKEN_PRIVILEGES_ATTRIBUTES,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Os 6 privilégios que o `RestrictedToken` descarta por padrão
/// (defesa contra escalada via exploit de Python/Node).
///
/// **Por que hardcoded:** a decisão de quais privilégios remover
/// é estrutural (D4 do ADR-0036). Adicionar/remover privilégios
/// desta lista exige ADR próprio — mesma proteção que o
/// `EnvAllowlist::REQUIRED` (peça 1) e o `PermissionSet` (Fase 3).
pub const DROPPED_PRIVILEGE_NAMES: &[&str] = &[
    "SeDebugPrivilege",
    "SeBackupPrivilege",
    "SeRestorePrivilege",
    "SeTakeOwnershipPrivilege",
    "SeLoadDriverPrivilege",
    "SeShutdownPrivilege",
];

/// Erro do `RestrictedToken`.
#[derive(Debug, Error)]
pub enum RestrictedTokenError {
    /// `OpenProcessToken` falhou (sem permissão de abrir o próprio
    /// token do processo, ou handle do processo inválido).
    #[error("OpenProcessToken falhou: {message}")]
    OpenTokenFailed { message: String },
    /// `LookupPrivilegeValueW` falhou para um dos 6 privilégios
    /// (nome inválido — indica bug, não condição runtime).
    #[error("LookupPrivilegeValueW('{name}') falhou: {message}")]
    LookupPrivilegeFailed { name: &'static str, message: String },
    /// `CreateRestrictedToken` falhou (SIDs inválidos, ou OS
    /// recusou a combinação).
    #[error("CreateRestrictedToken falhou: {message}")]
    CreateRestrictedFailed { message: String },
    /// `SetTokenInformation(TokenRestrictedSids)` falhou (token
    /// não permite adicionar restricted SIDs — raro).
    #[error("SetTokenInformation(TokenRestrictedSids) falhou: {0}")]
    SetRestrictedSidsFailed(windows::core::Error),
    /// `DuplicateTokenEx` falhou (não foi possível duplicar o
    /// token como primary — raro, geralmente indica corrupção
    /// de handle ou OS sem recursos).
    #[error("DuplicateTokenEx falhou: {0}")]
    DuplicateTokenFailed(windows::core::Error),
}

/// Handle para um Windows Restricted Token. Quando o
/// `RestrictedToken` é droppado, o handle é fechado via
/// `CloseHandle`.
///
/// **Não-clonable** (mesma justificativa do `JobObject`).
pub struct RestrictedToken {
    handle: HANDLE,
    /// LUIDs dos privilégios removidos. Armazenados para o caller
    /// poder inspecionar (debug) ou reaplicar.
    dropped_privileges: Vec<LUID>,
}

impl RestrictedToken {
    /// Cria um `RestrictedToken` a partir do **token do processo
    /// atual** (`GetCurrentProcess` + `OpenProcessToken` +
    /// `CreateRestrictedToken`). Os 6 privilégios de
    /// [`DROPPED_PRIVILEGE_NAMES`] são removidos. Na v1, nenhum
    /// SID é marcado como deny-only (defense-in-depth de SIDs é
    /// roadmap — o `ConvertStringSidToSidW` + `LocalFree` adiciona
    /// complexidade de lifetime que não cabe na peça 3).
    ///
    /// **Aplica-se a:** sandbox do `frederico-security`. O
    /// `SecurityJailResolver` (peça 4) usa este construtor para
    /// obter o token que vai ser passado a `CreateProcessAsUser`
    /// no spawn do filho.
    ///
    /// # Erros
    ///
    /// Falha se algum dos 6 nomes de privilégio não existir no
    /// OS (improvável — todos existem desde Windows XP) ou se o
    /// `CreateRestrictedToken` for rejeitado pelo kernel.
    pub fn from_current_process() -> Result<Self, RestrictedTokenError> {
        // 1. Abre o token do processo atual com `TOKEN_ALL_ACCESS`
        //    (precisamos de QUERY | DUP_HANDLE | ASSIGN_SECURITY
        //    para CreateRestrictedToken; ALL_ACCESS é o que o
        //    `windows` crate expõe de forma ergonômica).
        let mut current_token: HANDLE = HANDLE(std::ptr::null_mut());
        // SAFETY: `OpenProcessToken` toma o handle do processo +
        // máscara de acesso desejada + ponteiro pro handle de
        // saída. O handle do processo é `GetCurrentProcess()`
        // (pseudo-handle que sempre funciona); a máscara é
        // `TOKEN_ALL_ACCESS` (queremos controle total sobre o
        // token que vamos restringir). O ponteiro de saída
        // `&mut current_token` é válido pela duração da
        // chamada.
        let ok =
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut current_token) };
        if let Err(e) = ok {
            return Err(RestrictedTokenError::OpenTokenFailed {
                message: format!("{e:?}"),
            });
        }

        // 2. Resolve os LUIDs dos 6 privilégios e monta o array
        //    de `LUID_AND_ATTRIBUTES` para `CreateRestrictedToken`.
        let mut privileges_to_delete: Vec<LUID_AND_ATTRIBUTES> =
            Vec::with_capacity(DROPPED_PRIVILEGE_NAMES.len());
        for &name in DROPPED_PRIVILEGE_NAMES {
            let luid = lookup_privilege_value(name)?;
            // `Attributes = 0` indica "remover este privilégio do
            // token restrito" (vs. `SE_PRIVILEGE_ENABLED` que
            // adicionaria). Ver `winnt.h` SE_* constants.
            privileges_to_delete.push(LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: TOKEN_PRIVILEGES_ATTRIBUTES(0),
            });
        }

        // 3. `CreateRestrictedToken` cria um **novo** token
        //    restrito a partir do `current_token`. Passamos
        //    `None` para SidsToRestrict e SidsToDisable na v1
        //    (defense-in-depth de SIDs é roadmap).
        let mut restricted_token: HANDLE = HANDLE(std::ptr::null_mut());
        // SAFETY: `CreateRestrictedToken` toma o token existente +
        // flags (0 = sem flags especiais) + SidsToDisable (vazio
        // = não desabilita nenhum SID) + SidsToRestrict (None na
        // v1) + PrivilegesToDelete (nossos 6) + handle de saída.
        // O lifetime de `privileges_to_delete` é a duração da
        // chamada (a função Win32 lê o array durante a chamada,
        // não mantém referência). `current_token` veio de
        // `OpenProcessToken` bem-sucedido e fica aberto até o
        // fim desta função (fechado depois, independente do
        // resultado de CreateRestrictedToken).
        let result = unsafe {
            CreateRestrictedToken(
                current_token,
                CREATE_RESTRICTED_TOKEN_FLAGS(0),
                None,                        // SidsToDisable
                Some(&privileges_to_delete), // PrivilegesToDelete
                None,                        // SidsToRestrict
                &mut restricted_token,
            )
        };
        // Fecha o token original (não é mais necessário).
        // SAFETY: handle veio de OpenProcessToken bem-sucedido;
        // não há duplo-close (o handle sai de escopo aqui).
        unsafe {
            let _ = CloseHandle(current_token);
        }
        if let Err(e) = result {
            return Err(RestrictedTokenError::CreateRestrictedFailed {
                message: format!("{e:?}"),
            });
        }

        Ok(Self {
            handle: restricted_token,
            dropped_privileges: privileges_to_delete.iter().map(|la| la.Luid).collect(),
        })
    }

    /// Handle Win32 do token restrito. O `SecurityJailResolver`
    /// (peça 4) usa este handle como input para
    /// `CreateProcessAsUser` no spawn do filho do sandbox.
    #[must_use]
    pub const fn handle(&self) -> HANDLE {
        self.handle
    }

    /// LUIDs dos privilégios removidos. Útil para debug e
    /// auditoria — o caller pode inspecionar e logar
    /// `"dropped privileges: SeDebug=0x..., SeBackup=0x..., ..."`.
    #[must_use]
    pub fn dropped_privileges(&self) -> &[LUID] {
        &self.dropped_privileges
    }

    /// Duplica o token como **primary token** (necessário pra
    /// `CreateProcessAsUserW`). O `CreateRestrictedToken` retorna
    /// um token que pode ser usado tanto como primary quanto
    /// impersonation, mas `DuplicateTokenEx` com `TokenPrimary`
    /// é a forma explícita e robusta de garantir o tipo correto.
    ///
    /// **Caller é dono do handle retornado** — deve chamar
    /// `CloseHandle` (ou usar um wrapper RAII).
    pub fn duplicate_as_primary(&self) -> Result<HANDLE, RestrictedTokenError> {
        let mut new_handle: HANDLE = HANDLE(std::ptr::null_mut());
        // SAFETY: `DuplicateTokenEx` toma o token existente +
        // máscara de acesso + security attributes (None) +
        // ImpersonationLevel (SecurityAnonymous é suficiente
        // porque queremos criar um primary token) + TokenType
        // (TokenPrimary) + handle de saída.
        unsafe {
            DuplicateTokenEx(
                self.handle,
                TOKEN_ALL_ACCESS,
                None,
                SecurityAnonymous,
                TokenPrimary,
                &mut new_handle,
            )
        }
        .map_err(RestrictedTokenError::DuplicateTokenFailed)?;
        Ok(new_handle)
    }

    /// Setar `TokenIntegrityLevel` no token. O `Level` é o
    /// RID do SID de integrity (`SECURITY_MANDATORY_RID`
    /// + offset). Níveis padrão:
    /// - Low:    0x1000 (S-1-16-4096)
    /// - Medium: 0x2000 (S-1-16-8192)
    /// - High:   0x3000 (S-1-16-12288)
    /// - System: 0x4000 (S-1-16-16384)
    ///
    /// **Etapa 5+ (path safety):** a Etapa 5+ seta **Low**
    /// (0x1000) e adiciona `Mandatory Label\Low` no workdir.
    /// O processo Low não consegue acessar objetos Medium
    /// (parent do workdir, system files, etc) — DACL não
    /// importa, é a **integrity** que bloqueia.
    ///
    /// **Por que low e não medium:** com medium, o processo
    /// pode acessar QUALQUER arquivo do user (incluindo
    /// o parent do workdir). Com low, só consegue acessar
    /// objetos explicitamente marcados como low (ou sem
    /// label, que default = medium → bloqueia low).
    pub fn set_integrity_level(&self, level: u32) -> Result<(), RestrictedTokenError> {
        // Cria o SID S-1-16-<level> (SECURITY_MANDATORY_LABEL_AUTHORITY
        // = 0x10, 1 sub-authority = level).
        let mut sid_handle = windows::Win32::Security::PSID(std::ptr::null_mut());
        // SAFETY: aloca um SID novo do tipo mandatory label.
        unsafe {
            windows::Win32::Security::AllocateAndInitializeSid(
                &windows::Win32::Security::SID_IDENTIFIER_AUTHORITY {
                    Value: windows::Win32::Security::SECURITY_MANDATORY_LABEL_AUTHORITY.Value,
                },
                1,
                level,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                &mut sid_handle,
            )
        }
        .map_err(RestrictedTokenError::SetRestrictedSidsFailed)?;
        // Constrói o TOKEN_MANDATORY_LABEL com o SID.
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid_handle,
                Attributes: 0x00000000, // SE_GROUP_INTEGRITY (não enforced
                                        // nem enabled-by-default)
            },
        };
        // SAFETY: `SetTokenInformation` toma o handle do token +
        // TokenInformationClass + buffer + tamanho. `label`
        // vive até o fim desta função; Win32 copia o SID
        // internamente.
        let result = unsafe {
            SetTokenInformation(
                self.handle,
                windows::Win32::Security::TokenIntegrityLevel,
                &label as *const _ as *const _,
                std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
            )
        };
        // Libera o SID local — o token fez cópia.
        // SAFETY: `sid_handle` foi alocado por AllocateAndInitializeSid;
        // FreeSid é a API correta.
        unsafe {
            let _ = windows::Win32::Security::FreeSid(sid_handle);
        }
        result.map_err(RestrictedTokenError::SetRestrictedSidsFailed)?;
        Ok(())
    }
}

impl Drop for RestrictedToken {
    fn drop(&mut self) {
        // SAFETY: handle veio de `CreateRestrictedToken`
        // bem-sucedido. Não foi fechado em nenhum outro lugar
        // (struct sem método de "consumo"). Ignoramos o erro
        // (não há ação útil no Drop).
        let result = unsafe { CloseHandle(self.handle) };
        let _ = result;
    }
}

// `HANDLE` é Send + Sync por convenção da API Win32. Marcamos
// explicitamente pra confirmar que pode ser compartilhado via
// `Arc<RestrictedToken>` (caso de uso futuro no
// `SecurityJailResolver`).
unsafe impl Send for RestrictedToken {}
unsafe impl Sync for RestrictedToken {}

/// Helper: resolve o LUID de um privilégio pelo nome (ex.:
/// `"SeDebugPrivilege"` → `LUID { LowPart: ..., HighPart: ... }`).
///
/// Retorna `Err` com o nome (estático, pra fácil debug) se o
/// privilégio não existir no OS.
fn lookup_privilege_value(name: &'static str) -> Result<LUID, RestrictedTokenError> {
    // SAFETY: `LookupPrivilegeValueW` toma o system name (NULL =
    // local) + privilege name wide string + ponteiro pro LUID de
    // saída. O system name como NULL é documentado como "usa o
    // sistema local" (computador local). O privilege name wide
    // string é construído abaixo com null terminator. O LUID de
    // saída é stack-local e válido pela duração da chamada.
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    let result =
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(wide_name.as_ptr()), &mut luid) };
    if let Err(e) = result {
        return Err(RestrictedTokenError::LookupPrivilegeFailed {
            name,
            message: format!("{e:?}"),
        });
    }
    Ok(luid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `from_current_process()` retorna Ok em condições normais.
    /// O handle do token restrito é válido (não-null).
    #[test]
    fn from_current_process_succeeds() {
        let tok = RestrictedToken::from_current_process()
            .expect("from_current_process deve ter sucesso em Windows");
        assert!(
            !tok.handle().0.is_null(),
            "handle do token nao pode ser null"
        );
        // Drop roda aqui: fecha o handle.
    }

    /// `dropped_privileges()` tem 6 LUIDs (um por privilégio da
    /// constante `DROPPED_PRIVILEGE_NAMES`). Os LUIDs são
    /// distintos (cada privilégio tem um LUID único no OS).
    #[test]
    fn dropped_privileges_has_six_entries() {
        let tok = RestrictedToken::from_current_process().expect("from_current_process");
        let dropped = tok.dropped_privileges();
        assert_eq!(dropped.len(), DROPPED_PRIVILEGE_NAMES.len());
        assert_eq!(dropped.len(), 6);
        // Os LUIDs devem ser distintos (cada privilégio tem LUID
        // único; em prática LowPart difere sempre, HighPart é
        // 0 para privilégios padrão).
        let mut sorted: Vec<u32> = dropped.iter().map(|l| l.LowPart).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 6, "LUIDs devem ser distintos");
    }

    /// Dois `RestrictedToken`s independentes têm handles
    /// distintos. Não compartilham via `Arc`.
    #[test]
    fn two_restricted_tokens_have_distinct_handles() {
        let a = RestrictedToken::from_current_process().expect("a");
        let b = RestrictedToken::from_current_process().expect("b");
        assert_ne!(a.handle().0, b.handle().0);
    }
}
