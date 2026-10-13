//! Optional LLM semantic understanding provider (`--features llm`).
//!
//! Chat goes through [`LlmBackend`]. [`backend_for`] selects an OpenAI, Anthropic,
//! Gemini, or Ollama adapter from the model id.

mod adapters;
mod backend;
mod cache;
#[cfg(test)]
mod live;
mod parse;
#[cfg(test)]
mod recorded;

use crate::config::{LlmConfig, LlmMetrics, LlmMode};
use crate::connectors::types::{ObjectDescriptor, ProtoData};
use crate::content::ContentReader;
use crate::error::{GnosisError, Result};
use crate::ids::ProviderId;
use crate::knowledge::{AnalysisResult, Diagnostic, DiagnosticSeverity, KnowledgeRecord};
use crate::prompts::{select_prompt_kind, PromptKind, PromptPack, PROMPT_PACK_VERSION};
use crate::provider::{Support, UnderstandingProvider};
use crate::providers::util::ext_is;
use crate::scrub::secret_kind;
use crate::status::UnderstandingStatus;
use adapters::backend_for;
pub use adapters::{AnthropicAdapter, GeminiAdapter, OllamaAdapter, OpenAiAdapter};
use backend::{LlmBackend, LlmImage, LlmRequest};
pub use backend::{LlmCompletion, LlmVendor, StaticBackend};
use base64::Engine;
pub use parse::{parse_llm_json, strip_json_fence, LlmStamp};
use std::sync::Arc;

const DEFAULT_TEXT_LIMIT: usize = 120_000;

/// Multi-provider LLM semantic extractor.
pub struct LlmSemanticProvider {
    config: LlmConfig,
    backend: Arc<dyn LlmBackend>,
    text_limit: usize,
    metrics: Option<Arc<LlmMetrics>>,
}

impl LlmSemanticProvider {
    pub fn new(config: LlmConfig, metrics: Option<Arc<LlmMetrics>>) -> Self {
        let backend = backend_for(&config.model);
        Self::with_parts(config, backend, metrics)
    }

    /// Replace the vendor adapter. Tests pass [`StaticBackend`] so analysis never hits the network.
    pub fn with_backend(mut self, backend: Arc<dyn LlmBackend>) -> Self {
        self.backend = backend;
        self
    }

    fn with_parts(
        config: LlmConfig,
        backend: Arc<dyn LlmBackend>,
        metrics: Option<Arc<LlmMetrics>>,
    ) -> Self {
        if let Some(metrics) = &metrics {
            metrics
                .max_requests
                .store(config.max_requests, std::sync::atomic::Ordering::Relaxed);
            metrics
                .max_tokens
                .store(config.max_tokens, std::sync::atomic::Ordering::Relaxed);
        }
        Self {
            config,
            backend,
            text_limit: DEFAULT_TEXT_LIMIT,
            metrics,
        }
    }

    #[cfg(test)]
    pub fn with_text_limit(mut self, limit: usize) -> Self {
        self.text_limit = limit;
        self
    }

    fn specialized_full_ext(object: &ObjectDescriptor) -> bool {
        ext_is(
            &object.extension,
            &[
                "cpp", "cc", "cxx", "c", "h", "hpp", "hh", "rs", "ex", "exs", "md", "markdown",
                "mdx", "txt", "text", "log", "json", "yaml", "yml", "toml", "csv", "jpg", "jpeg",
                "tif", "tiff",
            ],
        )
    }
}

