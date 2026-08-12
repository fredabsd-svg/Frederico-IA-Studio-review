//! Testes de negação do `SecurityJailResolver` — a **headline** da
//! Etapa 2 da Fase 7.
//!
//! Ver [ADR-0031 §D3](../../../decisions/0031-fase-7-isolation-model-windows.md)
//! e [ADR-0036 §D2](../../../decisions/0036-security-jail-resolver-windows-job-objects.md).
//!
//! ## Dois testes, dois propósitos
//!
//! 1. **`fase5_etapa2a_incomplete_kill_parent_does_not_kill_grandchild`**
//!    — **controle negativo**. Prova que sem o `Job Object` (a
//!    Etapa 2.A da Fase 5, PR #22), `Child::kill()` no pai NÃO
//!    mata o neto. É a **regressão** que estamos fechando.
//!
//! 2. **`job_object_kills_tree_on_sandboxed_process_drop`** —
//!    **controle positivo**. Prova que com o `Job Object`
//!    per-invocation + `KILL_ON_JOB_CLOSE`, o **drop do
//!    `SandboxedProcess`** (que fecha o handle do Job
//!    per-invocation) mata a árvore inteira (pai + netos).
//!
//!    **Etapa 4 da Fase 7** (per-invocation Job): a v1 da Etapa 2
//!    dropava o `resolver` (root Job compartilhado). A Etapa 4
//!    muda pra per-spawn — cada `SandboxedProcess` carrega seu
//!    próprio Job. O nome do teste mudou de
//!    `job_object_kills_tree_on_resolver_drop` pra refletir.
//!    O resolver NÃO mata nada ao ser droppado; a v1 ainda
//!    funciona (compartilha o root Job, drop do resolver cascateia
//!    também), mas a Etapa 4 é o modelo correto.
//!
//! **Regra do user (2026-08-08): "Sandbox só se prova impedindo,
//! não funcionando."** A Etapa 2 + 4 fecha um gap de incompletude
//! da Fase 5 Etapa 2.A.
//!
//! ## Por que `python.exe` e não `cmd.exe`
//!
//! `python.exe` está disponível em qualquer máquina Windows com
//! Python. O `subprocess.Popen` em Python é a forma mais simples
//! de gerar netos cross-platform.
//!
//! ## Setup
//!
//! O teste precisa de `python.exe` no PATH. Em CI do GitHub
//! Actions (Windows runner), Python **não** está pré-instalado
//! por default. Solução: usar `where.exe python` e pular o teste
//! se não encontrar — degradação controlada.
//!
//! ## Por que `tasklist` em vez de `Child::wait` (Etapa 4)
//!
//! A v1 da Etapa 2 usava `tokio::process::Child::wait()` pra
//! confirmar a morte do pai — mas isso exigia `into_child`,
//! que consumia o `SandboxedProcess` e fechava o Job handle
//! prematuramente. A Etapa 4 remove `into_child` (o método
//! quebrava a garantia do per-invocation Job) e usa
//! `SandboxedProcess::stdout()` + drop explícito. A
//! confirmação da morte passa a ser via `tasklist /FI "PID eq
//! {pid}"` — tem lag inerente (~centenas de ms) que é
//! compensado por um sleep de 1s antes da checagem.

#![cfg(windows)]

use std::time::Duration;

use frederico_security::jail::{SandboxConfig, SecurityJailConfig, SecurityJailResolver};

