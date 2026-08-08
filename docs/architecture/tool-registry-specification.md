<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-03
Fase correspondente: 3 (Etapa 2) + Fase de Ligação (Etapa 1) + 7 (Etapa 1 — planejamento)
-->

> Última verificação: 2026-08-03 (Fase 3 + Fase de Ligação). Reflete a Etapa 2 da Fase 3 + a
> Etapa 1 da Fase de Ligação — crate `frederico-tool-registry`
> com a enum `ToolManifest` (todos os 22 campos do spec
> §"Contrato do manifesto" + builder fluente), `JsonSchema`
> validado pelo crate `jsonschema` 0.18, a `ToolRegistry` com
> `register`/`get`/`all`/`effective_tools` (interseção filtrada
> por `availability` + `health` + allowlist), a `Jail` (rejeita
> `..`/absoluto/UNC/letra de unidade/symlink — defesa contra a
> ameaça I3 do `security-threat-model.md`), a `validate_tool_call`
> (Passos 1, 2, 3, 4, 6, 7, 8, 9 do spec
> §7.7; Passos 5 e 10 ficam pras Etapas 3 e 5), o modelo de
> `ApprovalRequest`/`ApprovalDecision`/`ApprovalScope`, a trait
> `Tool` e a única ferramenta in-process do catálogo inicial:
> `FilesReadTool` (lê arquivo do workspace, paginação via
> `max_bytes` até 50 MB, jail aplicado no `execute`). Suíte do
> crate: 41 testes cobrindo builder, registry, jail (incluindo
> symlink), validação por passo, happy path do `files.read`,
> cenários de jail do `files.read`. **Decisão da Etapa 2:**
> catálogo inicial tem **apenas** `files.read` (uma ferramenta
> profunda vale mais que duas rasas); `files.write`/`files.list`/
> `files.edit` entram na Etapa 4. Ver
> [`docs/modules/tool-registry.md`](../modules/tool-registry.md)
> para o detalhamento por eixo do template §1.4.

# Especificação do Tool Registry

O Tool Registry é a **fonte única da verdade** sobre ferramentas (`PROMPT MESTRE` §7.2). A lista de ferramentas exibida na UI, os manifestos enviados ao modelo, as permissões, as execuções e os testes derivam **deste** registro. Listas manuais duplicadas em outros arquivos são defeito (ver `REGRAS §1.9`).

## Contrato do manifesto (`PROMPT MESTRE` §7.1)

```rust
struct ToolManifest {
    id: ToolId,
    namespace: String,           // "files" | "exec" | "web" | "github" | "memory" | "docs" | "brasil"
    version: SemVer,
    display_name: String,
    description: String,         // texto que o modelo lê

    input_schema: JsonSchema,    // validado por biblioteca canônica (jsonschema crate)
    output_schema: JsonSchema,

    category: ToolCategory,
    capabilities: Vec<String>,   // livre, ex: "fs.read", "pdf.parse"

    risk_level: RiskLevel,       // safe | moderate | high | critical

    requires_network: bool,
    requires_file_read: bool,
    requires_file_write: bool,
    requires_process_execution: bool,
    requires_user_approval: bool,

    supported_platforms: Vec<Platform>,
    supported_provider_modes: Vec<ProviderMode>,  // "native-tools" | "text-emulation"

    timeout_ms: u32,
    cancellable: bool,

    availability: Availability,  // available | disabled | missing | unhealthy
    health_message: Option<String>,

    worker_id: Option<WorkerId>, // None = executa no app principal
}
```

O **schema JSON** é validado por biblioteca canônica em Rust (`jsonschema` crate) e em TS (`ajv`). Versão de schema é parte do contrato.

## Interseção de inventário por execução (`PROMPT MESTRE` §7.4)

Antes de chamar o modelo, o sistema calcula a interseção abaixo. **Somente o resultado** é serializado e enviado ao modelo como `tools:`.

```text
ferramentas registradas
∩ ferramentas disponíveis (availability == available)
∩ ferramentas saudáveis (health == ok)
∩ ferramentas compatíveis com o modelo (provider_mode ∈ supported)
∩ ferramentas autorizadas para o assistente
∩ ferramentas autorizadas para o projeto
∩ ferramentas autorizadas para a execução
∩ ferramentas autorizadas pelo usuário
```

