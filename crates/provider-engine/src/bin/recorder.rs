//! `provider-recorder` — binário de captura de fixtures.
//!
//! Faz uma chamada real a um provedor LLM (formato
//! OpenAI-compatível) e grava os bytes brutos do stream de
//! resposta num arquivo `.jsonl` em `fixtures/`. O arquivo passa
//! por [`frederico_provider_engine::sanitize::check`] antes de ser
//! aceito; se algum token proibido vazar, o arquivo é deletado e o
//! processo aborta com `exit 1`.
//!
//! ## Uso
//!
//! ```text
//! OPENAI_API_KEY=sk-proj-... \
//!   cargo run -p frederico-provider-engine --bin recorder -- \
//!     --provider openai \
//!     --model gpt-4o-mini \
//!     --prompt "Diga olá" \
//!     --out crates/provider-engine/fixtures/openai/ola.jsonl
//! ```
//!
//! ## Política
//!
//! 1. A chave **nunca** é lida do `CredentialStore` (esse é o caminho
//!    do app em produção, com DPAPI). O recorder lê de uma env var
//!    explícita (`--api-key-env VAR`) para que CI noturno possa
//!    configurar o secret sem instalar o app.
//! 2. A chave **nunca** é logada. Nem no header do request, nem em
//!    mensagem de erro, nem em trace.
//! 3. O arquivo gravado é sanitizado antes do flush final. Se
//!    falhar, o arquivo é deletado.
//!
//! ## Suporte
//!
//! O recorder cobre o formato OpenAI-compat streaming (SSE com
//! `data: {...}\n\n`). Anthropic e Gemini têm formato diferente e
//! ficam para v1.1+.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use chrono::Utc;
use frederico_provider_engine::sanitize;
use futures::StreamExt;

/// Tabela de provedores conhecidos (formato OpenAI-compat). Cada
/// entrada mapeia o nome do provedor para o `base_url` da API.
const KNOWN_PROVIDERS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    ("openrouter", "https://openrouter.ai/api/v1"),
    ("deepseek", "https://api.deepseek.com/v1"),
    ("mistral", "https://api.mistral.ai/v1"),
    ("nvidia", "https://integrate.api.nvidia.com/v1"),
    ("ollama", "http://localhost:11434/v1"),
    ("lmstudio", "http://localhost:1234/v1"),
];

/// Args parseadas da linha de comando. O parser é manual (sem
/// `clap`) para evitar uma dep só para este binário.
struct Args {
    provider: String,
    model: String,
    prompt: String,
    out: PathBuf,
    api_key_env: String,
    base_url: Option<String>,
    max_tokens: Option<u32>,
}

