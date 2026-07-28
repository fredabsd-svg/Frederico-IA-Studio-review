<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-07-27
Fase correspondente: 3 (Etapa 1)
-->

> Última verificação: 2026-07-27. Reflete a Etapa 1 da Fase 3 — crate
> `frederico-agent-engine` com a enum `RunState` (22 variantes), a enum
> `RunEventKind` (25 variantes: 20 estruturais + 5 globais), o
> `RunEvent`, o `Budget`, a struct `Run` em memória, a tabela
> `TRANSITIONS` (21 arestas estruturais) + `GLOBAL_TRANSITIONS` (5
> arestas globais) e a função pura `apply_transition`. Suíte do crate
> com 46 testes por par. A `Run` em memória é o tipo de domínio; o
> registro persistido equivalente é `frederico_storage::Run`, estendido
> pela migração `0003_runs_and_checkpoints.sql` com 9 colunas novas +
> tabela `checkpoints` + view `runs_with_status` (mapeia os 22
> estados nos 6 status da Fase 2). Etapas 2 (tool-registry), 3
> (permissões), 4 (integração com executor), 5 (watchdog/recovery) e 6
> (UI) ainda não começaram. Ver
> [`docs/modules/agent-engine.md`](../modules/agent-engine.md) para o
> detalhamento por eixo do template §1.4. ADR-0009 documenta a decisão
> estrutural (crate novo + view para compatibilidade com Fase 2).

**Alterado em relação ao plano original:** o preâmbulo original deste
spec dizia "23 estados". A Etapa 1 da Fase 3 reconciliou: a enum
`RunState` tem **22** variantes e o spec, o ADR-0009, o
`RunState::COUNT` e a `CHECK` constraint do SQLite usam 22. A
contagem literal de variantes no spec (22) sempre foi a verdade; o
"23" do preâmbulo era um typo não detectado nas revisões anteriores.
`REGRAS §1.13` cobra a nota; este parágrafo é ela.

# Máquina de Estados do Agente

Cada `Run` (execução) percorre uma máquina de estados **explícita**. Não usamos `isLoading` / `isThinking` como variáveis de controle (`PROMPT MESTRE` §6.1). A enum de estados é fixa e o sistema rejeita transições inválidas como erro estruturado.

## Estados (`PROMPT MESTRE` §6.1)

```text
created
queued
preparing_context
retrieving_memory
validating_capabilities
calling_model
streaming
waiting_tool_call
validating_tool_call
waiting_user_approval
executing_tool
validating_tool_result
continuing_model
generating_artifact
validating_artifact
checkpointing
retrying
paused
completed
failed
cancelled
interrupted
```

23 estados. **Terminais**: `completed`, `failed`, `cancelled`, `interrupted`.

## Contratos

### Tipos do núcleo

```rust
enum RunState { /* 23 variantes do §6.1 */ }

struct Run {
    run_id: RunId,
    conversation_id: ConversationId,
    project_id: ProjectId,
    assistant_id: AssistantId,
    provider_id: ProviderId,
    model_id: ModelId,
    started_at: DateTime<Utc>,
    state: RunState,
    current_step: u32,
    budget: Budget,
    allowed_tools: Vec<ToolId>,
    artifacts: Vec<ArtifactId>,
    last_heartbeat_at: DateTime<Utc>,
}

struct Budget {
    max_steps: u32,
    max_tokens_in: u64,
    max_tokens_out: u64,
    max_cost_usd: Decimal,
    max_wall_clock: Duration,
}

struct Transition {
    from: RunState,
    to: RunState,
    event: RunEventKind,
    guard: Option<Guard>,    // ex: "max_steps não atingido", "worker saudável"
    effect: Vec<Effect>,     // ex: "persiste checkpoint", "emite evento de UI", "spawn de retry"
}

struct RunEvent {
    run_id: RunId,
    seq: u64,                // monotonicamente crescente por run_id
    ts: DateTime<Utc>,
    kind: RunEventKind,
    payload: serde_json::Value,
}
```

### Tabela de transições (resumo, não exaustivo)

