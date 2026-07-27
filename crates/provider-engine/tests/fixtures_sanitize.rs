//! Gate de CI: **toda** fixture em `fixtures/` passa pela
//! [`frederico_provider_engine::sanitize::check`]. Se alguém
//! commitou um JSONL com `Authorization: Bearer sk-...` por
//! acidente, este teste quebra o build.
//!
//! Roda offline — não fala com provedor nenhum, só lê arquivos do
//! disco. É o cinto-e-suspensório do `provider-recorder`.

use std::fs;
use std::path::Path;

use frederico_provider_engine::sanitize;

/// Pasta onde as fixtures vivem (relativa ao `Cargo.toml` do
/// `provider-engine`, que é o `cwd` em runtime de teste de
/// integração).
const FIXTURES_DIR: &str = "fixtures";

fn walk_jsonl(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // pasta não existe — pula sem falhar.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

#[test]
fn every_fixture_passes_sanitization() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES_DIR);
    assert!(
        fixtures_dir.exists(),
        "pasta de fixtures ausente: {} (esperado em <crate>/fixtures)",
        fixtures_dir.display()
    );

    let mut files = Vec::new();
    walk_jsonl(&fixtures_dir, &mut files);
    assert!(
        !files.is_empty(),
        "nenhuma fixture encontrada em {}; \
         o `provider-recorder` é responsável por popular essa pasta",
        fixtures_dir.display()
    );

    let mut failures: Vec<(std::path::PathBuf, sanitize::SanitizeError)> = Vec::new();
    for file in &files {
        let content = match fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => panic!("não consegui ler {}: {e}", file.display()),
        };
        if let Err(e) = sanitize::check(&content) {
            failures.push((file.clone(), e));
        }
    }

    if !failures.is_empty() {
        let mut msg = String::from("fixtures contaminadas detectadas — ABORT antes do commit:\n");
        for (file, e) in &failures {
            msg.push_str(&format!("  - {}: {e}\n", file.display()));
        }
        msg.push_str(
            "\n\
             Como recuperar:\n  \
             1. Apague a fixture (não tente editar — pode deixar resíduo).\n  \
             2. Re-grave com `cargo run -p frederico-provider-engine --bin recorder`.\n  \
             3. Confirme que a chave não está mais no ambiente antes de regravar.\n",
        );
        panic!("{msg}");
    }
}

#[test]
fn fixtures_have_recognizable_header() {
    // Todo fixture deve ter um header no formato `# key=value` (recorder)
    // ou `{"_fixture_header":{...}}` (fixtures antigas manuais). Sem
    // header, o teste de "que provedor é esse?" não funciona — é
    // informação obrigatória.
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES_DIR);
    let mut files = Vec::new();
    walk_jsonl(&fixtures_dir, &mut files);

    for file in &files {
        let content = fs::read_to_string(file).expect("lê fixture");
        let first_line = content.lines().next().unwrap_or("");
        let has_recorder_header = first_line.starts_with('#');
        let has_legacy_header = first_line.contains("\"_fixture_header\"");
        assert!(
            has_recorder_header || has_legacy_header,
            "{}: primeira linha não tem header reconhecido. \
             Esperado `# ...` (recorder novo) ou `{{\"_fixture_header\":...}}` (legado).\n\
             Primeira linha: {first_line:?}",
            file.display()
        );
    }
}