impl UnderstandingProvider for LlmSemanticProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("llm-semantic")
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        match self.config.mode {
            LlmMode::Always => Support::Partial,
            LlmMode::Fallback => {
                if Self::specialized_full_ext(object) {
                    Support::None
                } else if object.media_type.starts_with("text/")
                    && !ext_is(
                        &object.extension,
                        &["diff", "patch"], // review packs still useful
                    )
                {
                    // PlainText Partial covers most leftover text/*; stay out in fallback.
                    Support::None
                } else {
                    Support::Partial
                }
            }
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let mut bytes = Vec::new();
        content.read_to_end(&mut bytes).map_err(GnosisError::from)?;

        let kind = select_prompt_kind(object);
        let pack = PromptPack::get(kind);
        if !self.config.allow_secrets && !kind.wants_binary() {
            if let Some(kind_name) = secret_kind(&String::from_utf8_lossy(&bytes)) {
                return Ok(skipped(
                    UnderstandingStatus::PartiallyUnderstood,
                    format!("refused: content looks like a secret ({kind_name})"),
                ));
            }
        }

        let stamp = LlmStamp {
            model: self.config.model.clone(),
            prompt_kind: kind.as_str().to_string(),
        };
        if let Some(fingerprint) = proto.fingerprint.as_deref() {
            let key = cache::cache_key(
                fingerprint,
                &self.config.model,
                kind.as_str(),
                PROMPT_PACK_VERSION,
            );
            if let Some(raw) = cache::load(&self.config.cache_dir, &key) {
                if let Some(metrics) = &self.metrics {
                    metrics.record_cache_hit();
                }
                return finish_raw(&raw, object, &self.id(), &stamp, kind);
            }
        }

        if self.metrics.as_ref().is_some_and(|m| m.over_budget()) {
            return Ok(skipped(
                UnderstandingStatus::PartiallyUnderstood,
                "llm budget exhausted".into(),
            ));
        }

        let meta = format!(
            "path: {}\nfilename: {}\nmedia_type: {}\nsize: {}\nprompt_kind: {}\nmodel: {}\n",
            object.relative_path.display(),
            proto.filename,
            object.media_type,
            object.size,
            kind.as_str(),
            self.config.model,
        );

        let request = build_request(&pack, &meta, kind, object, &bytes, self.text_limit)?;
        let model = self.config.model.clone();
        let completion = match self.backend.complete(&model, &request) {
            Ok(completion) => completion,
            Err(err) => {
                if let Some(metrics) = &self.metrics {
                    metrics.record_error();
                }
                return Err(err);
            }
        };
        if let Some(metrics) = &self.metrics {
            metrics.record_success(
                completion.prompt_tokens,
                completion.completion_tokens,
                completion.total_tokens,
            );
        }
        let raw = completion.text;

        if let Some(fingerprint) = proto.fingerprint.as_deref() {
            let key = cache::cache_key(
                fingerprint,
                &self.config.model,
                kind.as_str(),
                PROMPT_PACK_VERSION,
            );
            let _ = cache::store(&self.config.cache_dir, &key, &raw);
        }
        finish_raw(&raw, object, &self.id(), &stamp, kind)
    }
}

fn finish_raw(
    raw: &str,
    object: &ObjectDescriptor,
    provider: &ProviderId,
    stamp: &LlmStamp,
    kind: PromptKind,
) -> Result<AnalysisResult> {
    let mut result = parse_llm_json(raw, object, provider, stamp)?;
    if let Some(reason) = result.classification_reason.as_mut() {
        reason.push_str(&format!(" (prompt={})", kind.as_str()));
    } else {
        result.classification_reason = Some(format!(
            "llm-semantic via {} prompt={}",
            stamp.model,
            kind.as_str()
        ));
    }
    result.record.classification_reason = result.classification_reason.clone();
    Ok(result)
}

fn skipped(status: UnderstandingStatus, reason: String) -> AnalysisResult {
    AnalysisResult {
        record: KnowledgeRecord {
            entities: Vec::new(),
            relationships: Vec::new(),
            diagnostics: vec![Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: reason.clone(),
            }],
            status: Some(status),
            classification_reason: Some(reason.clone()),
        },
        status,
        classification_reason: Some(reason),
    }
}