| De | Para | Evento | Notas |
|---|---|---|---|
| `created` | `queued` | `enqueue` | persistência obrigatória antes desta transição |
| `queued` | `preparing_context` | `dequeue` | scheduler escolhe |
| `preparing_context` | `retrieving_memory` | `context_ready` | |
| `retrieving_memory` | `validating_capabilities` | `memory_done` | pode ser pulado em 0ms se nada recuperado |
| `validating_capabilities` | `calling_model` | `capabilities_ok` | valida modelo, tools, permissões |
| `calling_model` | `streaming` | `first_token` | ou `continuing_model` se não-streaming |
| `streaming` | `waiting_tool_call` | `tool_call_emitted` | modelo pediu ferramenta |
| `streaming` | `continuing_model` | `message_complete` | modelo terminou sem tool_call |
| `waiting_tool_call` | `validating_tool_call` | `validate_call` | |
| `validating_tool_call` | `waiting_user_approval` | `approval_required` | se `requires_user_approval` |
| `validating_tool_call` | `executing_tool` | `approval_granted_or_not_required` | |
| `executing_tool` | `validating_tool_result` | `tool_returned` | |
| `validating_tool_result` | `continuing_model` | `result_valid` | |
| `continuing_model` | `calling_model` | `next_iteration` | dentro do orçamento |
| `continuing_model` | `generating_artifact` | `artifact_requested` | `docs.generate` etc. |
| `generating_artifact` | `validating_artifact` | `artifact_emitted` | arquivo deve existir no disco |
| `validating_artifact` | `checkpointing` | `artifact_valid` | ou `failed` |
| `validating_artifact` | `retrying` | `artifact_invalid` | ver `PROMPT MESTRE` §19.6 |
| qualquer não-terminal | `paused` | `user_pause` | recupera com `resume` |
| qualquer não-terminal | `cancelled` | `user_cancel` | terminal |
| qualquer não-terminal | `interrupted` | `watchdog_timeout` ou `app_crash_recovery` | terminal até `resume` |
| qualquer não-terminal | `failed` | `unrecoverable_error` | terminal |

A tabela completa vira código Rust na Fase 3 com cobertura de teste por par válido/inválido.

## Invariantes

- **Toda execução persiste no estado `created` antes de qualquer outra coisa.** Garante recuperação confiável de crash. *Teste: matar o app imediatamente após submeter mensagem e verificar que o `Run` está em `created` ou posterior no banco.*
- **Transições inválidas são erro estruturado**, nunca ignoradas. A função `apply_transition` é única e valida `from → to` contra a tabela.
- **Estados terminais são imutáveis.** `completed`, `failed`, `cancelled`, `interrupted` não saem.
- **Cada `Run` tem um `RunEvent` por transição**, com `seq` monotonicamente crescente por `run_id`. Sem lacunas sem motivo.
- **Watchdog marca como `interrupted` qualquer execução sem heartbeat** dentro de `PROMPT MESTRE` §12.2. Heartbeat é parte do `Run`, atualizado a cada operação custosa.
- **Cancelamento é transição válida a partir de qualquer estado não-terminal**, propagado por `CancellationToken` hierárquico (`PROMPT MESTRE` §9.4). Workers, modelos e subprocessos recebem o sinal; ferramentas que já iniciaram I/O são mortas via tree-kill no sandbox.
- **`retrying` é distinto de uma nova execução** — preserva o mesmo `RunId`, o histórico e os artefatos já produzidos (`PROMPT MESTRE` §6.3, item 8: "não repetir operações destrutivas automaticamente").

## Checkpoints (`PROMPT MESTRE` §6.2)

Pontos onde o estado completo do `Run` é serializado no SQLite (`checkpoints` table):

- antes de `calling_model`
- depois de cada `message_complete`
- antes de `executing_tool`
- depois de `validating_tool_result`
- depois de `generating_artifact`
- antes de `paused`, `cancelled`, `interrupted`
- periodicamente durante tarefas longas (intervalo configurável; default 30s)

## Não-objetivos

- Subestados (não há `executing_tool.waiting_for_io` — granularidade fica no `seq` e nos eventos).
- Máquina de estados por modelo/provedor (uma só máquina).
- Recuperação automática após `failed` — humano decide reiniciar (`PROMPT MESTRE` §6.3).
- Transições probabilísticas. Toda transição é determinística dado o evento.

## Decisões

Nenhuma nova. A enum de estados vem diretamente do `PROMPT MESTRE` §6.1; mudanças exigem ADR novo e atualização do spec.

## Referências

- `PROMPT MESTRE` §6 (motor central), §6.1 (estados), §6.2 (checkpoints), §6.3 (recuperação), §9.4 (cancelamento), §12 (watchdog)
- [`software-architecture.md`](./software-architecture.md) — onde o estado vive em processo
- [`process-architecture.md`](./process-architecture.md) — onde o estado vive em disco
- [`security-threat-model.md`](./security-threat-model.md) — integridade dos checkpoints
- [`testing-strategy.md`](./testing-strategy.md) — cobertura por invariante
