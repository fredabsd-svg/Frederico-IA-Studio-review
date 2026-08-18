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
//!   em timeout). O veredito vem de um deadline absoluto
//!   comparado ao `ExitTime` que o kernel registrou, **não** do
//!   nosso relógio nem de quem venceu a corrida de espera — ver
//!   o doc do método.
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
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;
use tokio::task;
use windows::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, TerminateProcess, WaitForSingleObject,
};

/// Distância entre a época do `FILETIME` (1601-01-01 UTC) e a época
/// Unix (1970-01-01 UTC), em unidades de 100 ns. Permite comparar o
/// `ExitTime` do `GetProcessTimes` com um instante obtido de
/// `SystemTime::now()` — no Windows, ambos vêm do **mesmo** relógio
/// de sistema (`SystemTime::now()` chama
/// `GetSystemTimePreciseAsFileTime` por baixo), então a comparação é
/// exata e não exige nenhuma feature nova do crate `windows`.
const UNIX_EPOCH_EM_FILETIME: u64 = 116_444_736_000_000_000;

/// "Agora" no mesmo relógio do `FILETIME` (unidades de 100 ns desde
/// 1601-01-01 UTC).
fn agora_em_filetime() -> u64 {
    let desde_unix = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_EM_FILETIME.saturating_add((desde_unix.as_nanos() / 100) as u64)
}

