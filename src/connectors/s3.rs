//! S3 connector — treat a bucket as a root folder and object keys as paths.
//!
//! URI form: `s3://bucket` or `s3://bucket/optional/prefix`.
//! Credentials and region use the default AWS chain unless `--region` is set.
//! A custom endpoint uses path-style addressing. TLS stays verified unless
//! `--s3-insecure` is set for that endpoint.

use crate::config::{S3ScanOptions, ScanConfig, ScanSource};
use crate::connectors::types::{cap_s3_map, etag_kind, ObjectDescriptor, S3ProtoData};
use crate::error::{GnosisError, Result};
use crate::events::PipelineEvent;
use crate::ids::ObjectId;
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Bucket + optional key prefix that defines the scan root.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    /// Key prefix under the bucket (no leading `/`). Empty means whole bucket.
    pub prefix: String,
}

impl S3Location {
    pub fn display_root(&self) -> PathBuf {
        if self.prefix.is_empty() {
            PathBuf::from(format!("s3://{}", self.bucket))
        } else {
            PathBuf::from(format!(
                "s3://{}/{}",
                self.bucket,
                self.prefix.trim_end_matches('/')
            ))
        }
    }

    /// Full S3 key for an object whose path is relative to this location.
    pub fn full_key(&self, relative: &Path) -> String {
        let rel = relative.to_string_lossy().replace('\\', "/");
        let rel = rel.trim_start_matches('/');
        if self.prefix.is_empty() {
            rel.to_string()
        } else {
            let prefix = self.prefix.trim_end_matches('/');
            if rel.is_empty() {
                prefix.to_string()
            } else {
                format!("{prefix}/{rel}")
            }
        }
    }
}

/// Parse `s3://bucket` or `s3://bucket/prefix/path`.
pub fn parse_s3_uri(input: &str) -> Result<S3Location> {
    let rest = input
        .strip_prefix("s3://")
        .or_else(|| input.strip_prefix("S3://"))
        .ok_or_else(|| GnosisError::Pipeline(format!("not an s3 URI: {input}")))?;

    if rest.is_empty() {
        return Err(GnosisError::Pipeline(
            "s3 URI missing bucket name (expected s3://bucket[/prefix])".into(),
        ));
    }

    let (bucket, prefix_raw) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };

    if bucket.is_empty()
        || bucket.contains('\\')
        || bucket.contains(' ')
        || bucket.contains('?')
        || bucket.contains('#')
    {
        return Err(GnosisError::Pipeline(format!(
            "invalid s3 bucket in URI: {input}"
        )));
    }

    let prefix = prefix_raw.trim_matches('/').to_string();
    Ok(S3Location {
        bucket: bucket.to_string(),
        prefix,
    })
}

pub fn is_s3_uri(input: &str) -> bool {
    let lower = input.get(..5).map(|s| s.eq_ignore_ascii_case("s3://"));
    lower == Some(true)
}

/// Strip `AKIA…` keys and `X-Amz-Signature` values from an error string.
pub fn redact_s3_error(text: &str) -> String {
    let scrubbed = crate::scrub::scrub_secrets(text);
    redact_amz_signature(&scrubbed)
}

fn redact_amz_signature(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &lower[index..];
        let found = ["x-amz-signature=", "x-amz-signature%3d"]
            .iter()
            .filter_map(|needle| rest.find(needle).map(|at| (at, needle.len())))
            .min_by_key(|(at, _)| *at);
        let Some((at, needle_len)) = found else {
            out.push_str(&text[index..]);
            break;
        };
        if at > 0 {
            out.push_str(&text[index..index + at]);
        }
        out.push_str("X-Amz-Signature=REDACTED");
        let mut end = index + at + needle_len;
        while end < bytes.len() {
            let ch = bytes[end];
            if ch == b'&' || ch.is_ascii_whitespace() {
                break;
            }
            end += 1;
        }
        index = end;
    }
    out
}

/// Retry HTTP 429 and 503 three times. An expired token is returned immediately.
pub fn retry_s3<T>(mut op: impl FnMut() -> Result<T>) -> Result<T> {
    const DELAYS_MS: [u64; 3] = [100, 200, 400];
    let mut attempt = 0;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) => {
                let text = err.to_string();
                let stop = s3_error_expired(&text)
                    || !s3_error_throttled(&text)
                    || attempt >= DELAYS_MS.len();
                if stop {
                    return Err(GnosisError::Pipeline(redact_s3_error(&text)));
                }
                std::thread::sleep(Duration::from_millis(DELAYS_MS[attempt]));
                attempt += 1;
            }
        }
    }
}

fn s3_error_expired(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("expiredtoken") || lower.contains("token has expired")
}

fn s3_error_throttled(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("429")
        || lower.contains("503")
        || lower.contains("slowdown")
        || lower.contains("serviceunavailable")
}

#[derive(Clone, Debug)]
pub struct S3ObjectMeta {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<SystemTime>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
    pub version_id: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    pub delete_marker: bool,
}

/// One list page. `continuation` is set when the service truncated the page.
#[derive(Clone, Debug)]
pub struct S3ListPage {
    pub objects: Vec<S3ObjectMeta>,
    pub continuation: Option<String>,
}

/// Minimal S3 surface so discovery and reads can be tested without AWS.
pub trait S3Backend: Send + Sync {
    fn list_page(&self, bucket: &str, prefix: &str, token: Option<&str>) -> Result<S3ListPage>;
    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>>;

    fn head_object(&self, bucket: &str, key: &str) -> Result<S3ObjectMeta> {
        let _ = (bucket, key);
        Err(GnosisError::Pipeline(
            "s3 head_object is not supported by this backend".into(),
        ))
    }

    fn get_object_tagging(&self, bucket: &str, key: &str) -> Result<BTreeMap<String, String>> {
        let _ = (bucket, key);
        Ok(BTreeMap::new())
    }