Em Rust:

```rust
fn effective_tools(registry: &ToolRegistry, run: &Run) -> Vec<ToolManifest> {
    registry.all()
        .into_iter()
        .filter(|t| t.availability == Availability::Available)
        .filter(|t| t.health == WorkerHealth::Ok)
        .filter(|t| t.supported_provider_modes.contains(&run.provider_mode))
        .filter(|t| run.assistant.allowed_tools.contains(&t.id))
        .filter(|t| run.project.allowed_tools.contains(&t.id))
        .filter(|t| run.allowed_tools.contains(&t.id))
        .filter(|t| user_consents(run.user, t))
        .cloned()
        .collect()
}
```

## Adaptação por provedor (`PROMPT MESTRE` §7.6)

Cada provedor tem um `ProviderToolAdapter` que:

- converte `ToolManifest` no formato do provedor (OpenAI `tools`, Anthropic `tools`, Gemini `functionDeclarations`, etc.);
- normaliza `tool_call_id` para um formato interno comum;
- detecta chamadas incompletas (truncamento, schema inválido);
- detecta `finish_reason` e erros específicos.

A regra: **condições `if (provider == "x")` espalhadas pelo código são defeito**. Toda adaptação vive no adapter.

## Validação antes de execução (`PROMPT MESTRE` §7.7)

Para cada `tool_call` emitido pelo modelo, em ordem:

1. ID existe no registro?
2. Versão bate com a do manifesto desta execução?
3. Está `available` e `healthy` **neste momento**?
4. Foi essa ferramenta que o modelo viu no inventário desta execução? (defesa contra manifest injection)
5. Permissões OK? (ver [`tool-permission-model.md`](./tool-permission-model.md))
6. Argumentos validam contra `input_schema`?
7. Caminhos normalizados e dentro do jail?
8. Limites aplicados (`timeoutMs`, tamanho máximo de output)?
9. Aprovação do usuário necessária e obtida?
10. Entrada de auditoria registrada.

Falha em qualquer etapa produz erro estruturado `TOOL_NOT_FOUND` (ou código equivalente) **sem fallback silencioso** (`PROMPT MESTRE` §7.7 final, §7.2).

## Catálogo inicial (`PROMPT MESTRE` §7.11)

Cada ferramenta nasce com manifesto, testes (do §7.10) e permissões próprias:

| Ferramenta | Função | Notas obrigatórias |
|---|---|---|
| `files.read` / `files.write` / `files.edit` / `files.list` | Arquivos do workspace | Jail de caminhos; leitura paginada com limite de contexto |
| `exec.python` / `exec.node` | Código nos runtimes embutidos | Via `sandbox-runner`; descrição enviada ao modelo lista libs **geradas na build** a partir do manifesto de pacotes |
| `exec.shell` | Comandos de terminal | `risk_level: high`; allowlist executa direto, resto exige aprovação com comando exato, denylist nunca executa |
| `web.search` / `web.open` | Busca e leitura de páginas | Via `browser-worker`; SSRF e IP privado bloqueados |
| `brasil.cnpj` | Consulta cadastral de CNPJ | BrasilAPI + provedor alternativo; nunca inventar dados não retornados |
| `github.clone` / `github.commit` / `github.push` / `github.pull_request` | Integração GitHub | Git portátil embutido; token via credenciais protegidas; escrita exige aprovação |
| `memory.save` / `memory.search` | Memória | Escrita segue política do `PROMPT MESTRE` §10.9; nunca automática por mensagem |
| `docs.generate` | Geração documental | Recebe `DocumentSpec` e delega ao kit correto |
| `docs.inspect` | Revisão de artefato | Abre o arquivo real e devolve estrutura e conteúdo |

Ferramentas fora deste catálogo só entram com manifesto completo, testes do §7.10 e atualização do painel.

## Invariantes

