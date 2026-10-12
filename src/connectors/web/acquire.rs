//! Gnosis-owned crawl contract. The bounded crawler enforces policy. Spider
//! adapts its pages into the same types.

use super::identity::{canonicalize, extension_for_media, in_scope, object_id_for, resolve_link};
use super::robots::Robots;
use super::ssrf::check_url;
use super::CrawlPolicy;
use crate::connectors::types::ObjectDescriptor;
use crate::error::{GnosisError, Result};
use crate::ids::ObjectId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcquisitionKind {
    BlockedRobots,
    BlockedScope,
    BlockedSsrf,
    BlockedScheme,
    BlockedCredentials,
    SkippedLimit,
    HttpError,
    TooLarge,
}

impl AcquisitionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BlockedRobots => "blocked_robots",
            Self::BlockedScope => "blocked_scope",
            Self::BlockedSsrf => "blocked_ssrf",
            Self::BlockedScheme => "blocked_scheme",
            Self::BlockedCredentials => "blocked_credentials",
            Self::SkippedLimit => "skipped_limit",
            Self::HttpError => "http_error",
            Self::TooLarge => "too_large",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AcquisitionOutcome {
    pub url: String,
    pub kind: AcquisitionKind,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct PageBody {
    pub bytes: Vec<u8>,
    pub requested_url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
}

/// Scan-scoped page bodies. Job rows store the URL, not the bytes.
#[derive(Clone, Default)]
pub struct BodyCache {
    pages: Arc<Mutex<HashMap<String, PageBody>>>,
}

impl BodyCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, key: &str, page: PageBody) {
        self.pages.lock().unwrap().insert(key.to_string(), page);
    }

    pub fn get(&self, key: &str) -> Option<PageBody> {
        self.pages.lock().unwrap().get(key).cloned()
    }
}

#[derive(Clone, Debug)]
pub struct AcquiredWebObject {
    pub descriptor: ObjectDescriptor,
    pub requested_url: String,
    pub final_url: String,
    pub status: u16,
    /// 304. The descriptor carries the stored size and validators so analysis is skipped.
    pub not_modified: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CrawlSummary {
    pub discovered: u64,
    pub fetched: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes: u64,
    pub outcomes: Vec<AcquisitionOutcome>,
    pub objects: Vec<AcquiredWebObject>,
}

/// Previously stored validators so a second crawl can send a conditional request.
#[derive(Clone, Debug, Default)]
pub struct StoredValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub media_type: String,
}

pub trait WebAcquirer: Send + Sync {
    fn crawl(
        &self,
        start_url: &str,
        policy: &CrawlPolicy,
        cache: &BodyCache,
        known: &HashMap<String, StoredValidators>,
        cancel: &AtomicBool,
    ) -> Result<CrawlSummary>;
}

/// Test double: one prepared page, no network.
pub struct FakeAcquirer {
    pub pages: Vec<(String, u16, String, Vec<u8>)>,
}

impl WebAcquirer for FakeAcquirer {
    fn crawl(
        &self,
        start_url: &str,
        policy: &CrawlPolicy,
        cache: &BodyCache,
        _known: &HashMap<String, StoredValidators>,
        _cancel: &AtomicBool,
    ) -> Result<CrawlSummary> {
        let mut summary = CrawlSummary::default();
        let canonical = canonicalize(start_url).map_err(GnosisError::Pipeline)?;
        summary.discovered = 1;
        let Some((_, status, content_type, bytes)) = self
            .pages
            .iter()
            .find(|(url, _, _, _)| canonicalize(url).ok().as_deref() == Some(canonical.as_str()))
        else {
            summary.failed = 1;
            summary.outcomes.push(AcquisitionOutcome {
                url: canonical,
                kind: AcquisitionKind::HttpError,
                detail: "fake acquirer has no page".into(),
            });
            return Ok(summary);
        };
        if bytes.len() as u64 > policy.max_object_bytes {
            summary.skipped = 1;
            summary.outcomes.push(AcquisitionOutcome {
                url: canonical.clone(),
                kind: AcquisitionKind::TooLarge,
                detail: "object exceeds max size".into(),
            });
            return Ok(summary);
        }
        let descriptor = descriptor_for(
            &canonical,
            &canonical,
            content_type,
            bytes.len() as u64,
            None,
            None,
        );
        cache.insert(
            descriptor.id.as_str(),
            PageBody {
                bytes: bytes.clone(),
                requested_url: canonical.clone(),
                final_url: canonical.clone(),
                status: *status,
                content_type: content_type.clone(),
            },
        );
        summary.fetched = 1;
        summary.bytes = bytes.len() as u64;
        summary.objects.push(AcquiredWebObject {
            descriptor,
            requested_url: canonical.clone(),
            final_url: canonical,
            status: *status,
            not_modified: false,
        });
        Ok(summary)
    }
}

