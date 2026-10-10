//! Replays captured vendor replies through the semantic provider. No network.

use super::backend::{LlmCompletion, LlmVendor};
use super::{LlmSemanticProvider, StaticBackend};
use crate::config::{LlmConfig, LlmMetrics, LlmMode};
use crate::connectors::types::{ObjectDescriptor, ProtoData};
use crate::content::BytesContentReader;
use crate::ids::ObjectId;
use crate::provider::UnderstandingProvider;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const ANTHROPIC: &str = include_str!("../../../fixtures/llm/recorded/anthropic.json");
const GEMINI: &str = include_str!("../../../fixtures/llm/recorded/gemini.json");
const OLLAMA: &str = include_str!("../../../fixtures/llm/recorded/ollama.json");

#[test]
fn recorded_replies_keep_llm_provenance() {
    for (raw, vendor, entity_name) in [
        (ANTHROPIC, LlmVendor::Anthropic, "ping"),
        (GEMINI, LlmVendor::Gemini, "ping"),
        (OLLAMA, LlmVendor::Ollama, "system"),
    ] {
        let fixture: Value = serde_json::from_str(raw).unwrap();
        let model = fixture["model"].as_str().unwrap();
        assert_eq!(fixture["vendor"].as_str(), Some(vendor.as_str()));
        let completion = LlmCompletion {
            text: fixture["text"].as_str().unwrap().to_string(),
            prompt_tokens: json_i32(&fixture, "prompt_tokens"),
            completion_tokens: json_i32(&fixture, "completion_tokens"),
            total_tokens: json_i32(&fixture, "total_tokens"),
        };
        let metrics = Arc::new(LlmMetrics::new());
        let mut cfg = LlmConfig::new(model, LlmMode::Always);
        cfg.cache_dir = PathBuf::from("/tmp/gnosis-recorded-unused");
        let provider = LlmSemanticProvider::new(cfg, Some(Arc::clone(&metrics)))
            .with_backend(Arc::new(StaticBackend::new(vendor, completion)));
        let mut reader = BytesContentReader::from_slice(b"def ping():\n    return 'pong'\n");
        let result = provider
            .analyze(&desc("ping.py"), &ProtoData::default(), &mut reader)
            .unwrap_or_else(|e| panic!("{vendor:?} parse failed: {e}"));
        let entity = result
            .record
            .entities
            .iter()
            .find(|e| e.name == entity_name)
            .unwrap_or_else(|| panic!("{vendor:?} missing {entity_name}"));
        assert_eq!(
            entity.attributes.get("origin").map(String::as_str),
            Some("llm")
        );
        assert_eq!(
            entity.attributes.get("review").map(String::as_str),
            Some("pending")
        );
        assert_eq!(
            entity.attributes.get("model").map(String::as_str),
            Some(model)
        );
        assert_eq!(metrics.requests.load(Ordering::Relaxed), 1);
    }
}

fn json_i32(value: &Value, key: &str) -> Option<i32> {
    value.get(key).and_then(Value::as_i64).map(|n| n as i32)
}

fn desc(rel: &str) -> ObjectDescriptor {
    let path = PathBuf::from(rel);
    ObjectDescriptor {
        id: ObjectId::new(format!("obj:{rel}")),
        path: path.clone(),
        relative_path: path,
        is_dir: false,
        size: 32,
        modified: None,
        media_type: "text/x-python".into(),
        extension: Some("py".into()),
    }
}