fn print_usage() {
    eprintln!(
        "uso: provider-recorder \\\n  \
         --provider <id> \\\n  \
         --model <model_id> \\\n  \
         --prompt <texto> \\\n  \
         --out <arquivo.jsonl> \\\n  \
         [--api-key-env <VAR>] \\\n  \
         [--base-url <url>] \\\n  \
         [--max-tokens <n>]\n\n\
         providers conhecidos (formato OpenAI-compat):\n{}",
        KNOWN_PROVIDERS
            .iter()
            .map(|(k, v)| format!("  - {k:<11} {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut provider = None;
        let mut model = None;
        let mut prompt = None;
        let mut out = None;
        let mut api_key_env = String::from("OPENAI_API_KEY"); // default razoável.
        let mut base_url: Option<String> = None;
        let mut max_tokens: Option<u32> = None;

        let mut iter = env::args().skip(1);
        while let Some(arg) = iter.next() {
            let mut value = || {
                iter.next()
                    .ok_or_else(|| format!("falta valor após `{arg}`"))
            };
            match arg.as_str() {
                "--provider" => provider = Some(value()?),
                "--model" => model = Some(value()?),
                "--prompt" => prompt = Some(value()?),
                "--out" => out = Some(PathBuf::from(value()?)),
                "--api-key-env" => api_key_env = value()?,
                "--base-url" => base_url = Some(value()?),
                "--max-tokens" => {
                    let v = value()?;
                    max_tokens = Some(v.parse().map_err(|_| {
                        format!("`--max-tokens` inválido: {v}")
                    })?);
                }
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("argumento desconhecido: `{other}`")),
            }
        }

        Ok(Args {
            provider: provider.ok_or("falta --provider")?,
            model: model.ok_or("falta --model")?,
            prompt: prompt.ok_or("falta --prompt")?,
            out: out.ok_or("falta --out")?,
            api_key_env,
            base_url,
            max_tokens,
        })
    }

    /// Resolve o `base_url` a partir da tabela ou do override.
    fn resolve_base_url(&self) -> String {
        if let Some(u) = &self.base_url {
            return u.clone();
        }
        KNOWN_PROVIDERS
            .iter()
            .find(|(k, _)| *k == self.provider)
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "provider `{}` não tem base_url padrão; use --base-url",
                    self.provider
                )
            })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("erro: {e}\n");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let api_key = match env::var(&args.api_key_env) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "erro: variável de ambiente `{}` não definida ou vazia.\n\
                 Dica: passe a chave via `--api-key-env OUTRA_VAR` ou configure `{}` no ambiente.",
                args.api_key_env, args.api_key_env
            );
            return ExitCode::from(2);
        }
    };

    let base_url = args.resolve_base_url();

    // Header opcional que o OpenRouter pede para atribuição.
    let extra_headers: Vec<(&str, &str)> = if args.provider == "openrouter" {
        vec![("HTTP-Referer", "https://frederico.local"), ("X-Title", "Frederico IA Studio")]
    } else {
        vec![]
    };

    // Monta o body da request no formato OpenAI-compat.
    let mut body = serde_json::json!({
        "model": args.model,
        "messages": [{"role": "user", "content": args.prompt}],
        "stream": true,
    });
    if let Some(m) = args.max_tokens {
        body["max_tokens"] = serde_json::json!(m);
    }

    eprintln!(
        "[recorder] provider={} model={} base_url={}",
        args.provider, args.model, base_url
    );
    eprintln!("[recorder] prompt: {} chars", args.prompt.len());
    eprintln!("[recorder] gravando em: {}", args.out.display());

    // Envia a request. A chave **nunca** entra em log nem em
    // mensagem de erro — usamos closures que só logam o status.
    let client = reqwest::Client::builder()
        .build()
        .expect("cliente reqwest");
    let mut req = client
        .post(format!("{base_url}/chat/completions"))
        .bearer_auth(&api_key)
        .json(&body);
    for (k, v) in &extra_headers {
        req = req.header(*k, *v);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[recorder] falha ao enviar request: {e}");
            return ExitCode::from(1);
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // A body de erro pode conter a chave ecoada; sanitizamos
        // antes de mostrar.
        eprintln!("[recorder] provedor respondeu {status}");
        match sanitize::check(&body) {
            Ok(()) => eprintln!("[recorder] body: {body}"),
            Err(san) => eprintln!(
                "[recorder] body contém token proibido (`{}` na linha {}); suprimido.",
                san.token, san.line
            ),
        }
        return ExitCode::from(1);
    }

    // Acumula os bytes brutos do stream.
    let mut out = String::new();
    out.push_str(&format!("# recorded_at={}\n", Utc::now().to_rfc3339()));
    out.push_str(&format!("# provider={}\n", args.provider));
    out.push_str(&format!("# model={}\n", args.model));
    out.push_str(&format!("# base_url={base_url}\n"));
    out.push_str("# format=openai-compat-sse\n");
    out.push_str("#\n");

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[recorder] erro no stream: {e}");
                // Sanitiza o que já temos antes de gravar (ou
                // deletar).
                finalize(out, &args.out).await;
                return ExitCode::from(1);
            }
        };
        out.push_str(&String::from_utf8_lossy(&chunk));
    }

    finalize(out, &args.out).await
}

/// Sanitiza e grava (ou deleta se falhar). Esta função é o portão
/// final: nada de credencial vaza pro disco.
async fn finalize(mut content: String, out: &PathBuf) -> ExitCode {
    if let Err(san) = sanitize::check(&content) {
        eprintln!(
            "[recorder] ABORT: fixture rejeitada por sanitização — token `{}` na linha {}.",
            san.token, san.line
        );
        // Garante que nada ficou em disco.
        let _ = tokio::fs::remove_file(out).await;
        return ExitCode::from(1);
    }

    // Append newline final se não tiver (alguns provedores não fecham
    // o stream com \n).
    if !content.ends_with('\n') {
        content.push('\n');
    }

    // Garante que o diretório existe.
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                eprintln!("[recorder] falha ao criar diretório: {e}");
                return ExitCode::from(1);
            }
        }
    }

    match tokio::fs::write(out, content.as_bytes()).await {
        Ok(()) => {
            eprintln!("[recorder] OK: gravado {} bytes em {}", content.len(), out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[recorder] falha ao gravar: {e}");
            ExitCode::from(1)
        }
    }
}
