//! `RawChild` — wrapper minimal sobre os handles de um processo
//! filho criado via `CreateProcessAsUserW` raw (Etapa 5+ da
//! Fase 7, path safety enforcement).
//!
//! **Por que existe:** o `tokio::process::Command::spawn` não
//! permite passar um token pro `CreateProcessW` (a Win32 API
//! interna usa o token do parent). Pra path safety, precisamos
//! spawnar o filho com o `RestrictedToken` (que tem
//! `TokenIntegrityLevel = Low`). Isso exige `CreateProcessAsUserW`
//! raw, que retorna um `HANDLE` pro processo + handles pros
//! pipes de stdout/stderr/stdin (criados antes via `CreatePipe`).
//!
//! O `RawChild` mantém esses handles raw e expõe uma API
//! compatível com `tokio::process::Child`:
//! - `id()` → PID
//! - `wait_with_timeout(timeout)` → espera com timeout (kill
//!   em timeout)
//! - `kill()` → `TerminateProcess`
//! - `stdout_handle()` → `Option<HANDLE>` (o handle raw do pipe
//!   de stdout; o caller wrappa em `tokio::fs::File`)
//! - `stderr_handle()` → `Option<HANDLE>` (idem pra stderr)
//!
//! **Diferenças vs `tokio::process::Child`:**
//! - `stdout_handle()` retorna o `HANDLE` raw, não um
//!   `ChildStdout` (que o `tokio` esconde atrás de
//!   `BorrowedHandle`). O caller faz `tokio::fs::File::from_std(
//!   unsafe { std::fs::File::from_raw_handle(h as RawHandle) })`
//!   pra wrappear em `AsyncRead`.
//! - O wait é via `WaitForSingleObject` em `spawn_blocking`
//!   (não usa IOCP do tokio). Mais overhead, mas simples.
//!
//! **Lifetime:** o `RawChild` é dono dos handles. Drop chama
//! `CloseHandle` em cada um.
#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::RawHandle;
use std::time::Duration;
use thiserror::Error;
use tokio::task;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
};

/// Erro do `RawChild`.
#[derive(Debug, Error)]
pub enum RawChildError {
    #[error("WaitForSingleObject falhou: {0}")]
    Wait(windows::core::Error),
    #[error("GetExitCodeProcess falhou: {0}")]
    ExitCode(windows::core::Error),
    #[error("TerminateProcess falhou: {0}")]
    Terminate(windows::core::Error),
}

/// Status retornado pelo `wait`/`wait_with_timeout`. Diferente
/// do `std::process::ExitStatus` (que não conseguimos construir
/// fora do crate), esse é um tipo nosso.
#[derive(Debug, Clone, Copy)]
pub struct RawExitStatus {
    /// Exit code do processo. `0` = sucesso; diferente de 0
    /// = erro (convenção C).
    pub code: u32,
}

impl RawExitStatus {
    /// True se exit code == 0.
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Wrapper sobre os handles de um processo filho criado via
/// `CreateProcessAsUserW` raw.
pub struct RawChild {
    pid: u32,
    process_handle: HANDLE,
    stdout_handle: Option<HANDLE>,
    stderr_handle: Option<HANDLE>,
    /// `true` se o processo já foi "consumido" (wait feito) —
    /// impede duplo-wait.
    consumed: bool,
}

impl RawChild {
    /// Constrói a partir dos handles retornados por
    /// `CreateProcessAsUserW`. Toma ownership de todos os
    /// handles (drop fecha).
    ///
    /// **Parâmetros:**
    /// - `process_handle`: `hProcess` do `PROCESS_INFORMATION`.
    /// - `thread_handle`: `hThread` — fechado aqui (não é
    ///   necessário depois do spawn; o processo filho já
    ///   está rodando).
    /// - `stdout_handle` / `stderr_handle`: read end dos pipes
    ///   (write end foi fechado pelo caller após `CreatePipe`).
    ///   `None` se stdout/stderr não foi piped.
    pub fn new(
        process_handle: HANDLE,
        thread_handle: HANDLE,
        stdout_handle: Option<HANDLE>,
        stderr_handle: Option<HANDLE>,
    ) -> Self {
        // Fecha o thread handle (não precisamos mais).
        // SAFETY: handle veio de CreateProcessAsUserW.
        if !thread_handle.is_invalid() && !thread_handle.0.is_null() {
            unsafe {
                let _ = CloseHandle(thread_handle);
            }
        }
        Self {
            pid: unsafe { windows::Win32::System::Threading::GetProcessId(process_handle) },
            process_handle,
            stdout_handle,
            stderr_handle,
            consumed: false,
        }
    }

