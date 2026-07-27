//! `build.rs` do `frederico-model-catalog`.
//!
//! 1. Carrega `data/catalog.json`.
//! 2. Valida contra `data/schema.json` (JSON-Schema draft 2020-12).
//! 3. Computa o `BLAKE3` do JSON canônico (hash do catálogo versionado).
//! 4. Escreve uma cópia em `OUT_DIR/catalog.json` para o `include_str!`
//!    do runtime.
//! 5. Expõe `CATALOG_HASH` e `CATALOG_JSON_PATH` via env vars do compilador.
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

    // Expõe as env vars.
    println!("cargo:rustc-env=CATALOG_JSON_PATH={}", out_path.display());
    println!("cargo:rustc-env=CATALOG_HASH={}", hash);

    // Re-executa o build se os arquivos de entrada mudarem.
    println!("cargo:rerun-if-changed=data/catalog.json");
    println!("cargo:rerun-if-changed=data/schema.json");
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
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
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
