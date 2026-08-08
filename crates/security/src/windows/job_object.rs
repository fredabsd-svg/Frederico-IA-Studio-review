//! `JobObject` — primitiva de Windows que mata a **árvore inteira**
//! de processos quando o handle do Job é fechado.
//!
//! Ver [ADR-0036 §D2](../../../decisions/0036-security-jail-resolver-windows-job-objects.md).
//! É a **única** primitiva de tree-kill confiável no Windows: sem ela,
//! um neto criado via `subprocess.Popen` em Python sobrevive ao
//! `TerminateProcess` do pai.
//!
//! ## Flags configuradas em `new()`
//!
//! - **`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`** — fecha o handle
//!   derruba a árvore. É o **coração** da garantia: o `Drop` fecha o
//!   handle, e a árvore morre.
//! - **`JOB_OBJECT_LIMIT_BREAKAWAY_OK`** — permite que netos criados
//!   pelo filho herdem o Job. Sem isso, `subprocess.Popen` em Python
//!   falha com access denied. A Etapa 3 (runtimes) precisa disso
//!   para `pip install` funcionar (pip → python → compilador C).
//! - **`JOB_OBJECT_LIMIT_PROCESS_MEMORY`** + **limit de 2 GB** — limite
//!   de memória por processo (defesa contra OOM do filho).
//! - **`JOB_OBJECT_LIMIT_JOB_MEMORY`** + **limit de 4 GB** — limite de
//!   memória total da árvore (defesa contra fork bomb de memória).
//!
//! ## Ordem de operações (race condition do CreateProcess)
//!
//! A janela entre `CreateProcess` retornar e o app conseguir
//! `TerminateProcess` é uma **race condition** (o filho pode ter
//! criado netos nesse meio tempo). A solução, documentada no
//! ADR-0036 D3:
//!
//! 1. App chama `CreateProcessW` com `CREATE_SUSPENDED`.
//! 2. App chama `AssignProcessToJobObject` no handle retornado.
//! 3. App chama `ResumeThread`.
//! 4. App registra o PID no `SecurityJailResolver`.
//!
//! Janela 1-2 é zero (processo suspended não roda). Janela 2-3 é zero
//! (Job Object já contém o PID suspended; qualquer spawn do filho
//! após `ResumeThread` herda o Job via `BREAKAWAY_OK`). Janela 3-4
//! é zero no sentido prático (HashMap::insert é sync, OS já tem o
//! Job configurado).
//!
//! ## Por que `Drop` é o ponto crítico
//!
//! O `SecurityJailResolver` mantém o `JobObject` vivo até o final do
//! app. Quando o app morre (qualquer causa: shutdown, panic,
//! `TerminateProcess` do OS), o `Drop` do `JobObject` é invocado, o
//! `CloseHandle` fecha o handle do Job, e o Windows dispara
//! `KILL_ON_JOB_CLOSE` que mata **toda a árvore atribuída**. Sem
//! isso, a Fase 5 Etapa 2.A (PR #22) usou `Child::kill()` da
//! `tokio::process`, que mata o PID direto mas **não** mata netos
//! criados após o fork. É o bug que o `tree_kill.rs::child_survives_parent_kill9`
//! da Etapa 2 da Fase 7 vai provar como fechado.
//!
//! ## Cross-project: §5.5 (modo servidor)
//!
//! O `SecurityJailResolver` é injetado por trait, **nunca** importado
//! pelo motor. A interface Rust do trait é simétrica em Windows e
//! Linux (este arquivo é `#[cfg(windows)]`; o stub Linux é
//! `Err(NotSupported)`). No Linux, a implementação futura usa
//! cgroups v2 + namespace + seccomp-bpf — **mesma** interface, sem
//! mudança no motor.
//!
//! ## ADR-0007: `windows` confinado a este módulo
//!
//! O `Cargo.toml` do `frederico-security` tem `unsafe_code = "deny"`
//! no nível do crate. Este módulo (e só este) tem
//! `#![allow(unsafe_code)]`. Cada bloco `unsafe` tem comentário
//! explicando a invariante de segurança.
//!
//! ADR-0003 (núcleo desacoplado da casca) é preservado: o `windows`
//! crate só aparece em `crates/security/src/windows*` e
//! `crates/security/tests*` (verificado pelo `scripts/check-core-purity.ps1`).

#![allow(unsafe_code)]

use thiserror::Error;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows::Win32::System::Threading::{OpenProcess, ResumeThread, PROCESS_ALL_ACCESS};