    /// PID do processo filho.
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Handle raw do processo (`HANDLE` Win32). Útil pra
    /// `OpenProcess` ou pra passar pra `AssignProcessToJobObject`.
    pub fn process_handle(&self) -> HANDLE {
        self.process_handle
    }

    /// Take do handle de stdout (read end do pipe). Converte
    /// pra `tokio::fs::File` no caller.
    pub fn take_stdout_handle(&mut self) -> Option<HANDLE> {
        self.stdout_handle.take()
    }

    /// Take do handle de stderr.
    pub fn take_stderr_handle(&mut self) -> Option<HANDLE> {
        self.stderr_handle.take()
    }

    /// Espera o processo terminar sem timeout. Bloqueia até o
    /// processo terminar (sucesso ou crash) ou até o handle ser
    /// invalidado. Para bounded wait, use [`wait_with_timeout`].
    ///
    /// **Cuidado:** sem timeout, um processo travado bloqueia o
    /// caller indefinidamente. A Etapa 4 da Fase 7 prefere
    /// sempre `wait_with_timeout` (o `collect_output` da
    /// `tool-registry` usa wall-clock enforcement real).
    pub async fn wait(&mut self) -> io::Result<RawExitStatus> {
        if self.consumed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            ));
        }
        // HANDLE (`*mut c_void`) não é Send; convertemos pra
        // isize (que é Send) e reconstruímos dentro do closure.
        // O cast é seguro em Windows (HANDLE cabe em isize).
        let process_handle_isize = self.process_handle.0 as isize;
        let wait_result = task::spawn_blocking(move || {
            // SAFETY: handle é válido (não fechado).
            // `INFINITE` em Win32 = `u32::MAX` (sem timeout).
            let h = HANDLE(process_handle_isize as *mut _);
            unsafe { WaitForSingleObject(h, u32::MAX) }
        })
        .await;

        match wait_result {
            Ok(WAIT_EVENT(0)) => {
                // WAIT_OBJECT_0 = signaled. Pega o exit code.
                self.consumed = true;
                self.get_exit_code()
            }
            Ok(other) => Err(io::Error::other(format!(
                "WaitForSingleObject retornou {other:?}"
            ))),
            Err(e) => Err(io::Error::other(format!("spawn_blocking join: {e}"))),
        }
    }

    /// Espera o processo terminar com timeout. Se o timeout
    /// expirar, chama `TerminateProcess` no child (que cascateia
    /// via Job Object → mata a árvore).
    pub async fn wait_with_timeout(&mut self, timeout: Duration) -> io::Result<RawExitStatus> {
        if self.consumed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            ));
        }
        // HANDLE (`*mut c_void`) não é Send; convertemos pra
        // isize (que é Send) e reconstruímos dentro do closure.
        let process_handle_isize = self.process_handle.0 as isize;
        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;

        // `WaitForSingleObject` é blocking → spawn_blocking.
        // `tokio::time::timeout` envolve o await com um timer.
        let wait_result = tokio::time::timeout(
            timeout,
            task::spawn_blocking(move || {
                // SAFETY: handle é válido (não fechado).
                let h = HANDLE(process_handle_isize as *mut _);
                unsafe { WaitForSingleObject(h, timeout_ms) }
            }),
        )
        .await;

        match wait_result {
            Ok(Ok(WAIT_TIMEOUT)) => {
                // Timeout: kill + retorna TimedOut.
                self.kill_inner()?;
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("wall-clock excedido (>{timeout:?})"),
                ))
            }
            Ok(Ok(WAIT_OBJECT_0)) => {
                // WAIT_OBJECT_0 (signaled). Pega o exit code.
                self.consumed = true;
                self.get_exit_code()
            }
            Ok(Ok(other)) => {
                // Outro retorno é erro.
                Err(io::Error::other(format!(
                    "WaitForSingleObject retornou {other:?}"
                )))
            }
            Ok(Err(e)) => Err(io::Error::other(format!("spawn_blocking join: {e}"))),
            Err(_) => {
                // tokio::time::timeout expired. Kill + TimedOut.
                self.kill_inner()?;
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("wall-clock excedido (>{timeout:?})"),
                ))
            }
        }
    }

    /// Mata o processo via `TerminateProcess`. Não cascateia
    /// pro neto sozinho — o drop do `Job` (no caller) é o que
    /// fecha o handle e dispara `KILL_ON_JOB_CLOSE`.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.kill_inner()
    }

    fn kill_inner(&mut self) -> io::Result<()> {
        // SAFETY: handle é válido (não fechado).
        let result = unsafe { TerminateProcess(self.process_handle, 1) };
        if let Err(e) = result {
            return Err(io::Error::other(format!("TerminateProcess: {e}")));
        }
        Ok(())
    }

    /// Pega o exit code via `GetExitCodeProcess`. Usado internamente
    /// após `wait` signaled.
    fn get_exit_code(&self) -> io::Result<RawExitStatus> {
        let mut code: u32 = 0;
        // SAFETY: handle é válido.
        unsafe { GetExitCodeProcess(self.process_handle, &mut code) }
            .map_err(|e| io::Error::other(format!("GetExitCodeProcess: {e}")))?;
        Ok(RawExitStatus { code })
    }
}

