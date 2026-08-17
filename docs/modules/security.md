<!--
Estado: parcialmente implementado
Verificado contra o código em: 2026-08-17
Fase correspondente: 1
-->

# Módulo `security`

> Crate: [`crates/security/`](../../crates/security/)
> Nome do pacote: `frederico-security`

## O que faz

Duas coisas que cresceram juntas:

1. **Traits de plataforma** que o núcleo usa para falar com o sistema
   operacional sem importar nada de Windows ou Tauri (ver
   [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md)).
   A casca implementa e injeta na inicialização; em testes, os fakes
   de `security::fake::*` substituem.
2. **O sandbox de execução da Fase 7** — Jail, Job Object, Restricted
   Token, filtro de ambiente, proxy de rede e validação de comando do
   `exec.shell`. Aqui o crate deixa de ser só declaração de trait e
   passa a ter implementação Windows real, isolada em `windows.rs` e
   submódulos.

O `unsafe` do crate é `forbid` no `Cargo.toml`, com exceção pontual e
comentada nos módulos que são ponte com a Win32.

## O que expõe

**Plataforma**

- `trait Platform` — superfície de plataforma do núcleo.
- `trait Clock` + `SystemClock` — fonte de tempo injetável.
- `trait CredentialStore` — cofre de chaves de **provedor de modelo**,
  chaveado por `ProviderId` (Fase 2,
  [ADR-0007](../decisions/0007-credential-store-trait.md)).
- `trait ServiceCredentialStore` + `ServiceCredentialKey` — cofre de
  credenciais de **serviço externo**, chaveado por `(serviço, conta)`
  (Etapa 2 da Fase 8,
  [ADR-0041](../decisions/0041-github-auth-e-matriz-de-autorizacao.md) §D1).
- `windows::WindowsCredentialStore` — implementação DPAPI dos dois
  cofres, via Windows Credential Manager.
- `mod fake` — `FakePlatform`, `FakePaths`, `FakeClock`,
  `FakeCredentialStore` (que também implementa os dois cofres).

**Sandbox (Fase 7)**

- `jail::{SecurityJailResolver, SandboxConfig, SandboxedProcess}`.
- `env_filter::{EnvFilter, EnvAllowlist}`.
- `network::*` — proxy HTTP/CONNECT local com allowlist deny-by-default
  e sink de auditoria.
- `exec_patterns::*` — validação de comando do `exec.shell`
  ([ADR-0044](../decisions/0044-exec-shell-com-resolucao-propria-de-programa.md)).
- `windows::{JobObject, RestrictedToken, set_low_integrity_label}`.
- `SecurityError` — erros do módulo.

## De quem depende / quem depende dele

- **Depende de:** `frederico-core`, `frederico-storage` (trait
  `AppPaths`), `async-trait`, `thiserror`, `secrecy`, `tokio`,
  `windows` (só sob `cfg(windows)`).
- **Usado por:** `frederico-desktop` (casca real), `frederico-app`
  (composição), `frederico-tool-registry` (as `exec.*` consomem o
  `SecurityJailResolver` e o `exec_patterns`).

## Decisões não óbvias / armadilhas

- **`provider` é nome de serviço reservado.** O alvo de uma chave de
  provedor no cofre é `Frederico-IA-Studio:provider:<id>`, e o de uma
  credencial de serviço é `Frederico-IA-Studio:<serviço>:<conta>`. Um
  serviço chamado `provider` com conta `openai` produziria **o mesmo
  alvo** da chave de API da OpenAI, e gravar nele a sobrescreveria.
  `ServiceCredentialKey::new` recusa o nome; validar caractere não
  pegaria, porque não há caractere ilegal em `provider`. Fixado em
  `service_key_refuses_the_reserved_provider_namespace`.
- **`:`, `*` e `?` são recusados nos componentes da chave.** O `:`
  separa o alvo; `*` e `?` são curinga no filtro do `CredEnumerateW`,
  e um serviço com `*` faria o `list_accounts` varrer o cofre inteiro.
- **`ServiceCredentialStore` usa `get_secret`/`set_secret`/
  `delete_secret`**, não `get`/`set`/`delete`. A
  `WindowsCredentialStore` implementa as duas traits, e nomes iguais
  dariam `error[E0034]` em qualquer chamador com ambas em escopo.
- **Credencial nunca vai para o ambiente do processo.** A Etapa 6+1 da
  Fase 7 provou por teste (`tests/env_credential_not_leaked.rs`) que
  um segredo no ambiente do pai vaza para o filho do sandbox quando o
  `EnvFilter` falha — e que a falha pode ser silenciosa.
- **`CredFree` é por bloco, não por item.** O `CredEnumerateW` aloca o
  array de ponteiros e todas as `CREDENTIALW` num **único** bloco;
  chamar `CredFree` em cada item é double-free com
  `STATUS_HEAP_CORRUPTION`.
- A guarda "núcleo sem dependências de plataforma" é verificada por
  `scripts/check-core-purity.ps1`, que falha se um crate de `crates/`
  declarar `tauri`, `windows`, `winapi` ou `winrt` fora das exceções
  registradas.

## Como testar isoladamente

```pwsh
cargo test -p frederico-security
```

Os testes de `tests/windows_credential_store.rs` falam com o **Windows
Credential Manager real** e se serializam num mutex global, porque o
cofre é um recurso do SO. Cada teste gera chaves únicas por PID +
contador e limpa no `Drop`.

## O que este módulo **não** faz

- Não implementa a UI de gerenciamento de credenciais (é da casca).
- Não decide **autorização** — o cofre guarda segredo; quem pode usá-lo
  para quê é o `PermissionSet` do `tool-registry`.
- Não fecha as lacunas de rede nomeadas no `SECURITY.md` (DNS
  exfiltration e bypass por socket raw exigem filtro no nível de
  processo, fora do escopo da Fase 8 por decisão do ADR-0039 §D4).
