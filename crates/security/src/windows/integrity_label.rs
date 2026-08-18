//! Helper para setar **Mandatory Label** num path (Etapa 5+ da
//! Fase 7, path safety enforcement).
//!
//! **Por que Mandatory Label e não DACL custom:** o approach
//! inicial era DACL custom permitindo SÓ um SID restritivo
//! próprio (random). Mais complexo, exigia:
//! 1. Gerar um SID random por invocação do
//!    `SecurityJailResolver::new` (`IsolatedSid::new_random`).
//! 2. Adicionar o SID como `TokenRestrictedSids` no token.
//! 3. Setar DACL no workdir permitindo SÓ esse SID.
//! 4. `CreateProcessAsUserW` raw com o token modificado.
//!
//! O approach atual é mais simples: **TokenIntegrityLevel = Low**
//! no token + **Mandatory Label\Low no workdir**. O Windows
//! checa a integridade do token contra o `Mandatory Label` do
//! objeto no momento do access check (DEPOIS do DACL). Se o
//! token tem integrity < label, o acesso é negado. Como a
//! maioria do filesystem tem label `Medium` (default), um
//! processo `Low` não consegue ler/escrever em nada que não
//! esteja explicitamente marcado como `Low` (ou sem label,
//! que default = `Medium`).
//!
//! **Resultado:** o processo filho (Low) só consegue ler/escrever
//! no workdir (que tem `Mandatory Label\Low`). Tenta
//! `open("..\\evil.txt")` no parent → parent tem label Medium
//! (default) → Low < Medium → DENY (Mandatory Label check).
//!
//! **Onde a Mandatory Label vive:** é um ACE no **SACL**
//! (System Access Control List), não no DACL. O ACE tem tipo
//! `SYSTEM_MANDATORY_LABEL_ACE_TYPE` (= 0x11) e SID =
//! S-1-16-<level>.
//!
//! **API:** `AddMandatoryAce` (do Windows advapi32) constrói o
//! ACE corretamente — NÃO usamos `SetEntriesInAclW` porque essa
//! é genérica pra DACL/SACL mas não sabe criar o ACE type
//! `SYSTEM_MANDATORY_LABEL_ACE_TYPE`. `AddMandatoryAce` foi
//! adicionada no Vista e é a API documentada pra Mandatory
//! Labels.
//!
//! **Por que `SetFileSecurityW` (e não `SetSecurityInfo`):**
//! `SetSecurityInfo` via handle falha com `E_ACCESSDENIED` mesmo
//! pro dono em alguns Windows — o handle retornado por
//! `CreateFileW` não ganha `ACCESS_SYSTEM_SECURITY` mesmo com
//! `WRITE_DAC | WRITE_OWNER` na desired_access. `SetFileSecurityW`
//! (filename direto) **funciona** — é o que o `icacls` usa.

use std::path::Path;
use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Security::SetFileSecurityW;
use windows::Win32::Security::{
    AddMandatoryAce, AllocateAndInitializeSid, FreeSid, InitializeAcl, ACE_FLAGS, ACE_REVISION,
    ACL, LABEL_SECURITY_INFORMATION, OBJECT_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_MANDATORY_LABEL_AUTHORITY, SID_IDENTIFIER_AUTHORITY,
};