impl Drop for RawChild {
    fn drop(&mut self) {
        // SAFETY: handles vieram de CreateProcessAsUserW /
        // CreatePipe; nunca fechados em outro lugar.
        if !self.process_handle.is_invalid() && !self.process_handle.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.process_handle);
            }
        }
        if let Some(h) = self.stdout_handle.take() {
            if !h.is_invalid() && !h.0.is_null() {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        }
        if let Some(h) = self.stderr_handle.take() {
            if !h.is_invalid() && !h.0.is_null() {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        }
    }
}

/// Wrappa um `HANDLE` (read end de um pipe) em `tokio::fs::File`
/// (que implementa `AsyncRead`).
///
/// **Por que existe:** `output.rs` precisa de um `AsyncRead`
/// pra ler stdout/stderr do processo filho. `tokio::process::ChildStdout`
/// faz isso, mas a gente não consegue construir um desses
/// diretamente. Solução: wrappa o `HANDLE` raw num
/// `tokio::fs::File` (que aceita `OwnedHandle` via `from_std`).
pub fn wrap_pipe_handle_as_async_file(handle: HANDLE) -> std::io::Result<tokio::fs::File> {
    use std::fs::File as StdFile;
    use std::os::windows::io::FromRawHandle;
    let raw: RawHandle = handle.0 as RawHandle;
    // SAFETY: `raw` é um handle válido (read end de um pipe
    // criado por CreatePipe). `File::from_raw_handle` toma
    // ownership. `tokio::fs::File::from_std` wrappa em async.
    let std_file = unsafe { StdFile::from_raw_handle(raw) };
    std_file
        .set_async_handle()
        .map_err(|e| std::io::Error::other(format!("tokio::fs::File::from_std: {e}")))
}

/// Trait extension pra `std::fs::File` que adiciona o método
/// `set_async_handle` (no `tokio::fs::File::from_std` é chamado
/// `set_async_handle` mas o trait extension é privado). Vamos
/// usar `from_std` direto.
pub trait FileExt {
    fn set_async_handle(self) -> std::io::Result<tokio::fs::File>;
}

impl FileExt for std::fs::File {
    fn set_async_handle(self) -> std::io::Result<tokio::fs::File> {
        Ok(tokio::fs::File::from_std(self))
    }
}

// `RawChild` é Send (HANDLEs são thread-safe por convenção).
unsafe impl Send for RawChild {}
unsafe impl Sync for RawChild {}
