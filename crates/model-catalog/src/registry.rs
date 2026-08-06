//! `SpecialistRegistry` — interface + implementação default.
//!
//! Ver o spec [`docs/architecture/subagent-architecture.md` §"Contrato"](../architecture/subagent-architecture.md)
//! e o [ADR-0030](../decisions/0030-specialist-registry-from-model-catalog.md).
//!
//! ## Decisões carregadas do ADR
//!
//! - **D2**: `SpecialistRegistry` é a **interface** (trait), não a
//!   fonte. `DefaultSpecialistRegistry` é a impl padrão que carrega
//!   bundled + override. O `SubagentRunner` da Etapa 4 consome
//!   `Arc<dyn SpecialistRegistry>` — não conhece a fonte (mesma
//!   abstração do `WorkerInvoker`, ADR-0024).
//! - **D4**: erro `UnknownSpecialist` **sempre** carrega a lista
//!   de IDs válidos. Zero fallback silencioso (§9.2). A UI da
//!   Etapa 6 renderiza como modal de "subagente não encontrado,
//!   disponíveis: [...]".
//! - **D1**: usuário pode **adicionar** novos IDs no override
//!   mas **não pode invadir** os 8 bundled. A `DefaultSpecialistRegistry`
//!   aplica essa regra: se o override tenta redeclarar um ID
//!   bundled, o bundled vence (sem merge silencioso, sem panic —
//!   warning explícito no log). Mesma família de "mais restritivo
//!   vence" do `permission_loader` (Etapa 3 PR 2).
//!
//! ## Override path
//!
//! `~/.config/frederico/specialists.toml` (Linux/macOS) ou
//! `%APPDATA%\Frederico\specialists.toml` (Windows). Resolvido via
//! crate `directories`. Se o arquivo não existir, o registry usa
//! só o bundled (caminho normal). **Erros de parse do override**
//! viram warning + fallback pro bundled (degradação declarada —
//! mesma família do `OpenRouter API key ausente` da Etapa 3 da
//! Fase de Ligação). Erro fatal seria pior: o app não inicia
//! por config corrompida do usuário.

use std::sync::Arc;

use thiserror::Error;

use crate::specialist::{
    parse_specialists_toml, SpecialistDefinition, SpecialistId, SpecialistSummary,
};

/// Erro estruturado do `SpecialistRegistry::get`.
///
/// **D4 do ADR-0030**: `UnknownSpecialist` **sempre** carrega a
/// lista de IDs válidos. Sem `valid: Vec<SpecialistId>` o erro
/// não compila (variant com campo obrigatório) — defesa em
/// profundidade contra o "esqueci de listar os válidos".
#[derive(Debug, Error)]
pub enum RegistryError {
    /// O ID solicitado não está no registry. `valid` lista os
    /// IDs conhecidos no momento da chamada (não uma snapshot —
    /// calculado toda vez, então reflete o estado atual do
    /// registry).
    #[error(
        "subagente '{requested}' não encontrado. Os subagentes disponíveis são: [{}]. \
         Cancele a operação ou escolha um dos disponíveis.",
        valid.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    )]
    UnknownSpecialist {
        requested: String,
        valid: Vec<SpecialistId>,
    },

    /// O `default_model` do `SpecialistDefinition` não está no
    /// `Catalog`. Diferente de "ID inválido" — o ID existe, o
    /// modelo é que não. UI mostra "modelo default do
    /// especialista X (gpt-4o) não está no catálogo de modelos.
    /// Verifique a configuração."
    #[error(
        "default_model '{requested}' do especialista '{specialist}' não encontrado no catálogo. \
         O especialista existe (ID válido) mas o modelo declarado não está disponível."
    )]
    DefaultModelNotFound {
        specialist: SpecialistId,
        requested: String,
    },

    /// Erro de configuração irrecuperável (arquivo de override
    /// presente mas ilegível por I/O). O registry loga warning
    /// e cai pro bundled (degradação declarada).
    #[error("falha ao ler configuração de especialistas em {path}: {cause}")]
    ConfigurationError {
        path: std::path::PathBuf,
        cause: String,
    },
}