/// Erro ao aplicar Mandatory Label num path.
#[derive(Debug, Error)]
pub enum IntegrityLabelError {
    #[error("AllocateAndInitializeSid (mandatory label) falhou: {0}")]
    AllocateSid(#[source] windows::core::Error),
    #[error("InitializeAcl falhou: {0}")]
    InitAcl(#[source] windows::core::Error),
    #[error("AddMandatoryAce falhou: {0}")]
    AddAce(#[source] windows::core::Error),
    #[error("SetFileSecurityW (write label) falhou em `{0}`: {1}")]
    ApplyAcl(String, u32),
}

/// Nível de integridade `Low` (S-1-16-4096). Constante do
/// Windows (`WINNT.H`, `SECURITY_MANDATORY_LOW_RID = 0x1000`).
pub const INTEGRITY_LEVEL_LOW: u32 = 0x1000;

/// Buffer que contém um SECURITY_DESCRIPTOR self-relative mínimo
/// (header + SACL com o label ACE Low). O `PSECURITY_DESCRIPTOR`
/// aponta pro início do buffer; o caller deve manter o
/// `Vec<u8>` vivo enquanto o SD estiver em uso.
pub struct LabelSd {
    pub sd: PSECURITY_DESCRIPTOR,
    pub buffer: Vec<u8>,
}

/// Constroi um SECURITY_DESCRIPTOR self-relative mínimo com
/// apenas o SACL (label ACE Low). Usado em dois lugares:
/// 1. `set_low_integrity_label` (workdir) — passa o SD pro
///    `SetFileSecurityW` (com `LABEL_SECURITY_INFORMATION`).
/// 2. `CreatePipe` (stdout/stderr do child) — passa o SD no
///    `SECURITY_ATTRIBUTES.lpSecurityDescriptor` pra que o
///    pipe nasça já rotulado (sem `SetSecurityInfo` depois,
///    que falharia por o handle do `CreatePipe` não ter
///    `WRITE_OWNER`).
///
/// **Por que o SD é self-relative:** ambos Windows API
/// (`SetFileSecurityW` e `SECURITY_ATTRIBUTES` em
/// `CreatePipe`) exigem SD self-relative.
///
/// **Por que o SD é mínimo (só SACL):** o workdir e o pipe
/// ficam com SACL setado mas owner/group/dacl herdados
/// (workdir: do SD atual, preservado pelo OS; pipe: do
/// caller). O label Low no SACL é o que bloqueia.
pub fn build_low_label_security_descriptor() -> Result<LabelSd, IntegrityLabelError> {
    // 1. Cria o SID S-1-16-4096 (Mandatory Label Low).
    let sid = unsafe { create_low_integrity_sid()? };

    // 2. Cria o SACL (256 bytes é suficiente pra 1 ACE).
    let mut acl_buf = vec![0u8; 256];
    let acl_ptr = acl_buf.as_mut_ptr() as *mut ACL;
    unsafe { InitializeAcl(acl_ptr, acl_buf.len() as u32, ACE_REVISION(2)) }
        .map_err(IntegrityLabelError::InitAcl)?;
    // **Policy obrigatória:** `SYSTEM_MANDATORY_POLICY_NO_WRITE_UP` (0x1)
    // é o que **faz** o rótulo bloquear escrita para cima
    // (processo Low não escreve em objeto Medium+). Sem
    // essa policy (mandatorypolicy=0), o ACE existe mas
    // **não bloqueia** — é só metadata. Mesmo motivo pelo
    // qual `icacls` mostra `(NW)` (= No Write Up) e não `()`.
    unsafe { AddMandatoryAce(acl_ptr, ACE_REVISION(2), ACE_FLAGS(0), 0x1, sid) }
        .map_err(IntegrityLabelError::AddAce)?;
    unsafe {
        let _ = FreeSid(sid);
    }

    // 3. Monta o SD **self-relative** escrevendo o header byte a
    //    byte. Nao dá pra usar a struct `SECURITY_DESCRIPTOR` do
    //    crate `windows` aqui: ela modela a forma **absoluta**,
    //    em que Owner/Group/Sacl/Dacl sao ponteiros de 8 bytes
    //    (a struct tem 40 bytes e `.Sacl` cai no offset 24). Na
    //    forma self-relative os quatro campos sao **offsets u32**
    //    nos bytes 4/8/12/16. Escrever pela struct punha o valor
    //    no lugar errado -- e ainda por cima dentro da area de
    //    dados que a copia do ACL sobrescrevia logo depois --,
    //    deixando OffsetSacl = 0. Com `SE_SACL_PRESENT` ligado e
    //    offset zero, o Windows le "SACL presente porem NULL":
    //    `SetFileSecurityW` devolve sucesso e nao aplica rotulo
    //    nenhum, em silencio.
    let acl_size = unsafe { (*acl_ptr).AclSize } as usize;
    let mut buffer = vec![0u8; 20 + acl_size];
    buffer[0] = 1; // Revision
    buffer[1] = 0; // Sbz1
                   // SE_SELF_RELATIVE (0x8000) | SE_SACL_PRESENT (0x10)
    buffer[2..4].copy_from_slice(&0x8010u16.to_le_bytes());
    buffer[4..8].copy_from_slice(&0u32.to_le_bytes()); // OffsetOwner
    buffer[8..12].copy_from_slice(&0u32.to_le_bytes()); // OffsetGroup
    buffer[12..16].copy_from_slice(&20u32.to_le_bytes()); // OffsetSacl
    buffer[16..20].copy_from_slice(&0u32.to_le_bytes()); // OffsetDacl
    buffer[20..20 + acl_size].copy_from_slice(&acl_buf[..acl_size]);

    let sd = PSECURITY_DESCRIPTOR(buffer.as_mut_ptr() as *mut _);
    Ok(LabelSd { sd, buffer })
}

/// Aplica `Mandatory Label\Low` (S-1-16-4096) no `path`. O
/// resultado: um processo com `TokenIntegrityLevel = Low` (ou
/// menor) consegue acessar esse path; um processo Medium não
/// consegue. **Inverso** pra arquivos com `Medium` (default) —
/// um processo Low é bloqueado.
///
/// **Implementação (SetFileSecurityW — equivalente ao `icacls`):**
/// `SetSecurityInfo` via handle falha com `E_ACCESSDENIED` mesmo
/// pro dono, em alguns Windows (a handle retornada por
/// `CreateFileW` não ganha `ACCESS_SYSTEM_SECURITY` mesmo com
/// `WRITE_DAC | WRITE_OWNER` na desired_access). `icacls` usa
/// `SetFileSecurityW` internamente, que **não depende de handle**
/// — usa o filename direto. Aqui seguimos o mesmo caminho:
/// monta o SD com `build_low_label_security_descriptor` (SACL
/// com label ACE Low, self-relative) e escreve via
/// `SetFileSecurityW(path, LABEL_SECURITY_INFORMATION, sd)`.
/// O OS faz merge com o SD atual (preserva owner/group/dacl).
///
/// **Por que não trocar pra Restricted SID (Plano B):** o user
/// tem o privilégio (icacls funciona), o problema é só na
/// minha escolha de API. Corrigir a API, não a estratégia.
///
/// **Erros:** ver `IntegrityLabelError`. Caller deve tratar
/// como hard-fail.
pub fn set_low_integrity_label(path: &Path) -> Result<(), IntegrityLabelError> {
    let path_str = path.to_string_lossy().into_owned();
    let path_wide = to_wide_null(&path_str);

    let label_sd = build_low_label_security_descriptor()?;

    let apply_ok = unsafe {
        SetFileSecurityW(
            PCWSTR(path_wide.as_ptr()),
            OBJECT_SECURITY_INFORMATION(LABEL_SECURITY_INFORMATION.0),
            label_sd.sd,
        )
    };
    if apply_ok.0 == 0 {
        let err = unsafe { windows::Win32::Foundation::GetLastError() }.0;
        return Err(IntegrityLabelError::ApplyAcl(path_str, err));
    }
    // `label_sd` sai do escopo aqui → `buffer` é droppada →
    // SD pointer fica inválido. OK porque SetFileSecurityW já
    // terminou.
    Ok(())
}

/// Cria o SID S-1-16-4096 (Mandatory Label Low) via
/// `AllocateAndInitializeSid` com
/// `SECURITY_MANDATORY_LABEL_AUTHORITY` (0x10) e 1 sub-authority
/// = 0x1000 (= 4096, o RID Low).
unsafe fn create_low_integrity_sid() -> Result<PSID, IntegrityLabelError> {
    let mut sid = PSID(std::ptr::null_mut());
    unsafe {
        AllocateAndInitializeSid(
            &SID_IDENTIFIER_AUTHORITY {
                Value: SECURITY_MANDATORY_LABEL_AUTHORITY.Value,
            },
            1,
            INTEGRITY_LEVEL_LOW,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut sid,
        )
    }
    .map_err(IntegrityLabelError::AllocateSid)?;
    Ok(sid)
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