    fn list_versions_page(
        &self,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
    ) -> Result<S3ListPage> {
        self.list_page(bucket, prefix, token)
    }

    fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<S3ObjectMeta>> {
        let mut out = Vec::new();
        let mut token = None;
        loop {
            let page = self.list_page(bucket, prefix, token.as_deref())?;
            let next = page.continuation;
            out.extend(page.objects);
            match next {
                Some(value) => token = Some(value),
                None => break,
            }
        }
        Ok(out)
    }
}

/// Live AWS SDK backend (default credential / region chain).
pub struct AwsS3Backend {
    client: aws_sdk_s3::Client,
    runtime: tokio::runtime::Runtime,
}

impl AwsS3Backend {
    /// `endpoint` selects path-style addressing. `insecure` skips TLS verification for that endpoint.
    pub fn new(region: Option<&str>, endpoint: Option<&str>, insecure: bool) -> Result<Self> {
        if insecure && endpoint.is_none() {
            return Err(GnosisError::Pipeline(
                "--s3-insecure requires --s3-endpoint".into(),
            ));
        }
        let endpoint = endpoint.map(str::to_string);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("gnosis-s3")
            .worker_threads(2)
            .build()
            .map_err(|e| GnosisError::Pipeline(format!("tokio runtime: {e}")))?;

        let client = runtime.block_on(async {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(region) = region {
                loader = loader.region(aws_config::Region::new(region.to_string()));
            }
            let shared = loader.load().await;
            let mut builder = aws_sdk_s3::config::Builder::from(&shared);
            if let Some(endpoint) = &endpoint {
                builder = builder.endpoint_url(endpoint).force_path_style(true);
            }
            if insecure {
                builder = builder.http_client(super::s3_tls::insecure_client());
            }
            aws_sdk_s3::Client::from_conf(builder.build())
        });

        Ok(Self { client, runtime })
    }
}

fn aws_error(context: &str, err: impl std::fmt::Display) -> GnosisError {
    GnosisError::Pipeline(format!("{context}: {err}"))
}

impl S3Backend for AwsS3Backend {
    fn list_page(&self, bucket: &str, prefix: &str, token: Option<&str>) -> Result<S3ListPage> {
        let bucket = bucket.to_string();
        let prefix = prefix.to_string();
        let token = token.map(str::to_string);
        retry_s3(|| {
            self.runtime.block_on(async {
                let mut req = self.client.list_objects_v2().bucket(&bucket);
                if !prefix.is_empty() {
                    req = req.prefix(&prefix);
                }
                if let Some(token) = token.as_deref() {
                    req = req.continuation_token(token);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|err| aws_error("s3 list_objects_v2", err))?;
                let mut objects = Vec::new();
                for obj in resp.contents() {
                    let Some(key) = obj.key() else {
                        continue;
                    };
                    if key.is_empty() || key.ends_with('/') {
                        continue;
                    }
                    let last_modified = obj.last_modified().and_then(aws_time);
                    objects.push(S3ObjectMeta {
                        key: key.to_string(),
                        size: obj.size().unwrap_or(0) as u64,
                        last_modified,
                        etag: obj.e_tag().map(strip_etag),
                        storage_class: obj.storage_class().map(|class| class.as_str().to_string()),
                        version_id: None,
                        user_metadata: BTreeMap::new(),
                        delete_marker: false,
                    });
                }
                let continuation = if resp.is_truncated() == Some(true) {
                    resp.next_continuation_token().map(str::to_string)
                } else {
                    None
                };
                Ok(S3ListPage {
                    objects,
                    continuation,
                })
            })
        })
    }

    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>> {
        let bucket = bucket.to_string();
        let key = key.to_string();
        retry_s3(|| {
            self.runtime.block_on(async {
                let mut req = self.client.get_object().bucket(&bucket).key(&key);
                if max_size > 0 {
                    req = req.range(format!("bytes=0-{}", max_size.saturating_sub(1)));
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|err| aws_error("s3 get_object", err))?;
                if let Some(len) = resp.content_length() {
                    if len > 0 && len as u64 > max_size {
                        return Err(GnosisError::Pipeline("s3 size limit".into()));
                    }
                }
                let aggregated =
                    resp.body.collect().await.map_err(|err| {
                        GnosisError::Pipeline(format!("s3 read body {key}: {err}"))
                    })?;
                let bytes = aggregated.into_bytes();
                let n = (max_size as usize).min(bytes.len());
                Ok(bytes[..n].to_vec())
            })
        })
    }

    fn head_object(&self, bucket: &str, key: &str) -> Result<S3ObjectMeta> {
        let bucket = bucket.to_string();
        let key = key.to_string();
        retry_s3(|| {
            self.runtime.block_on(async {
                let resp = self
                    .client
                    .head_object()
                    .bucket(&bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|err| aws_error("s3 head_object", err))?;
                let user_metadata = cap_s3_map(resp.metadata().cloned().unwrap_or_default());
                Ok(S3ObjectMeta {
                    key: key.clone(),
                    size: resp.content_length().unwrap_or(0) as u64,
                    last_modified: resp.last_modified().and_then(aws_time),
                    etag: resp.e_tag().map(strip_etag),
                    storage_class: resp.storage_class().map(|class| class.as_str().to_string()),
                    version_id: resp.version_id().map(str::to_string),
                    user_metadata,
                    delete_marker: false,
                })
            })
        })
    }

    fn get_object_tagging(&self, bucket: &str, key: &str) -> Result<BTreeMap<String, String>> {
        let bucket = bucket.to_string();
        let key = key.to_string();
        retry_s3(|| {
            self.runtime.block_on(async {
                let resp = self
                    .client
                    .get_object_tagging()
                    .bucket(&bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|err| aws_error("s3 get_object_tagging", err))?;
                let mut tags = BTreeMap::new();
                for tag in resp.tag_set() {
                    tags.insert(tag.key().to_string(), tag.value().to_string());
                }
                Ok(cap_s3_map(tags))
            })
        })
    }

