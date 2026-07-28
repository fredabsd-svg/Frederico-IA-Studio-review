//! `ToolRegistry` — fonte única da verdade sobre ferramentas.
//!
//! Regra do spec `tool-registry-specification.md` §"Tool Registry":
//! "A lista de ferramentas exibida na UI, os manifestos enviados ao
//! modelo, as permissões, as execuções e os testes derivam **deste**
//! registro. Listas manuais duplicadas em outros arquivos são
//! defeito (ver `REGRAS §1.9`)."
//!
//! A Etapa 2 entrega o registro e a função `effective_tools` (a
//! interseção do spec §"Interseção de inventário por execução"). A
//! Etapa 4 vai consumir `effective_tools` para serializar o
//! `tools:` enviado ao provedor; a Etapa 6 vai consumir `all` para
//! montar a UI de configuração.

use std::collections::HashMap;

use frederico_core::ToolId;

use crate::manifest::{ToolManifest, WorkerHealth};

/// Registro de ferramentas. Thread-safe via interior mutability
/// (a Etapa 4 vai usar `Arc<RwLock<ToolRegistry>>` no `AppState`).
#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    by_id: HashMap<ToolId, ToolManifest>,
    /// Saúde observada por ferramenta. A Etapa 5 popula via
    /// healthcheck; a Etapa 2 só usa o default (`Ok`).
    health: HashMap<ToolId, WorkerHealth>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra um manifesto. Substitui se já existir (a Etapa 4 usa
    /// pra hot-reload de manifesto via migração numerada).
    pub fn register(&mut self, manifest: ToolManifest) {
        let id = manifest.id.clone();
        self.by_id.insert(id.clone(), manifest);
        // Default razoável: se a ferramenta não tem health, é `Ok`
        // (assumimos "in-process" sem worker).
        self.health.entry(id).or_insert(WorkerHealth::Ok);
    }

    pub fn get(&self, id: &ToolId) -> Option<&ToolManifest> {
        self.by_id.get(id)
    }

    pub fn all(&self) -> Vec<&ToolManifest> {
        let mut v: Vec<&ToolManifest> = self.by_id.values().collect();
        // Ordem estável: por `id`. Importante pra `effective_tools`
        // ser determinístico (modelo recebe a mesma lista no mesmo
        // ordem em todo PR).
        v.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        v
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Define a saúde observada de uma ferramenta. A Etapa 5 chama
    /// isso a partir do healthcheck. A Etapa 2 não usa.
    pub fn set_health(&mut self, id: ToolId, health: WorkerHealth) {
        self.health.insert(id, health);
    }

    pub fn health(&self, id: &ToolId) -> WorkerHealth {
        self.health
            .get(id)
            .copied()
            .unwrap_or(WorkerHealth::Unhealthy)
    }

    /// Calcula a interseção final do inventário para uma execução.
    /// Spec `tool-registry-specification.md` §"Interseção de inventário
    /// por execução".
    ///
    /// Versão Etapa 2: implementa os filtros que **não dependem** da
    /// Etapa 3 (permissões) nem da Etapa 4 (interseção com
    /// `Run.allowed_tools`). Esses dois filtros viram `unimplemented!`
    /// explícito na assinatura — quando a Etapa 3 fechar, eles
    /// aparecem aqui.
    #[must_use]
    pub fn effective_tools(&self, allowed_for_run: &[ToolId]) -> Vec<ToolManifest> {
        self.all()
            .into_iter()
            // 1. Disponível e saudável
            .filter(|t| t.is_callable_now(self.health(&t.id)))
            // 2. Está na lista permitida para este Run (específico
            //    da Etapa 4; a Etapa 2 passa a lista direto)
            .filter(|t| allowed_for_run.contains(&t.id))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{JsonSchema, RiskLevel, ToolCategory};

    fn make_manifest(id: &str) -> ToolManifest {
        ToolManifest::builder(ToolId::new(id), "files")
            .display_name(id)
            .description(format!("tool {id}"))
            .risk_level(RiskLevel::Safe)
            .category(ToolCategory::Files)
            .input_schema(JsonSchema::any())
            .output_schema(JsonSchema::any())
            .build()
            .unwrap()
    }

    #[test]
    fn register_and_get() {
        let mut r = ToolRegistry::new();
        r.register(make_manifest("files.read"));
        assert_eq!(r.len(), 1);
        let m = r.get(&ToolId::new("files.read")).unwrap();
        assert_eq!(m.display_name, "files.read");
    }

    #[test]
    fn all_returns_sorted_by_id() {
        let mut r = ToolRegistry::new();
        r.register(make_manifest("files.write"));
        r.register(make_manifest("files.read"));
        r.register(make_manifest("files.list"));
        let ids: Vec<&str> = r.all().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["files.list", "files.read", "files.write"]);
    }

    #[test]
    fn effective_tools_filters_by_run_allowlist() {
        let mut r = ToolRegistry::new();
        r.register(make_manifest("files.read"));
        r.register(make_manifest("files.write"));
        // Só files.read é permitido para este Run
        let eff = r.effective_tools(&[ToolId::new("files.read")]);
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].id, ToolId::new("files.read"));
    }

    #[test]
    fn effective_tools_excludes_disabled() {
        let mut r = ToolRegistry::new();
        r.register(make_manifest("files.read"));
        r.register(
            ToolManifest::builder(ToolId::new("files.write"), "files")
                .display_name("files.write")
                .description("write")
                .disabled()
                .build()
                .unwrap(),
        );
        let eff = r.effective_tools(&[ToolId::new("files.read"), ToolId::new("files.write")]);
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0].id, ToolId::new("files.read"));
    }

    #[test]
    fn effective_tools_excludes_unhealthy() {
        let mut r = ToolRegistry::new();
        r.register(make_manifest("files.read"));
        r.set_health(ToolId::new("files.read"), WorkerHealth::Unhealthy);
        let eff = r.effective_tools(&[ToolId::new("files.read")]);
        assert!(eff.is_empty());
    }
}
