use crate::connectors::s3::S3Location;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Where objects are discovered from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScanSource {
    /// Local directory (and optional Git enrichment).
    Filesystem { root: PathBuf },
    /// S3 bucket treated as a root folder; object keys are paths.
    S3 {
        location: S3Location,
        /// Optional AWS region override (otherwise default chain / env).
        region: Option<String>,
    },
}

impl ScanSource {
    pub fn connector_name(&self) -> &'static str {
        match self {
            Self::Filesystem { .. } => "filesystem",
            Self::S3 { .. } => "s3",
        }
    }

    pub fn display_root(&self) -> PathBuf {
        match self {
            Self::Filesystem { root } => root.clone(),
            Self::S3 { location, .. } => location.display_root(),
        }
    }

    pub fn uses_git(&self) -> bool {
        matches!(self, Self::Filesystem { .. })
    }
}

/// When to invoke the optional LLM semantic provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmMode {
    /// Claim objects that would otherwise only get the generic Weak fallback
    /// (images, PDFs/OCR, unknown binaries). Deterministic Full providers still win.
    #[default]
    Fallback,
    /// Claim Partial support for every object. Deterministic Full providers still win;
    /// LLM beats PlainText Partial / Generic when registered ahead of them.
    Always,
}

impl LlmMode {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fallback" => Ok(Self::Fallback),
            "always" => Ok(Self::Always),
            other => Err(format!(
                "invalid llm mode '{other}' (expected fallback|always)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fallback => "fallback",
            Self::Always => "always",
        }
    }
}

/// Optional multi-provider LLM configuration (`--features llm`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    /// Model id. A `vendor::model` prefix forces the adapter; otherwise the name
    /// selects OpenAI (`gpt-…`), Anthropic (`claude-…`), Gemini (`gemini-…`), or Ollama.
    pub model: String,
    pub mode: LlmMode,
    /// Directory of JSON cache files keyed by content, model, and prompt version.
    pub cache_dir: PathBuf,
    /// Stop calling the model after this many requests. `0` means unlimited.
    pub max_requests: u64,
    /// Stop calling the model after this many total tokens. `0` means unlimited.
    pub max_tokens: u64,
    /// When false, objects that look like they contain secrets are not sent.
    pub allow_secrets: bool,
}

impl LlmConfig {
    pub fn new(model: impl Into<String>, mode: LlmMode) -> Self {
        Self {
            model: model.into(),
            mode,
            cache_dir: PathBuf::from(".gnosis/llm-cache"),
            max_requests: 0,
            max_tokens: 0,
            allow_secrets: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanConfig {
    pub source: ScanSource,
    /// Display / store root (`filesystem` path or `s3://bucket[/prefix]`).
    pub root: PathBuf,
    pub max_object_size: u64,
    pub excluded_paths: Vec<String>,
    pub concurrency: usize,
    pub event_history_len: usize,
    pub output_path: PathBuf,
    pub skip_output_dir_name: String,
    pub queue_capacity: usize,
    /// Persistent job database path (redb). Default: `.gnosis/jobs.redb`.
    pub job_db_path: PathBuf,
    /// Persistent knowledge database (redb). Default: `.gnosis/knowledge.redb`.
    pub knowledge_db_path: PathBuf,
    /// Ignore the knowledge store and analyze every object.
    pub reanalyze: bool,
    /// Worker poll interval when the queue is empty.
    pub job_poll_ms: u64,
    /// Auto-retry policy for failed analyze jobs.
    pub retry: crate::jobs::RetryPolicy,
    /// When set (and built with `--features llm`), register the LLM semantic provider.
    pub llm: Option<LlmConfig>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        let root = PathBuf::from(".");
        Self {
            source: ScanSource::Filesystem { root: root.clone() },
            root,
            max_object_size: 2 * 1024 * 1024,
            excluded_paths: vec![
                "target".into(),
                "node_modules".into(),
                ".git".into(),
                "knowledge.okf".into(),
                ".gnosis".into(),
            ],
            concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(8),
            event_history_len: 200,
            output_path: PathBuf::from("knowledge.okf"),
            skip_output_dir_name: "knowledge.okf".into(),
            queue_capacity: 1024,
            job_db_path: PathBuf::from(".gnosis/jobs.redb"),
            knowledge_db_path: PathBuf::from(".gnosis/knowledge.redb"),
            reanalyze: false,
            job_poll_ms: 25,
            retry: crate::jobs::RetryPolicy::default(),
            llm: None,
        }
    }
}

impl ScanConfig {
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            source: ScanSource::Filesystem { root: root.clone() },
            root,
            ..Self::default()
        }
    }

    pub fn with_source(source: ScanSource) -> Self {
        let root = source.display_root();
        Self {
            source,
            root,
            ..Self::default()
        }
    }

    pub fn connector_name(&self) -> &'static str {
        self.source.connector_name()
    }
}

/// Live counters for optional LLM semantic extraction (shared with the provider).
#[derive(Debug, Default)]
pub struct LlmMetrics {
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    pub cache_hits: AtomicU64,
    pub prompt_tokens: AtomicU64,
    pub completion_tokens: AtomicU64,
    pub total_tokens: AtomicU64,
    /// Configured cap copied for the TUI. `0` means unlimited.
    pub max_requests: AtomicU64,
    /// Configured cap copied for the TUI. `0` means unlimited.
    pub max_tokens: AtomicU64,
}