    fn list_versions_page(
        &self,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
    ) -> Result<S3ListPage> {
        let bucket = bucket.to_string();
        let prefix = prefix.to_string();
        let token = token.map(str::to_string);
        retry_s3(|| {
            self.runtime.block_on(async {
                let mut req = self.client.list_object_versions().bucket(&bucket);
                if !prefix.is_empty() {
                    req = req.prefix(&prefix);
                }
                if let Some(token) = token.as_deref() {
                    if let Some((key, version)) = token.split_once('\u{1}') {
                        req = req.key_marker(key).version_id_marker(version);
                    }
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|err| aws_error("s3 list_object_versions", err))?;
                let mut objects = Vec::new();
                for obj in resp.versions() {
                    let Some(key) = obj.key() else {
                        continue;
                    };
                    if key.is_empty() || key.ends_with('/') {
                        continue;
                    }
                    objects.push(S3ObjectMeta {
                        key: key.to_string(),
                        size: obj.size().unwrap_or(0) as u64,
                        last_modified: obj.last_modified().and_then(aws_time),
                        etag: obj.e_tag().map(strip_etag),
                        storage_class: obj.storage_class().map(|class| class.as_str().to_string()),
                        version_id: obj.version_id().map(str::to_string),
                        user_metadata: BTreeMap::new(),
                        delete_marker: false,
                    });
                }
                for marker in resp.delete_markers() {
                    let Some(key) = marker.key() else {
                        continue;
                    };
                    objects.push(S3ObjectMeta {
                        key: key.to_string(),
                        size: 0,
                        last_modified: marker.last_modified().and_then(aws_time),
                        etag: None,
                        storage_class: None,
                        version_id: marker.version_id().map(str::to_string),
                        user_metadata: BTreeMap::new(),
                        delete_marker: true,
                    });
                }
                let continuation = if resp.is_truncated() == Some(true) {
                    match (resp.next_key_marker(), resp.next_version_id_marker()) {
                        (Some(key), Some(version)) => Some(format!("{key}\u{1}{version}")),
                        _ => None,
                    }
                } else {
                    None
                };
                Ok(S3ListPage {
                    objects,
                    continuation,
                })
            })
        })
    }
}

fn strip_etag(etag: &str) -> String {
    etag.trim_matches('"').to_string()
}

fn aws_time(stamp: &aws_sdk_s3::primitives::DateTime) -> Option<SystemTime> {
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(stamp.secs() as u64))
}

struct MemObj {
    bytes: Vec<u8>,
    size: Option<u64>,
    etag: Option<String>,
    storage_class: Option<String>,
    version_id: Option<String>,
    metadata: BTreeMap<String, String>,
    tags: BTreeMap<String, String>,
    tag_error: Option<String>,
    delete_marker: bool,
    modified: Option<SystemTime>,
}

impl MemObj {
    fn plain(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            size: None,
            etag: None,
            storage_class: None,
            version_id: None,
            metadata: BTreeMap::new(),
            tags: BTreeMap::new(),
            tag_error: None,
            delete_marker: false,
            modified: None,
        }
    }

    fn declared_size(&self) -> u64 {
        self.size.unwrap_or(self.bytes.len() as u64)
    }

    fn etag(&self) -> String {
        self.etag
            .clone()
            .unwrap_or_else(|| blake3::hash(&self.bytes).to_hex().to_string())
    }

    fn meta(&self) -> S3ObjectMeta {
        S3ObjectMeta {
            key: String::new(),
            size: self.declared_size(),
            last_modified: self.modified,
            etag: Some(self.etag()),
            storage_class: self.storage_class.clone(),
            version_id: self.version_id.clone(),
            user_metadata: self.metadata.clone(),
            delete_marker: self.delete_marker,
        }
    }
}

/// In-memory backend for tests. CI uses this instead of a MinIO container.
pub struct MemoryS3Backend {
    objects: Mutex<BTreeMap<String, BTreeMap<String, MemObj>>>,
    page_size: Mutex<usize>,
    scripts: Mutex<VecDeque<String>>,
    requests: AtomicU64,
    body_reads: AtomicU64,
}

