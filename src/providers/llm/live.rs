//! One-shot connectivity probe. Ignored in CI. Run with:
//! `cargo test --features llm --lib providers::llm::live::record_live_replies -- --ignored --nocapture`

use super::backend::{LlmBackend, LlmRequest};
use super::{AnthropicAdapter, GeminiAdapter, OllamaAdapter};
use crate::scrub::scrub_secrets;
use std::path::PathBuf;

struct Probe {
    vendor: &'static str,
    model: &'static str,
    backend: Box<dyn LlmBackend>,
}

#[test]
#[ignore = "calls OpenAI, Anthropic, Gemini, and local Ollama; writes fixtures/llm/recorded"]
fn record_live_replies() {
    let request = LlmRequest {
        system: "Reply with one JSON object and no markdown. Use status \"partial\". \
                 Include exactly one entity."
            .into(),
        user_text: "File ping.py is:\n\ndef ping():\n    return \"pong\"\n\n\
                    Extract a function entity named ping with confidence low. \
                    summary should mention ping. relationships and diagnostics are empty arrays."
            .into(),
        image: None,
        max_tokens: Some(256),
    };

    // OpenAI (`gpt-4o-mini`) was probed separately: the key authenticates, and the
    // API returns 429 `credit_balance_exhausted`. No completion fixture for it.
    let probes = [
        Probe {
            vendor: "anthropic",
            model: "claude-haiku-4-5-20251001",
            backend: Box::new(AnthropicAdapter::new()),
        },
        Probe {
            vendor: "gemini",
            model: "gemini-3.5-flash-lite",
            backend: Box::new(GeminiAdapter::new()),
        },
        Probe {
            vendor: "ollama",
            model: "tinyllama:latest",
            backend: Box::new(OllamaAdapter::new()),
        },
    ];

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/llm/recorded");
    std::fs::create_dir_all(&dir).unwrap();

    let mut failures = Vec::new();
    for probe in &probes {
        let path = dir.join(format!("{}.json", probe.vendor));
        if path.exists() {
            eprintln!("skip existing {}", path.display());
            continue;
        }
        match probe.backend.complete(probe.model, &request) {
            Ok(completion) => {
                let body = serde_json::json!({
                    "vendor": probe.vendor,
                    "model": probe.model,
                    "text": scrub_secrets(&completion.text),
                    "prompt_tokens": completion.prompt_tokens,
                    "completion_tokens": completion.completion_tokens,
                    "total_tokens": completion.total_tokens,
                });
                std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap() + "\n").unwrap();
                eprintln!(
                    "{} ok model={} tokens={:?} bytes={} -> {}",
                    probe.vendor,
                    probe.model,
                    completion.total_tokens,
                    completion.text.len(),
                    path.display()
                );
            }
            Err(err) => {
                let message = scrub_secrets(&err.to_string());
                eprintln!("{} FAIL {message}", probe.vendor);
                failures.push(format!("{}: {message}", probe.vendor));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "live probes failed:\n{}",
        failures.join("\n")
    );
}