/// Converte um `FILETIME` (par de `u32`) no `u64` de 100 ns.
fn filetime_para_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

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
    ///
    /// ## Por que o veredito não é medido pelo nosso relógio
    ///
    /// A v1 (Etapa 4/5+ da Fase 7) corria **dois relógios
    /// relativos** e aceitava o primeiro que fosse observado:
    /// `WaitForSingleObject(h, timeout_ms)` dentro de um
    /// `spawn_blocking`, envolvido por um `tokio::time::timeout`.
    /// Os dois só começam a contar quando alguma thread nossa
    /// recebe CPU, e isso quebrava o contrato de duas formas
    /// (ambas medidas em 2026-08-17, com carga artificial de CPU):
    ///
    /// 1. **O orçamento crescia com o atraso de despacho.** O
    ///    `spawn_blocking` só rodava 1,6 s a 4,1 s depois de ser
    ///    enfileirado; o `WaitForSingleObject` então contava os
    ///    2 s *a partir dali*. O orçamento efetivo virava
    ///    `atraso + wall_clock`, sem teto.
    /// 2. **O estouro podia virar sucesso (fail-open).** Se o
    ///    atraso passasse do tempo de vida do filho, o
    ///    `WaitForSingleObject` encontrava o processo **já
    ///    encerrado**, devolvia `WAIT_OBJECT_0` com exit code 0,
    ///    e a ferramenta reportava **sucesso** para um run que
    ///    estourou o wall clock em 5×. Foi exatamente o que o
    ///    `wall_clock_kills_long_running_process` viu
    ///    (`elapsed=13.4s ok=true` com `max_wall_clock_ms=2000`).
    ///
    /// A v2 separa as duas coisas:
    ///
    /// - **Correção (fail-closed)** — o veredito sai de um
    ///   **deadline absoluto**, tirado uma única vez aqui, antes
    ///   de qualquer despacho de thread, e comparado com o
    ///   `ExitTime` que o **kernel** registrou para o processo
    ///   (`GetProcessTimes`). Quem decide é o kernel, não o
    ///   escalonador: um filho que viveu além do orçamento é
    ///   sempre reportado como estouro, ainda que só percebamos
    ///   isso segundos depois; e um filho que coube no orçamento
    ///   nunca é reportado como estouro, ainda que só o
    ///   observemos tarde.
    /// - **Prontidão (matar cedo)** — `timeout_at` no mesmo
    ///   deadline absoluto, e a espera bloqueante limitada ao que
    ///   **resta** do orçamento. Isso é o que depende de
    ///   escalonamento, e é só isso que a carga degrada: sob
    ///   starvation o filho pode sobreviver além dos 2 s até
    ///   conseguirmos rodar o `TerminateProcess` — mas o
    ///   resultado devolvido continua correto.
    pub async fn wait_with_timeout(&mut self, timeout: Duration) -> io::Result<RawExitStatus> {
        if self.consumed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "processo ja foi consumido",
            ));
        }
        // Âncora do wall clock, tirada **uma vez**, antes de
        // enfileirar qualquer thread. Duas leituras do mesmo
        // instante:
        // - `deadline` (monotônico) governa a espera e o timer;
        // - `deadline_ft` (relógio de sistema) é a única régua do
        //   veredito, porque é o relógio em que o kernel carimba
        //   o `ExitTime` do processo.
        let deadline = Instant::now() + timeout;
        let deadline_ft = agora_em_filetime().saturating_add((timeout.as_nanos() / 100) as u64);

        // HANDLE (`*mut c_void`) não é Send; convertemos pra
        // isize (que é Send) e reconstruímos dentro do closure.
        let process_handle_isize = self.process_handle.0 as isize;

        // `WaitForSingleObject` é blocking → spawn_blocking.
        // `timeout_at` (não `timeout`) porque o deadline já é
        // absoluto: o timer não pode ser reancorado no instante
        // em que esta linha por acaso executar.
        let wait_result = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            task::spawn_blocking(move || {
                // Espera só o que **resta** do orçamento no
                // instante em que esta thread de fato rodou. Se o
                // despacho atrasou além do deadline, `restante` é
                // zero e o wait vira um poll — o atraso não
                // estende mais o orçamento do filho.
                let restante = deadline.saturating_duration_since(Instant::now());
                let ms = restante.as_millis().min(u32::MAX as u128) as u32;
                // SAFETY: handle é válido (não fechado).
                let h = HANDLE(process_handle_isize as *mut _);
                unsafe { WaitForSingleObject(h, ms) }
            }),
        )
        .await;

        // Erros estruturais (join do blocking pool, retorno
        // inesperado do Win32) são falha de verdade e não passam
        // pela decisão de wall clock — confundir os dois esconderia
        // bug nosso atrás de "o filho demorou".
        match wait_result {
            Ok(Err(e)) => return Err(io::Error::other(format!("spawn_blocking join: {e}"))),
            Ok(Ok(r)) if r != WAIT_OBJECT_0 && r != WAIT_TIMEOUT => {
                return Err(io::Error::other(format!(
                    "WaitForSingleObject retornou {r:?}"
                )));
            }
            // `WAIT_OBJECT_0`, `WAIT_TIMEOUT` ou `Err(Elapsed)`:
            // todos são apenas "a corrida terminou". O que
            // aconteceu de fato, quem responde é o kernel abaixo.
            _ => {}
        }

        self.veredito_do_wall_clock(deadline_ft, timeout)
    }

    /// Ponto **único** de decisão do wall clock. Não pergunta que
    /// via da corrida venceu nem quando fomos escalonados;
    /// pergunta ao kernel o que aconteceu com o processo e
    /// **quando**.
    ///
    /// - Processo ainda vivo depois do deadline → estouro real:
    ///   `TerminateProcess` + `TimedOut`.
    /// - Processo já encerrado **dentro** do orçamento → sucesso,
    ///   com o exit code de verdade (mesmo que só tenhamos
    ///   percebido muito depois — nada de falso positivo).
    /// - Processo já encerrado **depois** do orçamento → estouro,
    ///   ainda que o `WaitForSingleObject` tenha devolvido
    ///   `WAIT_OBJECT_0`. É este ramo que fecha o fail-open da v1.
    fn veredito_do_wall_clock(
        &mut self,
        deadline_ft: u64,
        timeout: Duration,
    ) -> io::Result<RawExitStatus> {
        // Poll não-bloqueante (0 ms): pode rodar na worker thread.
        // SAFETY: handle é válido (não fechado).
        let sinalizado = unsafe { WaitForSingleObject(self.process_handle, 0) };

        if sinalizado == WAIT_OBJECT_0 {
            self.consumed = true;
            let saiu_em = self.exit_filetime()?;
            if saiu_em <= deadline_ft {
                return self.get_exit_code();
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("wall-clock excedido (>{timeout:?})"),
            ));
        }

        self.kill_inner()?;
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("wall-clock excedido (>{timeout:?})"),
        ))
    }

    /// `ExitTime` do processo (100 ns desde 1601-01-01 UTC), como o
    /// kernel carimbou. Só faz sentido depois que o processo
    /// encerrou.
    fn exit_filetime(&self) -> io::Result<u64> {
        let mut criacao = FILETIME::default();
        let mut saida = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut usuario = FILETIME::default();
        // SAFETY: handle é válido; os quatro out-params são nossos.
        unsafe {
            GetProcessTimes(
                self.process_handle,
                &mut criacao,
                &mut saida,
                &mut kernel,
                &mut usuario,
            )
        }
        .map_err(|e| io::Error::other(format!("GetProcessTimes: {e}")))?;
        Ok(filetime_para_u64(saida))
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
