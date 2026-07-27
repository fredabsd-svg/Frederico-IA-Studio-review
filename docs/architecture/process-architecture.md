<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 1
-->

# Arquitetura de Processos

O Frederico IA Studio roda como um **app principal** que embute o núcleo Rust e a casca Tauri, mais um conjunto de **workers sidecar** que são processos separados. Cada worker é um executável distribuído com o app (ou baixado como pacote oficial assinado). Nenhum worker abre API em `localhost` (`PROMPT MESTRE` §5.3).

## Topologia

```text
┌─────────────────────────────────────┐
│  App principal (apps/desktop)        │
│   ├── Tauri runtime (WebView)        │
│   ├── Núcleo Rust (in-process)       │
│   └── Gerenciador de workers         │
└──┬──────┬──────┬──────┬────────────┘
   │      │      │      │  IPC (JSON / named pipe, sem localhost)
   ▼      ▼      ▼      ▼
[document-worker] [sandbox-runner] [browser-worker] [runtime-manager]
  (Python)         (Rust)          (Rust+headless)    (Rust)
```

Os workers iniciais são previstos no `PROMPT MESTRE` §5.3. Workers adicionais só nascem com ADR.

## Contratos

### Manifesto do worker (handshake inicial, `PROMPT MESTRE` §7.3)

```rust
struct WorkerManifest {
    worker_id: WorkerId,
    version: SemVer,                  // handshake de versão
    capabilities: Vec<String>,        // "docx.write", "ocr.run", "sandbox.exec.python", etc.
    tools: Vec<ToolManifest>,         // subset das ferramentas deste worker (ver tool-registry-specification.md)
    dependencies: Vec<Dependency>,    // python-docx 1.1.0, tesseract 5.3, lang pack por.traineddata, etc.
    health: WorkerHealth,             // ok | degraded | unhealthy
    compatibility: CompatibilityInfo, // OS mínimo, arquitetura, runtime mínimo
}
```

### Mensagem IPC (envelope genérico)

```rust
struct IpcMessage {
    protocol_version: u32,
    request_id: Uuid,
    op: String,                       // "tool.invoke" | "tool.cancel" | "worker.ping" | "worker.shutdown" | ...
    payload: serde_json::Value,       // validado por schema específico da `op`
    auth: WorkerAuth,                 // token de curta duração emitido pelo app principal
}
```

O `protocol_version` é global do envelope. A `op` carrega o schema do payload, referenciado por ID no `packages/shared-contracts/`.

### Descoberta na inicialização (`PROMPT MESTRE` §7.3)

Sequência obrigatória no boot do app:

1. Carrega ferramentas **internas** (registradas estaticamente no binário principal).
2. Spawn de cada worker registrado em `config/workers.toml`.
3. Handshake `worker.hello` → manifest + schema + saúde.
4. Validação de `protocol_version` (incompatível = falha de boot).
5. Healthcheck ativo (ping com TTL curto).
6. Inventário consolidado no `ToolRegistry`.
7. Workers que falham no handshake são marcados `unhealthy`; suas ferramentas ficam `unavailable` na UI e no cálculo de interseção (ver [`tool-registry-specification.md`](./tool-registry-specification.md)).

## Invariantes

- **Nenhum worker abre porta TCP** em `localhost` ou em qualquer interface de rede. Auditável: script de teste tenta `netstat -an` durante boot e falha se houver porta aberta por worker.
- **Variáveis de ambiente do processo pai não são herdadas pelos workers** (`PROMPT MESTRE` §22.5). O worker recebe um env construído por allowlist. Teste E2E injeta `OPENAI_API_KEY` no env do app, executa ferramenta em worker, e o filho **não** vê a variável.
- **Comunicação é autenticada** — token de curta duração (≤ 15 min), escopado ao worker, revogável. Worker que reapresenta token revogado é morto.
- **Workers são stateless do ponto de vista do app** — morte e ressubida não corrompem estado, porque estado vive no SQLite via checkpoints (ver [`agent-state-machine.md`](./agent-state-machine.md)).
- **Worker lento é morto por watchdog** no `timeoutMs` declarado no `ToolManifest`. Watchdog não é configurável por chamada — é o valor do manifesto.
- **Zero polling no app principal.** Comunicação só por eventos, jamais por loop de checagem (`PROMPT MESTRE` §23.7).

## Não-objetivos

- Comunicação via HTTP local (mesmo com TLS) — viola a invariante.
- Workers como bibliotecas dinâmicas carregadas no processo principal — viola isolamento e o `PROMPT MESTRE` §22.
- Workers em rede (entre máquinas) — fora do escopo da v1.
- Mais de uma instância do mesmo worker simultânea na v1.

## Decisões

- [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md) — por que a comunicação é via contrato serializável.
- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — por que `document-worker` é Python embutido.

## Referências

- `PROMPT MESTRE` §5.3 (processos auxiliares), §7.3 (descoberta), §7.8 (resultado de ferramenta), §22 (sandbox), §22.5 (segredos e rede)
- [`tool-registry-specification.md`](./tool-registry-specification.md)
- [`security-threat-model.md`](./security-threat-model.md)
- [`testing-strategy.md`](./testing-strategy.md)