/// **Controle negativo — regressão da Fase 5 Etapa 2.A.**
///
/// Spawna `python -c "subprocess.Popen(...)"` (que gera um neto
/// via `subprocess.Popen`), mata o **pai** via `Child::kill()` do
/// `tokio::process` (a estratégia da Fase 5 Etapa 2.A), e afirma
/// que o **neto continua vivo** (a falha que estamos fechando).
///
/// **Sem o `Job Object`:** o pai morre, o neto sobrevive. **É
/// exatamente isso que esperamos ver** — o test prova a
/// incompletude da Fase 5.
///
/// O neto é limpo no final (best-effort) pra não deixar processo
/// zumbi.
#[tokio::test]
async fn fase5_etapa2a_incomplete_kill_parent_does_not_kill_grandchild() {
    let python = match find_python() {
        Some(p) => p,
        None => {
            eprintln!(
                "[tree_kill] python.exe não encontrado; teste pulado. \
                 Para rodar, instale Python 3 ou adicione ao PATH."
            );
            return;
        }
    };

    // 1. Script do pai: spawna neto (sleep 60s) e printa o PID
    //    dele. O pai espera o neto terminar.
    let parent_script = r#"
import subprocess, sys
grandchild = subprocess.Popen(
    [sys.executable, '-c', 'import time; time.sleep(60)'],
)
print(grandchild.pid, flush=True)
grandchild.wait()
"#;

    // 2. Spawna o pai **sem** SecurityJailResolver — usando
    //    `tokio::process::Command` direto. Isso simula exatamente
    //    o que a Fase 5 Etapa 2.A (PR #22) fazia.
    let mut child = tokio::process::Command::new(&python)
        .arg("-c")
        .arg(parent_script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn do pai");
    let parent_pid = child.id().expect("parent pid");
    eprintln!("[tree_kill-regressao] pai spawnado: pid={parent_pid}");

    // 3. Lê o PID do neto do stdout.
    let stdout = child.stdout.take().expect("stdout piped");
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("read_line do stdout do pai");
    let grandchild_pid: u32 = line
        .trim()
        .parse()
        .expect("stdout do pai deve ter o PID do neto");
    eprintln!("[tree_kill-regressao] neto spawnado: pid={grandchild_pid}");

    // 4. Mata o pai via `Child::kill()` (a estratégia da Fase 5
    //    Etapa 2.A). SEM o Job Object, o pai morre, mas o neto
    //    continua.
    child.start_kill().expect("kill do pai");
    eprintln!("[tree_kill-regressao] pai morto via start_kill");

    // 5. Espera o pai terminar.
    use tokio::time::timeout;
    let parent_wait = timeout(Duration::from_secs(2), child.wait()).await;
    match parent_wait {
        Ok(Ok(status)) => eprintln!("[tree_kill-regressao] pai terminou: {status:?}"),
        _ => eprintln!("[tree_kill-regressao] pai não terminou em 2s (anormal)"),
    }

    // 6. Verifica se o neto está vivo **antes** de tentar matá-lo.
    //    O jeito confiável em Windows é `tasklist /FI` — se retornar
    //    exit code 0 com o PID listado, o processo está vivo.
    //    O `tasklist` tem lag (mostra processos como vivos por
    //    centenas de ms depois de morrerem), então damos 500ms
    //    pra estabilizar.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let tasklist_output = tokio::process::Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {grandchild_pid}"))
        .arg("/NH")
        .output()
        .await
        .expect("tasklist do neto");
    let tasklist_stdout = String::from_utf8_lossy(&tasklist_output.stdout);
    eprintln!(
        "[tree_kill-regressao] tasklist output: status={:?} stdout={tasklist_stdout:?}",
        tasklist_output.status
    );
    let neto_vivo =
        tasklist_output.status.success() && tasklist_stdout.contains(&grandchild_pid.to_string());
    assert!(
        neto_vivo,
        "ESPERADO que o neto CONTINUE VIVO (regressão da Fase 5). \
         Se esta assertion falhar, o Windows mudou o comportamento \
         default de herança de Job (improvável). \
         tasklist output: {tasklist_stdout:?}"
    );

    // 7. Cleanup best-effort do neto (que está vivo por design
    //    deste test). Usa `taskkill /F` em vez de `Child::kill`
    //    porque não temos um `Child` aberto pro neto.
    let _ = tokio::process::Command::new("taskkill")
        .arg("/F")
        .arg("/PID")
        .arg(grandchild_pid.to_string())
        .output()
        .await;

    eprintln!(
        "[tree_kill-regressao] PASSOU — neto (pid={grandchild_pid}) \
         SOBREVIVEU à morte do pai, como esperado. Fase 5 Etapa 2.A \
         era quebrada; Etapa 2 da Fase 7 corrige com Job Object."
    );
}

/// **Controle positivo — a fix da Etapa 2 + Etapa 4 da Fase 7.**
///
/// Spawna `python` que cria um neto via `subprocess.Popen`, sob o
/// `SecurityJailResolver` (Job Object **per-invocation** com
/// `KILL_ON_JOB_CLOSE`). Depois, **dropa o `SandboxedProcess`**
/// (o que fecha o handle do Job). O `KILL_ON_JOB_CLOSE` deve
/// matar a árvore toda (pai + neto).
///
/// **Mudança da Etapa 4 da Fase 7 (per-invocation Job):** o test
/// anterior (`job_object_kills_tree_on_resolver_drop`) dropava o
/// `resolver`. Com o per-invocation Job Object, **dropar o
/// resolver NÃO mata nada** — cada `SandboxedProcess` carrega
/// seu próprio Job. O novo contrato: o caller dropar o
/// `SandboxedProcess` (via `Run` cancelado, fim de `execute`, ou
/// timeout) mata a árvore.
///
/// **API usada:** `SandboxedProcess::stdout()` (Etapa 4) — toma
/// a handle de stdout **sem** consumir o `SandboxedProcess`. O
/// `SandboxedProcess` continua vivo (Job handle aberto) até o
/// `drop(child)` explícito. O `into_child` da v1 foi **removido**
/// porque consumia o `SandboxedProcess` e fechava o Job
/// prematuramente — exatamente o bug que o per-invocation Job foi
/// criado pra evitar.
///
/// **Verificação da morte:** tasklist com `/FI "PID eq {pid}"`
/// retorna exit 0 e output vazio se o processo está morto. O
/// `tasklist` tem lag (mostra processos como vivos por centenas
/// de ms depois de morrerem), então damos 1s pra estabilizar.
#[tokio::test]
async fn job_object_kills_tree_on_sandboxed_process_drop() {
    let python = match find_python() {
        Some(p) => p,
        None => {
            eprintln!("[tree_kill] python.exe não encontrado; teste pulado.");
            return;
        }
    };

    let resolver = SecurityJailResolver::new(SecurityJailConfig::secure_default())
        .expect("SecurityJailResolver::new");
    let grandchild_script = "import time; time.sleep(60)";
    let parent_script = format!(
        r#"
import subprocess, sys
g = subprocess.Popen([sys.executable, '-c', '{grandchild_script}'])
print(g.pid, flush=True)
g.wait()
"#
    );
    // **Etapa 5+ da Fase 7 (path safety):** o `set_low_integrity_label`
    // rotula o workdir via `SetFileSecurityW` — exige que o workdir
    // seja **owned pelo user** (a Etapa 4 v1 usava `current_dir()` que
    // apontava pra `C:\src\Frederico-IA\crates\security` — não é
    // owned pelo user de CI, dava `ERROR_ACCESS_DENIED` 5). Solução:
    // tempdir (que o `tempfile` cria em `%TEMP%` owned pelo user).
    // O `tempdir` precisa viver até o final do test — não usar
    // `let _ =` que droparia no fim do statement.
    let workdir = tempfile::tempdir().expect("tempdir workdir");
    let config = SandboxConfig::new(
        python,
        vec!["-c".to_string(), parent_script],
        workdir.path().to_path_buf(),
    );
    let mut child = resolver.spawn(config).expect("spawn");
    let parent_pid = child.pid();

    // Toma stdout SEM consumir o SandboxedProcess (a v1 da Etapa 2
    // usava `into_child` que fechava o Job prematuramente — bug
    // consertado na Etapa 4 com `stdout()`/`stderr()`, e a Etapa 5+
    // da Fase 7 troca por `take_stdout_handle()` que devolve
    // o HANDLE raw wrappado em `tokio::fs::File`).
    use frederico_security::raw_child::wrap_pipe_handle_as_async_file;
    let stdout_handle = child.take_stdout_handle().expect("stdout piped");
    let stdout = wrap_pipe_handle_as_async_file(stdout_handle).expect("wrap stdout");
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read_line");
    let grandchild_pid: u32 = line.trim().parse().expect("parse pid");
    eprintln!("[tree_kill] pai={parent_pid} neto={grandchild_pid}");

    // Espera 200ms pro neto estar rodando.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Drop do `SandboxedProcess`. O `Option<JobObject>` é
    // droppado (fechando o handle do Job), e o Windows
    // dispara `KILL_ON_JOB_CLOSE` matando a árvore inteira
    // (pai + neto).
    drop(child);

    // Espera o tasklist estabilizar. KILL_ON_JOB_CLOSE é rápido
    // (sub-segundo) mas o `tasklist` tem cache local.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Verifica se o PAI morreu. Se `tasklist` ainda lista o
    // PID, o KILL_ON_JOB_CLOSE falhou em cascatear.
    let parent_alive = pid_still_alive(parent_pid).await;
    assert!(
        !parent_alive,
        "FALHA: pai (pid={parent_pid}) ainda vivo 1s após drop do SandboxedProcess. \
         KILL_ON_JOB_CLOSE não cascateou. \
         Possível causa: o `SandboxedProcess` não está mais no Job, ou o Job handle \
         não foi fechado no Drop."
    );
    eprintln!("[tree_kill] pai (pid={parent_pid}) morto confirmado via tasklist");

    // Verifica se o NETO morreu também. Este é o teste da
    // **headline** da Etapa 2 da Fase 7: a Etapa 2.A da Fase 5
    // (PR #22) tinha exatamente esse bug — `Child::kill()` matava
    // o pai, neto sobrevivia. A Etapa 2 + 4 da Fase 7 fecha o
    // gap com `KILL_ON_JOB_CLOSE` do per-invocation Job.
    let grandchild_alive = pid_still_alive(grandchild_pid).await;
    assert!(
        !grandchild_alive,
        "FALHA: neto (pid={grandchild_pid}) ainda vivo após drop do SandboxedProcess. \
         KILL_ON_JOB_CLOSE não cascateou pro neto. ESTE É O BUG DA ETAPA 2.A DA FASE 5 \
         (PR #22) QUE A ETAPA 2 + 4 DA FASE 7 DEVERIA FECHAR."
    );
    eprintln!(
        "[tree_kill] neto (pid={grandchild_pid}) morto confirmado via tasklist — \
         Etapa 2 + 4 da Fase 7 fecha o gap da Etapa 2.A da Fase 5"
    );
}

/// Helper: retorna `true` se o `pid` ainda está vivo no OS
/// (consulta `tasklist /FI "PID eq {pid}" /NH`). Tem lag
/// inerente do tasklist (~centenas de ms) — caller deve
/// esperar antes de chamar.
async fn pid_still_alive(pid: u32) -> bool {
    let out = tokio::process::Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .arg("/NH")
        .arg("/FO")
        .arg("CSV")
        .output()
        .await
        .expect("tasklist");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // tasklist CSV: `"Image Name","PID","Session Name","Session#","Mem Usage"`.
    // Se o processo existe, o PID está no output. Se não
    // existe, output é "INFO: No tasks are running which match the specified criteria."
    stdout.contains(&pid.to_string())
}

/// Helper: encontra `python.exe` no PATH. Retorna o caminho
/// completo ou None se não existir.
///
/// **Pula o stub do WindowsApps** (`C:\Users\<user>\AppData\Local\
/// Microsoft\WindowsApps\python.exe`) — esse stub requer Python
/// instalado via Microsoft Store e falha com `ERROR_INVALID_PARAMETER`
/// (87) quando invocado sem o Store Python. Em ambientes sem Store
/// Python, o stub é a primeira entrada de `where python`, então
/// filtrar é necessário.
fn find_python() -> Option<std::path::PathBuf> {
    for name in &["python", "python3", "py"] {
        if let Ok(out) = std::process::Command::new("where").arg(name).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                for line in s.lines() {
                    let path = std::path::PathBuf::from(line.trim());
                    // Pula o stub do WindowsApps (não é Python
                    // real, é um launcher que requer Store Python).
                    let path_str = path.to_string_lossy();
                    if path_str.contains("WindowsApps") {
                        continue;
                    }
                    return Some(path);
                }
            }
        }
    }
    None
}
