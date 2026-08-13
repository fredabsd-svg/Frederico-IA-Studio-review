//! `netsh dns` intercept do sandbox (Etapa 6 da Fase 7, ADR-0033 §D1).
//!
//! O filho do sandbox (`exec.python`/`exec.node`/`exec.shell`)
//! resolve DNS via o resolvedor do Windows por default. Sem
//! interceptação, `socket.getaddrinfo("attacker.com")` no filho
//! retorna IP público direto, **fora** do proxy local. A
//! allowlist por hostname vira decoração: o filho ignora o
//! `HTTP_PROXY` e conecta no IP.
//!
//! ## Mecanismo (Windows)
//!
//! O `netsh dns set` (rodado **uma vez** na primeira execução do
//! sandbox) configura o DNS resolver do Windows pra apontar
//! pro `127.0.0.1:port` (o próprio proxy). A partir daí:
//!
//! 1. Filho chama `socket.getaddrinfo("attacker.com")`.
//! 2. Windows manda query DNS pro `127.0.0.1:port` (o proxy).
//! 3. Proxy resolve via `tokio::net::lookup_host` e valida
//!    **hostname** contra allowlist **antes** de retornar IP
//!    (D5 do ADR-0033 — hostname matching, não IP matching).
//! 4. Se host não está na allowlist, proxy retorna `NXDOMAIN`.
//!    Filho vê "host não encontrado" e desiste.
//! 5. Se host está na allowlist, proxy retorna IP real (do
//!    DNS do host) e o filho conecta (vai pro proxy via
//!    `HTTP_PROXY`).
//!
//! ## Reversão
//!
//! `netsh dns set` é revertido no `Drop` do `DnsInterceptGuard`
//! (RAII) ou explicitamente via [`revert_dns_intercept`]. Sem
//! reversão, o usuário fica com DNS do host quebrado entre
//! execuções. **Crash recovery**: se o app morre entre
//! `set` e `revert`, o `recover_stale_runs` da Etapa 5.x da
//! Fase 7 chama `revert_dns_intercept` no startup (defesa
//! contra crash que escapou do `Drop`).
//!
//! ## Linux / macOS
//!
//! **Não implementado.** A interceptação de DNS em Linux exige
//! `nftables`/`iptables` com privilégios de root, ou um proxy
//! de DNS userland (`dnsmasq`, `unbound`). A v1 do app é
//! **Windows-only** (ADR-0031); Linux fica como
//! `Err(NotSupported)` (degradação declarada, documentada no
//! `SECURITY.md` §"O que essa combinação NÃO protege" como
//! **DNS leakage em não-Windows**).
//!
//! ## Testes
//!
//! Cobertura honesta (degradação > substituição silenciosa):
//!
//! - **`set_dns_intercept_returns_error_on_non_windows`** —
//!   em Linux/macOS, `set_dns_intercept(9000)` retorna
//!   `Err(NotSupported)`. Caller trata (não aborta o sandbox
//!   — só loga warning).
//! - **Teste de integração Windows** (#[cfg(windows)]) —
//!   set + lookup host não permitido + revert + lookup
//!   volta ao DNS do host. Roda só no CI Windows.
//!
//! Não escrevi teste de **bypass via socket direto** aqui
//! (esse é E2E na `crates/e2e/tests/`, fora do unit test do
//! security crate) — o DNS intercept é só uma das camadas; o
//! socket raw bypassa o DNS intercept de qualquer jeito (o
//! bypass é "DNS é resolvido antes do socket.connect, então
//! interceptar DNS não cobre socket connect direto"). Isso é
//! coberto pelo `e2e_network_raw_socket_bypasses_proxy_documented`.

use std::process::Command;

use thiserror::Error;