pub struct BoundedCrawler;

impl WebAcquirer for BoundedCrawler {
    fn crawl(
        &self,
        start_url: &str,
        policy: &CrawlPolicy,
        cache: &BodyCache,
        known: &HashMap<String, StoredValidators>,
        cancel: &AtomicBool,
    ) -> Result<CrawlSummary> {
        let start = canonicalize(start_url).map_err(GnosisError::Pipeline)?;
        if let Err(detail) = check_url(&start, policy.allow_private_network) {
            let kind = if detail.contains("credential") {
                AcquisitionKind::BlockedCredentials
            } else if detail.contains("scheme") {
                AcquisitionKind::BlockedScheme
            } else {
                AcquisitionKind::BlockedSsrf
            };
            return Ok(CrawlSummary {
                discovered: 1,
                blocked: 1,
                outcomes: vec![AcquisitionOutcome {
                    url: start,
                    kind,
                    detail,
                }],
                ..CrawlSummary::default()
            });
        }

        let client = reqwest::blocking::Client::builder()
            .user_agent(&policy.user_agent)
            .timeout(Duration::from_millis(policy.timeout_ms.max(1)))
            .redirect(reqwest::redirect::Policy::none())
            .gzip(true)
            .build()
            .map_err(|e| GnosisError::Pipeline(format!("http client: {e}")))?;

        let robots = if policy.respect_robots {
            fetch_robots(&client, &start, policy)
        } else {
            Robots::default()
        };

        let mut summary = CrawlSummary::default();
        let mut queue = VecDeque::new();
        let mut seen = HashSet::new();
        queue.push_back((start.clone(), 0u32));
        seen.insert(start.clone());

        let mut wave = Vec::new();
        loop {
            while let Some((url, depth)) = queue.pop_front() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(GnosisError::Cancelled);
                }
                summary.discovered += 1;
                if summary.fetched + wave.len() as u64 >= policy.max_pages {
                    summary.skipped += 1;
                    summary.outcomes.push(AcquisitionOutcome {
                        url,
                        kind: AcquisitionKind::SkippedLimit,
                        detail: "max pages".into(),
                    });
                    continue;
                }
                if summary.bytes >= policy.max_total_bytes {
                    summary.skipped += 1;
                    summary.outcomes.push(AcquisitionOutcome {
                        url,
                        kind: AcquisitionKind::SkippedLimit,
                        detail: "max total bytes".into(),
                    });
                    continue;
                }
                if !in_scope_start(&start, &url, policy) {
                    summary.blocked += 1;
                    summary.outcomes.push(AcquisitionOutcome {
                        url,
                        kind: AcquisitionKind::BlockedScope,
                        detail: "outside crawl scope".into(),
                    });
                    continue;
                }
                if let Err(detail) = check_url(&url, policy.allow_private_network) {
                    summary.blocked += 1;
                    summary.outcomes.push(AcquisitionOutcome {
                        url,
                        kind: AcquisitionKind::BlockedSsrf,
                        detail,
                    });
                    continue;
                }
                if policy.respect_robots {
                    if let Ok(parsed) = url::Url::parse(&url) {
                        if !robots.allows(parsed.path()) {
                            summary.blocked += 1;
                            summary.outcomes.push(AcquisitionOutcome {
                                url,
                                kind: AcquisitionKind::BlockedRobots,
                                detail: "robots.txt".into(),
                            });
                            continue;
                        }
                    }
                }
                wave.push((url, depth));
                if wave.len() >= policy.max_concurrency.max(1) {
                    flush_wave(
                        &client,
                        policy,
                        cache,
                        known,
                        &mut summary,
                        &mut seen,
                        &mut queue,
                        &mut wave,
                    );
                }
            }
            if wave.is_empty() {
                break;
            }
            flush_wave(
                &client,
                policy,
                cache,
                known,
                &mut summary,
                &mut seen,
                &mut queue,
                &mut wave,
            );
        }
        Ok(summary)
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_wave(
    client: &reqwest::blocking::Client,
    policy: &CrawlPolicy,
    cache: &BodyCache,
    known: &HashMap<String, StoredValidators>,
    summary: &mut CrawlSummary,
    seen: &mut HashSet<String>,
    queue: &mut VecDeque<(String, u32)>,
    wave: &mut Vec<(String, u32)>,
) {
    if wave.is_empty() {
        return;
    }
    if policy.delay_ms > 0 && summary.fetched > 0 {
        std::thread::sleep(Duration::from_millis(policy.delay_ms));
    }
    let batch = std::mem::take(wave);
    let fetched = std::thread::scope(|scope| {
        let mut joins = Vec::new();
        for (url, depth) in batch {
            let id = object_id_for(&url);
            let prior = known.get(&id).or_else(|| known.get(&url)).cloned();
            joins.push(scope.spawn(move || {
                let result = fetch_page(client, &url, policy, prior.as_ref());
                (url, depth, result)
            }));
        }
        joins
            .into_iter()
            .map(|join| join.join().expect("web fetch panicked"))
            .collect::<Vec<_>>()
    });
    for (url, depth, result) in fetched {
        commit_fetch(summary, cache, seen, queue, policy, url, depth, result);
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_fetch(
    summary: &mut CrawlSummary,
    cache: &BodyCache,
    seen: &mut HashSet<String>,
    queue: &mut VecDeque<(String, u32)>,
    policy: &CrawlPolicy,
    url: String,
    depth: u32,
    result: std::result::Result<Fetch, FetchFail>,
) {
    match result {
        Ok(Fetch::NotModified {
            etag,
            modified,
            size,
            media_type,
        }) => {
            let descriptor = descriptor_for(&url, &url, &media_type, size, etag, modified);
            summary.fetched += 1;
            summary.objects.push(AcquiredWebObject {
                descriptor,
                requested_url: url.clone(),
                final_url: url,
                status: 304,
                not_modified: true,
            });
        }
        Ok(Fetch::Page {
            requested,
            final_url,
            status,
            content_type,
            etag,
            modified,
            bytes,
            links,
        }) => {
            if status >= 400 {
                summary.failed += 1;
                summary.outcomes.push(AcquisitionOutcome {
                    url: requested,
                    kind: AcquisitionKind::HttpError,
                    detail: format!("HTTP {status}"),
                });
                return;
            }
            let len = bytes.len() as u64;
            if len > policy.max_object_bytes || summary.bytes + len > policy.max_total_bytes {
                summary.skipped += 1;
                summary.outcomes.push(AcquisitionOutcome {
                    url: requested,
                    kind: AcquisitionKind::TooLarge,
                    detail: "response exceeds byte limit".into(),
                });
                return;
            }
            summary.bytes += len;
            summary.fetched += 1;
            let descriptor =
                descriptor_for(&final_url, &final_url, &content_type, len, etag, modified);
            cache.insert(
                descriptor.id.as_str(),
                PageBody {
                    bytes: bytes.clone(),
                    requested_url: requested.clone(),
                    final_url: final_url.clone(),
                    status,
                    content_type,
                },
            );
            summary.objects.push(AcquiredWebObject {
                descriptor,
                requested_url: requested,
                final_url: final_url.clone(),
                status,
                not_modified: false,
            });
            if depth < policy.max_depth {
                for link in links {
                    if seen.insert(link.clone()) {
                        queue.push_back((link, depth + 1));
                    }
                }
            }
        }
        Err(fail) => {
            match fail.kind {
                AcquisitionKind::SkippedLimit | AcquisitionKind::TooLarge => summary.skipped += 1,
                AcquisitionKind::HttpError => summary.failed += 1,
                _ => summary.blocked += 1,
            }
            summary.outcomes.push(AcquisitionOutcome {
                url,
                kind: fail.kind,
                detail: fail.detail,
            });
        }
    }
}

fn in_scope_start(start: &str, url: &str, policy: &CrawlPolicy) -> bool {
    in_scope(
        start,
        url,
        matches!(policy.scope, super::CrawlScope::IncludeSubdomains),
    )
}

fn descriptor_for(
    id_url: &str,
    path_url: &str,
    content_type: &str,
    size: u64,
    etag: Option<String>,
    modified: Option<SystemTime>,
) -> ObjectDescriptor {
    let media = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_string();
    let extension = extension_for_media(&media);
    let path = std::path::PathBuf::from(path_url);
    ObjectDescriptor {
        id: ObjectId::new(object_id_for(id_url)),
        path: path.clone(),
        relative_path: path,
        is_dir: false,
        size,
        modified,
        media_type: if media.is_empty() {
            "application/octet-stream".into()
        } else {
            media
        },
        extension,
        etag,
    }
}

struct FetchFail {
    kind: AcquisitionKind,
    detail: String,
}

fn fetch_fail(kind: AcquisitionKind, detail: impl Into<String>) -> FetchFail {
    FetchFail {
        kind,
        detail: detail.into(),
    }
}

fn fail_from_check(detail: String) -> FetchFail {
    let kind = if detail.contains("credential") {
        AcquisitionKind::BlockedCredentials
    } else if detail.contains("scheme") {
        AcquisitionKind::BlockedScheme
    } else {
        AcquisitionKind::BlockedSsrf
    };
    FetchFail { kind, detail }
}

enum Fetch {
    NotModified {
        etag: Option<String>,
        modified: Option<SystemTime>,
        size: u64,
        media_type: String,
    },
    Page {
        requested: String,
        final_url: String,
        status: u16,
        content_type: String,
        etag: Option<String>,
        modified: Option<SystemTime>,
        bytes: Vec<u8>,
        links: Vec<String>,
    },
}

fn fetch_page(
    client: &reqwest::blocking::Client,
    url: &str,
    policy: &CrawlPolicy,
    known: Option<&StoredValidators>,
) -> std::result::Result<Fetch, FetchFail> {
    let mut current = url.to_string();
    let requested = url.to_string();
    for hop in 0..=policy.max_redirects {
        let mut builder = client.get(&current);
        if hop == 0 {
            if let Some(known) = known {
                if let Some(etag) = &known.etag {
                    builder = builder.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                if let Some(lm) = &known.last_modified {
                    builder = builder.header(reqwest::header::IF_MODIFIED_SINCE, lm);
                }
            }
        }
        let response = builder
            .send()
            .map_err(|e| fetch_fail(AcquisitionKind::HttpError, e.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            if hop == policy.max_redirects {
                return Err(fetch_fail(AcquisitionKind::SkippedLimit, "redirect limit"));
            }
            let Some(location) = allowed_header(response.headers(), "location") else {
                return Err(fetch_fail(
                    AcquisitionKind::HttpError,
                    "redirect missing location",
                ));
            };
            let Some(next) = resolve_link(&current, location) else {
                return Err(fetch_fail(
                    AcquisitionKind::BlockedScheme,
                    "redirect target is not an http(s) URL",
                ));
            };
            check_url(&next, policy.allow_private_network).map_err(fail_from_check)?;
            current = next;
            continue;
        }
        if status.as_u16() == 304 {
            let known = known.cloned().unwrap_or_default();
            return Ok(Fetch::NotModified {
                etag: known.etag,
                modified: known.modified,
                size: known.size,
                media_type: if known.media_type.is_empty() {
                    "application/octet-stream".into()
                } else {
                    known.media_type
                },
            });
        }
        let content_type = allowed_header(response.headers(), "content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        if let Some(len) =
            allowed_header(response.headers(), "content-length").and_then(|v| v.parse::<u64>().ok())
        {
            if len > policy.max_object_bytes {
                return Err(fetch_fail(
                    AcquisitionKind::TooLarge,
                    "content-length exceeds max size",
                ));
            }
        }
        let etag =
            allowed_header(response.headers(), "etag").map(|v| v.trim_matches('"').to_string());
        let modified =
            allowed_header(response.headers(), "last-modified").and_then(parse_http_date);
        let bytes = response
            .bytes()
            .map_err(|e| fetch_fail(AcquisitionKind::HttpError, e.to_string()))?;
        if bytes.len() as u64 > policy.max_object_bytes {
            return Err(fetch_fail(
                AcquisitionKind::TooLarge,
                "body exceeds max size",
            ));
        }
        let links = if content_type.to_ascii_lowercase().contains("html") {
            extract_links(&current, &String::from_utf8_lossy(&bytes))
        } else {
            Vec::new()
        };
        let final_url = canonicalize(&current).unwrap_or(current);
        return Ok(Fetch::Page {
            requested,
            final_url,
            status: status.as_u16(),
            content_type,
            etag,
            modified,
            bytes: bytes.to_vec(),
            links,
        });
    }
    Err(fetch_fail(AcquisitionKind::SkippedLimit, "redirect limit"))
}

fn allowed_header<'a>(
    headers: &'a reqwest::header::HeaderMap,
    name: &'static str,
) -> Option<&'a str> {
    if !header_allowlist()
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
    {
        return None;
    }
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn extract_links(page: &str, html: &str) -> Vec<String> {
    let mut links = Vec::new();
    let dom = scraper::Html::parse_document(html);
    let Ok(selector) = scraper::Selector::parse("a[href]") else {
        return links;
    };
    for element in dom.select(&selector) {
        if let Some(href) = element.value().attr("href") {
            if let Some(abs) = resolve_link(page, href) {
                links.push(abs);
            }
        }
    }
    links.sort();
    links.dedup();
    links
}

fn fetch_robots(client: &reqwest::blocking::Client, start: &str, policy: &CrawlPolicy) -> Robots {
    let Ok(mut url) = url::Url::parse(start) else {
        return Robots::default();
    };
    url.set_path("/robots.txt");
    url.set_query(None);
    url.set_fragment(None);
    if check_url(url.as_str(), policy.allow_private_network).is_err() {
        return Robots::default();
    }
    let Ok(response) = client.get(url).send() else {
        return Robots::default();
    };
    if !response.status().is_success() {
        return Robots::default();
    }
    response
        .text()
        .map(|body| Robots::parse(&body))
        .unwrap_or_default()
}

fn parse_http_date(value: &str) -> Option<SystemTime> {
    // RFC 1123. A small parser avoids pulling the full time feature set.
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let day: u64 = parts[1].parse().ok()?;
    let month = match parts[2] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i32 = parts[3].parse().ok()?;
    let time = parts.get(4).copied().unwrap_or("00:00:00");
    let mut clock = time.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let min: u64 = clock.next()?.parse().ok()?;
    let sec: u64 = clock.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day as i32)? as u64;
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

fn days_from_civil(year: i32, month: u32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u32;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day as u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era as i64 * 146097 + doe as i64 - 719468)
}

/// Fetch one URL when a rerun finds the body cache empty.
pub fn refetch_body(url: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let policy = CrawlPolicy {
        max_pages: 1,
        max_depth: 0,
        max_object_bytes: max_bytes,
        respect_robots: false,
        allow_private_network: false,
        ..CrawlPolicy::default()
    };
    let cache = BodyCache::new();
    let crawler = BoundedCrawler;
    let summary = crawler.crawl(
        url,
        &policy,
        &cache,
        &HashMap::new(),
        &AtomicBool::new(false),
    )?;
    let Some(object) = summary.objects.first() else {
        return Err(GnosisError::Pipeline(format!(
            "refetch produced no page for {url}"
        )));
    };
    cache
        .get(object.descriptor.id.as_str())
        .map(|page| page.bytes)
        .ok_or_else(|| GnosisError::Pipeline(format!("refetch cache miss for {url}")))
}

pub fn header_allowlist() -> &'static [&'static str] {
    &[
        "content-type",
        "content-length",
        "etag",
        "last-modified",
        "content-language",
        "location",
    ]
}
