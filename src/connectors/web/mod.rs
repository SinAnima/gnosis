//! HTTP(S) acquisition. The `spider` implementation stays private so the rest of
//! Gnosis only sees these types.

mod acquire;
mod identity;
mod robots;
mod spider;
mod ssrf;

pub use acquire::{
    refetch_body, AcquiredWebObject, AcquisitionKind, AcquisitionOutcome, BodyCache,
    BoundedCrawler, CrawlSummary, FakeAcquirer, PageBody, StoredValidators, WebAcquirer,
};
pub use identity::{
    canonicalize, classify_target, in_scope, resolve_link, sanitize_display, TargetClass,
};
pub use robots::Robots;
pub use spider::SpiderAcquirer;
pub use ssrf::address_is_blocked;

use serde::{Deserialize, Serialize};

/// How far links may leave the start URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrawlScope {
    /// Same scheme, host, and port as the start URL.
    SameOrigin,
    /// Same origin, plus hosts that are subdomains of the start host.
    IncludeSubdomains,
}

/// Limits for one crawl. Defaults match the documented CLI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlPolicy {
    pub max_pages: u64,
    pub max_depth: u32,
    pub max_concurrency: usize,
    pub delay_ms: u64,
    pub timeout_ms: u64,
    pub max_redirects: u32,
    pub max_object_bytes: u64,
    pub max_total_bytes: u64,
    pub scope: CrawlScope,
    pub respect_robots: bool,
    /// Explicit opt-in for loopback, private, and link-local addresses.
    pub allow_private_network: bool,
    pub user_agent: String,
}

impl Default for CrawlPolicy {
    fn default() -> Self {
        let max_object_bytes = 2 * 1024 * 1024;
        Self {
            max_pages: 50,
            max_depth: 2,
            max_concurrency: 4,
            delay_ms: 250,
            timeout_ms: 15_000,
            max_redirects: 5,
            max_object_bytes,
            max_total_bytes: 50 * max_object_bytes,
            scope: CrawlScope::SameOrigin,
            respect_robots: true,
            allow_private_network: false,
            user_agent: format!("gnosis/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl CrawlPolicy {
    pub fn with_object_limit(mut self, max_object_bytes: u64) -> Self {
        self.max_object_bytes = max_object_bytes;
        if self.max_total_bytes < max_object_bytes {
            self.max_total_bytes = max_object_bytes;
        }
        self
    }
}
