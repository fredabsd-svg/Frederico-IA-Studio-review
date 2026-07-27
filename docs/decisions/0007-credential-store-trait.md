# 0007 — `CredentialStore` no `frederico-security` (DPAPI) e a regra "nunca um shim de texto puro"

## Contexto

A Fase 2 cadastra credenciais de provedores (chaves de API da OpenAI, Anthropic, OpenRouter, DeepSeek, Mistral, NIM). Essas credenciais são o ativo mais sensível do app, depois dos dados do usuário.

O `security-threat-model.md` §"Credenciais" já declara o alvo:

- **Onde**: Windows Credential Manager (DPAPI), vinculadas ao usuário Windows.
- **Onde nunca**: `.env`, SQLite em texto puro, logs, JSON, frontend, memória semântica.
- **Quem acessa**: apenas o adapter do provedor, no momento de uso.
- **Quem nunca acessa**: workers (eles recebem tarefa, não credencial); frontend (apenas referência opaca, ex: `openai-configured: true`).
- **Teste automático obrigatório**: nenhuma variável de ambiente do processo do sandbox contém valor de credencial cadastrada (I1 do threat model).

E a entrada T1 do threat model é a rede de segurança: "Filtro de logging + varredura automatizada em CI; forçar print de credencial em código de teste → teste falha".

Hoje o `frederico-security` só tem os traits `AppPaths` e `Clock` (via `Platform`). Não existe ainda uma porta de entrada para credenciais. O `provider-engine` está prestes a ser criado (ADR-0005) e vai precisar ler chaves a cada request.

O risco concreto que este ADR existe para evitar é o **shim temporário de texto puro**: alguém, com a desculpa de "primeiro a gente faz funcionar, depois a gente troca", coloca uma `OPENAI_API_KEY` lida de env, ou um campo `api_key: String` em um `config.toml`, ou um parâmetro `String` na função do adapter. Esse shim:

1. entra no teste e na fixture,
2. vaza no log quando algum `Debug`/`Display` é chamado por acidente,
3. vaza no snapshot/recording do provider (ADR-0008),
4. vaza no golden file de teste (mais um motivo para o ADR-0008 ter sanitização),
5. **nunca mais sai** — porque agora há código que depende dele, e remover é reescrever.

A história do projeto anterior tem esse padrão exato como causa raiz de incidentes. Esta decisão é a parede de concreto contra ele.

## Decisão

### Trait `CredentialStore`

Adicionar ao trait `Platform` em `crates/security/src/lib.rs`, ao lado de `paths()` e `clock()`:

```rust
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn get(&self, provider: &ProviderId) -> Result<Option<SecretString>, SecurityError>;
    async fn set(&self, provider: &ProviderId, value: &SecretStr) -> Result<(), SecurityError>;
    async fn delete(&self, provider: &ProviderId) -> Result<(), SecurityError>;
    async fn list_providers(&self) -> Result<Vec<ProviderId>, SecurityError>;
}
```

- `SecretString`/`SecretStr` vêm do crate `secrecy` — wrappers que **não implementam `Display` nem `Debug`**, só `ExposeSecret`. Esquecer de chamar `.expose_secret()` é erro de compilação.
- O crate `provider-engine` recebe o `CredentialStore` por trait object, nunca um `String`/`&str` com a chave.

### Implementações

- **Windows** (`crates/security/src/windows/credential_store.rs`, gateado por `#[cfg(windows)]`): usa `windows-rs` + `CredWriteW`/`CredReadW` da Win32 para Windows Credential Manager. Credenciais são criptografadas com DPAPI e atreladas ao usuário Windows — outro usuário da mesma máquina não consegue ler. Esta é a **única** parte do núcleo com dependência de plataforma, e ela é `cfg(windows)`, então o gate de pureza do ADR-0003 continua valendo.
- **Fake** (`crates/security/src/fake/credential_store.rs`): `HashMap<ProviderId, SecretString>` em memória, atrás de `Arc<Mutex<...>>`. É o que os testes usam e o que roda se a implementação Windows ainda não estiver completa.

### A regra "nunca um shim de texto puro" — disciplina verificável

A regra é: **do primeiro commit da Fase 2 em diante, o trait `CredentialStore` existe. Se a implementação Windows não está pronta, só a `FakeCredentialStore` roda. Nenhum caminho de código pode aceitar uma chave em texto puro de config, env, ou frontend.**

Para tornar a regra verificável, três mecanismos:

1. **Lint estendido em `scripts/check-core-purity.ps1`**: o script varrre `crates/provider-engine/` e falha se encontrar:
   - `use std::env::var` ou `std::env::var(` ou `dotenv` ou `dotenvy`
   - `use serde_json` parseando arquivos de config no `provider-engine` (config de provedor é via `CredentialStore`, não via arquivo)
   - O script **não** falha em `crates/security/src/windows/` (a dependência `windows` é esperada lá).
2. **Teste de contrato em `provider-engine`**: um teste de integração que monkey-patcha o env (via `set_var` numa thread filha antes do `fork`/`spawn`), executa um ciclo completo de chat com `FakeCredentialStore`, e verifica que (a) a chave configurada veio do trait, (b) o env do processo do adapter não contém a chave.
3. **Teste de segredo em log**: quando o motor emite log de debug (nível `trace` ou `debug`), nenhuma string registrada contém um valor que veio de `SecretString::expose_secret`. Implementado como subscriber de `tracing` que filtra valores que casam com o prefixo conhecido de cada provider (`sk-`, `sk-ant-`, `gsk_`, `or-`, ...) e falha o teste se casar.

### Fronteira com a UI

O frontend nunca vê a chave. O contrato IPC expõe:

```rust
enum AppOp {
    // ...
    ProviderListConfigured,                    // -> Vec<ProviderId>
    ProviderSetCredential { provider: ProviderId, value: SecretString },
    ProviderDeleteCredential { provider: ProviderId },
    ProviderStatus { provider: ProviderId },   // -> { configured: bool, last_ok: Option<Timestamp> }
    ProviderTestConnection { provider: ProviderId, model: ModelId },
}
```

A UI envia `SecretString` apenas no momento do `Set`, e o `services/` da casca é a única camada que faz isso. O objeto no estado React nunca contém a chave — só `provider.configured: boolean`.

### Migração de credenciais entre máquinas

Fora de escopo da v1. O usuário que troca de máquina recadastra. Exportar DPAPI entre perfis é um problema conhecido do Windows (não trivial sem ferramenta de linha de comando específica); não cabe na Fase 2.

## Alternativas descartadas

- **Crate `keyring` (cross-platform)**. Descartada: dependência a mais para auditar; v1 é Windows-only e o alvo declarado é DPAPI. Adotar um crate genérico agora paga o custo de generalidade sem o benefício.
- **`.env` ou SQLite plaintext**. Explicitamente proibido pelo threat model. Listar aqui só para registrar que foi considerado e rejeitado.
- **`OPENAI_API_KEY` como env var de processo**. Explicitamente proibido pela invariante I1 do threat model — mesmo no processo do app principal, ler essa env var é o atalho que vaza no log, na fixture, no debug print, no panic message. Não existe.
- **Tokens de curta duração emitidos pelo app para o adapter**. Descartada: é uma camada que faz sentido entre app e workers (Fase 5+), não entre app e provedor. O provedor recebe a chave, sempre — não há outro lado para emitir um token. Confundir as duas camadas é o tipo de erro que este ADR existe para evitar.
- **Cofre de segredos dedicado** (HashiCorp Vault, Azure Key Vault, ...). Descartada: serviço externo, autenticação própria, mais um segredo para proteger. v1 desktop offline-first não tem isso.

## Consequências

**Mais fácil:**

- Nenhum caminho de código tem atalho para texto puro, então não há vetor de vazamento por esse canal.
- Windows Credential Manager isola a chave do DB e dos logs; rotação é `delete` + `set`.
- Trait no `Platform` mantém o núcleo puro (o impl Windows vive em `cfg(windows)`).
- `SecretString` do `secrecy` torna o vazamento por `Debug`/`Display` em erro de compilação — não em convenção.

**Mais difícil:**

- A implementação DPAPI é a única parte Windows-specific do núcleo. Requer `windows-rs` na árvore de deps do `frederico-security` (feature flag, não propagada).
- Toda a suíte de testes do `provider-engine` precisa do `FakeCredentialStore` configurado — o que é trabalho, não atrito.
- O lint estendido em `check-core-purity.ps1` tem uma responsabilidade a mais; se quebrar, bloqueia o CI.
- O teste de segredo em log requer monkey-patching do `tracing` subscriber — não é trivial. Mas é o tipo de teste que se escreve uma vez e protege o resto do projeto.
- Migração entre máquinas é trabalho manual do usuário; documentar isso no README de credenciais é obrigatório.
