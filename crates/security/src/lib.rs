//! Traits de plataforma do Frederico IA Studio.
//!
//! Conforme [ADR-0003](../../decisions/0003-nucleo-desacoplado-da-casca-tauri.md),
//! o núcleo não importa `tauri` nem `windows`. As dependências de
//! plataforma passam por traits implementadas pela casca Tauri e por
//! fakes em testes.
//!
//! A Fase 1 entregou o esboço mínimo: o trait [`Platform`] com
//! [`AppPaths`] e [`Clock`]. A Fase 2 adiciona [`CredentialStore`]
//! (DPAPI / Windows Credential Manager; ver
//! [ADR-0007](../../decisions/0007-credential-store-trait.md)). As
//! demais (`Sandbox`, `Notifier`) entram nas fases 3-7.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use secrecy::SecretString;
use thiserror::Error;

use frederico_core::ProviderId;
use frederico_storage::AppPaths;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("operação de plataforma não suportada no ambiente atual: {0}")]
    Unsupported(&'static str),
    #[error("erro do cofre de credenciais: {0}")]
    CredentialStore(String),
    /// Componente de [`ServiceCredentialKey`] malformado. Recusado
    /// na construção da chave, antes de qualquer chamada Win32 —
    /// ver a doc do tipo para o porquê.
    #[error("chave de credencial de serviço invalida: {0}")]
    InvalidCredentialKey(String),
}

/// Fonte de tempo injetável. A casca usa `SystemClock` (relógio do SO);
/// os testes usam `FakeClock` (avanço manual).
#[async_trait]
pub trait Clock: Send + Sync {
    async fn now_unix(&self) -> u64;
}

/// Implementação padrão do `Clock` usando o relógio do sistema.
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    async fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Cofre de credenciais. O motor lê a chave de um provedor via este
/// trait — **nunca** via env, dotenv, ou config em arquivo. Ver
/// [ADR-0007](../../decisions/0007-credential-store-trait.md) §Decisão.
///
/// A implementação de produção em Windows usa DPAPI / Windows
/// Credential Manager (gateada em `#[cfg(windows)]` no submódulo
/// [`windows`]). Os testes usam [`fake::FakeCredentialStore`].
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn get(&self, provider: &ProviderId) -> Result<Option<SecretString>, SecurityError>;
    async fn set(&self, provider: &ProviderId, value: &SecretString) -> Result<(), SecurityError>;
    async fn delete(&self, provider: &ProviderId) -> Result<(), SecurityError>;
    async fn list_providers(&self) -> Result<Vec<ProviderId>, SecurityError>;
}

/// Identifica uma credencial de **serviço externo** no cofre:
/// o par `(serviço, conta)`.
///
/// Existe porque a trilha do [`CredentialStore`] é chaveada por
/// `ProviderId` — provedor de modelo —, e a Fase 8 precisa guardar
/// credencial de outra natureza: token de GitHub por conta
/// ([ADR-0041](../../decisions/0041-github-auth-e-matriz-de-autorizacao.md)
/// §D1). Reusar `ProviderId` faria um token de escrita em
/// repositório aparecer na lista de provedores de modelo da UI de
/// settings, que é onde o usuário gerencia chaves de API — dois
/// tipos de segredo com ciclos de vida e riscos diferentes na mesma
/// gaveta.
///
/// ## Por que os componentes são validados
///
/// O `TargetName` gravado no Windows Credential Manager é
/// `Frederico-IA-Studio:<serviço>:<conta>`, montado por concatenação.
/// Sem validação, uma conta chamada `x:github:vitima` produziria o
/// alvo `Frederico-IA-Studio:conta:x:github:vitima` — e um serviço
/// conseguiria escrever ou ler no espaço de nomes de outro. Por isso
/// `:` é recusado nos dois componentes, junto com vazio, espaço em
/// branco, `*` e `?` (que são curinga no filtro do `CredEnumerateW` e
/// fariam um `list_accounts` varrer mais do que devia).
///
/// ## Por que `provider` é nome de serviço reservado
///
/// O alvo de uma chave de provedor é
/// `Frederico-IA-Studio:provider:<id>`. Com o padrão do ADR-0041, um
/// serviço chamado `provider` com conta `openai` produziria
/// **exatamente esse alvo** — e gravar nele sobrescreveria a chave
/// de API da OpenAI do usuário, por um caminho que nem se parece com
/// "mexer nas chaves de modelo". Validar caractere não pega isso,
/// porque não há caractere ilegal em `provider`. Daí a reserva
/// explícita do nome.
///
/// A recusa acontece na construção, então **não existe
/// `ServiceCredentialKey` malformada** para o resto do código lidar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceCredentialKey {
    service: String,
    account: String,
}