/// Interface do registry. Consumida pelo `SubagentRunner` (Etapa 4)
/// e pelo Tauri command `ListSpecialists` (Etapa 3) via
/// `Arc<dyn SpecialistRegistry>`.
///
/// **Por que trait, não struct concreta:** extensibilidade
/// futura sem mexer no `SubagentRunner` (registry carregado de
/// servidor, registry carregado de arquivo de projeto, registry
/// de testes com catálogo mockado). Mesma justificativa do
/// `WorkerInvoker` (ADR-0024) e do `JailResolver` (ADR-0022).
pub trait SpecialistRegistry: Send + Sync {
    /// Busca um especialista pelo ID. Retorna
    /// `Err(RegistryError::UnknownSpecialist { requested, valid })`
    /// se não existir — com `valid` sempre presente.
    fn get(&self, id: &SpecialistId) -> Result<&SpecialistDefinition, RegistryError>;

    /// Lista todos os especialistas conhecidos (bundled +
    /// override). Ordem: bundled primeiro (ordem do
    /// `default.toml`), depois override novo (ordem do
    /// arquivo). Sem deduplicação — IDs únicos garantidos pelo
    /// construtor do `DefaultSpecialistRegistry`.
    fn list(&self) -> Vec<&SpecialistDefinition>;

    /// Valida um ID (string crua, como vem do modelo) e
    /// devolve o `SpecialistId` canônico ou erro com a lista
    /// de válidos. Usado pelo `SubagentRunner` antes de delegar
    /// (defesa em profundidade — o registry também checa no
    /// `get`, mas validar uma vez no boundary é mais barato).
    fn validate_id(&self, id: &str) -> Result<SpecialistId, RegistryError>;
}

/// Helper **free** (não método do trait) que monta a lista de
/// `SpecialistSummary` consumida pelo `ListSpecialists` Tauri
/// command. Precisa ser free porque a versão com closure
/// genérica quebra dyn-compatibility do trait (Rust não
/// monomorfiza métodos de trait objects). O `resolve_capabilities`
/// é fornecido pelo caller — tipicamente o
/// `build_specialist_registry` em `composition.rs` consulta o
/// `Catalog::find_model` e converte o `CapabilitySet` em
/// `Vec<String>`.
pub fn list_summaries<F>(
    registry: &dyn SpecialistRegistry,
    resolve_capabilities: F,
) -> Vec<SpecialistSummary>
where
    F: Fn(&SpecialistDefinition) -> Vec<String>,
{
    registry
        .list()
        .into_iter()
        .map(|def| {
            let caps = resolve_capabilities(def);
            SpecialistSummary::from_definition(def, caps)
        })
        .collect()
}

/// Implementação default. Carrega bundled do `OUT_DIR` (env var
/// `SPECIALISTS_TOML_PATH` setada pelo `build.rs`) e merge com
/// override do usuário (`~/.config/frederico/specialists.toml`).
///
/// **Construtor**: [`DefaultSpecialistRegistry::load`] é a forma
/// padrão. [`DefaultSpecialistRegistry::from_parts`] existe pros
/// testes (permite injetar bundled + override custom).
pub struct DefaultSpecialistRegistry {
    /// Ordem: bundled primeiro, depois override novo (sem
    /// duplicatas — IDs únicos enforçados no construtor).
    definitions: Vec<SpecialistDefinition>,
    /// Cache de IDs canônicos pra mensagem de erro rápida.
    /// Recalculado de `definitions` se invalidar (não acontece
    /// em runtime — `definitions` é imutável depois do `load`).
    valid_ids: Vec<SpecialistId>,
}

impl DefaultSpecialistRegistry {
    /// Carrega bundled + override do path default. Loga warning
    /// se o override existe mas falha parse (degradação
    /// declarada — bundled vence).
    pub fn load() -> Self {
        let bundled = match Self::load_bundled() {
            Ok(defs) => defs,
            Err(e) => {
                // Bundled é embedded pelo build.rs e validado no
                // build. Se chegou aqui, é bug — o build
                // deveria ter quebrado. Panic é justificado.
                panic!("specialists/default.toml bundled não carregou: {e}");
            }
        };
        let override_defs = match Self::load_override() {
            Ok(defs) => defs,
            Err(OverrideLoadError::NotFound) => Vec::new(),
            Err(OverrideLoadError::Parse { path, cause }) => {
                tracing::warn!(
                    specialists.override = %path.display(),
                    "specialists.toml do usuário falhou no parse; usando só bundled. \
                     Causa: {cause}. Corrija o TOML ou delete o arquivo."
                );
                Vec::new()
            }
            Err(OverrideLoadError::Io { path, cause }) => {
                tracing::warn!(
                    specialists.override = %path.display(),
                    "specialists.toml do usuário falhou no I/O; usando só bundled. \
                     Causa: {cause}. Verifique permissões do arquivo."
                );
                Vec::new()
            }
        };
        Self::from_parts(bundled, override_defs)
    }

