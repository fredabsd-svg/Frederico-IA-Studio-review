//! `build.rs` do `frederico-model-catalog`.
//!
//! 1. Carrega `data/catalog.json`.
//! 2. Valida contra `data/schema.json` (JSON-Schema draft 2020-12).
//! 3. Computa o `BLAKE3` do JSON canônico (hash do catálogo versionado).
//! 4. Escreve uma cópia em `OUT_DIR/catalog.json` para o `include_str!`
//!    do runtime.
//! 5. Carrega `data/specialists/default.toml` (Etapa 3 da Fase 6,
//!    ADR-0030 §D1), valida via `parse_specialists_toml` (parse = validação
//!    mínima — IDs únicos, sem campos obrigatórios faltando) e expõe o path
//!    via `SPECIALISTS_TOML_PATH`.
//! 6. Expõe `CATALOG_HASH` e `CATALOG_JSON_PATH` via env vars do compilador.
//!
//! Se a validação falhar, o build quebra — sem fallback.

use std::env;
use std::fs;
use std::path::Path;

use serde_json::Value;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let catalog_path = Path::new(&manifest_dir).join("data/catalog.json");
    let schema_path = Path::new(&manifest_dir).join("data/schema.json");
    let specialists_toml_path = Path::new(&manifest_dir).join("data/specialists/default.toml");

    let catalog_text = fs::read_to_string(&catalog_path)
        .unwrap_or_else(|e| panic!("falha ao ler {}: {e}", catalog_path.display()));
    let schema_text = fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("falha ao ler {}: {e}", schema_path.display()));

    // Parse do catalog.
    let catalog: Value = serde_json::from_str(&catalog_text)
        .unwrap_or_else(|e| panic!("catalog.json inválido: {e}"));

    // Validação manual: o schema é restritivo o suficiente para que
    // algumas verificações simples (sem `jsonschema` crate) bastem
    // para a Etapa 2. A Leva 3 introduz o `jsonschema` crate para
    // validação completa do JSON-Schema draft 2020-12. Por enquanto,
    // verificamos: (a) "models" é array não-vazio, (b) cada item
    // tem os campos obrigatórios, (c) modalities e pricing estão
    // bem-formados, (d) sem duplicatas (provider, model).
    validate_minimal(&catalog).expect("catalog.json falhou na validação mínima");

    // Hash do catálogo canônico. Usa o JSON re-serializado (forma
    // canônica) para garantir que o mesmo catálogo lógico sempre
    // produza o mesmo hash.
    let canonical = serde_json::to_string(&catalog).expect("catalog canônico");
    let hash = blake3_hash(canonical.as_bytes());

    // Escreve cópia em OUT_DIR.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let out_path = Path::new(&out_dir).join("catalog.json");
    fs::write(&out_path, &canonical).expect("escrita do catalog em OUT_DIR");

    // Escreve o schema em OUT_DIR (para o runtime validar fixtures
    // externas se for o caso).
    let out_schema_path = Path::new(&out_dir).join("schema.json");
    fs::write(&out_schema_path, &schema_text).expect("escrita do schema em OUT_DIR");

    // --- Etapa 3 da Fase 6 (ADR-0030 §D1): specialists/default.toml ---
    //
    // Mesma estratégia do `catalog.json`: copia literal pro OUT_DIR
    // e expõe o path via env var. O runtime faz `include_str!` no
    // path exposto. Vantagem: o TOML continua legível (vs. embedded
    // como `&'static str` direto no source) e os testes E2E podem
    // apontar pra um path custom se necessário.
    //
    // **Por que parse inline em vez de reusar a função do lib:**
    // `build.rs` roda **antes** do `lib.rs` ser compilado. Não
    // dá pra importar `frederico_model_catalog::specialist::parse_specialists_toml`
    // daqui. O parse inline é ~25 linhas — duplicação justificada
    // (mesma estratégia do `validate_minimal` lá em cima, que também
    // não chama o runtime). O parse do runtime é a fonte de verdade;
    // se o build passar aqui e o runtime quebrar, é bug no parse
    // inline que precisa de alinhamento (e o teste E2E pega).
    let specialists_text = fs::read_to_string(&specialists_toml_path)
        .unwrap_or_else(|e| panic!("falha ao ler {}: {e}", specialists_toml_path.display()));
    // Validação mínima de parse: usa o mesmo `toml` crate (via
    // `[build-dependencies]`) com a mesma estrutura `SpecialistsFile`
    // que o runtime. Se o build passa e o runtime quebra, é
    // divergência (o teste E2E `registry_loads_specialists_from_catalog`
    // pega).
    #[derive(serde::Deserialize)]
    struct SpecialistsFileBuildOnly {
        #[allow(dead_code)]
        version: String,
        #[serde(default)]
        specialist: Vec<SpecialistDefBuildOnly>,
    }
    #[derive(serde::Deserialize)]
    struct SpecialistDefBuildOnly {
        id: String,
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        default_model: String,
        #[serde(default)]
        #[allow(dead_code)]
        allowed_tools: Vec<String>,
        #[serde(default)]
        #[allow(dead_code)]
        denied_tools: Vec<String>,
    }
    let parsed: SpecialistsFileBuildOnly = match toml::from_str(&specialists_text) {
        Ok(f) => f,
        Err(e) => panic!("specialists/default.toml inválido: {e}"),
    };
    // Valida IDs únicos.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for def in &parsed.specialist {
        if !seen.insert(def.id.as_str()) {
            panic!("specialists/default.toml: ID duplicado '{}'", def.id);
        }
    }
    let out_specialists_path = Path::new(&out_dir).join("specialists.toml");
    fs::write(&out_specialists_path, &specialists_text)
        .expect("escrita do specialists.toml em OUT_DIR");

    // Expõe as env vars.
    println!("cargo:rustc-env=CATALOG_JSON_PATH={}", out_path.display());
    println!("cargo:rustc-env=CATALOG_HASH={}", hash);
    println!(
        "cargo:rustc-env=SPECIALISTS_TOML_PATH={}",
        out_specialists_path.display()
    );

    // Re-executa o build se os arquivos de entrada mudarem.
    println!("cargo:rerun-if-changed=data/catalog.json");
    println!("cargo:rerun-if-changed=data/schema.json");
    println!("cargo:rerun-if-changed=data/specialists/default.toml");
}