- **Ferramenta fora do registro nunca executa.** Não há caminho de código que invoque uma tool sem passar pelo validador. *Teste: monkey-patch do executor para chamar uma `ToolId` inventada produz `TOOL_NOT_FOUND`.*
- **Modelo recebe apenas a interseção**, nunca o registro completo. *Teste: capturar a mensagem enviada ao provedor e provar que ela contém exatamente as ferramentas da interseção.*
- **Inventário é recalculado a cada execução**, não cacheado cross-execução.
- **Mudança de inventário durante a execução** (worker morre, ferramenta vira `unhealthy`) **invalida `tool_call`s pendentes** daquele tipo com `TOOL_UNAVAILABLE`.
- **Resultado volta ao mesmo modelo e à mesma execução** que invocou (`PROMPT MESTRE` §7.8). O `ToolResult` carrega `tool_call_id` que casa com a invocação.
- **Subagente recebe apenas ferramentas autorizadas para ele** (interseção adicional; ver [`tool-permission-model.md`](./tool-permission-model.md)).
- **Compatibilidade por modelo é checada dinamicamente**, não confiada em flag estática. Modelos sem suporte real a tool calling são marcados como tal e a UI bloqueia fluxos incompatíveis (`PROMPT MESTRE` §7.5).

## Não-objetivos

- Ferramentas definidas pelo usuário (DSL) na v1.
- Emulação textual de tool calling como caminho normal — apenas experimental, atrás de flag e cobertura de teste (`PROMPT MESTRE` §7.5).
- Fallback para ferramenta "parecida" quando a chamada falha — `TOOL_NOT_FOUND` é a resposta.
- Plugins carregados dinamicamente de fontes externas.
- Auto-geração de manifesto a partir de código de ferramenta — geramos **a partir de arquivos de manifesto versionados**, validados por schema.

## Decisões

### D-2026-08-03: `Tool::execute` recebe `&ToolContext` (Etapa 1 da Fase de Ligação)

