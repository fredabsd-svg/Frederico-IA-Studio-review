//! E2E — `SpecialistRegistry` (Fase 6, Etapa 3, ADR-0030).
//!
//! Caminho exercitado: **`build_specialist_registry(catalog)` (a
//! mesma factory que a casca Tauri chama no `setup`)** →
//! `SpecialistBundle::list_summaries` / `::get` → `Catalog`
//! pareado (resolução de `default_model` para `capability_tags`).
//!
//! Estes 2 testes são a **prova de caminho real** do registry
//! em Etapa 3. A Etapa 3 não invoca subagentes (a Etapa 4 fecha
//! isso), então o que esta bateria prova é:
//!
//! 1. **`registry_loads_specialists_from_catalog`**: o
//!    `build_specialist_registry(catalog)` carrega os 8 bundled
//!    do `data/specialists/default.toml` (embedded pelo
//!    `build.rs`), pareia com o `Catalog` e devolve os
//!    summaries com `capability_tags` resolvidas (i.e., a
//!    UI pode renderizar badges sem precisar consultar o
//!    catálogo de novo).
//! 2. **`specialist_unknown_id_returns_structured_error`**: o
//!    `get(unknown)` retorna `Err(UnknownSpecialist { requested,
//!    valid })` com a lista dos 8 bundled — o frontend da
//!    Etapa 6 consome o `valid` pra renderizar modal de
//!    "subagente não encontrado, disponíveis: [...]".
//!
//! Os outros 2 testes da Etapa 3 (`permission_set_inherited_from_assistant_project_user`
//! e `effective_permission_set_is_subset_of_parent`) são
//! responsabilidade do **PR 2** (PR cortando na fronteira de
//! segurança, conforme decisão registrada na conversa de
//! 2026-08-06) — eles provam o invariante "subagente ⊆ pai"
//! no caminho de produção e precisam do `permission_loader`.
//! O gate `check-e2e-gate.ps1` da Fase de Ligação Etapa 6 vai
//! pegar se o status.md prometer eles nesta linha de cobertura
//! sem o PR 2 ter mergeado.
//!
//! Ver [`docs/architecture/multimodel-architecture.md` §"E2E de
//! cobertura planejado por etapa"](../../docs/architecture/multimodel-architecture.md#e2e-de-cobertura-planejado-por-etapa)
//! (alvo declarado na Etapa 1, atualizado por etapa conforme
//! cada PR mergea) e
//! [`docs/architecture/testing-strategy.md` §3](../../docs/architecture/testing-strategy.md)
//! (regra da composição compartilhada).

use frederico_app::composition::{build_specialist_registry, SpecialistBundle};
use frederico_model_catalog::Catalog;
use frederico_model_catalog::{RegistryError, SpecialistId};

mod common;

// Catálogo embutido (mesmo `Catalog::load()` que a casca usa).
fn bundled_catalog() -> std::sync::Arc<Catalog> {
    std::sync::Arc::new(Catalog::load().clone())
}

// 8 IDs bundled no `data/specialists/default.toml` (ADR-0030 §D1,
// lista literal do `subagent-architecture.md` §"Bundled default").
const BUNDLED_IDS: &[&str] = &[
    "revisor",
    "pesquisador",
    "testador",
    "validador",
    "sumador",
    "arquiteto",
    "critico",
    "executor",
];

