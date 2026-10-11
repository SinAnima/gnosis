//! S3 connector — treat a bucket as a root folder and object keys as paths.
//!
//! URI form: `s3://bucket` or `s3://bucket/optional/prefix`.
//! Credentials and region use the default AWS chain unless `--region` is set.

use crate::config::ScanConfig;
use crate::connectors::types::ObjectDescriptor;
use crate::error::{GnosisError, Result};
use crate::events::PipelineEvent;
use crate::ids::ObjectId;
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

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

#[derive(Clone, Debug)]
pub struct S3ObjectMeta {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<SystemTime>,
    pub etag: Option<String>,
}

/// Minimal S3 surface so discovery/read can be unit-tested without AWS.
pub trait S3Backend: Send + Sync {
    fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<S3ObjectMeta>>;
    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>>;
}

/// Live AWS SDK backend (default credential / region chain).
pub struct AwsS3Backend {
    client: aws_sdk_s3::Client,
    runtime: tokio::runtime::Runtime,
}

impl AwsS3Backend {
    pub fn new(region: Option<&str>) -> Result<Self> {
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
            aws_sdk_s3::Client::new(&shared)
        });

        Ok(Self { client, runtime })
    }
}

impl S3Backend for AwsS3Backend {
    fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<S3ObjectMeta>> {
        self.runtime.block_on(async {
            let mut out = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut req = self.client.list_objects_v2().bucket(bucket);
                if !prefix.is_empty() {
                    req = req.prefix(prefix);
                }
                if let Some(t) = token.as_ref() {
                    req = req.continuation_token(t);
                }
                let resp = req.send().await.map_err(|e| {
                    GnosisError::Pipeline(format!("s3 list_objects_v2 s3://{bucket}/{prefix}: {e}"))
                })?;

                for obj in resp.contents() {
                    let key = match obj.key() {
                        Some(k) if !k.is_empty() && !k.ends_with('/') => k.to_string(),
                        _ => continue,
                    };
                    let size = obj.size().unwrap_or(0) as u64;
                    let last_modified = obj.last_modified().and_then(|t| {
                        SystemTime::UNIX_EPOCH
                            .checked_add(std::time::Duration::from_secs(t.secs() as u64))
                    });
                    out.push(S3ObjectMeta {
                        key,
                        size,
                        last_modified,
                        etag: obj.e_tag().map(|tag| tag.trim_matches('"').to_string()),
                    });
                }

                if resp.is_truncated().unwrap_or(false) {
                    token = resp.next_continuation_token().map(str::to_string);
                    if token.is_none() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Ok(out)
        })
    }

    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>> {
        self.runtime.block_on(async {
            let mut req = self.client.get_object().bucket(bucket).key(key);
            if max_size > 0 {
                // Inclusive byte range: 0..(max_size-1)
                req = req.range(format!("bytes=0-{}", max_size.saturating_sub(1)));
            }
            let resp = req.send().await.map_err(|e| {
                GnosisError::Pipeline(format!("s3 get_object s3://{bucket}/{key}: {e}"))
            })?;

            let aggregated = resp
                .body
                .collect()
                .await
                .map_err(|e| GnosisError::Pipeline(format!("s3 read body {key}: {e}")))?;
            let bytes = aggregated.into_bytes();
            let n = (max_size as usize).min(bytes.len());
            Ok(bytes[..n].to_vec())
        })
    }
}

/// In-memory backend for tests.
#[derive(Default)]
pub struct MemoryS3Backend {
    /// bucket → (key → bytes)
    pub objects: Mutex<BTreeMap<String, BTreeMap<String, Vec<u8>>>>,
    pub modified: Mutex<BTreeMap<(String, String), SystemTime>>,
}

impl MemoryS3Backend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, bucket: &str, key: &str, bytes: impl Into<Vec<u8>>) {
        let mut map = self.objects.lock().unwrap();
        map.entry(bucket.to_string())
            .or_default()
            .insert(key.to_string(), bytes.into());
    }
}

impl S3Backend for MemoryS3Backend {
    fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<S3ObjectMeta>> {
        let map = self.objects.lock().unwrap();
        let modified = self.modified.lock().unwrap();
        let Some(keys) = map.get(bucket) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (key, bytes) in keys {
            if !prefix.is_empty() && !key.starts_with(prefix) {
                continue;
            }
            if key.ends_with('/') {
                continue;
            }
            out.push(S3ObjectMeta {
                key: key.clone(),
                size: bytes.len() as u64,
                last_modified: modified.get(&(bucket.to_string(), key.clone())).copied(),
                etag: Some(blake3::hash(bytes).to_hex().to_string()),
            });
        }
        Ok(out)
    }

    fn get_object(&self, bucket: &str, key: &str, max_size: u64) -> Result<Vec<u8>> {
        let map = self.objects.lock().unwrap();
        let bytes = map.get(bucket).and_then(|b| b.get(key)).ok_or_else(|| {
            GnosisError::Pipeline(format!("s3 object not found: s3://{bucket}/{key}"))
        })?;
        let n = (max_size as usize).min(bytes.len());
        Ok(bytes[..n].to_vec())
    }
}

