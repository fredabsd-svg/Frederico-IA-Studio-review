# worker-stub.ps1 — stub PowerShell do worker sidecar pra teste
# E2E do `WorkerManager::spawn_external`.
#
# Implementa o **protocolo completo** (`worker.hello`/`app.ack`/
# `app.ping`/`app.shutdown`/`tool.invoke`/`tool.result`/`worker.error`)
# sobre `System.IO.Pipes.NamedPipeServerStream` do .NET, em
# PowerShell 5.1 (síncrono, bloqueante — bounded pelo timeout do
# manager). O `protocol_version` é 1, mesmo do envelope Rust.
#
# **Argumentos:**
#   $args[0] — caminho do manifesto JSON. Default: `manifest-stub.json`
#              no mesmo diretório deste script.
#
# **Como é usado nos testes:**
#   O `tests/external_worker.rs` spawna `powershell.exe -NoProfile
#   -ExecutionPolicy Bypass -File worker-stub.ps1 <manifest>` via
#   `WorkerManager::spawn_external` e valida o handshake + roundtrip
#   de `tool.invoke` + shutdown limpo.
#
# **Não é o document-worker real.** O document-worker Python (em
# `workers/document-worker/`) é a implementação de produção;
# este stub é só pra exercitar o lado do `spawn_external` no CI
# sem depender de Python/pywin32 (que o `bootstrap.ps1` instala
# em outra etapa).

param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot "manifest-stub.json")
)

$ErrorActionPreference = 'Stop'

# 1. Gera nome único pro pipe (UUID curto).
$pipeName = "frederico-stub-" + [Guid]::NewGuid().ToString("N").Substring(0, 12)

# 2. Carrega o manifesto do JSON. O `ConvertFrom-Json` produz
#    um PSCustomObject que vira hashtable no `ConvertTo-Json` —
#    o shape casa com o `WorkerManifest` Rust.
$manifest = Get-Content -Path $ManifestPath -Raw | ConvertFrom-Json

# 3. Cria o `NamedPipeServerStream`. `InOut` (bidirecional),
#    `maxInstances=1` (só o app principal conecta — não aceitamos
#    clients paralelos; mesma semântica do
#    `first_pipe_instance(true)` na `windows_pipes.rs`).
$pipe = New-Object System.IO.Pipes.NamedPipeServerStream(
    $pipeName,
    [System.IO.Pipes.PipeDirection]::InOut,
    1,
    [System.IO.Pipes.PipeTransmissionMode]::Byte,
    [System.IO.Pipes.PipeOptions]::None
)

# 4. Anuncia o pipe pro app via stdout. **PRIMEIRA linha do
#    stdout** — o `spawn_external` espera exatamente
#    `READY <name>` (parse em `parse_ready_line`). Nada pode
#    vir antes (nem banner do PowerShell — `-NoProfile` suprime).
#
#    **Por que `[Console]::WriteLine` e não `Write-Output`:** o
#    `Write-Output` do PowerShell usa o pipeline de output
#    que pode ser **bufferizado** quando o stdout não é um
#    terminal (caso típico: redirecionado pra pipe via
#    `tokio::process::Command`). O `spawn_external` lê
#    stdout com timeout 10s e pode receber EOF antes do
#    buffer flushar. `[Console]::WriteLine` é
#    `System.Console.WriteLine` direto, sem pipeline,
#    sem buffering — o `READY` chega no pipe imediatamente.
[Console]::Out.WriteLine("READY $pipeName")
[Console]::Out.Flush()

# 5. Espera o app conectar (bloqueante). Quando o app chama
#    `connect_pipe_client` (no Rust), o `CreateFileW` do lado
#    kernel resolve o `ConnectNamedPipe` daqui e segue.
$pipe.WaitForConnection()