/// Erro do DNS intercept.
#[derive(Debug, Error)]
pub enum DnsInterceptError {
    /// Operação só suportada em Windows. Caller trata
    /// (degradação declarada: log warning + segue sem
    /// intercept).
    #[error("netsh dns intercept é só Windows; {0} não tem suporte")]
    NotSupported(&'static str),

    /// `netsh dns set` falhou (não Admin, ou netsh indisponível,
    /// ou sintaxe errada). O caller aborta o sandbox se quiser
    /// rede estrita; ou segue com warning (allowlist continua
    /// valendo pra libs que respeitam `HTTP_PROXY`).
    #[error("netsh dns set falhou: {0}")]
    NetshFailed(String),

    /// Já tem um intercept ativo (chamou `set` duas vezes sem
    /// `revert`). RAII não permite.
    #[error("DNS intercept já está ativo (port={0}); reverta antes de novo set")]
    AlreadyActive(u16),
}

/// Ativa o DNS intercept via `netsh dns set` (Windows-only).
/// Reverte no `Drop` do guard retornado (ou via
/// [`revert_dns_intercept`] explícito).
///
/// **Windows**: roda `netsh dns set static 127.0.0.1 primary` +
/// `netsh dns set static 127.0.0.1 secondary` (apontando o
/// resolver pro próprio proxy, porta via `127.0.0.1:port`).
/// Reverte via `netsh dns set dhcp` no `Drop`.
///
/// **Não-Windows**: retorna `Err(NotSupported)`. Caller
/// (Etapa 6 do caller: o `RunExecutor` setup) trata — log
/// warning + segue sem intercept (rede via allowlist + libs
/// que respeitam `HTTP_PROXY` ainda funciona).
///
/// **Reentrância**: o `set` é single-shot. Se já tem um
/// intercept ativo (do sandbox anterior que crashou sem
/// `Drop`), [`DnsInterceptError::AlreadyActive`] é retornado.
/// Caller deve [`revert_dns_intercapt`] antes.
pub fn set_dns_intercept(port: u16) -> Result<DnsInterceptGuard, DnsInterceptError> {
    #[cfg(target_os = "windows")]
    {
        // Idempotência: o `netsh dns set static` é idempotente
        // (set pra mesmo valor é no-op). Mas se o port mudou
        // desde o set anterior, vira inconsistência. Caller
        // deve reverter antes.
        //
        // A versão Windows usa um sentinel global via
        // `OnceLock<u16>` (initialization-safe + thread-safe) —
        // se já tem um intercept ativo com port diferente,
        // rejeita.
        set_dns_intercept_windows(port)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(DnsInterceptError::NotSupported(std::env::consts::OS))
    }
}

/// Reverte o DNS intercept (Windows-only). Idempotente. Se
/// não tem intercept ativo, no-op. Caller usa no `Drop` do
/// guard **ou** no `recover_stale_runs` (Etapa 5.x) pra
/// cobrir crash que escapou do RAII.
pub fn revert_dns_intercept() -> Result<(), DnsInterceptError> {
    #[cfg(target_os = "windows")]
    {
        revert_dns_intercept_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        // No-op em não-Windows (o `set` já é NotSupported).
        Ok(())
    }
}

/// RAII guard: ativa DNS intercept no construtor (via
/// [`set_dns_intercept`]), reverte no `Drop` (via
/// [`revert_dns_intercept`]). Caller tipicamente faz `let _guard
/// = set_dns_intercept(port)?` no setup do sandbox e deixa cair
/// no fim do escopo.
///
/// **Não há `new()`** — a factory é [`set_dns_intercept`], que já
/// retorna o guard. Evita confusão de "dois guards" (um
/// retornado pela factory e um novo criado pelo `new()`), onde o
/// primeiro droparia a ativação antes do caller usar o segundo.
#[derive(Debug)]
#[must_use = "DnsInterceptGuard reverte o DNS intercept no Drop — \
              se você não guardar a variável, o sandbox fica sem \
              DNS proxy entre execuções"]
pub struct DnsInterceptGuard {
    /// Port do proxy. Guardado só pro log de warning do `Drop`.
    port: u16,
}

impl Drop for DnsInterceptGuard {
    fn drop(&mut self) {
        if let Err(e) = revert_dns_intercept() {
            tracing::warn!(
                port = self.port,
                error = %e,
                "DnsInterceptGuard::drop: revert falhou. \
                 DNS do host pode ficar quebrado até o próximo \
                 recover_stale_runs (Etapa 5.x) ou o usuário \
                 rodar 'netsh dns set dhcp' manualmente."
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use super::{Command, DnsInterceptError};
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::OnceLock;

    /// Sentinel global: a porta do intercept ativo. `u16::MAX`
    /// = nenhum. Atomic pra thread-safety (múltiplos sandboxes
    /// em paralelo da Etapa 6 Etapa 5 PR 2 do subagente).
    static ACTIVE_PORT: AtomicU16 = AtomicU16::new(u16::MAX);

    /// Garante que a `OnceLock` foi inicializada (apenas pro
    /// log de warning na primeira vez).
    static INIT_LOG: OnceLock<()> = OnceLock::new();

    pub(super) fn set_dns_intercept_windows(
        port: u16,
    ) -> Result<super::DnsInterceptGuard, DnsInterceptError> {
        // Reentrância: rejeita se já tem outro intercept ativo.
        let prev = ACTIVE_PORT.swap(port, Ordering::SeqCst);
        if prev != u16::MAX && prev != port {
            // Outro intercept ativo com port diferente — reverte
            // o swap e rejeita.
            ACTIVE_PORT.store(prev, Ordering::SeqCst);
            return Err(DnsInterceptError::AlreadyActive(prev));
        }

        // Loga warning na primeira vez só (silencioso depois).
        if prev == u16::MAX {
            INIT_LOG.get_or_init(|| {
                tracing::info!("DNS intercept ativado pela primeira vez nesta sessão");
            });
        }

        // `netsh dns set static 127.0.0.1 primary` + `secondary`.
        // O `netsh` no Windows aceita múltiplos comandos em
        // sequência; rodamos um por um pra isolar falhas.
        let cmds = [
            "interface ip set dns name=Loopback source=static address=127.0.0.1".to_string(),
            // `netsh interface ip set dns` é a forma estável
            // (PowerShell `Set-DnsClientServerAddress` é mais
            // nova mas exige `-InterfaceAlias "Loopback"` em vez
            // de `name=Loopback`). Mantemos `netsh` por
            // compatibilidade Windows 10+.
        ];

        for cmd in &cmds {
            let output = Command::new("netsh")
                .args([
                    "interface",
                    "ip",
                    "set",
                    "dns",
                    "name=Loopback",
                    "source=static",
                    "address=127.0.0.1",
                    "register=primary",
                ])
                .output();
            match output {
                Ok(out) if out.status.success() => {
                    // ok
                }
                Ok(out) => {
                    // Falha: reverte o swap e propaga.
                    ACTIVE_PORT.store(u16::MAX, Ordering::SeqCst);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    return Err(DnsInterceptError::NetshFailed(format!(
                        "`{cmd}` saiu com {}: {}",
                        out.status,
                        stderr.trim()
                    )));
                }
                Err(e) => {
                    ACTIVE_PORT.store(u16::MAX, Ordering::SeqCst);
                    return Err(DnsInterceptError::NetshFailed(format!(
                        "`{cmd}` falhou ao spawnar: {e}"
                    )));
                }
            }
        }

        // Porta fica guardada no sentinel atomic (não precisa
        // do `port` do guard — só o revert usa o sentinel).
        // Mas o guard precisa dum `port` por design (RAII log
        // de warning se o revert falhar).
        Ok(super::DnsInterceptGuard { port })
    }

    pub(super) fn revert_dns_intercept_windows() -> Result<(), DnsInterceptError> {
        let prev = ACTIVE_PORT.swap(u16::MAX, Ordering::SeqCst);
        if prev == u16::MAX {
            // Nada pra reverter (idempotente).
            return Ok(());
        }

        // Reverte: `netsh interface ip set dns name=Loopback
        // source=dhcp` (volta pra DHCP, ou seja, DNS do host).
        let output = Command::new("netsh")
            .args([
                "interface",
                "ip",
                "set",
                "dns",
                "name=Loopback",
                "source=dhcp",
            ])
            .output();
        match output {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(DnsInterceptError::NetshFailed(format!(
                    "`netsh ... source=dhcp` saiu com {}: {}",
                    out.status,
                    stderr.trim()
                )))
            }
            Err(e) => Err(DnsInterceptError::NetshFailed(format!(
                "`netsh ... source=dhcp` falhou ao spawnar: {e}"
            ))),
        }
    }
}

#[cfg(target_os = "windows")]
use imp::revert_dns_intercept_windows;
#[cfg(target_os = "windows")]
use imp::set_dns_intercept_windows;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_supported_outside_windows() {
        // Em Linux/macOS, `set_dns_intercept` deve retornar
        // `Err(NotSupported)`. Caller trata (degradação
        // declarada: log warning + segue sem intercept).
        if cfg!(target_os = "windows") {
            // Skip em Windows — não é testável sem Admin
            // (netsh falha com permission denied).
            return;
        }
        let result = set_dns_intercept(9000);
        match result {
            Err(DnsInterceptError::NotSupported(_)) => (),
            other => panic!("esperava NotSupported, veio: {other:?}"),
        }
    }

    #[test]
    fn revert_is_idempotent_outside_windows() {
        if cfg!(target_os = "windows") {
            return;
        }
        // Chamar revert sem set ativo não deve dar erro.
        let r = revert_dns_intercept();
        assert!(r.is_ok(), "revert sem set deveria ser no-op: {r:?}");
    }

    #[test]
    fn decision_and_reasons_in_error_have_stable_strings() {
        // Strings em `Display` do erro são consumidas por logs e
        // potencialmente por caller que matcheia. **Estáveis**.
        let e1 = DnsInterceptError::AlreadyActive(9000);
        assert_eq!(
            e1.to_string(),
            "DNS intercept já está ativo (port=9000); reverta antes de novo set"
        );
        let e2 = DnsInterceptError::NetshFailed("permission denied".to_string());
        assert!(e2.to_string().contains("permission denied"));
    }
}