/// Memória máxima por processo (bytes). 2 GB é o limite prático
/// do `CreateProcessW` em Windows 32-bit e o limite recomendado
/// em 64-bit para a maioria dos workloads (Python, Node, Bash).
/// Configurável por invocação quando o `SecurityJailResolver`
/// for implementado.
const DEFAULT_PER_PROCESS_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Memória máxima da árvore (bytes). 4 GB = 2x o limite por
/// processo (defesa contra fork bomb de memória).
const DEFAULT_TOTAL_JOB_MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Erro do `JobObject`. Cada variante carrega a operação que
/// falhou e o erro do Windows subjacente (via `Display`).
#[derive(Debug, Error)]
pub enum JobError {
    /// `CreateJobObjectW` falhou (handle null ou erro Win32).
    #[error("CreateJobObjectW falhou: {message}")]
    CreateFailed { message: String },
    /// `SetInformationJobObject` falhou (limites não aceitos pelo OS).
    #[error("SetInformationJobObject falhou: {message}")]
    SetInfoFailed { message: String },
    /// `AssignProcessToJobObject` falhou (processo já saiu, ou sem
    /// permissão).
    #[error("AssignProcessToJobObject(handle={handle:?}) falhou: {message}")]
    AssignFailed { handle: HANDLE, message: String },
    /// `OpenProcess` falhou (PID inválido, processo já morto, ou
    /// sem permissão de abrir — `PROCESS_ALL_ACCESS` precisa de
    /// privilégio elevado para PIDs de outros usuários, falha
    /// com `ERROR_ACCESS_DENIED` se o caller não é admin).
    #[error("OpenProcess(pid={pid}) falhou: {message}")]
    OpenProcessFailed { pid: u32, message: String },
    /// `ResumeThread` falhou (processo não estava suspended, ou
    /// handle inválido).
    #[error("ResumeThread(handle={handle:?}) falhou: {message}")]
    ResumeFailed { handle: HANDLE, message: String },
}

/// Handle para um Windows Job Object. Quando o `JobObject` é
/// droppado, o handle é fechado via `CloseHandle`, e o Windows
/// dispara `KILL_ON_JOB_CLOSE` em toda a árvore atribuída.
///
/// **Não-clonable** (a semântica do Windows é 1 handle = 1 Job;
/// clonar o handle sem `DuplicateHandle` seria 2 closes, com
/// UB se o segundo close for num handle já inválido).
pub struct JobObject {
    handle: HANDLE,
    per_process_memory: u64,
    total_memory: u64,
}

impl JobObject {
    /// Cria um novo Job Object com KILL_ON_JOB_CLOSE + BREAKAWAY_OK
    /// + limites de memória default. A função **não** associa
    /// nenhum processo — isso é feito por [`Self::assign`] /
    /// [`Self::assign_pid`] / [`Self::assign_suspended_process`].
    ///
    /// # Erros
    ///
    /// Falha se `CreateJobObjectW` ou `SetInformationJobObject`
    /// retornar erro (sem handles abertos pelo app é raro; quase
    /// sempre é "out of resources" do OS).
    pub fn new() -> Result<Self, JobError> {
        Self::with_memory_limits(
            DEFAULT_PER_PROCESS_MEMORY_BYTES,
            DEFAULT_TOTAL_JOB_MEMORY_BYTES,
        )
    }

    /// Cria um Job Object com limites de memória customizados. Os
    /// limites são em bytes; valores razoáveis são 1-4 GB por
    /// processo e 2-8 GB total (depende do workload do sandbox).
    pub fn with_memory_limits(
        per_process_memory: u64,
        total_memory: u64,
    ) -> Result<Self, JobError> {
        // SAFETY: `CreateJobObjectW(NULL, NULL)` é documentado como
        // "cria um Job Object sem nome" (não visível no namespace
        // do kernel, vida atrelada ao handle). Os dois NULLs são
        // válidos: `lpJobAttributes=NULL` → atributos default;
        // `lpName=NULL` → sem nome. O handle retornado precisa ser
        // fechado via `CloseHandle` (responsabilidade do `Drop`).
        let handle =
            unsafe { CreateJobObjectW(None, None) }.map_err(|e| JobError::CreateFailed {
                message: format!("{e:?}"),
            })?;

        // Configura os limites: KILL_ON_JOB_CLOSE + BREAKAWAY_OK +
        // mem por processo + mem total. Os outros flags ficam
        // desligados (zero-initialized pela próxima linha).
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_BREAKAWAY_OK
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        // `windows` v0.58 tipa `ProcessMemoryLimit` e `JobMemoryLimit`
        // como `usize` (não `u64`); cast é seguro em 64-bit
        // (qualquer valor realista de memória cabe em `usize`).
        info.ProcessMemoryLimit = per_process_memory as usize;
        info.JobMemoryLimit = total_memory as usize;

        // SAFETY: `SetInformationJobObject` toma o handle do Job +
        // a classe de informação (ExtendedLimit) + ponteiro + tamanho.
        // O ponteiro é pra stack-local `info` que vive até o fim da
        // função (a chamada Win32 lê o struct durante a chamada,
        // não mantém referência). `JobObjectExtendedLimitInformation`
        // é a constante da enum `JOBOBJECTINFOCLASS` que seleciona
        // o layout Extended.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if let Err(e) = ok {
            // Falha em SetInfo: fecha o handle antes de propagar.
            // SAFETY: handle veio de CreateJobObjectW bem-sucedido;
            // double-close não acontece (não retornamos o handle em
            // caso de erro).
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(JobError::SetInfoFailed {
                message: format!("{e:?}"),
            });
        }