/// 1. **`registry_loads_specialists_from_catalog`**
///
/// `build_specialist_registry(catalog)` carrega bundled + override e
/// pareia com o catálogo. O `list_summaries` resolve as capabilities
/// do `default_model` de cada especialista.
///
/// **Prova do caminho de produção:** o factory é o mesmo
/// `frederico_app::composition::build_specialist_registry` que a
/// casca Tauri chama no `setup` (mesma `Arc<Catalog>`). Não
/// chamamos o construtor de `DefaultSpecialistRegistry` direto
/// — passamos pelo factory pra eliminar o risco de drift entre
/// "o que o teste exercita" e "o que a casca usa em produção"
/// (regra do ADR-0022 §D4 + memory "Cobertura de invariante
/// no caminho de produção, não no crate").
#[test]
fn registry_loads_specialists_from_catalog() {
    let catalog = bundled_catalog();
    let bundle = build_specialist_registry(catalog);

    // Lista completa de summaries (caminho de produção via
    // `list_summaries`).
    let summaries = bundle.list_summaries();

    // 8 bundled, na ordem do `default.toml`. Override vazio
    // (test não cria `~/.config/frederico/specialists.toml`).
    assert_eq!(
        summaries.len(),
        8,
        "esperado 8 especialistas bundled; encontrados: {:?}",
        summaries.iter().map(|s| s.id.as_str()).collect::<Vec<_>>()
    );

    let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();
    for expected in BUNDLED_IDS {
        assert!(
            ids.contains(expected),
            "ID bundled '{expected}' ausente do registry; IDs presentes: {ids:?}"
        );
    }

    // Capabilities resolvidas (não vazias pros 8 — todos os
    // `default_model` existem no catálogo). Pelo menos `gpt-4o`
    // e `gpt-4o-mini` (que o `default.toml` referencia) estão
    // no `data/catalog.json` desde a Etapa 2 da Fase 0.
    for s in &summaries {
        assert!(
            !s.default_model_capabilities.is_empty(),
            "summary '{}' tem default_model_capabilities vazia (default_model='{}') — \
             provavelmente o default_model não está no catálogo",
            s.id.as_str(),
            s.default_model,
        );
    }

    // `capability_tags` espelha `default_model_capabilities` (mesmo
    // conteúdo, shape que a UI prefere).
    for s in &summaries {
        assert_eq!(
            s.capability_tags,
            s.default_model_capabilities,
            "tags devem espelhar capabilities (id={})",
            s.id.as_str()
        );
    }

    // `get("revisor")` resolve pro bundled.
    let revisor = bundle
        .get(&SpecialistId::new("revisor"))
        .expect("revisor bundled");
    assert_eq!(revisor.id.as_str(), "revisor");
    assert_eq!(revisor.default_model.as_str(), "gpt-4o");
    // Default não-vazio do SpecialistMaxSteps.
    assert_eq!(
        revisor.max_steps.expect("max_steps set no default.toml").0,
        30
    );
}

/// 2. **`specialist_unknown_id_returns_structured_error`**
///
/// `get(unknown)` retorna `Err(UnknownSpecialist)` com a lista
/// dos 8 bundled em `valid`. O frontend da Etapa 6 renderiza
/// essa lista direto no modal de erro (sem fallback
/// silencioso — §9.2 do PROMPT MESTRE).
///
/// **Por que `valid.len() == 8`:** o registry carrega só os
/// bundled neste teste (sem override de usuário). Se o usuário
/// tiver `~/.config/frederico/specialists.toml` no CI/dev
/// machine, o teste vê mais — por isso a asserção é `len >= 8`
/// (não `== 8`). O `contains` dos 8 bundled é o que garante
/// que pelo menos o que veio do app está lá.
#[test]
fn specialist_unknown_id_returns_structured_error() {
    let catalog = bundled_catalog();
    let bundle: SpecialistBundle = build_specialist_registry(catalog);

    let err = bundle
        .get(&SpecialistId::new("fantasma"))
        .expect_err("ID inexistente deve falhar");

    // A mensagem PT-BR do `Display` lista os IDs válidos (a
    // UI da Etapa 6 renderiza ela direto). Captura **antes**
    // do `match` porque o `match` por valor deixa `err` em
    // estado "partially moved" e o borrow checker não deixa
    // chamar `to_string()` depois.
    let msg = err.to_string();

    match err {
        RegistryError::UnknownSpecialist { requested, valid } => {
            assert_eq!(requested, "fantasma");
            // Pelo menos os 8 bundled estão em `valid` (pode
            // haver mais se o usuário tem override).
            assert!(
                valid.len() >= 8,
                "expected pelo menos 8 IDs válidos, got {}: {:?}",
                valid.len(),
                valid.iter().map(|s| s.as_str()).collect::<Vec<_>>()
            );
            let valid_strs: Vec<&str> = valid.iter().map(|s| s.as_str()).collect();
            for expected in BUNDLED_IDS {
                assert!(
                    valid_strs.contains(expected),
                    "ID bundled '{expected}' ausente de `valid`: {valid_strs:?}"
                );
            }
        }
        other => panic!("variant errado: {other:?}"),
    }

    assert!(msg.contains("'fantasma'"));
    assert!(msg.contains("revisor"), "msg deve listar válidos: {msg}");
    assert!(
        msg.contains("pesquisador"),
        "msg deve listar válidos: {msg}"
    );

    // `validate_id` é o outro portão (chamado pelo
    // `SubagentRunner` da Etapa 4 antes de delegar) —
    // garante que retorna o mesmo erro estruturado quando o
    // caller passa uma string crua (como o modelo faz).
    let err2 = bundle
        .validate_id("fantasma")
        .expect_err("validate_id também deve falhar");
    assert!(matches!(err2, RegistryError::UnknownSpecialist { .. }));
}