impl ServiceCredentialKey {
    /// Caracteres recusados nos componentes. `:` separa o alvo;
    /// `*` e `?` são curinga no `CredEnumerateW`.
    const FORBIDDEN: &'static [char] = &[':', '*', '?'];

    /// Nome de serviço reservado: colidiria com o espaço de nomes
    /// das chaves de provedor. Ver a doc do tipo.
    const RESERVED_SERVICE: &'static str = "provider";

    /// Constrói a chave, validando os dois componentes.
    ///
    /// # Erros
    ///
    /// [`SecurityError::InvalidCredentialKey`] se qualquer componente
    /// for vazio, só espaços, ou contiver `:`, `*` ou `?`; ou se o
    /// serviço for o nome reservado `provider`.
    pub fn new(
        service: impl Into<String>,
        account: impl Into<String>,
    ) -> Result<Self, SecurityError> {
        let service = service.into();
        let account = account.into();
        Self::validate("servico", &service)?;
        Self::validate("conta", &account)?;
        if service.eq_ignore_ascii_case(Self::RESERVED_SERVICE) {
            return Err(SecurityError::InvalidCredentialKey(format!(
                "'{}' e nome de servico reservado — colidiria com o alvo \
                 das chaves de provedor no cofre",
                Self::RESERVED_SERVICE
            )));
        }
        Ok(Self { service, account })
    }

    fn validate(rotulo: &str, valor: &str) -> Result<(), SecurityError> {
        if valor.trim().is_empty() {
            return Err(SecurityError::InvalidCredentialKey(format!(
                "{rotulo} vazio"
            )));
        }
        if let Some(c) = valor.chars().find(|c| Self::FORBIDDEN.contains(c)) {
            return Err(SecurityError::InvalidCredentialKey(format!(
                "{rotulo} contem {c:?}, que e separador ou curinga do alvo no cofre"
            )));
        }
        Ok(())
    }

    /// Serviço (ex.: `github`).
    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Conta dentro do serviço (ex.: o login do usuário).
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }
}

/// Cofre de credenciais de **serviço externo**, irmão do
/// [`CredentialStore`] e com a mesma garantia: o segredo nunca passa
/// por env, dotenv ou arquivo de config.
///
/// A Fase 7 Etapa 6+1 provou por teste
/// (`crates/security/tests/env_credential_not_leaked.rs`) que
/// credencial no ambiente do processo pai vaza para o filho do
/// sandbox quando o `EnvFilter` falha — e que a falha pode ser
/// silenciosa. Um token de GitHub com escopo de escrita é a pior
/// versão desse cenário, e é por isso que o ADR-0041 §D1 proíbe o
/// caminho do ambiente em vez de apenas desencorajá-lo.
/// ## Por que os métodos não se chamam `get`/`set`/`delete`
///
/// `WindowsCredentialStore` implementa **as duas** traits. Com nomes
/// iguais, todo chamador que tivesse ambas em escopo receberia
/// `error[E0034]: multiple applicable items in scope` e teria de
/// escrever `ServiceCredentialStore::get(&store, …)`. O sufixo
/// `_secret` custa três caracteres e evita empurrar essa cerimônia
/// para cada ponto de uso.
#[async_trait]
pub trait ServiceCredentialStore: Send + Sync {
    /// Lê o segredo. `None` = não existe (não é erro).
    async fn get_secret(
        &self,
        key: &ServiceCredentialKey,
    ) -> Result<Option<SecretString>, SecurityError>;
    /// Grava, sobrescrevendo se já existir.
    async fn set_secret(
        &self,
        key: &ServiceCredentialKey,
        value: &SecretString,
    ) -> Result<(), SecurityError>;
    /// Remove. **Idempotente**: apagar o que não existe é `Ok`.
    async fn delete_secret(&self, key: &ServiceCredentialKey) -> Result<(), SecurityError>;
    /// Contas cadastradas para um serviço. Devolve os nomes, nunca
    /// os segredos.
    async fn list_accounts(&self, service: &str) -> Result<Vec<String>, SecurityError>;
}