impl LlmMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one successful chat completion (token fields may be missing from some providers).
    pub fn record_success(
        &self,
        prompt_tokens: Option<i32>,
        completion_tokens: Option<i32>,
        total_tokens: Option<i32>,
    ) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let prompt = prompt_tokens.unwrap_or(0).max(0) as u64;
        let completion = completion_tokens.unwrap_or(0).max(0) as u64;
        let total = total_tokens
            .map(|t| t.max(0) as u64)
            .unwrap_or(prompt + completion);
        self.prompt_tokens.fetch_add(prompt, Ordering::Relaxed);
        self.completion_tokens
            .fetch_add(completion, Ordering::Relaxed);
        self.total_tokens.fetch_add(total, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// True when another model call would pass a configured cap.
    pub fn over_budget(&self) -> bool {
        let max_requests = self.max_requests.load(Ordering::Relaxed);
        let max_tokens = self.max_tokens.load(Ordering::Relaxed);
        (max_requests > 0 && self.requests.load(Ordering::Relaxed) >= max_requests)
            || (max_tokens > 0 && self.total_tokens.load(Ordering::Relaxed) >= max_tokens)
    }
}

#[cfg(test)]
mod llm_metrics_tests {
    use super::*;

    #[test]
    fn records_success_and_errors() {
        let m = LlmMetrics::new();
        m.record_success(Some(10), Some(5), None);
        m.record_success(Some(2), Some(3), Some(9));
        m.record_error();
        assert_eq!(m.requests.load(Ordering::Relaxed), 3);
        assert_eq!(m.errors.load(Ordering::Relaxed), 1);
        assert_eq!(m.prompt_tokens.load(Ordering::Relaxed), 12);
        assert_eq!(m.completion_tokens.load(Ordering::Relaxed), 8);
        assert_eq!(m.total_tokens.load(Ordering::Relaxed), 24); // 15 + 9
        m.max_requests.store(3, Ordering::Relaxed);
        assert!(m.over_budget());
        m.record_cache_hit();
        assert_eq!(m.cache_hits.load(Ordering::Relaxed), 1);
    }
}

#[derive(Debug, Default)]
pub struct ScanMetrics {
    pub objects_discovered: AtomicU64,
    pub bytes_considered: AtomicU64,
    pub understood: AtomicU64,
    pub partial: AtomicU64,
    pub unknown: AtomicU64,
    pub failed: AtomicU64,
    pub entities: AtomicU64,
    pub relationships: AtomicU64,
    pub objects_analyzed: AtomicU64,
    pub objects_skipped: AtomicU64,
    counted: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Shared with [`crate::providers::LlmSemanticProvider`] when LLM is enabled.
    pub llm: std::sync::Arc<LlmMetrics>,
    started: std::sync::Mutex<Option<Instant>>,
}

impl ScanMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self) {
        *self.started.lock().unwrap() = Some(Instant::now());
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn snapshot(&self, queue_depth: usize) -> crate::events::MetricsSnapshot {
        crate::events::MetricsSnapshot {
            objects_discovered: self.objects_discovered.load(Ordering::Relaxed),
            bytes_considered: self.bytes_considered.load(Ordering::Relaxed),
            understood: self.understood.load(Ordering::Relaxed),
            partial: self.partial.load(Ordering::Relaxed),
            unknown: self.unknown.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            entities: self.entities.load(Ordering::Relaxed),
            relationships: self.relationships.load(Ordering::Relaxed),
            queue_depth,
            elapsed_ms: self.elapsed_ms(),
            llm_requests: self.llm.requests.load(Ordering::Relaxed),
            llm_errors: self.llm.errors.load(Ordering::Relaxed),
            llm_cache_hits: self.llm.cache_hits.load(Ordering::Relaxed),
            llm_prompt_tokens: self.llm.prompt_tokens.load(Ordering::Relaxed),
            llm_completion_tokens: self.llm.completion_tokens.load(Ordering::Relaxed),
            llm_total_tokens: self.llm.total_tokens.load(Ordering::Relaxed),
            llm_max_requests: self.llm.max_requests.load(Ordering::Relaxed),
            llm_max_tokens: self.llm.max_tokens.load(Ordering::Relaxed),
        }
    }

    pub fn unrecord_status(&self, status: crate::status::UnderstandingStatus) {
        use crate::status::UnderstandingStatus::*;
        let counter = match status {
            Understood => &self.understood,
            PartiallyUnderstood => &self.partial,
            Unknown => &self.unknown,
            Failed => &self.failed,
        };
        counter.fetch_sub(1, Ordering::Relaxed);
    }

    /// Remember that this object's status was added to the counters.
    pub fn mark_counted(&self, id: &str) {
        self.counted.lock().unwrap().insert(id.to_string());
    }

    /// True once, if `mark_counted` was called for this id.
    pub fn take_counted(&self, id: &str) -> bool {
        self.counted.lock().unwrap().remove(id)
    }

    pub fn record_status(&self, status: crate::status::UnderstandingStatus) {
        use crate::status::UnderstandingStatus::*;
        match status {
            Understood => self.understood.fetch_add(1, Ordering::Relaxed),
            PartiallyUnderstood => self.partial.fetch_add(1, Ordering::Relaxed),
            Unknown => self.unknown.fetch_add(1, Ordering::Relaxed),
            Failed => self.failed.fetch_add(1, Ordering::Relaxed),
        };
    }
}