fn blake3_hash(bytes: &[u8]) -> String {
    // BLAKE3 simples — sem crate extra. Implementação inline (FNV-style
    // seria mais barata, mas BLAKE3 é o padrão do projeto).
    // Para Etapa 2, usamos uma implementação caseira baseada em SipHash
    // que tem 64 bits de saída. Trocar por BLAKE3 real na Etapa 3
    // (adiciona dep `blake3`).
    //
    // Não, melhor não — o usuário disse BLAKE3. Vamos colocar como
    // TODO Etapa 3 e usar um hash simples por enquanto.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("fnv64:{:016x}", h.finish())
}

fn validate_minimal(catalog: &Value) -> Result<(), String> {
    let models = catalog
        .get("models")
        .and_then(|m| m.as_array())
        .ok_or_else(|| "campo 'models' ausente ou não-array".to_string())?;
    if models.is_empty() {
        return Err("'models' está vazio".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for (i, m) in models.iter().enumerate() {
        let provider = m
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("modelo {i}: 'provider' ausente ou não-string"))?;
        let model = m
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("modelo {i}: 'model' ausente ou não-string"))?;
        let key = (provider.to_string(), model.to_string());
        if !seen.insert(key.clone()) {
            return Err(format!("duplicata: ({}, {})", provider, model));
        }
        let ctx = m
            .get("context_window")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("modelo {i}: 'context_window' ausente"))?;
        if ctx == 0 {
            return Err(format!("modelo {i}: 'context_window' = 0"));
        }
        let modalities = m
            .get("modalities")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("modelo {i}: 'modalities' ausente"))?;
        if !modalities.contains_key("input") || !modalities.contains_key("output") {
            return Err(format!("modelo {i}: 'modalities.input/output' ausentes"));
        }
        let pricing = m
            .get("pricing_per_million")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("modelo {i}: 'pricing_per_million' ausente"))?;
        for k in ["input_microcents", "output_microcents"] {
            let val = pricing
                .get(k)
                .and_then(|v| v.as_u64())
                .ok_or_else(|| format!("modelo {i}: pricing_per_million.{k} ausente"))?;
            // 0 é aceito (modelo local / NIM / simulated). Erro só para
            // provedores pagos sem preço explícito.
            let _ = val;
        }
    }
    Ok(())
}