impl MemoryS3Backend {
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(BTreeMap::new()),
            page_size: Mutex::new(1_000),
            scripts: Mutex::new(VecDeque::new()),
            requests: AtomicU64::new(0),
            body_reads: AtomicU64::new(0),
        }
    }

    pub fn put(&self, bucket: &str, key: &str, bytes: impl Into<Vec<u8>>) {
        self.insert(bucket, key, MemObj::plain(bytes.into()));
    }

    pub fn put_etag(&self, bucket: &str, key: &str, bytes: impl Into<Vec<u8>>, etag: &str) {
        let mut obj = MemObj::plain(bytes.into());
        obj.etag = Some(etag.to_string());
        self.insert(bucket, key, obj);
    }

    pub fn put_sized(&self, bucket: &str, key: &str, size: u64) {
        let mut obj = MemObj::plain(Vec::new());
        obj.size = Some(size);
        self.insert(bucket, key, obj);
    }

    pub fn put_rich(
        &self,
        bucket: &str,
        key: &str,
        bytes: impl Into<Vec<u8>>,
        storage_class: &str,
        metadata: BTreeMap<String, String>,
        tags: BTreeMap<String, String>,
    ) {
        let mut obj = MemObj::plain(bytes.into());
        obj.storage_class = Some(storage_class.to_string());
        obj.metadata = metadata;
        obj.tags = tags;
        self.insert(bucket, key, obj);
    }

    pub fn put_tag_error(&self, bucket: &str, key: &str, bytes: impl Into<Vec<u8>>, error: &str) {
        let mut obj = MemObj::plain(bytes.into());
        obj.tag_error = Some(error.to_string());
        self.insert(bucket, key, obj);
    }

    pub fn put_delete_marker(&self, bucket: &str, key: &str) {
        let mut obj = MemObj::plain(Vec::new());
        obj.delete_marker = true;
        self.insert(bucket, key, obj);
    }

    pub fn remove(&self, bucket: &str, key: &str) {
        if let Some(keys) = self.objects.lock().unwrap().get_mut(bucket) {
            keys.remove(key);
        }
    }

    pub fn set_page_size(&self, size: usize) {
        *self.page_size.lock().unwrap() = size.max(1);
    }

    pub fn push_error(&self, message: &str) {
        self.scripts.lock().unwrap().push_back(message.to_string());
    }

    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn body_reads(&self) -> u64 {
        self.body_reads.load(Ordering::Relaxed)
    }

    fn insert(&self, bucket: &str, key: &str, obj: MemObj) {
        self.objects
            .lock()
            .unwrap()
            .entry(bucket.to_string())
            .or_default()
            .insert(key.to_string(), obj);
    }

    fn bump(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    fn pop_error(&self) -> Option<GnosisError> {
        self.scripts
            .lock()
            .unwrap()
            .pop_front()
            .map(GnosisError::Pipeline)
    }

    fn page(
        &self,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
        include_delete: bool,
    ) -> Result<S3ListPage> {
        self.bump();
        if let Some(err) = self.pop_error() {
            return Err(err);
        }
        let map = self.objects.lock().unwrap();
        let Some(keys) = map.get(bucket) else {
            return Ok(S3ListPage {
                objects: Vec::new(),
                continuation: None,
            });
        };
        let mut listed: Vec<S3ObjectMeta> = keys
            .iter()
            .filter(|(key, obj)| {
                (prefix.is_empty() || key.starts_with(prefix))
                    && !key.ends_with('/')
                    && (include_delete || !obj.delete_marker)
            })
            .map(|(key, obj)| {
                let mut meta = obj.meta();
                meta.key = key.clone();
                meta
            })
            .collect();
        listed.sort_by(|a, b| a.key.cmp(&b.key));
        let start = token
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let page_size = *self.page_size.lock().unwrap();
        if start >= listed.len() {
            return Ok(S3ListPage {
                objects: Vec::new(),
                continuation: None,
            });
        }
        let end = (start + page_size).min(listed.len());
        let continuation = if end < listed.len() {
            Some(end.to_string())
        } else {
            None
        };
        Ok(S3ListPage {
            objects: listed[start..end].to_vec(),
            continuation,
        })
    }

    fn lookup(&self, bucket: &str, key: &str) -> Result<MemObj> {
        self.bump();
        if let Some(err) = self.pop_error() {
            return Err(err);
        }
        let map = self.objects.lock().unwrap();
        map.get(bucket)
            .and_then(|keys| keys.get(key))
            .map(|obj| MemObj {
                bytes: obj.bytes.clone(),
                size: obj.size,
                etag: obj.etag.clone(),
                storage_class: obj.storage_class.clone(),
                version_id: obj.version_id.clone(),
                metadata: obj.metadata.clone(),
                tags: obj.tags.clone(),
                tag_error: obj.tag_error.clone(),
                delete_marker: obj.delete_marker,
                modified: obj.modified,
            })
            .ok_or_else(|| {
                GnosisError::Pipeline(format!("s3 object not found: s3://{bucket}/{key}"))
            })
    }
}

impl Default for MemoryS3Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl S3Backend for MemoryS3Backend {
    fn list_page(&self, bucket: &str, prefix: &str, token: Option<&str>) -> Result<S3ListPage> {
        retry_s3(|| self.page(bucket, prefix, token, false))
    }

    fn list_versions_page(
        &self,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
    ) -> Result<S3ListPage> {
        retry_s3(|| self.page(bucket, prefix, token, true))
    }

    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>> {
        retry_s3(|| {
            let obj = self.lookup(bucket, key)?;
            if obj.delete_marker {
                return Err(GnosisError::Pipeline(
                    "s3 delete marker is not downloaded".into(),
                ));
            }
            if obj.declared_size() > max_size {
                return Err(GnosisError::Pipeline("s3 size limit".into()));
            }
            self.body_reads.fetch_add(1, Ordering::Relaxed);
            let n = (max_size as usize).min(obj.bytes.len());
            Ok(obj.bytes[..n].to_vec())
        })
    }

    fn head_object(&self, bucket: &str, key: &str) -> Result<S3ObjectMeta> {
        retry_s3(|| {
            let obj = self.lookup(bucket, key)?;
            let mut meta = obj.meta();
            meta.key = key.to_string();
            Ok(meta)
        })
    }

    fn get_object_tagging(&self, bucket: &str, key: &str) -> Result<BTreeMap<String, String>> {
        retry_s3(|| {
            let obj = self.lookup(bucket, key)?;
            if let Some(error) = obj.tag_error {
                return Err(GnosisError::Pipeline(error));
            }
            Ok(obj.tags)
        })
    }
}

pub struct S3Connector {
    location: S3Location,
    config: ScanConfig,
    backend: Arc<dyn S3Backend>,
    discovered_keys: Mutex<Vec<String>>,
    protos: Mutex<BTreeMap<String, S3ProtoData>>,
    notes: Mutex<BTreeMap<String, Vec<String>>>,
    warnings: Mutex<Vec<String>>,
    requests: AtomicU64,
}

impl S3Connector {
    pub fn new(location: S3Location, config: ScanConfig, backend: Arc<dyn S3Backend>) -> Self {
        Self {
            location,
            config,
            backend,
            discovered_keys: Mutex::new(Vec::new()),
            protos: Mutex::new(BTreeMap::new()),
            notes: Mutex::new(BTreeMap::new()),
            warnings: Mutex::new(Vec::new()),
            requests: AtomicU64::new(0),
        }
    }