    /// Carrega o `default.toml` bundled (env var
    /// `SPECIALISTS_TOML_PATH`, setada pelo `build.rs`).
    fn load_bundled() -> Result<Vec<SpecialistDefinition>, String> {
        let path = env!("SPECIALISTS_TOML_PATH");
        let text = std::fs::read_to_string(path).map_err(|e| format!("leitura de {path}: {e}"))?;
        parse_specialists_toml(&text)
    }

    /// Carrega o override do usuário.
    /// `~/.config/frederico/specialists.toml` (Unix) ou
    /// `%APPDATA%\Frederico\specialists.toml` (Windows).
    fn load_override() -> Result<Vec<SpecialistDefinition>, OverrideLoadError> {
        let path = match override_path() {
            Some(p) => p,
            None => return Err(OverrideLoadError::NotFound),
        };
        if !path.exists() {
            return Err(OverrideLoadError::NotFound);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| OverrideLoadError::Io {
            path: path.clone(),
            cause: e.to_string(),
        })?;
        parse_specialists_toml(&text).map_err(|cause| OverrideLoadError::Parse { path, cause })
    }

    /// Construtor que recebe bundled + override separadamente
    /// (pros testes). Aplica a regra de merge:
    ///
    /// - IDs presentes no **bundled** e no **override**: o
    ///   **bundled vence**. O override é logado como warning
    ///   (defesa contra "usuário redefine o que vem no app
    ///   sem entender o impacto"). Mesma família do "mais
    ///   restritivo vence" do `permission_loader` (PR 2).
    /// - IDs novos no **override**: aceitos.
    /// - IDs duplicados **dentro** do override: o segundo
    ///   vence, com warning (defensivo — o usuário pode ter
    ///   duplicado sem querer).
    fn from_parts(
        bundled: Vec<SpecialistDefinition>,
        override_defs: Vec<SpecialistDefinition>,
    ) -> Self {
        let mut definitions = bundled.clone();
        let mut seen: std::collections::HashSet<String> =
            bundled.iter().map(|d| d.id.as_str().to_string()).collect();

        for def in override_defs {
            if seen.contains(def.id.as_str()) {
                tracing::warn!(
                    specialist.id = %def.id.as_str(),
                    "specialists.toml tentou redefinir ID bundled; bundled vence. \
                     Para customizar um especialista bundled, mude os campos \
                     via outro mecanismo (Etapa futura)."
                );
                continue;
            }
            seen.insert(def.id.as_str().to_string());
            definitions.push(def);
        }

        let valid_ids: Vec<SpecialistId> = definitions.iter().map(|d| d.id.clone()).collect();
        Self {
            definitions,
            valid_ids,
        }
    }
}

fn override_path() -> Option<std::path::PathBuf> {
    use directories::ProjectDirs;
    let dirs = ProjectDirs::from("studio", "frederico", "ia")?;
    Some(dirs.config_dir().join("specialists.toml"))
}

/// Erro interno de `load_override`. Privado — o caller só
/// precisa da semântica ("não encontrado" vs "parse falhou"
/// vs "I/O falhou" pra dar mensagem certa).
#[derive(Debug)]
enum OverrideLoadError {
    NotFound,
    Parse {
        path: std::path::PathBuf,
        cause: String,
    },
    Io {
        path: std::path::PathBuf,
        cause: String,
    },
}

impl SpecialistRegistry for DefaultSpecialistRegistry {
    fn get(&self, id: &SpecialistId) -> Result<&SpecialistDefinition, RegistryError> {
        self.definitions
            .iter()
            .find(|d| &d.id == id)
            .ok_or_else(|| RegistryError::UnknownSpecialist {
                requested: id.as_str().to_string(),
                valid: self.valid_ids.clone(),
            })
    }

    fn list(&self) -> Vec<&SpecialistDefinition> {
        self.definitions.iter().collect()
    }

    fn validate_id(&self, id: &str) -> Result<SpecialistId, RegistryError> {
        let sid = SpecialistId::new(id);
        self.get(&sid).map(|_| sid)
    }
}

impl std::fmt::Debug for DefaultSpecialistRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSpecialistRegistry")
            .field("count", &self.definitions.len())
            .field("ids", &self.valid_ids)
            .finish()
    }
}