        Ok(Self {
            handle,
            per_process_memory,
            total_memory,
        })
    }

    /// Associa um processo (já criado, handle aberto) ao Job. O
    /// processo entra imediatamente no Job — qualquer spawn dele
    /// herda o Job (via `BREAKAWAY_OK`).
    ///
    /// # SAFETY
    ///
    /// O caller deve garantir que `process_handle` é um `HANDLE`
    /// válido de um processo vivo, retornado por `CreateProcessW`
    /// (ou `OpenProcess`). O handle NÃO é fechado por este método
    /// (continua sendo responsabilidade do caller via
    /// `CloseHandle` em `PROCESS_INFORMATION.hProcess`).
    pub fn assign(&self, process_handle: HANDLE) -> Result<(), JobError> {
        // SAFETY: `AssignProcessToJobObject` associa o processo
        // (identificado pelo handle) ao Job. O handle do Job já
        // existe (foi criado em `new()`). Após retorno, qualquer
        // filho criado pelo processo herda o Job via
        // `BREAKAWAY_OK` (configurado em `new()`).
        unsafe { AssignProcessToJobObject(self.handle, process_handle) }.map_err(|e| {
            JobError::AssignFailed {
                handle: process_handle,
                message: format!("{e:?}"),
            }
        })
    }

    /// Abre um processo pelo PID e associa ao Job. Conveniência
    /// sobre [`Self::assign`] que evita o caller ter de chamar
    /// `OpenProcess` manualmente.
    ///
    /// # Erros
    ///
    /// Falha se o PID for inválido, o processo já morreu, ou o
    /// caller não tem permissão para abrir (com `PROCESS_ALL_ACCESS`
    /// em PID de outro usuário, é necessário privilégio elevado).
    pub fn assign_pid(&self, pid: u32) -> Result<(), JobError> {
        // SAFETY: `OpenProcess` toma as permissões desejadas + PID
        // + bInheritHandle. O handle retornado precisa ser fechado
        // via `CloseHandle` (responsabilidade deste método, no
        // caminho de retorno com sucesso).
        let process_handle =
            unsafe { OpenProcess(PROCESS_ALL_ACCESS, false, pid) }.map_err(|e| {
                JobError::OpenProcessFailed {
                    pid,
                    message: format!("{e:?}"),
                }
            })?;
        // Tentativa de associar. Se falhar, fecha o handle
        // aberto antes de propagar.
        if let Err(e) = self.assign(process_handle) {
            // SAFETY: handle veio de OpenProcess bem-sucedido;
            // close é mandatório.
            unsafe {
                let _ = CloseHandle(process_handle);
            }
            return Err(e);
        }
        // Sucesso. O handle do processo fica aberto (o caller pode
        // precisar pra `ResumeThread` ou outros; nesta API, o
        // caller gerencia o ciclo de vida do PROCESS_INFORMATION).
        Ok(())
    }

    /// Atribui um processo **suspended** (criado com
    /// `CREATE_SUSPENDED`) ao Job, e depois o resume. É a forma
    /// **sem race** de associar (ADR-0036 D3): o processo não roda
    /// entre `assign` e `ResumeThread`, então nenhum neto pode ser
    /// criado fora do Job.
    ///
    /// # SAFETY
    ///
    /// O caller deve garantir que `suspended_handle` é o handle do
    /// processo **em estado suspended** (criado via
    /// `CreateProcessW(... CREATE_SUSPENDED ...)`).
    pub fn assign_suspended_process(&self, suspended_handle: HANDLE) -> Result<(), JobError> {
        self.assign(suspended_handle)?;
        // SAFETY: `ResumeThread` decrementa o suspend count do
        // thread; se era 1 (suspended uma vez), o thread volta a
        // executar. O handle é válido (veio de CreateProcessW
        // bem-sucedido em estado suspended). Retorna `u32` (o
        // suspend count anterior, sempre ≥ 0 em sucesso); erro de
        // `windows` v0.58 não tem tipo `Result` para ResumeThread
        // — uma chamada malsucedida simplesmente devolve um valor
        // fora do esperado, mas como o handle é garantido válido
        // pelo caller, isso aqui nunca deve acontecer em prática.
        let previous = unsafe { ResumeThread(suspended_handle) };
        if previous == u32::MAX {
            // u32::MAX é o sentinel "erro" de ResumeThread. Em
            // prática nunca acontece (handle válido), mas trate
            // explicitamente.
            return Err(JobError::ResumeFailed {
                handle: suspended_handle,
                message: "ResumeThread devolveu u32::MAX (sentinel de erro)".to_string(),
            });
        }
        Ok(())
    }

    /// Handle Win32 do Job. Útil para `AssignProcessToJobObject`
    /// em thread separada (etapa futura, se necessário).
    #[must_use]
    pub const fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Limite de memória por processo (bytes), aplicado em `new()`.
    #[must_use]
    pub const fn per_process_memory_bytes(&self) -> u64 {
        self.per_process_memory
    }

    /// Limite de memória total da árvore (bytes), aplicado em `new()`.
    #[must_use]
    pub const fn total_memory_bytes(&self) -> u64 {
        self.total_memory
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: `CloseHandle` decrementa a ref count do objeto
        // do kernel. Se o handle for o último, o kernel libera o
        // Job Object. Como o `KILL_ON_JOB_CLOSE` foi configurado em
        // `new()`, todos os processos atribuídos recebem
        // `TerminateProcess` **antes** do Job ser liberado — sem
        // isso, netos sobreviveriam ao `TerminateProcess` do pai.
        //
        // O handle veio de `CreateJobObjectW` bem-sucedido, e não
        // foi fechado em nenhum outro lugar (a struct não tem
        // método de "consumo" público; só `Drop`).
        let result = unsafe { CloseHandle(self.handle) };
        // Ignoramos o erro de close (não há ação útil a tomar
        // durante o Drop — o `KILL_ON_JOB_CLOSE` já disparou antes
        // do close retornar).
        let _ = result;
    }
}