pub struct S3Connector<B: S3Backend = AwsS3Backend> {
    location: S3Location,
    config: ScanConfig,
    backend: Arc<B>,
    /// Full keys discovered in the last `discover` call (for neighbor lookup).
    discovered_keys: Mutex<Vec<String>>,
}

impl<B: S3Backend> S3Connector<B> {
    pub fn new(location: S3Location, config: ScanConfig, backend: Arc<B>) -> Self {
        Self {
            location,
            config,
            backend,
            discovered_keys: Mutex::new(Vec::new()),
        }
    }

    pub fn location(&self) -> &S3Location {
        &self.location
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

        let listed = self
            .backend
            .list_objects(&self.location.bucket, &list_prefix)?;

        let mut objects = Vec::new();
        let mut keys = Vec::new();
        let excluded = &self.config.excluded_paths;
        let skip_name = &self.config.skip_output_dir_name;

        for meta in listed {
            if cancel.load(Ordering::Relaxed) {
                return Err(GnosisError::Cancelled);
            }

            let relative = match strip_prefix_key(&meta.key, &self.location.prefix) {
                Some(r) if !r.is_empty() => r,
                _ => continue,
            };

            if key_is_excluded(relative, excluded, skip_name) {
                continue;
            }

            let desc = descriptor_for_key(
                &self.location,
                relative,
                meta.size,
                meta.last_modified,
                meta.etag.clone(),
            );
            let _ = events.send(PipelineEvent::ObjectDiscovered {
                id: desc.id.clone(),
                path: desc.relative_path.clone(),
            });
            keys.push(meta.key);
            objects.push(desc);
        }

        *self.discovered_keys.lock().unwrap() = keys;
        Ok(objects)
    }

    pub fn read_object_bytes(&self, object: &ObjectDescriptor, max_size: u64) -> Result<Vec<u8>> {
        let key = self.location.full_key(&object.relative_path);
        self.backend
            .get_object(&self.location.bucket, &key, max_size)
    }

    pub fn collect_neighbors(&self, object: &ObjectDescriptor, limit: usize) -> Vec<String> {
        let rel = object.relative_path.to_string_lossy().replace('\\', "/");
        let parent = match rel.rsplit_once('/') {
            Some((p, _)) => p,
            None => "",
        };
        let keys = self.discovered_keys.lock().unwrap();
        let mut names = Vec::new();
        for full in keys.iter() {
            let Some(relative) = strip_prefix_key(full, &self.location.prefix) else {
                continue;
            };
            let (obj_parent, name) = match relative.rsplit_once('/') {
                Some((p, n)) => (p, n),
                None => ("", relative),
            };
            if obj_parent == parent && !name.is_empty() && !names.iter().any(|n| n == name) {
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
        .any(|part| part == skip_name || excluded.iter().any(|e| e.as_str() == part))
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
        .map(|e| e.to_string_lossy().into_owned());
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
    use crossbeam_channel::unbounded;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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
        backend.put("b", "data/", b""); // directory marker skipped by list

        let config = ScanConfig::with_source(crate::config::ScanSource::S3 {
            location: S3Location {
                bucket: "b".into(),
                prefix: String::new(),
            },
            region: None,
        });
        let connector = S3Connector::new(
            S3Location {
                bucket: "b".into(),
                prefix: String::new(),
            },
            config,
            backend,
        );
        let (tx, _rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let objects = connector.discover(&tx, &cancel).unwrap();
        let rels: Vec<_> = objects
            .iter()
            .map(|o| o.relative_path.to_string_lossy().into_owned())
            .collect();
        assert!(rels.contains(&"src/main.rs".into()));
        assert!(rels.contains(&"src/lib.rs".into()));
        assert!(!rels.iter().any(|r| r.contains("target")));
        assert!(!rels.iter().any(|r| r.contains("knowledge.okf")));
    }

    #[test]
    fn discover_respects_prefix_as_root() {
        let backend = Arc::new(MemoryS3Backend::new());
        backend.put("b", "docs/readme.md", b"# hi");
        backend.put("b", "docs/api/spec.md", b"spec");
        backend.put("b", "other/x.md", b"no");

        let location = S3Location {
            bucket: "b".into(),
            prefix: "docs".into(),
        };
        let config = ScanConfig::with_source(crate::config::ScanSource::S3 {
            location: location.clone(),
            region: None,
        });
        let connector = S3Connector::new(location, config, backend.clone());
        let (tx, _rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let objects = connector.discover(&tx, &cancel).unwrap();
        let rels: Vec<_> = objects
            .iter()
            .map(|o| o.relative_path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rels.len(), 2);
        assert!(rels.contains(&"readme.md".into()));
        assert!(rels.contains(&"api/spec.md".into()));

        let readme = objects
            .iter()
            .find(|o| o.relative_path.ends_with("readme.md"))
            .unwrap();
        let bytes = connector.read_object_bytes(readme, 1024).unwrap();
        assert_eq!(bytes, b"# hi");
        assert_eq!(readme.path.to_string_lossy(), "s3://b/docs/readme.md");
        assert!(readme.etag.is_some());
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
}
