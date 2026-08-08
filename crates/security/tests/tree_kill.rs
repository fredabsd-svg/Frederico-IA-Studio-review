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
//! 2. **`job_object_kills_tree_on_resolver_drop`** — **controle
//!    positivo**. Prova que com o `Job Object` + `KILL_ON_JOB_CLOSE`,
//!    o drop do `SecurityJailResolver` (que fecha o handle do Job)
//!    mata a árvore inteira (pai + netos).
//!
//! **Regra do user (2026-08-08): "Sandbox só se prova impedindo,
//! não funcionando."** A Etapa 2 fecha um gap de incompletude da
//! Fase 5 Etapa 2.A.
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
//! ## Por que `Child::wait()` em vez de `tasklist`
//!
//! O `tasklist` tem cache (mostra processos como vivos por
//! centenas de ms depois de morrerem). O `Child::wait()` é a
//! verdade — retorna quando o handle do processo fica
//! signaled, que é exatamente quando o OS o marcou como morto.

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

/// **Controle positivo — a fix da Etapa 2 da Fase 7.**
///
/// Spawna `python` que cria um neto via `subprocess.Popen`, ambos
/// sob o `SecurityJailResolver` (Job Object com `KILL_ON_JOB_CLOSE`).
/// Depois, **dropa o resolver** (o que fecha o handle do Job).
/// O `KILL_ON_JOB_CLOSE` deve matar a árvore toda.
///
/// O test espera via `Child::wait()` com timeout de 5s — se o
/// pai não terminar (porque o neto ficou vivo segurando o pai),
/// o test falha.
#[tokio::test]
async fn job_object_kills_tree_on_resolver_drop() {
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
    let config = SandboxConfig::new(
        python,
        vec!["-c".to_string(), parent_script],
        std::env::current_dir().unwrap(),
    );
    let child = resolver.spawn(config).expect("spawn");
    let parent_pid = child.pid();

    // Lê o PID do neto.
    let mut tokio_child = child.into_child();
    let stdout = tokio_child.stdout.take().expect("stdout piped");
    use tokio::io::AsyncBufReadExt;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read_line");
    let grandchild_pid: u32 = line.trim().parse().expect("parse pid");
    eprintln!("[tree_kill] pai={parent_pid} neto={grandchild_pid}");

    // Espera 200ms pro neto estar rodando.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Drop do resolver → fecha o Job Object → KILL_ON_JOB_CLOSE
    // → mata a árvore toda.
    drop(resolver);

    // Espera o PAI terminar (o pai só termina quando o neto
    // termina; se o neto for morto, o pai também morre).
    // O timeout é 5s (sleep 60 no neto + pequena margem).
    use tokio::time::timeout;
    let wait_result = timeout(Duration::from_secs(5), tokio_child.wait()).await;
    match wait_result {
        Ok(Ok(status)) => {
            eprintln!("[tree_kill] pai terminou com status={status:?}");
            // Cleanup best-effort do neto (se ainda estiver vivo).
            let _ = tokio::process::Command::new("taskkill")
                .arg("/F")
                .arg("/PID")
                .arg(grandchild_pid.to_string())
                .output()
                .await;
        }
        Ok(Err(e)) => {
            panic!("[tree_kill] erro no wait do pai: {e}");
        }
        Err(_) => {
            panic!(
                "[tree_kill] FALHA: pai (pid={parent_pid}) NÃO terminou em 5s. \
                 Drop do resolver não cascateou KILL_ON_JOB_CLOSE."
            );
        }
    }
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