**Mudança breaking** na Etapa 1 da Fase de Ligação
([`docs/architecture/process-architecture.md`](./process-architecture.md)):
a trait `Tool` carrega um `ToolContext` além dos `arguments`.
A entrega é o `Jail` resolvido por `ConversationId` (ADR-0022 §D3)
mais os IDs imutáveis do run (`conversation_id`, `run_id`,
`message_id`). `#[non_exhaustive]` no `ToolContext` permite
acrescentar campos depois (Etapa 7: `workspace:
Option<WorkspaceSnapshot>`) sem nova quebra.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn manifest(&self) -> &ToolManifest;
    async fn execute(
        &self,
        ctx: &ToolContext,
        arguments: &serde_json::Value,
    ) -> ToolResult;
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolContext {
    pub conversation_id: ConversationId,
    pub run_id: RunId,
    pub message_id: MessageId,
    pub jail: Jail,
}
```

**Por que o contexto, não só o `Jail`:** o `conversation_id` é o
que identifica o escopo; ferramentas que precisem correlacionar
com a tabela de auditoria (`tool_audit`) usam `run_id`;
ferramentas que precisem associar com a mensagem do journal usam
`message_id`. Passar isso como argumento de função separado
polui a assinatura de cada `Tool`; carregar tudo no contexto
mantém o `execute(ctx, args)` estável.

**Carregamento:** o `RunExecutor` resolve o `conversation_id`
**uma vez por run** (query única no `RunRepo::get(run_id)` no
início do `run()`) e o `Jail` (via `JailResolver`). O
`ToolContext` é construído por tool_call com custo O(1), sem
I/O por chamada. O `conversation_id` é imutável durante o run.

**Posição do `JailResolver` trait:** mora no
`frederico-tool-registry` (não no `frederico-app`, como o plano
original do ADR-0022 §D2 dizia) por uma razão prática: a
`FilesReadTool` precisa de uma referência ao trait, e o
`frederico-tool-registry` não pode depender do `frederico-app`
(seria ciclo — `frederico-app` já depende de `tool-registry`).
A `FileSystemJailResolver` (a impl default usada em produção)
continua no `frederico-app`. ADR-0022 §D2 foi revisado com
esta nota.

Decisões relacionadas:

- [`security-threat-model.md`](./security-threat-model.md) — ameaças ao registro.
- [`tool-permission-model.md`](./tool-permission-model.md) — permissões hierárquicas.

## Status por ferramenta da Fase 7 (Etapa 1, 2026-08-08)

Atualização da tabela do §"Catálogo inicial" para refletir o que a Fase 7 Etapa 1 planejou. As 5 ferramentas abaixo são **planejadas** (estado `especificado`, ainda sem código) — a Etapa 2 em diante da Fase 7 implementa cada uma no seu respective step. Cada linha referencia o ADR que fecha a decisão e a etapa que implementa.

| Ferramenta | Namespace | Etapa da Fase 7 | ADR | Status |
|---|---|---|---|---|
| `files.write` | `files` | 5 | [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) | `especificado` (atomic write + backup + audit) |
| `files.edit` | `files` | 5 | [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) | `especificado` (find literal + replace_all + atomic) |
| `files.list` | `files` | 5 | (sem ADR próprio, herda de ADR-0035 + Jail) | `especificado` (apenas lista diretório, sem ler conteúdo) |
| `exec.python` | `exec` | 4 | [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md) + [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) | `especificado` (sob SecurityJailResolver, escopo `OneTurn` default) |
| `exec.node` | `exec` | 4 | mesmo par de ADRs | `especificado` (mesma forma, runtime Node) |
| `exec.shell` | `exec` | 6 | [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md) D3 | `especificado` (sempre `OneExecution`, Denylist + Allowlist) |

**`web.search`, `web.open`, `brasil.cnpj`, `github.*`, `memory.*`** continuam com o status anterior (Fase 2 a Fase 6) e **não** entram na Fase 7. Especificamente, as ferramentas `github.*` são **adiadas para Fase 8** pelo [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) (escopo da Fase 7 vira só execução isolada).

**Catálogo implementado em produção (2026-08-08):** apenas `files.read` (Etapa 2 da Fase 3 + Etapa 1 da Fase de Ligação). As 5 ferramentas da tabela acima entram no `ToolRegistry` à medida que cada etapa da Fase 7 fecha — gate `check-e2e-gate.ps1` valida consistência com a coluna `E2E de cobertura` do `status.md`.

**Onde os detalhes de cada ferramenta nova vivem:**

- `files.write` / `files.edit` / `files.list` — sem spec próprio; o §"Catálogo inicial" deste documento + o [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) definem o contrato completo. Decisão consciente: o `tool-registry-specification.md` é o **inventário** (catálogo + validação), e o ADR é a **política** (semântica de sobrescrita). Sem duplicação.
- `exec.python` / `exec.node` / `exec.shell` — [`exec-tools-specification.md`](./exec-tools-specification.md) é o spec completo. Este spec referencia; não duplica.
- Sandbox que envolve as `exec.*` — [`windows-sandbox-design.md`](./windows-sandbox-design.md) (aprimorado na Etapa 1, 2026-08-08).
- Runtimes portáteis consumidos por `exec.python` / `exec.node` — [`runtimes-architecture.md`](./runtimes-architecture.md) (novo, Etapa 1, 2026-08-08).

## Referências

- `PROMPT MESTRE` §7 (inventário), §7.1-§7.11
- [`tool-permission-model.md`](./tool-permission-model.md)
- [`process-architecture.md`](./process-architecture.md)
- [`security-threat-model.md`](./security-threat-model.md)
- [`testing-strategy.md`](./testing-strategy.md) — testes obrigatórios do §7.10
- [`windows-sandbox-design.md`](./windows-sandbox-design.md) — sandbox da Fase 7
- [`runtimes-architecture.md`](./runtimes-architecture.md) — Python + Node portáteis
- [`exec-tools-specification.md`](./exec-tools-specification.md) — `exec.python` / `exec.node` / `exec.shell`
- [ADR-0031](../decisions/0031-fase-7-isolation-model-windows.md) — modelo de isolamento
- [ADR-0032](../decisions/0032-fase-7-scope-reduction.md) — escopo da Fase 7
- [ADR-0033](../decisions/0033-sandbox-network-policy.md) — política de rede
- [ADR-0034](../decisions/0034-fase-7-write-exec-approval-policy.md) — política de aprovação
- [ADR-0035](../decisions/0035-fase-7-file-ops-overwrite-semantics.md) — semântica de sobrescrita
- [ADR-0036](../decisions/0036-security-jail-resolver-windows-job-objects.md) — `SecurityJailResolver`
