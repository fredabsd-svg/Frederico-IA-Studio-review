<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 1
-->

# Arquitetura de Software

O Frederico IA Studio é organizado em **dois anéis concêntricos que não compartilham tipos nem dependências de plataforma**: o **núcleo** (lógica de negócio, testável sem GUI) e a **casca** (Tauri + React, injeta plataforma Windows). Esta separação é obrigatória desde a v1 (ver ADR-0003).

## Layout do repositório

Adotado literalmente do `PROMPT MESTRE` §5.4. Ver detalhes em [ADR-0002](../decisions/0002-monorepo-layout.md).

```text
apps/desktop/           # casca Tauri + React + TypeScript + Vite
crates/                 # núcleo Rust (um crate por subsistema)
workers/                # sidecars empacotados (executáveis separados)
packages/               # código compartilhado entre casca e workers
docs/                   # toda a documentação
tests/                  # suítes (unit, integration, e2e, security, recovery, documents, installer)
```

Cada crate em `crates/` nasce com responsabilidade clara e teste próprio. **Não criamos crate por substantivo da especificação** (`PROMPT MESTRE` §5.3 "Pragmatismo").

## Crates previstos na fundação (Fase 1)

Lista inicial, não exaustiva — novos crates são extraídos quando um existente crescer demais:

- `core` — tipos compartilhados, erros, identificadores opacos, utilitários de plataforma.
- `storage` — SQLite, migrações numeradas, FTS5, queries base.
- `agent-engine` — máquina de estados do `Run`, checkpoints, recuperação (ver [`agent-state-machine.md`](./agent-state-machine.md)).
- `tool-registry` — manifesto, descoberta, interseção por execução (ver [`tool-registry-specification.md`](./tool-registry-specification.md)).
- `provider-engine` — adaptadores de provedor (OpenAI-compat, OpenAI, Anthropic, Gemini, DeepSeek, Mistral, NVIDIA).
- `model-catalog` — metadados, preços, capacidades, teste de compatibilidade.
- `execution-engine` — coordenação entre motor, tools e persistência.
- `security` — traits de plataforma (credenciais, sandbox, paths), implementações Windows, threat model aplicado.
- `diagnostics` — logs estruturados, telemetria, tela de diagnóstico.

Crates de Fase 2+ (`memory-engine`, `multimodel-engine`, `subagent-engine`, `document-engine`, `github-engine`, `project-engine`) começam como submódulos de crates maiores e são extraídos quando precisarem.

## Contratos

### Identificadores opacos (núcleo, Rust)

```rust
struct RunId(Uuid);
struct ConversationId(Uuid);
struct ProjectId(Uuid);
struct AssistantId(Uuid);
struct ProviderId(String);
struct ModelId(String);
struct ToolId(String);
struct WorkerId(String);
struct CheckpointId(Uuid);
struct ArtifactId(Uuid);
```

Identificadores não são serializados como caminhos do sistema de arquivos (preparação para multiusuário, ADR-0003).

### Trait de plataforma (núcleo)

```rust
trait Platform: Send + Sync {
    fn env(&self) -> &dyn EnvironmentProvider;
    fn credentials(&self) -> &dyn CredentialStore;
    fn sandbox(&self) -> &dyn SandboxLauncher;
    fn paths(&self) -> &dyn AppPaths;
    fn notifier(&self) -> &dyn Notifier;
    fn clock(&self) -> &dyn Clock;            // injetável em testes
}
```

A casca Tauri implementa `Platform` para Windows usando `windows-rs`, DPAPI, AppContainer/Job Objects, e diretórios resolvidos por `dirs`/`known-folders`. Os testes usam `FakePlatform` em `crates/security/src/fake/`.

### Camada de serviços do frontend

`apps/desktop/src/services/` é a **única** camada que faz `invoke` do Tauri. Componentes React nunca chamam `invoke` diretamente. O módulo exporta funções tipadas que traduzem chamadas para o contrato em `packages/shared-contracts/`. Isso permite trocar IPC por HTTP/WebSocket amanhã sem alterar componentes (ADR-0003).

## Invariantes

- **Núcleo roda sem Tauri.** Suítes de integração do motor executam sem `apps/desktop` carregado; testes de plataforma usam `FakePlatform`. *Critério de verificação contínuo: o motor só funciona dentro do aplicativo desktop é violação desta invariante e da decisão ADR-0003.*
- **Nenhum crate em `crates/` importa `tauri`, `windows`, ou caminhos absolutos de SO.** Detectado por lint customizado ou `cargo deny` no CI (a definir na Fase 1).
- **Identificadores não dependem de caminhos do sistema de arquivos.** Preparação para multiusuário (ADR-0003).
- **Nenhum worker abre servidor em `localhost`.** Comunicação por IPC, named pipes ou canais internos. (Ver [`process-architecture.md`](./process-architecture.md).)
- **Nenhum caminho de arquivo é construído concatenando strings em mais de um lugar.** Tudo passa por `AppPaths::resolve(LogicalPath)`, e a normalização é responsabilidade única desse ponto. Teste E2E tenta `..\..\etc\passwd` em todas as tools de arquivo e rejeita.
- **Núcleo abaixo de 500 MB em repouso.** Nenhum modelo de aprendizado carregado na inicialização (`PROMPT MESTRE` §23.7).

## Não-objetivos

- Camada de abstração sobre GPU, áudio, Bluetooth, sensores — não usados pela v1.
- Sistema de plugins carregáveis dinamicamente.
- Suporte a múltiplas versões do mesmo crate rodando no mesmo processo.
- Internacionalização além de pt-BR na v1 (a estrutura de strings é preparada, mas apenas pt-BR é ativado).
- Qualquer dependência que o usuário precise instalar manualmente (`PROMPT MESTRE` §5.2).

## Decisões

- [ADR-0002](../decisions/0002-monorepo-layout.md) — layout do monorepo.
- [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md) — desacoplamento núcleo/casca.
- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — por que Python embutido nos kits de documentos.

## Referências

- `PROMPT MESTRE` §5 (plataforma e arquitetura), §5.4 (layout), §5.5 (núcleo desacoplado), §23.7 (desempenho)
- [`process-architecture.md`](./process-architecture.md) — processos, IPC, workers
- [`agent-state-machine.md`](./agent-state-machine.md) — máquina de estados
- [`security-threat-model.md`](./security-threat-model.md) — superfície de ataque
- [`testing-strategy.md`](./testing-strategy.md) — como invariantes viram testes