    pub fn location(&self) -> &S3Location {
        &self.location
    }

    pub fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }

    pub fn proto_for(&self, id: &ObjectId) -> Option<S3ProtoData> {
        self.protos.lock().unwrap().get(id.as_str()).cloned()
    }

    pub fn notes_for(&self, id: &ObjectId) -> Vec<String> {
        self.notes
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn withheld_body(&self, object: &ObjectDescriptor) -> bool {
        self.options().metadata_only
            || self
                .notes_for(&object.id)
                .iter()
                .any(|note| note.contains("s3 size limit"))
    }

    fn options(&self) -> S3ScanOptions {
        match &self.config.source {
            ScanSource::S3 { options, .. } => options.clone(),
            _ => S3ScanOptions::default(),
        }
    }

    fn allow(&self) -> bool {
        let cap = self.options().max_requests;
        if cap == 0 {
            self.requests.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let previous = self.requests.fetch_add(1, Ordering::Relaxed);
        if previous >= cap {
            self.requests.fetch_sub(1, Ordering::Relaxed);
            self.warn("s3 request budget reached");
            return false;
        }
        true
    }

    fn warn(&self, message: &str) {
        let mut warnings = self.warnings.lock().unwrap();
        if !warnings.iter().any(|existing| existing == message) {
            warnings.push(message.to_string());
        }
    }

    fn note(&self, id: &ObjectId, message: &str) {
        self.notes
            .lock()
            .unwrap()
            .entry(id.as_str().to_string())
            .or_default()
            .push(message.to_string());
    }

    pub fn discover(
        &self,
        events: &Sender<PipelineEvent>,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<ObjectDescriptor>> {
        let list_prefix = if self.location.prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.location.prefix.trim_end_matches('/'))
        };
        let versions = self.options().versions;
        let mut objects = Vec::new();
        let mut keys = Vec::new();
        let mut token = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(GnosisError::Cancelled);
            }
            if !self.allow() {
                let _ = events.send(PipelineEvent::Warning {
                    message: "s3 request budget reached".into(),
                });
                break;
            }
            let page = if versions {
                self.backend.list_versions_page(
                    &self.location.bucket,
                    &list_prefix,
                    token.as_deref(),
                )
            } else {
                self.backend
                    .list_page(&self.location.bucket, &list_prefix, token.as_deref())
            };
            let page = page?;
            let next = page.continuation;
            for meta in page.objects {
                if cancel.load(Ordering::Relaxed) {
                    return Err(GnosisError::Cancelled);
                }
                if meta.delete_marker {
                    continue;
                }
                if !key_is_safe(&meta.key) {
                    let message = "s3 skipped malformed key".to_string();
                    self.warn(&message);
                    let _ = events.send(PipelineEvent::Warning { message });
                    continue;
                }
                let relative = match strip_prefix_key(&meta.key, &self.location.prefix) {
                    Some(relative) if !relative.is_empty() => relative,
                    _ => continue,
                };
                if key_is_excluded(
                    relative,
                    &self.config.excluded_paths,
                    &self.config.skip_output_dir_name,
                ) {
                    continue;
                }
                let desc = descriptor_for_key(
                    &self.location,
                    relative,
                    meta.size,
                    meta.last_modified,
                    meta.etag.clone(),
                );
                let proto = self.inspect_meta(&meta, &desc, events)?;
                self.protos
                    .lock()
                    .unwrap()
                    .insert(desc.id.as_str().to_string(), proto);
                let _ = events.send(PipelineEvent::ObjectDiscovered {
                    id: desc.id.clone(),
                    path: desc.relative_path.clone(),
                });
                keys.push(meta.key);
                objects.push(desc);
            }
            match next {
                Some(value) => token = Some(value),
                None => break,
            }
        }
        *self.discovered_keys.lock().unwrap() = keys;
        Ok(objects)
    }

    fn inspect_meta(
        &self,
        meta: &S3ObjectMeta,
        desc: &ObjectDescriptor,
        events: &Sender<PipelineEvent>,
    ) -> Result<S3ProtoData> {
        let mut proto = S3ProtoData {
            storage_class: meta.storage_class.clone(),
            etag: meta.etag.clone(),
            etag_kind: meta
                .etag
                .as_deref()
                .map(etag_kind)
                .unwrap_or("single")
                .to_string(),
            version_id: meta.version_id.clone(),
            user_metadata: BTreeMap::new(),
            tags: BTreeMap::new(),
        };
        if !self.options().inspect {
            return Ok(proto);
        }
        let key = &meta.key;
        if self.allow() {
            match self.backend.head_object(&self.location.bucket, key) {
                Ok(head) => {
                    if head.storage_class.is_some() {
                        proto.storage_class = head.storage_class;
                    }
                    if head.etag.is_some() {
                        proto.etag_kind = head
                            .etag
                            .as_deref()
                            .map(etag_kind)
                            .unwrap_or("single")
                            .to_string();
                        proto.etag = head.etag;
                    }
                    if head.version_id.is_some() {
                        proto.version_id = head.version_id;
                    }
                    proto.user_metadata = cap_s3_map(head.user_metadata);
                }
                Err(err) => {
                    let message = format!("s3 head failed for {}: {err}", desc.id);
                    self.note(&desc.id, &message);
                    self.warn(&message);
                    let _ = events.send(PipelineEvent::Warning { message });
                }
            }
        }
        if self.allow() {
            match self.backend.get_object_tagging(&self.location.bucket, key) {
                Ok(tags) => proto.tags = cap_s3_map(tags),
                Err(err) if is_access_denied(&err) => {
                    let message = format!("s3 tag access denied for {}", desc.id);
                    self.note(&desc.id, &message);
                    self.warn(&message);
                    let _ = events.send(PipelineEvent::Warning { message });
                }
                Err(err) => {
                    let message = format!("s3 tags failed for {}: {err}", desc.id);
                    self.note(&desc.id, &message);
                    self.warn(&message);
                    let _ = events.send(PipelineEvent::Warning { message });
                }
            }
        }
        Ok(proto)
    }

    pub fn read_object_bytes(&self, object: &ObjectDescriptor, max_size: u64) -> Result<Vec<u8>> {
        if self.options().metadata_only {
            return Ok(Vec::new());
        }
        if object.size > max_size {
            self.note(&object.id, "s3 size limit");
            return Err(GnosisError::Pipeline("s3 size limit".into()));
        }
        if !self.allow() {
            self.note(&object.id, "s3 request budget reached");
            return Ok(Vec::new());
        }
        let key = self.location.full_key(&object.relative_path);
        self.backend
            .get_object(&self.location.bucket, &key, max_size)
    }

    pub fn collect_neighbors(&self, object: &ObjectDescriptor, limit: usize) -> Vec<String> {
        let rel = object.relative_path.to_string_lossy().replace('\\', "/");
        let parent = match rel.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => "",
        };
        let keys = self.discovered_keys.lock().unwrap();
        let mut names = Vec::new();
        for full in keys.iter() {
            let Some(relative) = strip_prefix_key(full, &self.location.prefix) else {
                continue;
            };
            let (obj_parent, name) = match relative.rsplit_once('/') {
                Some((parent, name)) => (parent, name),
                None => ("", relative),
            };
            if obj_parent == parent
                && !name.is_empty()
                && !names.iter().any(|existing| existing == name)
            {
                names.push(name.to_string());
            }
            if names.len() >= limit {
                break;
            }
        }
        names.sort();
        names.truncate(limit);
        names
    }
}