fn build_request(
    pack: &PromptPack,
    meta: &str,
    kind: PromptKind,
    object: &ObjectDescriptor,
    bytes: &[u8],
    text_limit: usize,
) -> Result<LlmRequest> {
    let mut user_text = format!("{}\n\n{}", pack.user_preamble, meta);
    let image = if kind.wants_binary() {
        user_text.push_str(
            "\n\nBinary content is attached. Extract knowledge per the system instructions.",
        );
        Some(LlmImage {
            media_type: object.media_type.clone(),
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            name: object
                .relative_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string),
        })
    } else {
        let text = String::from_utf8_lossy(bytes);
        let truncated = if text.len() > text_limit {
            format!(
                "{}\n\n…[truncated {} of {} chars]",
                &text[..text_limit],
                text_limit,
                text.len()
            )
        } else {
            text.into_owned()
        };
        user_text.push_str(&format!(
            "\n\n----- BEGIN CONTENT -----\n{truncated}\n----- END CONTENT -----"
        ));
        None
    };

    Ok(LlmRequest {
        system: pack.system.clone(),
        user_text,
        image,
        max_tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmMode;
    use crate::ids::ObjectId;
    use std::path::PathBuf;

    fn desc(rel: &str, media: &str) -> ObjectDescriptor {
        let path = PathBuf::from(rel);
        let extension = path.extension().map(|e| e.to_string_lossy().to_string());
        ObjectDescriptor {
            id: ObjectId::new(format!("obj:{rel}")),
            path: path.clone(),
            relative_path: path,
            is_dir: false,
            size: 10,
            modified: None,
            media_type: media.into(),
            extension,
            etag: None,
        }
    }

    fn provider(mode: LlmMode) -> LlmSemanticProvider {
        LlmSemanticProvider::new(LlmConfig::new("gpt-4o-mini", mode), None)
    }

    #[test]
    fn fallback_skips_specialized_claims_images() {
        let p = provider(LlmMode::Fallback);
        let proto = ProtoData::default();
        assert_eq!(
            p.probe(&desc("src/lib.rs", "text/x-rust"), &proto),
            Support::None
        );
        assert_eq!(
            p.probe(&desc("img.png", "image/png"), &proto),
            Support::Partial
        );
        assert_eq!(
            p.probe(&desc("img.jpg", "image/jpeg"), &proto),
            Support::None
        );
        assert_eq!(
            p.probe(&desc("img.tif", "image/tiff"), &proto),
            Support::None
        );
        assert_eq!(
            p.probe(&desc("doc.pdf", "application/pdf"), &proto),
            Support::Partial
        );
    }

    #[test]
    fn always_claims_partial_everywhere() {
        let p = provider(LlmMode::Always);
        let proto = ProtoData::default();
        assert_eq!(
            p.probe(&desc("src/lib.rs", "text/x-rust"), &proto),
            Support::Partial
        );
        assert_eq!(
            p.probe(&desc("img.png", "image/png"), &proto),
            Support::Partial
        );
    }

    #[test]
    fn budget_and_secrets_skip_the_model() {
        use crate::content::BytesContentReader;
        use std::sync::atomic::Ordering;

        let metrics = Arc::new(LlmMetrics::new());
        metrics.max_requests.store(1, Ordering::Relaxed);
        metrics.record_success(Some(1), Some(1), Some(2));
        let mut cfg = LlmConfig::new("gpt-4o-mini", LlmMode::Always);
        cfg.max_requests = 1;
        let p = LlmSemanticProvider::new(cfg, Some(Arc::clone(&metrics)));
        let mut reader = BytesContentReader::from_slice(b"fn main() {}");
        let result = p
            .analyze(
                &desc("a.rs", "text/x-rust"),
                &ProtoData::default(),
                &mut reader,
            )
            .unwrap();
        assert!(result
            .classification_reason
            .unwrap()
            .contains("budget exhausted"));

        let mut cfg = LlmConfig::new("gpt-4o-mini", LlmMode::Always);
        cfg.allow_secrets = false;
        let p = LlmSemanticProvider::new(cfg, None);
        let mut reader = BytesContentReader::from_slice(b"api_key=abcdefghijklmnop");
        let result = p
            .analyze(
                &desc("a.rs", "text/x-rust"),
                &ProtoData::default(),
                &mut reader,
            )
            .unwrap();
        let reason = result.classification_reason.unwrap();
        assert!(reason.contains("secret"));
        assert!(!reason.contains("abcdefghijklmnop"));
    }

    #[test]
    fn cache_hit_skips_the_model() {
        use crate::content::BytesContentReader;
        use std::sync::atomic::Ordering;

        let dir = std::env::temp_dir().join(format!("gnosis-llm-hit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = cache::cache_key("fp-1", "gpt-4o-mini", "code", PROMPT_PACK_VERSION);
        cache::store(
            &dir,
            &key,
            r#"{"summary":"cached","status":"partial","entities":[{"kind":"module","name":"auth","confidence":"low"}]}"#,
        )
        .unwrap();

        let metrics = Arc::new(LlmMetrics::new());
        let mut cfg = LlmConfig::new("gpt-4o-mini", LlmMode::Always);
        cfg.cache_dir = dir.clone();
        let p = LlmSemanticProvider::new(cfg, Some(Arc::clone(&metrics)));
        let proto = ProtoData {
            fingerprint: Some("fp-1".into()),
            ..ProtoData::default()
        };
        let mut reader = BytesContentReader::from_slice(b"fn main() {}");
        let result = p
            .analyze(&desc("a.rs", "text/x-rust"), &proto, &mut reader)
            .unwrap();
        assert_eq!(result.record.entities.len(), 1);
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("origin")
                .map(String::as_str),
            Some("llm")
        );
        assert_eq!(metrics.cache_hits.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.requests.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