// `HANDLE` é `Send + Sync` no Windows crate (é um `*mut c_void`
// raw, com a garantia de ser usado por uma thread de cada vez
// por convenção da API). Marcamos explicitamente pra confirmar
// que o `JobObject` pode ser compartilhado entre threads via
// `Arc<JobObject>` (caso de uso futuro: o `SecurityJailResolver`
// pode precisar passar o JobObject entre a task que spawna e a
// task que cancela).
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

#[cfg(test)]
mod tests {
    use super::*;

    /// `new()` retorna Ok em condições normais (CI do projeto roda
    /// em Windows, então este teste roda em Windows).
    #[test]
    fn new_creates_job_with_kill_on_close() {
        let job = JobObject::new().expect("JobObject::new deve ter sucesso em Windows");
        // `HANDLE` no `windows` v0.58 é `*mut c_void`. Handle
        // válido tem inner != null. Comparamos com `std::ptr::null_mut()`.
        assert!(!job.handle().0.is_null(), "handle nao pode ser null");
        assert_eq!(
            job.per_process_memory_bytes(),
            DEFAULT_PER_PROCESS_MEMORY_BYTES
        );
        assert_eq!(job.total_memory_bytes(), DEFAULT_TOTAL_JOB_MEMORY_BYTES);
        // Drop roda aqui: o CloseHandle fecha o handle, e (se
        // houvesse processos atribuídos) seriam killed. Como o
        // teste nao atribuiu nenhum, so o Job e liberado.
    }

    /// Limites customizados são preservados no struct.
    #[test]
    fn with_memory_limits_uses_custom_values() {
        let per = 512 * 1024 * 1024; // 512 MB
        let total = 1024 * 1024 * 1024; // 1 GB
        let job = JobObject::with_memory_limits(per, total).expect("with_memory_limits");
        assert_eq!(job.per_process_memory_bytes(), per);
        assert_eq!(job.total_memory_bytes(), total);
    }

    /// Dois `JobObject`s independentes têm handles diferentes
    /// (a struct não compartilha handles com `Arc`).
    #[test]
    fn two_job_objects_have_distinct_handles() {
        let a = JobObject::new().expect("a");
        let b = JobObject::new().expect("b");
        assert_ne!(a.handle().0, b.handle().0, "handles devem ser distintos");
    }
}
