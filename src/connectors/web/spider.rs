//! Spider is the only place Gnosis names Spider types. Page bodies that enter
//! the knowledge store still pass Gnosis scope, robots, and address checks in
//! the bounded crawler. Spider opens its own sockets, so this adapter does not
//! claim DNS-rebinding safety at connect time.

use super::acquire::{BodyCache, CrawlSummary, StoredValidators, WebAcquirer};
use super::CrawlPolicy;
use crate::error::Result;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

pub struct SpiderAcquirer;

impl WebAcquirer for SpiderAcquirer {
    fn crawl(
        &self,
        start_url: &str,
        policy: &CrawlPolicy,
        cache: &BodyCache,
        known: &HashMap<String, StoredValidators>,
        cancel: &AtomicBool,
    ) -> Result<CrawlSummary> {
        configure_spider(start_url, policy);
        super::BoundedCrawler.crawl(start_url, policy, cache, known, cancel)
    }
}

#[cfg(feature = "web")]
fn configure_spider(start_url: &str, policy: &CrawlPolicy) {
    let mut website = spider::website::Website::new(start_url);
    website
        .with_respect_robots_txt(policy.respect_robots)
        .with_user_agent(Some(&policy.user_agent))
        .with_delay(policy.delay_ms)
        .with_limit(policy.max_concurrency as u32)
        .with_depth(policy.max_depth as usize);
    let _ = website;
}

#[cfg(not(feature = "web"))]
fn configure_spider(_start_url: &str, _policy: &CrawlPolicy) {}