# 6. Embrulha em reader/writer. `AutoFlush = $true` garante que
#    o `WriteLine` já vai pro pipe (importante pro
#    `read_line().await` do outro lado).
#
#    **Por que `UTF8Encoding($false)` (sem BOM):** o default
#    `Encoding.UTF8` do .NET adiciona BOM no início do
#    stream. O `IpcMessage::decode_line` agora tolera BOM
#    (defesa em profundidade), mas o caminho limpo é não
#    enviar — o `serde_json::from_slice` tem que trabalhar
#    menos.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$reader = New-Object System.IO.StreamReader($pipe, $utf8NoBom)
$writer = New-Object System.IO.StreamWriter($pipe, $utf8NoBom)
$writer.AutoFlush = $true

# 7. Salva o auth token quando recebe o `app.ack`. Antes disso,
#    `tool.invoke` aceita tudo (handshake em andamento); depois,
#    valida estritamente.
$script:authToken = $null

function Send-IpcMessage {
    param(
        [string]$RequestId,
        [string]$Op,
        [hashtable]$Payload,
        [string]$Auth = $null
    )
    $msg = @{
        protocol_version = 1
        request_id       = $RequestId
        op               = $Op
        payload          = $Payload
    }
    if ($null -ne $Auth) {
        $msg.auth = $Auth
    }
    $json = $msg | ConvertTo-Json -Depth 10 -Compress
    $writer.WriteLine($json)
}

# 8. Envia o `worker.hello` **imediatamente** após o connect.
#    O manager lê essa mensagem, gera o `WorkerAuth`, e responde
#    com `app.ack` carregando o token. Daí em diante, toda
#    `tool.invoke` traz o token — o stub valida.
$helloRequestId = [Guid]::NewGuid().ToString()
$helloPayload = @{
    worker_id     = $manifest.worker_id
    version       = $manifest.version
    capabilities  = $manifest.capabilities
    dependencies  = $manifest.dependencies
    health        = "unhealthy"
    compatibility = $manifest.compatibility
}
Send-IpcMessage -RequestId $helloRequestId -Op "worker.hello" -Payload $helloPayload

# 9. Loop principal. `ReadLine` retorna `$null` quando o peer
#    fecha a conexão (EOF limpo) — sai do loop. O `actor_task`
#    do manager detecta esse EOF e sai também.
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }

    $msg = $line | ConvertFrom-Json
    switch ($msg.op) {
        # `app.ack` carrega o `WorkerAuth` (token). Salva pra
        # validar `tool.invoke` daqui pra frente. Mesmo
        # protocolo do `FakeWorker` (Etapa 2A).
        "app.ack" {
            $script:authToken = $msg.auth
        }
        "app.ping" {
            $pongPayload = @{
                status       = "ok"
                env_received = @{}
            }
            Send-IpcMessage -RequestId $msg.request_id -Op "worker.pong" -Payload $pongPayload
        }
        "app.shutdown" {
            # O manager não espera response — basta fechar o
            # pipe e sair. O EOF que o `read_line` do manager
            # vê é o sinal de "shutdown completo".
            $pipe.Close()
            return
        }
        "tool.invoke" {
            # Valida o token se já temos um (depois do
            # handshake). Erro de auth é `worker.error` com
            # `code = "process_unauthorized"` (mesmo código do
            # `FakeWorker`).
            if ($null -ne $script:authToken) {
                if ($msg.auth -ne $script:authToken) {
                    $errPayload = @{
                        code    = "process_unauthorized"
                        message = "token ausente ou inválido"
                    }
                    Send-IpcMessage -RequestId $msg.request_id -Op "worker.error" -Payload $errPayload
                    continue
                }
            }
            $resultPayload = @{
                ok           = $true
                echo         = $msg.payload
                env_received = @{}
            }
            Send-IpcMessage -RequestId $msg.request_id -Op "tool.result" -Payload $resultPayload
        }
        default {
            # Ignora opcodes desconhecidos (compatibilidade com
            # workers que falam um superset).
        }
    }
}

# EOF do peer ou `app.shutdown` — fecha o pipe.
$pipe.Close()