// Helper pra casca e testes: constrói o `Arc<dyn SpecialistRegistry>`
// padrão sem precisar importar a struct concreta.
pub fn default_registry() -> Arc<dyn SpecialistRegistry> {
    Arc::new(DefaultSpecialistRegistry::load())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specialist::{SpecialistMaxSteps, SpecialistTimeoutMs};

    fn def(id: &str) -> SpecialistDefinition {
        SpecialistDefinition {
            id: SpecialistId::new(id),
            name: format!("{id} name"),
            description: format!("{id} desc"),
            purpose: format!("{id} purpose"),
            default_model: frederico_core::ModelId::new("gpt-4o"),
            allowed_model_capabilities: vec!["tools".into()],
            allowed_tools: vec![frederico_core::ToolId::new("files.read")],
            denied_tools: vec![],
            max_steps: Some(SpecialistMaxSteps(30)),
            timeout_ms: Some(SpecialistTimeoutMs(300_000)),
            token_budget: None,
            cost_budget_microcents: None,
        }
    }

    #[test]
    fn from_parts_keeps_bundled_only_when_override_empty() {
        let bundled = vec![def("revisor"), def("pesquisador")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![]);
        assert_eq!(r.list().len(), 2);
        assert!(r.get(&SpecialistId::new("revisor")).is_ok());
        assert!(r.get(&SpecialistId::new("pesquisador")).is_ok());
    }

    #[test]
    fn from_parts_merges_override_new_ids() {
        let bundled = vec![def("revisor")];
        let override_defs = vec![def("custom-especialista")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, override_defs);
        assert_eq!(r.list().len(), 2);
        assert!(r.get(&SpecialistId::new("revisor")).is_ok());
        assert!(r.get(&SpecialistId::new("custom-especialista")).is_ok());
    }

    #[test]
    fn from_parts_override_cannot_invade_bundled() {
        // Override tenta redefinir o "revisor" com outro purpose.
        // Bundled vence (warning logado).
        let mut invasao = def("revisor");
        invasao.purpose = "redefinido pelo usuário".into();
        let bundled = vec![def("revisor")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![invasao]);
        assert_eq!(r.list().len(), 1);
        let r = r.get(&SpecialistId::new("revisor")).expect("revisor");
        assert_eq!(r.purpose, "revisor purpose", "bundled venceu");
    }

    #[test]
    fn get_unknown_returns_structured_error_with_valid_list() {
        let bundled = vec![def("revisor"), def("pesquisador")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![]);
        let err = r
            .get(&SpecialistId::new("inexistente"))
            .expect_err("deve falhar");
        match err {
            RegistryError::UnknownSpecialist { requested, valid } => {
                assert_eq!(requested, "inexistente");
                assert_eq!(valid.len(), 2);
                assert!(valid.iter().any(|s| s.as_str() == "revisor"));
                assert!(valid.iter().any(|s| s.as_str() == "pesquisador"));
            }
            other => panic!("variant errado: {other:?}"),
        }
    }

    #[test]
    fn get_unknown_error_message_lists_valid_ids() {
        let bundled = vec![def("revisor"), def("pesquisador")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![]);
        let err = r.get(&SpecialistId::new("fantasma")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'fantasma'"));
        assert!(msg.contains("revisor"));
        assert!(msg.contains("pesquisador"));
    }

    #[test]
    fn validate_id_returns_specialist_id_or_error() {
        let bundled = vec![def("revisor")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![]);
        assert!(r.validate_id("revisor").is_ok());
        let err = r.validate_id("fantasma").unwrap_err();
        assert!(matches!(err, RegistryError::UnknownSpecialist { .. }));
    }

    #[test]
    fn list_summaries_calls_resolver_for_each_def() {
        let bundled = vec![def("revisor"), def("pesquisador")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, vec![]);
        let summaries = list_summaries(&r, |_def| vec!["tools".into(), "code".into()]);
        assert_eq!(summaries.len(), 2);
        for s in &summaries {
            assert_eq!(s.default_model_capabilities, vec!["tools", "code"]);
            assert_eq!(s.capability_tags, vec!["tools", "code"]);
        }
    }

    #[test]
    fn list_preserves_bundled_then_override_order() {
        let bundled = vec![def("b1"), def("b2")];
        let override_defs = vec![def("o1")];
        let r = DefaultSpecialistRegistry::from_parts(bundled, override_defs);
        let ids: Vec<&str> = r.list().iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["b1", "b2", "o1"]);
    }
}