fn is_access_denied(err: &GnosisError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("accessdenied") || text.contains("access denied")
}

fn key_is_safe(key: &str) -> bool {
    if key.contains('\0') || key.starts_with('/') {
        return false;
    }
    !key.split('/').any(|part| part == "..")
}

fn strip_prefix_key<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    let key = key.trim_start_matches('/');
    if prefix.is_empty() {
        return Some(key);
    }
    let prefix = prefix.trim_matches('/');
    let with_slash = format!("{prefix}/");
    key.strip_prefix(&with_slash)
        .or_else(|| if key == prefix { Some("") } else { None })
}

fn key_is_excluded(relative: &str, excluded: &[String], skip_name: &str) -> bool {
    relative
        .split('/')
        .any(|part| part == skip_name || excluded.iter().any(|item| item.as_str() == part))
}

fn descriptor_for_key(
    location: &S3Location,
    relative: &str,
    size: u64,
    modified: Option<SystemTime>,
    etag: Option<String>,
) -> ObjectDescriptor {
    let relative_path = PathBuf::from(relative);
    let path = PathBuf::from(format!(
        "s3://{}/{}",
        location.bucket,
        location.full_key(&relative_path)
    ));
    let extension = relative_path
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned());
    let media_type = mime_guess::from_path(&relative_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    ObjectDescriptor {
        id: ObjectId::new(format!("obj:{}", relative.replace('\\', "/"))),
        path,
        relative_path,
        is_dir: false,
        size,
        modified,
        media_type,
        extension,
        etag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScanSource;
    use crate::intrinsic::{extract_intrinsic, FactValue};
    use crate::knowledge_db::KnowledgeDb;
    use crate::pipeline::Pipeline;
    use crate::providers::default_registry;
    use crate::status::UnderstandingStatus;
    use crossbeam_channel::unbounded;
    use std::sync::atomic::AtomicBool;

    fn location(prefix: &str) -> S3Location {
        S3Location {
            bucket: "b".into(),
            prefix: prefix.into(),
        }
    }

    fn config_for(prefix: &str, options: S3ScanOptions) -> ScanConfig {
        ScanConfig::with_source(ScanSource::S3 {
            location: location(prefix),
            region: None,
            options,
        })
    }

    fn connector(
        prefix: &str,
        options: S3ScanOptions,
        backend: Arc<MemoryS3Backend>,
    ) -> S3Connector {
        S3Connector::new(location(prefix), config_for(prefix, options), backend)
    }

    fn discover(connector: &S3Connector) -> Vec<ObjectDescriptor> {
        let (tx, _rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        connector.discover(&tx, &cancel).unwrap()
    }

    #[test]
    fn parse_bucket_and_prefix() {
        let loc = parse_s3_uri("s3://my-bucket").unwrap();
        assert_eq!(loc.bucket, "my-bucket");
        assert_eq!(loc.prefix, "");
        assert_eq!(loc.display_root(), PathBuf::from("s3://my-bucket"));

        let loc = parse_s3_uri("s3://my-bucket/docs/api/").unwrap();
        assert_eq!(loc.bucket, "my-bucket");
        assert_eq!(loc.prefix, "docs/api");
        assert_eq!(loc.display_root(), PathBuf::from("s3://my-bucket/docs/api"));
    }

    #[test]
    fn parse_rejects_bad_uris() {
        assert!(parse_s3_uri("https://x").is_err());
        assert!(parse_s3_uri("s3://").is_err());
        assert!(parse_s3_uri("s3://bucket with spaces").is_err());
    }

    #[test]
    fn discover_maps_keys_to_paths_and_excludes() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "src/main.rs", b"fn main() {}");
        backend.put("b", "src/lib.rs", b"");
        backend.put("b", "target/out.bin", b"x");
        backend.put("b", "knowledge.okf/index.md", b"#");
        backend.put("b", "data/", b"");

        let connector = connector("", S3ScanOptions::default(), backend);
        let rels: Vec<_> = discover(&connector)
            .iter()
            .map(|object| object.relative_path.to_string_lossy().into_owned())
            .collect();
        assert!(rels.contains(&"src/main.rs".into()));
        assert!(rels.contains(&"src/lib.rs".into()));
        assert!(!rels.iter().any(|rel| rel.contains("target")));
        assert!(!rels.iter().any(|rel| rel.contains("knowledge.okf")));
    }

    #[test]
    fn discover_respects_prefix_as_root() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "docs/readme.md", b"# hi");
        backend.put("b", "docs/api/spec.md", b"spec");
        backend.put("b", "docs-old/legacy.md", b"no");
        backend.put("b", "other/x.md", b"no");

        let connector = connector("docs", S3ScanOptions::default(), backend.clone());
        let objects = discover(&connector);
        let rels: Vec<_> = objects
            .iter()
            .map(|object| object.relative_path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rels.len(), 2);
        assert!(rels.contains(&"readme.md".into()));
        assert!(rels.contains(&"api/spec.md".into()));

        let readme = objects
            .iter()
            .find(|object| object.relative_path.ends_with("readme.md"))
            .unwrap();
        let bytes = connector.read_object_bytes(readme, 1024).unwrap();
        assert_eq!(bytes, b"# hi");
        assert_eq!(readme.path.to_string_lossy(), "s3://b/docs/readme.md");
        assert!(readme.etag.is_some());
        assert_eq!(backend.body_reads(), 1);
    }

    #[test]
    fn etag_changes_when_bytes_change() {
        let backend = MemoryS3Backend::new();
        backend.put("b", "a.h", b"int x;");
        let first = backend.list_objects("b", "").unwrap();
        backend.put("b", "a.h", b"int y;");
        let second = backend.list_objects("b", "").unwrap();
        assert_ne!(first[0].etag, second[0].etag);
        let again = backend.list_objects("b", "").unwrap();
        assert_eq!(second[0].etag, again[0].etag);
    }

    #[test]
    fn prefix_boundary_excludes_sibling() {
        let backend = MemoryS3Backend::new();
        backend.put("b", "docs/a.txt", b"a");
        backend.put("b", "docs-old/a.txt", b"old");
        let listed = backend.list_objects("b", "docs/").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "docs/a.txt");
    }

    #[test]
    fn pagination_returns_a_continuation_token() {
        let backend = MemoryS3Backend::new();
        backend.set_page_size(1);
        backend.put("b", "a.txt", b"a");
        backend.put("b", "b.txt", b"b");
        let first = backend.list_page("b", "", None).unwrap();
        assert_eq!(first.objects.len(), 1);
        assert!(first.continuation.is_some());
        let second = backend
            .list_page("b", "", first.continuation.as_deref())
            .unwrap();
        assert_eq!(second.objects.len(), 1);
        assert!(second.continuation.is_none());
        assert_ne!(first.objects[0].key, second.objects[0].key);
    }

    #[test]
    fn request_budget_stops_a_truncated_listing() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.set_page_size(1);
        backend.put("b", "a.txt", b"a");
        backend.put("b", "b.txt", b"b");
        let options = S3ScanOptions {
            max_requests: 1,
            ..S3ScanOptions::default()
        };
        let connector = connector("", options, backend);
        let objects = discover(&connector);
        assert_eq!(objects.len(), 1);
        assert!(connector
            .warnings()
            .iter()
            .any(|warning| warning.contains("budget")));
    }

    #[test]
    fn retry_429_then_success() {
        let backend = MemoryS3Backend::new();
        backend.push_error("s3 http 429");
        backend.put("b", "a.txt", b"a");
        let listed = backend.list_objects("b", "").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(backend.requests(), 2);
    }

    #[test]
    fn expired_token_is_not_retried_and_credentials_are_redacted() {
        let backend = MemoryS3Backend::new();
        backend.push_error(
            "ExpiredToken AKIAIOSFODNN7EXAMPLE X-Amz-Signature=deadbeef0123&X-Amz-Date=1",
        );
        let err = backend.list_objects("b", "").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("ExpiredToken"));
        assert!(!text.contains("AKIA"));
        assert!(!text.contains("deadbeef"));
        assert!(text.contains("REDACTED"));
        assert_eq!(backend.requests(), 1);
    }

    #[test]
    fn multipart_etag_stays_opaque() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put_etag("b", "blob.bin", b"hello", "abc-2");
        let connector = connector("", S3ScanOptions::default(), backend);
        let objects = discover(&connector);
        assert_eq!(objects[0].etag.as_deref(), Some("abc-2"));
        let proto = connector.proto_for(&objects[0].id).unwrap();
        assert_eq!(proto.etag.as_deref(), Some("abc-2"));
        assert_eq!(proto.etag_kind, "multipart");
        let evidence = extract_intrinsic(
            &objects[0],
            &crate::connectors::types::ProtoData {
                s3: Some(proto),
                ..crate::connectors::types::ProtoData::default()
            },
            b"",
        );
        let etag = evidence
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "s3:etag")
            .unwrap();
        assert_eq!(etag.value, FactValue::String("abc-2".into()));
        assert!(evidence.intrinsic.facts.iter().any(|fact| {
            fact.key == "s3:etag_kind" && fact.value == FactValue::String("multipart".into())
        }));
        assert!(evidence
            .intrinsic
            .facts
            .iter()
            .all(|fact| !matches!(fact.value, FactValue::Digest(_))));
    }

    #[test]
    fn tag_denial_is_a_warning() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put_tag_error("b", "note.bin", b"x", "AccessDenied");
        let options = S3ScanOptions {
            inspect: true,
            ..S3ScanOptions::default()
        };
        let connector = connector("", options, backend);
        let objects = discover(&connector);
        assert_eq!(objects.len(), 1);
        let proto = connector.proto_for(&objects[0].id).unwrap();
        assert!(proto.tags.is_empty());
        assert!(connector
            .warnings()
            .iter()
            .any(|warning| warning.contains("access denied")));
    }

    #[test]
    fn malformed_keys_are_skipped() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "../secret.bin", b"x");
        backend.put("b", "/abs.bin", b"x");
        backend.put("b", "a\0b.bin", b"x");
        backend.put("b", "ok.bin", b"ok");
        let connector = connector("", S3ScanOptions::default(), backend);
        let objects = discover(&connector);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].relative_path.to_string_lossy(), "ok.bin");
        assert!(!connector.warnings().is_empty());
    }

    #[test]
    fn huge_declared_size_does_not_read_the_body() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put_sized("b", "huge.bin", 9_000_000_000_000);
        let err = backend.get_object("b", "huge.bin", 1024).unwrap_err();
        assert!(err.to_string().contains("s3 size limit"));
        assert_eq!(backend.body_reads(), 0);

        let connector = connector("", S3ScanOptions::default(), backend.clone());
        let objects = discover(&connector);
        let err = connector.read_object_bytes(&objects[0], 1024).unwrap_err();
        assert!(err.to_string().contains("s3 size limit"));
        assert_eq!(backend.body_reads(), 0);
    }

    #[test]
    fn metadata_only_does_not_read_the_body() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "note.bin", b"hello");
        let options = S3ScanOptions {
            metadata_only: true,
            ..S3ScanOptions::default()
        };
        let connector = connector("", options, backend.clone());
        let objects = discover(&connector);
        let bytes = connector.read_object_bytes(&objects[0], 1024).unwrap();
        assert!(bytes.is_empty());
        assert_eq!(backend.body_reads(), 0);
        assert!(connector.proto_for(&objects[0].id).unwrap().etag.is_some());
    }

    #[test]
    fn delete_marker_is_not_downloaded() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "live.bin", b"live");
        backend.put_delete_marker("b", "gone.bin");
        let options = S3ScanOptions {
            versions: true,
            ..S3ScanOptions::default()
        };
        let connector = connector("", options, backend.clone());
        let objects = discover(&connector);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].relative_path.to_string_lossy(), "live.bin");
        assert_eq!(backend.body_reads(), 0);
    }

    #[test]
    fn inspect_records_metadata_and_tags() {
        let backend = Arc::new(MemoryS3Backend::new());
        let mut metadata = BTreeMap::new();
        metadata.insert("author".into(), "ada".into());
        let mut tags = BTreeMap::new();
        tags.insert("env".into(), "test".into());
        backend.put_rich("b", "note.bin", b"x", "STANDARD", metadata, tags);
        let options = S3ScanOptions {
            inspect: true,
            ..S3ScanOptions::default()
        };
        let connector = connector("", options, backend);
        let objects = discover(&connector);
        let proto = connector.proto_for(&objects[0].id).unwrap();
        assert_eq!(proto.storage_class.as_deref(), Some("STANDARD"));
        assert_eq!(
            proto.user_metadata.get("author").map(String::as_str),
            Some("ada")
        );
        assert_eq!(proto.tags.get("env").map(String::as_str), Some("test"));
    }

    #[test]
    fn cap_drops_the_33rd_metadata_key() {
        let mut entries = Vec::new();
        for index in 0..40 {
            entries.push((format!("k{index:02}"), "v".repeat(300)));
        }
        let capped = cap_s3_map(entries);
        assert_eq!(capped.len(), 32);
        assert!(capped.values().all(|value| value.chars().count() <= 256));
    }

    fn run_scan(dir: &Path, backend: Arc<MemoryS3Backend>, options: S3ScanOptions, max_size: u64) {
        let mut config = config_for("", options);
        config.max_object_size = max_size;
        config.concurrency = 1;
        config.job_db_path = dir.join("jobs.redb");
        config.knowledge_db_path = dir.join("knowledge.redb");
        let pipeline = Pipeline::new(config, default_registry())
            .unwrap()
            .with_s3_backend(backend);
        let (tx, rx) = unbounded();
        let drain = std::thread::spawn(move || while rx.recv().is_ok() {});
        pipeline.run(tx).unwrap();
        drain.join().unwrap();
    }

    #[test]
    fn second_scan_drops_a_deleted_key_and_skips_a_matching_etag() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "keep.bin", b"same");
        backend.put("b", "drop.bin", b"gone");
        run_scan(dir.path(), backend.clone(), S3ScanOptions::default(), 1024);
        assert_eq!(backend.body_reads(), 2);
        backend.remove("b", "drop.bin");
        run_scan(dir.path(), backend.clone(), S3ScanOptions::default(), 1024);
        assert_eq!(backend.body_reads(), 2);
        let db = KnowledgeDb::open(dir.path().join("knowledge.redb")).unwrap();
        let ids: Vec<_> = db
            .load_all()
            .unwrap()
            .into_iter()
            .map(|row| row.object.descriptor.id.as_str().to_string())
            .collect();
        assert!(ids.iter().any(|id| id.contains("keep.bin")));
        assert!(!ids.iter().any(|id| id.contains("drop.bin")));
    }

    #[test]
    fn metadata_only_scan_is_not_understood() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put_etag("b", "note.bin", b"hello", "abc-2");
        let options = S3ScanOptions {
            metadata_only: true,
            ..S3ScanOptions::default()
        };
        run_scan(dir.path(), backend.clone(), options, 1024);
        assert_eq!(backend.body_reads(), 0);
        let db = KnowledgeDb::open(dir.path().join("knowledge.redb")).unwrap();
        let row = db.load_all().unwrap().into_iter().next().unwrap();
        assert_ne!(row.object.status, UnderstandingStatus::Understood);
        let facts = &row.evidence.unwrap().intrinsic.facts;
        assert!(facts.iter().any(|fact| {
            fact.key == "s3:etag" && fact.value == FactValue::String("abc-2".into())
        }));
    }
}