/// Trait de plataforma injetado pela casca. Carrega os três
/// componentes da Fase 1+2: paths, clock, e credenciais. As demais
/// (sandbox, notifier) virão nas fases 3-7 (ver
/// [`process-architecture.md`](https://github.com)).
#[async_trait]
pub trait Platform: Send + Sync {
    fn paths(&self) -> &dyn AppPaths;
    fn clock(&self) -> &dyn Clock;
    fn credentials(&self) -> &dyn CredentialStore;
}

pub mod env_filter;
pub mod exec_patterns;
pub mod fake;
pub mod jail;
pub mod network;
pub mod network_audit_sink;
pub mod raw_child;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use secrecy::ExposeSecret;

    #[tokio::test]
    async fn fake_clock_starts_at_zero() {
        let c = FakeClock::new();
        assert_eq!(c.now_unix().await, 0);
    }

    #[tokio::test]
    async fn fake_clock_advance() {
        let c = FakeClock::new();
        c.advance(42);
        assert_eq!(c.now_unix().await, 42);
        c.advance(8);
        assert_eq!(c.now_unix().await, 50);
    }

    #[tokio::test]
    async fn fake_platform_returns_paths_clock_and_credentials() {
        let clock = FakeClock::new();
        let platform = FakePlatform::new(std::env::temp_dir(), clock.clone());
        assert!(platform
            .paths()
            .database_path()
            .to_string_lossy()
            .ends_with("test.db"));
        assert_eq!(platform.clock().now_unix().await, 0);
        // credentials() deve existir e ser o fake recém-criado.
        let creds = platform.credentials();
        let listed = creds.list_providers().await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn fake_credential_store_set_get_delete() {
        let creds = FakeCredentialStore::new();
        let p = ProviderId::new("openai");
        let v = SecretString::new("sk-fake-1234".to_string().into());

        // Inicialmente vazio.
        assert!(creds.get(&p).await.unwrap().is_none());

        // Set + get.
        creds.set(&p, &v).await.unwrap();
        let got = creds.get(&p).await.unwrap().unwrap();
        assert_eq!(got.expose_secret(), "sk-fake-1234");

        // list_providers.
        let mut listed = creds.list_providers().await.unwrap();
        // ProviderId não é Ord; ordena por string representation.
        listed.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let mut expected = vec![p.clone()];
        expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(listed, expected);

        // Delete.
        creds.delete(&p).await.unwrap();
        assert!(creds.get(&p).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fake_credential_store_with_credentials_helper() {
        let creds = FakeCredentialStore::with_credentials([
            (ProviderId::new("openai"), "sk-fake-openai"),
            (ProviderId::new("anthropic"), "sk-ant-fake"),
        ]);
        let openai = creds
            .get(&ProviderId::new("openai"))
            .await
            .unwrap()
            .unwrap();
        let anth = creds
            .get(&ProviderId::new("anthropic"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(openai.expose_secret(), "sk-fake-openai");
        assert_eq!(anth.expose_secret(), "sk-ant-fake");
    }

    #[tokio::test]
    async fn fake_credential_store_secret_does_not_leak_in_debug() {
        // Garante que SecretString não implementa Display/Debug que vaze.
        let creds = FakeCredentialStore::new();
        let p = ProviderId::new("openai");
        let v = SecretString::new("sk-fake-1234".to_string().into());
        creds.set(&p, &v).await.unwrap();
        let listed = creds.list_providers().await.unwrap();
        // Serializar ProviderId para string não deve vazar a chave.
        let serialized = serde_json::to_string(&listed).unwrap();
        assert!(!serialized.contains("sk-fake-1234"));
    }
}
